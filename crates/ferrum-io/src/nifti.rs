//! Minimal NIfTI-1 reader and writer (`.nii`, `.nii.gz`).

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use ferrum_domain::{
    Dims3, Geometry, IntensityRange, LabelMap, LoadedSeries, ProgressSink, RepositoryError, SeriesDescriptor,
    SeriesMetadata, Volume, VolumeRepository,
};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use glam::{Mat3, Vec3};
use rayon::prelude::*;

use crate::error::IoError;
use crate::files::{collect_files, detect, FileKind};

const HEADER_SIZE: usize = 348;

/// Parsed subset of the NIfTI-1 header.
#[derive(Debug, Clone, PartialEq)]
pub struct NiftiHeader {
    /// Volume dimensions.
    pub dims: Dims3,
    /// Voxel size in mm.
    pub spacing: Vec3,
    /// NIfTI datatype code.
    pub datatype: i16,
    /// Byte offset of voxel data.
    pub vox_offset: usize,
    /// Intensity scaling slope (0 = none).
    pub scl_slope: f32,
    /// Intensity scaling intercept.
    pub scl_inter: f32,
    /// Little-endian file.
    pub little_endian: bool,
    /// Free-text description.
    pub description: String,
    /// World step of each voxel axis (`i`, `j`, `k`) in RAS millimetres, from
    /// `sform` (preferred) or `qform`; `None` if the file has neither.
    pub axes: Option<[Vec3; 3]>,
    /// RAS world position of the centre of voxel `(0, 0, 0)`; zero when the
    /// file has no orientation.
    pub origin: Vec3,
}

/// RAS ↔ LPS (the conversion is its own inverse).
fn ras_lps(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, -v.y, v.z)
}

/// How stored voxel axes map onto the viewer's canonical patient frame
/// (LPS, the DICOM convention: `+x` left, `+y` posterior, `+z` superior).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reorientation {
    /// `target[a]`: canonical axis that stored axis `a` becomes.
    pub target: [usize; 3],
    /// `flip[a]`: stored axis `a` runs against its canonical axis.
    pub flip: [bool; 3],
}

impl Reorientation {
    /// No change.
    pub const IDENTITY: Reorientation = Reorientation { target: [0, 1, 2], flip: [false; 3] };

    /// Derives the reorientation from voxel axis directions in RAS space,
    /// snapping oblique axes to the closest canonical axis.
    pub fn from_ras_axes(axes: [Vec3; 3]) -> Self {
        Self::between(axes.map(ras_lps), Mat3::IDENTITY)
    }

    /// Derives the reorientation that maps stored voxel axes with LPS
    /// directions `axes` onto the grid whose axis directions are the columns
    /// of `frame`, snapping oblique axes to the closest grid axis.
    pub fn between(axes: [Vec3; 3], frame: Mat3) -> Self {
        let lps = axes.map(|d| Vec3::new(d.dot(frame.x_axis), d.dot(frame.y_axis), d.dot(frame.z_axis)));
        let mut target = [usize::MAX; 3];
        let mut flip = [false; 3];
        // Greedy assignment by largest absolute component, never reusing an axis.
        let mut order = [0usize, 1, 2];
        order.sort_by(|&a, &b| lps[b].abs().max_element().total_cmp(&lps[a].abs().max_element()));
        let mut used = [false; 3];
        for a in order {
            let d = lps[a];
            let best = (0..3).filter(|&t| !used[t]).max_by(|&x, &y| d[x].abs().total_cmp(&d[y].abs()));
            let Some(t) = best else { return Self::IDENTITY };
            used[t] = true;
            target[a] = t;
            flip[a] = d[t] < 0.0;
        }
        Self { target, flip }
    }

