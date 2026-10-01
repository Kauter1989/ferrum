//! Minimal NIfTI-1 reader and writer (`.nii`, `.nii.gz`).

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use ferrum_domain::{
    Dims3, IntensityRange, LoadedSeries, ProgressSink, RepositoryError, SeriesDescriptor, SeriesMetadata, Volume,
    VolumeRepository,
};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use glam::Vec3;
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
    /// Direction of each voxel axis (`i`, `j`, `k`) in RAS world space, from
    /// `sform` (preferred) or `qform`; `None` if the file has neither.
    pub axes: Option<[Vec3; 3]>,
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
        let lps = axes.map(|d| Vec3::new(-d.x, -d.y, d.z));
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
    pub fn apply(&self, values: &[f32], dims: Dims3, spacing: Vec3) -> (Vec<f32>, Dims3, Vec3) {
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
        let mut out = vec![0f32; values.len()];
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
        let axes = if sform_code > 0 {
            let row = |r: usize| Vec3::new(c.f32(280 + 16 * r), c.f32(284 + 16 * r), c.f32(288 + 16 * r));
            let (r0, r1, r2) = (row(0), row(1), row(2));
            Some([Vec3::new(r0.x, r1.x, r2.x), Vec3::new(r0.y, r1.y, r2.y), Vec3::new(r0.z, r1.z, r2.z)])
        } else if qform_code > 0 {
            let (b, cq, d) = (c.f32(256), c.f32(260), c.f32(264));
            let a = (1.0 - (b * b + cq * cq + d * d)).max(0.0).sqrt();
            let qfac = if c.f32(76) < 0.0 { -1.0 } else { 1.0 };
            Some([
                Vec3::new(a * a + b * b - cq * cq - d * d, 2.0 * (b * cq + a * d), 2.0 * (b * d - a * cq)),
                Vec3::new(2.0 * (b * cq - a * d), a * a + cq * cq - b * b - d * d, 2.0 * (cq * d + a * b)),
                Vec3::new(2.0 * (b * d + a * cq), 2.0 * (cq * d - a * b), a * a + d * d - cq * cq - b * b) * qfac,
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

/// Reads a NIfTI-1 file into a volume (first 3D volume if 4D), reoriented
/// into the canonical LPS frame when the header carries an orientation.
pub fn read_nifti(path: &Path) -> Result<(Volume, NiftiHeader), IoError> {
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
    let values: Vec<f32> = raw
        .par_chunks_exact(bpv)
        .map(|b| {
            let v = match (dt, le) {
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
            };
            v as f32 * slope + inter
        })
        .collect();
    let orientation = h.axes.map(Reorientation::from_ras_axes).unwrap_or(Reorientation::IDENTITY);
    let (values, dims, spacing) = orientation.apply(&values, h.dims, h.spacing);
    Ok((volume_from_physical_par(dims, spacing, &values)?, h))
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

/// Writes `volume` as NIfTI-1 (`uint16` + scaling so physical values are
/// preserved). Gzip compression is used when `path` ends in `.gz`.
pub fn write_nifti(volume: &Volume, path: &Path) -> Result<(), IoError> {
    let mut hdr = vec![0u8; HEADER_SIZE + 4];
    let put_i16 = |h: &mut Vec<u8>, off: usize, v: i16| h[off..off + 2].copy_from_slice(&v.to_le_bytes());
    let put_i32 = |h: &mut Vec<u8>, off: usize, v: i32| h[off..off + 4].copy_from_slice(&v.to_le_bytes());
    let put_f32 = |h: &mut Vec<u8>, off: usize, v: f32| h[off..off + 4].copy_from_slice(&v.to_le_bytes());
    let d = volume.dims();
    let s = volume.spacing();
    let r = volume.range();
    put_i32(&mut hdr, 0, HEADER_SIZE as i32);
    put_i16(&mut hdr, 40, 3);
    for (i, v) in [d.x, d.y, d.z, 1, 1, 1, 1].iter().enumerate() {
        put_i16(&mut hdr, 42 + 2 * i, *v as i16);
    }
    put_i16(&mut hdr, 70, 512); // uint16
    put_i16(&mut hdr, 72, 16); // bitpix
    put_f32(&mut hdr, 76, 1.0);
    for (i, v) in [s.x, s.y, s.z].iter().enumerate() {
        put_f32(&mut hdr, 80 + 4 * i, *v);
    }
    put_f32(&mut hdr, 108, (HEADER_SIZE + 4) as f32);
    put_f32(&mut hdr, 112, r.span() / f32::from(u16::MAX));
    put_f32(&mut hdr, 116, r.min);
    hdr[123] = 2; // xyzt_units: mm
                  // sform: canonical LPS voxel axes expressed in RAS (x and y negated)
    put_i16(&mut hdr, 254, 1);
    put_f32(&mut hdr, 280, -s.x);
    put_f32(&mut hdr, 300, -s.y);
    put_f32(&mut hdr, 320, s.z);
    let desc = b"FERRUM export";
    hdr[148..148 + desc.len()].copy_from_slice(desc);
    hdr[344..348].copy_from_slice(b"n+1\0");

    let f = File::create(path).map_err(|e| IoError::os(path, e))?;
    let mut w: Box<dyn Write> = if path.to_string_lossy().to_ascii_lowercase().ends_with(".gz") {
        Box::new(GzEncoder::new(BufWriter::new(f), flate2::Compression::fast()))
    } else {
        Box::new(BufWriter::new(f))
    };
    w.write_all(&hdr).map_err(|e| IoError::os(path, e))?;
    let body: Vec<u8> = volume.data().iter().flat_map(|v| v.to_le_bytes()).collect();
    w.write_all(&body).map_err(|e| IoError::os(path, e))?;
    w.flush().map_err(|e| IoError::os(path, e))?;
    Ok(())
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
        let ax = h.axes.unwrap();
        assert!((ax[0] - Vec3::NEG_X).length() < 1e-6);
        assert!((ax[1] - Vec3::Y).length() < 1e-6);
        assert!((ax[2] - Vec3::Z).length() < 1e-6);
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