    /// Applies the reorientation to `values` of a grid with `dims` and
    /// `spacing`, returning the canonical grid.
    pub fn apply<T: Copy + Default + Send + Sync>(
        &self,
        values: &[T],
        dims: Dims3,
        spacing: Vec3,
    ) -> (Vec<T>, Dims3, Vec3) {
        if *self == Self::IDENTITY {
            return (values.to_vec(), dims, spacing);
        }
        let src = [dims.x, dims.y, dims.z];
        let mut dst = [0u32; 3];
        let mut sp = [0f32; 3];
        for a in 0..3 {
            dst[self.target[a]] = src[a];
            sp[self.target[a]] = spacing[a];
        }
        let out_dims = Dims3::new(dst[0], dst[1], dst[2]);
        let mut out = vec![T::default(); values.len()];
        out.par_chunks_mut(out_dims.slice_len()).enumerate().for_each(|(ok, slab)| {
            for oj in 0..dst[1] as usize {
                for oi in 0..dst[0] as usize {
                    slab[oi + oj * dst[0] as usize] = values[self.source_index([oi, oj, ok], src)];
                }
            }
        });
        (out, out_dims, Vec3::from(sp))
    }

    /// Linear index, in the source grid of size `src`, of the voxel that
    /// lands at canonical position `o`.
    fn source_index(&self, o: [usize; 3], src: [u32; 3]) -> usize {
        let mut s = [0usize; 3];
        for a in 0..3 {
            let v = o[self.target[a]];
            s[a] = if self.flip[a] { src[a] as usize - 1 - v } else { v };
        }
        s[0] + src[0] as usize * (s[1] + src[1] as usize * s[2])
    }
}

fn read_all(path: &Path) -> Result<Vec<u8>, IoError> {
    let f = File::open(path).map_err(|e| IoError::os(path, e))?;
    let mut buf = Vec::new();
    let gz = path.to_string_lossy().to_ascii_lowercase().ends_with(".gz");
    if gz {
        GzDecoder::new(BufReader::new(f)).read_to_end(&mut buf).map_err(|e| IoError::os(path, e))?;
    } else {
        BufReader::new(f).read_to_end(&mut buf).map_err(|e| IoError::os(path, e))?;
    }
    Ok(buf)
}

struct Cursor<'a> {
    b: &'a [u8],
    le: bool,
}

impl Cursor<'_> {
    fn arr<const N: usize>(&self, off: usize) -> [u8; N] {
        let mut a = [0u8; N];
        a.copy_from_slice(&self.b[off..off + N]);
        a
    }
    fn i16(&self, off: usize) -> i16 {
        let a = self.arr::<2>(off);
        if self.le {
            i16::from_le_bytes(a)
        } else {
            i16::from_be_bytes(a)
        }
    }
    fn i32(&self, off: usize) -> i32 {
        let a = self.arr::<4>(off);
        if self.le {
            i32::from_le_bytes(a)
        } else {
            i32::from_be_bytes(a)
        }
    }
    fn f32(&self, off: usize) -> f32 {
        let a = self.arr::<4>(off);
        if self.le {
            f32::from_le_bytes(a)
        } else {
            f32::from_be_bytes(a)
        }
    }
}

impl NiftiHeader {
    /// Parses a header from the first 348 bytes of `bytes`.
    pub fn parse(bytes: &[u8], path: &Path) -> Result<Self, IoError> {
        if bytes.len() < HEADER_SIZE {
            return Err(IoError::invalid(path, "file shorter than NIfTI header"));
        }
        let le = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) == HEADER_SIZE as i32;
        let c = Cursor { b: bytes, le };
        if c.i32(0) != HEADER_SIZE as i32 {
            return Err(IoError::invalid(path, "bad sizeof_hdr"));
        }
        let ndim = c.i16(40);
        if !(1..=7).contains(&ndim) {
            return Err(IoError::invalid(path, format!("bad dim[0] = {ndim}")));
        }
        let d = |i: usize| if i as i16 <= ndim { c.i16(40 + 2 * i).max(1) as u32 } else { 1 };
        let p = |i: usize| {
            let v = c.f32(76 + 4 * i).abs();
            if v.is_finite() && v > 0.0 {
                v
            } else {
                1.0
            }
        };
        let sform_code = c.i16(254);
        let qform_code = c.i16(252);
        let mut origin = Vec3::ZERO;
        let axes = if sform_code > 0 {
            origin = Vec3::new(c.f32(292), c.f32(308), c.f32(324));
            let row = |r: usize| Vec3::new(c.f32(280 + 16 * r), c.f32(284 + 16 * r), c.f32(288 + 16 * r));
            let (r0, r1, r2) = (row(0), row(1), row(2));
            Some([Vec3::new(r0.x, r1.x, r2.x), Vec3::new(r0.y, r1.y, r2.y), Vec3::new(r0.z, r1.z, r2.z)])
        } else if qform_code > 0 {
            let (b, cq, d) = (c.f32(256), c.f32(260), c.f32(264));
            let a = (1.0 - (b * b + cq * cq + d * d)).max(0.0).sqrt();
            let qfac = if c.f32(76) < 0.0 { -1.0 } else { 1.0 };
            origin = Vec3::new(c.f32(268), c.f32(272), c.f32(276));
            Some([
                Vec3::new(a * a + b * b - cq * cq - d * d, 2.0 * (b * cq + a * d), 2.0 * (b * d - a * cq)) * p(1),
                Vec3::new(2.0 * (b * cq - a * d), a * a + cq * cq - b * b - d * d, 2.0 * (cq * d + a * b)) * p(2),
                Vec3::new(2.0 * (b * d + a * cq), 2.0 * (cq * d - a * b), a * a + d * d - cq * cq - b * b) * qfac * p(3),
            ])
        } else {
            None
        }
        .filter(|ax| ax.iter().all(|v| v.is_finite() && v.length() > 1e-6));
        let desc_bytes = &bytes[148..228];
        let end = desc_bytes.iter().position(|&b| b == 0).unwrap_or(desc_bytes.len());
        Ok(Self {
            dims: Dims3::new(d(1), d(2), d(3)),
            spacing: Vec3::new(p(1), p(2), p(3)),
            datatype: c.i16(70),
            vox_offset: c.f32(108).max(HEADER_SIZE as f32) as usize,
            scl_slope: c.f32(112),
            scl_inter: c.f32(116),
            little_endian: le,
            description: String::from_utf8_lossy(&desc_bytes[..end]).into_owned(),
            axes,
            origin: if origin.is_finite() { origin } else { Vec3::ZERO },
        })
    }

    fn bytes_per_voxel(&self) -> Option<usize> {
        match self.datatype {
            2 | 256 => Some(1),
            4 | 512 => Some(2),
            8 | 768 | 16 => Some(4),
            64 => Some(8),
            _ => None,
        }
    }
}

/// Reads the header and every voxel of the first 3D volume as physical
/// values (scaling applied), in stored order.
fn read_values(path: &Path) -> Result<(Vec<f32>, NiftiHeader), IoError> {
    let bytes = read_all(path)?;
    let h = NiftiHeader::parse(&bytes, path)?;
    let bpv =
        h.bytes_per_voxel().ok_or_else(|| IoError::decode(path, format!("unsupported datatype {}", h.datatype)))?;
    let n = h.dims.voxel_count();
    let end = h.vox_offset + n * bpv;
    if bytes.len() < end {
        return Err(IoError::invalid(path, "voxel data truncated"));
    }
    let raw = &bytes[h.vox_offset..end];
    let (le, dt) = (h.little_endian, h.datatype);
    let slope = if h.scl_slope == 0.0 || !h.scl_slope.is_finite() { 1.0 } else { h.scl_slope };
    let inter = if h.scl_inter.is_finite() { h.scl_inter } else { 0.0 };
    let values: Vec<f32> = raw.par_chunks_exact(bpv).map(|b| decode_value(dt, le, b) as f32 * slope + inter).collect();
    Ok((values, h))
}

fn decode_value(dt: i16, le: bool, b: &[u8]) -> f64 {
    match (dt, le) {
        (2, _) => f64::from(b[0]),
        (256, _) => f64::from(b[0] as i8),
        (4, true) => f64::from(i16::from_le_bytes([b[0], b[1]])),
        (4, false) => f64::from(i16::from_be_bytes([b[0], b[1]])),
        (512, true) => f64::from(u16::from_le_bytes([b[0], b[1]])),
        (512, false) => f64::from(u16::from_be_bytes([b[0], b[1]])),
        (8, true) => f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        (8, false) => f64::from(i32::from_be_bytes([b[0], b[1], b[2], b[3]])),
        (768, true) => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        (768, false) => f64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
        (16, true) => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        (16, false) => f64::from(f32::from_be_bytes([b[0], b[1], b[2], b[3]])),
        (_, true) => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        (_, false) => f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
    }
}

/// Patient-space geometry (LPS) of the grid produced by reorienting a file
/// with header `h` by `r`.
fn canonical_geometry(h: &NiftiHeader, r: &Reorientation) -> Geometry {
    let Some(axes) = h.axes else { return Geometry::default() };
    let src = [h.dims.x, h.dims.y, h.dims.z];
    let mut cols = [Vec3::ZERO; 3];
    let mut origin = h.origin;
    for a in 0..3 {
        let unit = ras_lps(axes[a].normalize_or_zero());
        cols[r.target[a]] = if r.flip[a] { -unit } else { unit };
        if r.flip[a] {
            origin += axes[a] * (src[a] - 1) as f32;
        }
    }
    Geometry { origin: ras_lps(origin), direction: Mat3::from_cols(cols[0], cols[1], cols[2]) }
}

/// Reads a NIfTI-1 file into a volume (first 3D volume if 4D), reoriented
/// into the canonical LPS frame when the header carries an orientation.
pub fn read_nifti(path: &Path) -> Result<(Volume, NiftiHeader), IoError> {
    let (values, h) = read_values(path)?;
    let orientation = h.axes.map(Reorientation::from_ras_axes).unwrap_or(Reorientation::IDENTITY);
    let (values, dims, spacing) = orientation.apply(&values, h.dims, h.spacing);
    let geometry = canonical_geometry(&h, &orientation);
    Ok((volume_from_physical_par(dims, spacing, &values)?.with_geometry(geometry), h))
}

/// Reads a NIfTI-1 label map and resamples its axes onto the grid of
/// `volume` (axis permutation and flips only). Fails if the values are not
/// integers in `0..=255` or the grids differ in size.
pub fn read_label_nifti(path: &Path, volume: &Volume) -> Result<LabelMap, IoError> {
    let (values, h) = read_values(path)?;
    if let Some(v) = values.iter().find(|v| !(v.fract() == 0.0 && (0.0..=255.0).contains(*v))) {
        return Err(IoError::invalid(path, format!("label value {v} is not an integer in 0..=255")));
    }
    let labels: Vec<u8> = values.par_iter().map(|&v| v as u8).collect();
    let orientation = h
        .axes
        .map(|ax| Reorientation::between(ax.map(ras_lps), volume.geometry().direction))
        .unwrap_or(Reorientation::IDENTITY);
    let (labels, dims, _) = orientation.apply(&labels, h.dims, h.spacing);
    if dims != volume.dims() {
        return Err(IoError::invalid(
            path,
            format!("label map is {}×{}×{}, the volume is {}×{}×{}", dims.x, dims.y, dims.z, volume.dims().x, volume.dims().y, volume.dims().z),
        ));
    }
    LabelMap::from_data(dims, labels).map_err(|e| IoError::invalid(path, e.to_string()))
}

/// Parallel equivalent of [`Volume::from_physical`].
fn volume_from_physical_par(dims: Dims3, spacing: Vec3, values: &[f32]) -> Result<Volume, IoError> {
    let (min, max) = values
        .par_iter()
        .filter(|v| v.is_finite())
        .fold(|| (f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)))
        .reduce(|| (f32::INFINITY, f32::NEG_INFINITY), |(a, b), (c, d)| (a.min(c), b.max(d)));
    let (min, max) = if min.is_finite() { (min, max) } else { (0.0, 1.0) };
    let range = IntensityRange::new(min, max)?;
    let data = values.par_iter().map(|&v| range.to_storage(v)).collect();
    Ok(Volume::new(dims, spacing, range, data)?)
}

/// Voxel encoding of a written file.
struct Encoding {
    datatype: i16,
    bitpix: i16,
    slope: f32,
    inter: f32,
    intent: i16,
    description: &'static [u8],
}

/// Builds a NIfTI-1 header (plus the 4-byte extension flag) for a grid with
/// the dims, spacing and patient geometry of `volume`.
fn header_bytes(volume: &Volume, enc: &Encoding) -> Vec<u8> {
    let mut hdr = vec![0u8; HEADER_SIZE + 4];
    let put_i16 = |h: &mut Vec<u8>, off: usize, v: i16| h[off..off + 2].copy_from_slice(&v.to_le_bytes());
    let put_i32 = |h: &mut Vec<u8>, off: usize, v: i32| h[off..off + 4].copy_from_slice(&v.to_le_bytes());
    let put_f32 = |h: &mut Vec<u8>, off: usize, v: f32| h[off..off + 4].copy_from_slice(&v.to_le_bytes());
    let d = volume.dims();
    let s = volume.spacing();
    put_i32(&mut hdr, 0, HEADER_SIZE as i32);
    put_i16(&mut hdr, 40, 3);
    for (i, v) in [d.x, d.y, d.z, 1, 1, 1, 1].iter().enumerate() {
        put_i16(&mut hdr, 42 + 2 * i, *v as i16);
    }
    put_i16(&mut hdr, 68, enc.intent);
    put_i16(&mut hdr, 70, enc.datatype);
    put_i16(&mut hdr, 72, enc.bitpix);
    put_f32(&mut hdr, 76, 1.0);
    for (i, v) in [s.x, s.y, s.z].iter().enumerate() {
        put_f32(&mut hdr, 80 + 4 * i, *v);
    }
    put_f32(&mut hdr, 108, (HEADER_SIZE + 4) as f32);
    put_f32(&mut hdr, 112, enc.slope);
    put_f32(&mut hdr, 116, enc.inter);
    hdr[123] = 2; // xyzt_units: mm
    // sform: the patient geometry of the grid, LPS converted to RAS
    let g = volume.geometry();
    put_i16(&mut hdr, 254, 1);
    let cols = [g.direction.x_axis * s.x, g.direction.y_axis * s.y, g.direction.z_axis * s.z].map(ras_lps);
    let origin = ras_lps(g.origin);
    for r in 0..3 {
        for (c, col) in cols.iter().enumerate() {
            put_f32(&mut hdr, 280 + 16 * r + 4 * c, col[r]);
        }
        put_f32(&mut hdr, 292 + 16 * r, origin[r]);
    }
    hdr[148..148 + enc.description.len()].copy_from_slice(enc.description);
    hdr[344..348].copy_from_slice(b"n+1\0");
    hdr
}

/// Writes header and body, gzip-compressed when `path` ends in `.gz`.
fn write_file(path: &Path, hdr: &[u8], body: &[u8]) -> Result<(), IoError> {
    let f = File::create(path).map_err(|e| IoError::os(path, e))?;
    let mut w: Box<dyn Write> = if path.to_string_lossy().to_ascii_lowercase().ends_with(".gz") {
        Box::new(GzEncoder::new(BufWriter::new(f), flate2::Compression::fast()))
    } else {
        Box::new(BufWriter::new(f))
    };
    w.write_all(hdr).map_err(|e| IoError::os(path, e))?;
    w.write_all(body).map_err(|e| IoError::os(path, e))?;
    w.flush().map_err(|e| IoError::os(path, e))?;
    Ok(())
}

/// Writes `volume` as NIfTI-1 (`uint16` + scaling so physical values are
/// preserved) with its patient geometry in the sform. Gzip compression is
/// used when `path` ends in `.gz`.
pub fn write_nifti(volume: &Volume, path: &Path) -> Result<(), IoError> {
    let r = volume.range();
    let enc = Encoding {
        datatype: 512,
        bitpix: 16,
        slope: r.span() / f32::from(u16::MAX),
        inter: r.min,
        intent: 0,
        description: b"FERRUM export",
    };
    let body: Vec<u8> = volume.data().iter().flat_map(|v| v.to_le_bytes()).collect();
    write_file(path, &header_bytes(volume, &enc), &body)
}

/// Writes `labels` (on the grid of `volume`) as a `uint8` NIfTI-1 label map
/// (`intent_code` = label) with the volume's geometry, so other tools place
/// it on the original image.
pub fn write_label_nifti(labels: &LabelMap, volume: &Volume, path: &Path) -> Result<(), IoError> {
    if labels.dims() != volume.dims() {
        return Err(IoError::Inconsistent("label map and volume grids differ".into()));
    }
    let enc = Encoding { datatype: 2, bitpix: 8, slope: 0.0, inter: 0.0, intent: 1002, description: b"FERRUM labels" };
    write_file(path, &header_bytes(volume, &enc), labels.data())
}

/// NIfTI data source: every file is a separate series.
#[derive(Debug, Default, Clone, Copy)]
pub struct NiftiRepository;

impl VolumeRepository for NiftiRepository {
    fn name(&self) -> &str {
        "NIfTI"
    }

    fn scan(&self, paths: &[PathBuf], progress: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        let files: Vec<PathBuf> = collect_files(paths).into_iter().filter(|f| detect(f) == FileKind::Nifti).collect();
        let total = files.len().max(1);
        let out: Vec<SeriesDescriptor> = files
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                progress.report((i + 1) as f32 / total as f32, "Reading NIfTI headers");
                let mut head = vec![0u8; HEADER_SIZE];
                let ok = if f.to_string_lossy().to_ascii_lowercase().ends_with(".gz") {
                    File::open(f).ok().and_then(|x| GzDecoder::new(x).read_exact(&mut head).ok())
                } else {
                    File::open(f).ok().and_then(|mut x| x.read_exact(&mut head).ok())
                };
                ok?;
                let h = NiftiHeader::parse(&head, f).ok()?;
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                Some(SeriesDescriptor {
                    id: f.to_string_lossy().into_owned(),
                    format: "NIfTI".into(),
                    description: format!("{name} — {}×{}×{}", h.dims.x, h.dims.y, h.dims.z),
                    modality: String::new(),
                    dims: h.dims,
                    sources: vec![f.clone()],
                })
            })
            .collect();
        if out.is_empty() {
            Err(RepositoryError::NothingFound)
        } else {
            Ok(out)
        }
    }

    fn load(&self, series: &SeriesDescriptor, progress: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        let path = series.sources.first().ok_or(RepositoryError::NothingFound)?;
        progress.report(0.1, "Reading NIfTI");
        let (volume, h) = read_nifti(path)?;
        progress.report(1.0, "Volume loaded");
        let attributes = vec![
            ("Format".into(), "NIfTI-1".into()),
            ("Datatype".into(), h.datatype.to_string()),
            ("Description".into(), h.description.clone()),
        ];
        Ok(LoadedSeries {
            volume,
            metadata: SeriesMetadata {
                modality: String::new(),
                // header descriptions are often tool versions; the file name is more useful
                description: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(h.description),
                default_window: None,
                attributes,
                study: ferrum_domain::StudyInfo::default(),
                source: path.clone(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::NoProgress;

    fn sample_volume() -> Volume {
        let dims = Dims3::new(5, 4, 3);
        let vals: Vec<f32> = (0..dims.voxel_count()).map(|i| i as f32 * 10.0 - 200.0).collect();
        Volume::from_physical(dims, Vec3::new(0.5, 0.75, 2.0), &vals).unwrap()
    }

    #[test]
    fn roundtrip_plain_and_gz() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["v.nii", "v.nii.gz"] {
            let p = dir.path().join(name);
            let v = sample_volume();
            write_nifti(&v, &p).unwrap();
            let (back, h) = read_nifti(&p).unwrap();
            assert_eq!(h.dims, v.dims());
            assert_eq!(back.spacing(), v.spacing());
            for idx in [0usize, 7, 59] {
                let (i, j, k) = ((idx % 5) as u32, ((idx / 5) % 4) as u32, (idx / 20) as u32);
                let a = v.physical(i, j, k).unwrap();
                let b = back.physical(i, j, k).unwrap();
                assert!((a - b).abs() < 0.05, "{a} vs {b}");
            }
        }
    }

    #[test]
    fn repository_scans_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        write_nifti(&sample_volume(), &dir.path().join("a.nii.gz")).unwrap();
        let repo = NiftiRepository;
        let s = repo.scan(&[dir.path().to_path_buf()], &NoProgress).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].dims, Dims3::new(5, 4, 3));
        let l = repo.load(&s[0], &NoProgress).unwrap();
        assert_eq!(l.volume.dims(), Dims3::new(5, 4, 3));
    }

    #[test]
    fn reorientation_from_typical_ras_affine_flips_anterior_axis() {
        // i -> -x (left), j -> +y (anterior), k -> +z: j must be flipped.
        let r = Reorientation::from_ras_axes([Vec3::new(-1.0, 0.0, 0.0), Vec3::Y, Vec3::Z]);
        assert_eq!(r.target, [0, 1, 2]);
        assert_eq!(r.flip, [false, true, false]);
        let vals: Vec<f32> = (0..8).map(|v| v as f32).collect();
        let (out, d, _) = r.apply(&vals, Dims3::new(2, 2, 2), Vec3::ONE);
        assert_eq!(d, Dims3::new(2, 2, 2));
        assert_eq!(&out[0..4], &[2.0, 3.0, 0.0, 1.0]);
    }

    #[test]
    fn reorientation_permutes_sagittal_storage() {
        // stored axes: i -> superior, j -> posterior (RAS -y), k -> left (RAS -x)
        let r = Reorientation::from_ras_axes([Vec3::Z, Vec3::NEG_Y, Vec3::NEG_X]);
        assert_eq!(r.target, [2, 1, 0]);
        assert_eq!(r.flip, [false; 3]);
        let dims = Dims3::new(4, 3, 2);
        let vals: Vec<f32> = (0..dims.voxel_count()).map(|v| v as f32).collect();
        let (out, d, sp) = r.apply(&vals, dims, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(d, Dims3::new(2, 3, 4));
        assert_eq!(sp, Vec3::new(3.0, 2.0, 1.0));
        // canonical (x=1, y=2, z=3) comes from stored (i=3, j=2, k=1)
        assert_eq!(out[d.index(1, 2, 3)], vals[dims.index(3, 2, 1)]);
    }

    #[test]
    fn written_files_carry_identity_orientation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("o.nii");
        write_nifti(&sample_volume(), &p).unwrap();
        let (_, h) = read_nifti(&p).unwrap();
        assert_eq!(Reorientation::from_ras_axes(h.axes.unwrap()), Reorientation::IDENTITY);
    }

    #[test]
    fn qform_quaternion_is_decoded() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("q.nii");
        write_nifti(&sample_volume(), &p).unwrap();
        let mut bytes = std::fs::read(&p).unwrap();
        bytes[254..256].copy_from_slice(&0i16.to_le_bytes()); // no sform
        bytes[252..254].copy_from_slice(&1i16.to_le_bytes()); // qform
        for (off, v) in [(256, 0.0f32), (260, 1.0), (264, 0.0), (76, -1.0)] {
            bytes[off..off + 4].copy_from_slice(&v.to_le_bytes());
        }
        let h = NiftiHeader::parse(&bytes, &p).unwrap();
        let ax = h.axes.unwrap().map(Vec3::normalize);
        assert!((ax[0] - Vec3::NEG_X).length() < 1e-6);
        assert!((ax[1] - Vec3::Y).length() < 1e-6);
        assert!((ax[2] - Vec3::Z).length() < 1e-6);
    }

    fn coronal_volume() -> Volume {
        // DICOM-like coronal stack: i → left, j → inferior, k → posterior
        let g = Geometry { origin: Vec3::new(-50.0, -20.0, 80.0), direction: Mat3::from_cols(Vec3::X, Vec3::NEG_Z, Vec3::Y) };
        sample_volume().with_geometry(g)
    }

    #[test]
    fn geometry_roundtrips_through_the_sform() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("g.nii");
        let g = Geometry { origin: Vec3::new(-120.5, 30.0, -7.25), direction: Mat3::IDENTITY };
        let v = sample_volume().with_geometry(g);
        write_nifti(&v, &p).unwrap();
        let (back, _) = read_nifti(&p).unwrap();
        assert_eq!(back.geometry(), g);
        // an oblique-free permuted grid comes back reoriented to LPS but in the same place
        let v = coronal_volume();
        write_nifti(&v, &p).unwrap();
        let (back, _) = read_nifti(&p).unwrap();
        assert_eq!(back.dims(), Dims3::new(5, 3, 4));
        assert!((back.geometry().direction - Mat3::IDENTITY).abs_diff_eq(Mat3::ZERO, 1e-6));
        // canonical (i, j, k) = source (i, 3 - k, j): both sit at the same patient point
        let (i, j, k) = (3.0, 1.0, 2.0);
        let a = back.voxel_to_patient(Vec3::new(i, j, k));
        let b = v.voxel_to_patient(Vec3::new(i, 3.0 - k, j));
        assert!((a - b).length() < 1e-4, "{a} vs {b}");
    }

    #[test]
    fn ras_files_get_their_lps_origin() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ras.nii");
        write_nifti(&sample_volume(), &p).unwrap();
        let mut bytes = std::fs::read(&p).unwrap();
        // typical tool output: i → -x (left), j → +y (anterior), k → +z, origin in RAS
        let put = |b: &mut Vec<u8>, off: usize, v: f32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
        for (off, v) in [(280, -0.5), (284, 0.0), (288, 0.0), (292, 10.0)] {
            put(&mut bytes, off, v);
        }
        for (off, v) in [(296, 0.0), (300, 0.75), (304, 0.0), (308, -20.0)] {
            put(&mut bytes, off, v);
        }
        for (off, v) in [(312, 0.0), (316, 0.0), (320, 2.0), (324, 5.0)] {
            put(&mut bytes, off, v);
        }
        std::fs::write(&p, &bytes).unwrap();
        let (v, h) = read_nifti(&p).unwrap();
        assert_eq!(h.origin, Vec3::new(10.0, -20.0, 5.0));
        // canonical voxel 0 is stored voxel (0, 3, 0): RAS (10, -20 + 2.25, 5)
        assert_eq!(v.geometry().origin, Vec3::new(-10.0, 17.75, 5.0));
        assert_eq!(v.geometry().direction, Mat3::IDENTITY);
    }

    #[test]
    fn label_maps_roundtrip_on_the_volume_grid() {
        let dir = tempfile::tempdir().unwrap();
        for v in [sample_volume(), coronal_volume()] {
            let p = dir.path().join("labels.nii.gz");
            let data: Vec<u8> = (0..v.dims().voxel_count()).map(|i| (i % 4) as u8).collect();
            let labels = LabelMap::from_data(v.dims(), data).unwrap();
            write_label_nifti(&labels, &v, &p).unwrap();
            assert_eq!(read_label_nifti(&p, &v).unwrap(), labels);
            let other = Volume::from_physical(Dims3::new(2, 2, 2), Vec3::ONE, &[0.0; 8]).unwrap();
            assert!(read_label_nifti(&p, &other).is_err());
            assert!(write_label_nifti(&labels, &other, &p).is_err());
        }
    }

    #[test]
    fn label_maps_must_hold_small_integers() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("l.nii");
        let v = sample_volume();
        write_label_nifti(&LabelMap::from_data(v.dims(), vec![1; 60]).unwrap(), &v, &p).unwrap();
        let mut bytes = std::fs::read(&p).unwrap();
        bytes[112..116].copy_from_slice(&0.5f32.to_le_bytes()); // scl_slope
        std::fs::write(&p, &bytes).unwrap();
        let err = read_label_nifti(&p, &v).unwrap_err().to_string();
        assert!(err.contains("0.5"), "{err}");
    }

    #[test]
    fn rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.nii");
        std::fs::write(&p, vec![1u8; 400]).unwrap();
        assert!(read_nifti(&p).is_err());
        let short = dir.path().join("short.nii");
        std::fs::write(&short, vec![0u8; 10]).unwrap();
        assert!(read_nifti(&short).is_err());
    }
}
