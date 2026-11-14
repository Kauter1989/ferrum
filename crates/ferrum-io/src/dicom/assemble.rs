//! Parallel decoding of pixel data and assembly of the volume.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use dicom_pixeldata::PixelDecoder;
use ferrum_domain::{Dims3, Geometry, IntensityRange, ProgressSink, Volume};
use glam::{Mat3, Vec3};
use rayon::prelude::*;

use super::header::{Photometric, SliceHeader};
use super::series::OrderedSeries;
use crate::error::IoError;
use crate::nifti::Reorientation;

/// Decodes the requested `frames` of one file into physical values
/// (Modality LUT applied). Colour images are converted to luminance.
pub fn decode_frames(path: &Path, frames: &[u32]) -> Result<Vec<Vec<f32>>, IoError> {
    let obj = dicom_object::open_file(path).map_err(|e| IoError::parse(path, e))?;
    let decoded = obj.decode_pixel_data().map_err(|e| IoError::decode(path, e))?;
    let spp = usize::from(decoded.samples_per_pixel().max(1));
    frames
        .iter()
        .map(|&f| {
            let raw: Vec<f32> = decoded.to_vec_frame(f).map_err(|e| IoError::decode(path, e))?;
            Ok(if spp == 1 {
                raw
            } else {
                raw.chunks_exact(spp)
                    .map(|px| 0.299 * px[0] + 0.587 * px[1] + 0.114 * px.get(2).copied().unwrap_or(0.0))
                    .collect()
            })
        })
        .collect()
}

/// Builds a volume from ordered planes.
///
/// Decoding runs in parallel over files. Progress is reported per decoded
/// plane; cancellation is checked before each file.
pub fn assemble(
    headers: &[SliceHeader],
    ordered: &OrderedSeries,
    progress: &dyn ProgressSink,
) -> Result<Volume, IoError> {
    let first = headers.first().ok_or_else(|| IoError::Inconsistent("series has no slices".into()))?;
    let (w, h) = (first.columns, first.rows);
    let n = ordered.planes.len();
    if n == 0 {
        return Err(IoError::Inconsistent("series has no image planes".into()));
    }
    let done = AtomicUsize::new(0);
    let tick = |count: usize| {
        let d = done.fetch_add(count, Ordering::Relaxed) + count;
        progress.report(d as f32 / n as f32, "Decoding pixel data");
    };

    // Each file is decoded once, with all of its frames the series uses.
    let mut by_file: BTreeMap<usize, Vec<(usize, u32)>> = BTreeMap::new();
    for (slot, p) in ordered.planes.iter().enumerate() {
        by_file.entry(p.header).or_default().push((slot, p.frame));
    }
    let decoded: Vec<Vec<(usize, Vec<f32>)>> = by_file
        .into_par_iter()
        .map(|(header, wanted)| {
            if progress.is_cancelled() {
                return Err(IoError::Cancelled);
            }
            let path = &headers[header].path;
            let frames: Vec<u32> = wanted.iter().map(|&(_, f)| f).collect();
            let out = decode_frames(path, &frames)?;
            if out.len() != wanted.len() {
                return Err(IoError::invalid(path, "not every frame decoded"));
            }
            tick(wanted.len());
            Ok(wanted.into_iter().map(|(slot, _)| slot).zip(out).collect())
        })
        .collect::<Result<_, _>>()?;
    let mut planes: Vec<Vec<f32>> = vec![Vec::new(); n];
    for (slot, plane) in decoded.into_iter().flatten() {
        planes[slot] = plane;
    }

    let plane_len = w as usize * h as usize;
    if let Some((i, _)) = planes.iter().enumerate().find(|(_, p)| p.len() != plane_len) {
        let path = &headers[ordered.planes[i].header].path;
        return Err(IoError::invalid(path, format!("expected {plane_len} pixels, got {}", planes[i].len())));
    }

    let (min, max) = planes
        .par_iter()
        .flat_map_iter(|p| p.iter().copied())
        .filter(|v| v.is_finite())
        .fold(|| (f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| (a.min(v), b.max(v)))
        .reduce(|| (f32::INFINITY, f32::NEG_INFINITY), |(a, b), (c, d)| (a.min(c), b.max(d)));
    let (min, max) = if min.is_finite() { (min, max) } else { (0.0, 1.0) };
    // CT in HU is a whole number of Hounsfield units: such data are stored exactly
    let range = IntensityRange::for_data(min, max, |min, step| {
        planes.par_iter().all(|p| p.iter().all(|v| IntensityRange::is_on_grid(*v, min, step)))
    })?;
    let invert = first.photometric == Photometric::Monochrome1;

    let dims = Dims3::new(w, h, n as u32);
    let mut data = vec![0u16; dims.voxel_count()];
    data.par_chunks_mut(plane_len).zip(planes.par_iter()).for_each(|(dst, src)| {
        for (d, &s) in dst.iter_mut().zip(src) {
            let v = if invert { min + max - s } else { s };
            *d = range.to_storage(v);
        }
    });

    let px = first.pixel_spacing.unwrap_or(glam::Vec2::ONE);
    let spacing = Vec3::new(px.x, px.y, ordered.slice_spacing.max(1e-3) as f32);
    progress.report(1.0, "Volume assembled");
    let geometry = plane_geometry(&headers[ordered.planes[0].header], ordered.planes[0].frame);
    let (data, dims, spacing, geometry) = to_canonical(data, dims, spacing, geometry);
    Ok(Volume::new(dims, spacing, range, data)?.with_geometry(geometry))
}

/// The stored grid (`i` along the rows, `j` along the columns, `k` along
/// the slice normal) brought into the canonical LPS frame, as the NIfTI
/// reader does: axes are permuted and flipped onto the closest of `+x`
/// left, `+y` posterior, `+z` superior. Every voxel keeps its patient
/// position; an oblique remainder stays in the direction.
pub fn to_canonical(
    data: Vec<u16>,
    dims: Dims3,
    spacing: Vec3,
    geometry: Geometry,
) -> (Vec<u16>, Dims3, Vec3, Geometry) {
    let d = geometry.direction;
    let axes = [d.x_axis, d.y_axis, d.z_axis];
    let r = Reorientation::between(axes, Mat3::IDENTITY);
    if r == Reorientation::IDENTITY {
        return (data, dims, spacing, geometry);
    }
    let src = [dims.x, dims.y, dims.z];
    let mut cols = [Vec3::ZERO; 3];
    let mut origin = geometry.origin;
    for a in 0..3 {
        cols[r.target[a]] = if r.flip[a] { -axes[a] } else { axes[a] };
        if r.flip[a] {
            origin += axes[a] * spacing[a] * src[a].saturating_sub(1) as f32;
        }
    }
    let (data, dims, spacing) = r.apply(&data, dims, spacing);
    (data, dims, spacing, Geometry { origin, direction: Mat3::from_cols(cols[0], cols[1], cols[2]) })
}

/// Patient-space placement of a stack whose first plane is frame `frame`
/// of `first`: the origin is its Image Position (per frame for an Enhanced
/// file), the `i`/`j` axes are its row/column
/// directions and `k` is their cross product (planes are ordered along it).
pub fn plane_geometry(first: &SliceHeader, frame: u32) -> Geometry {
    let info = first.frames_info.get(frame as usize);
    let origin = info.and_then(|f| f.position).or(first.position).map_or(Vec3::ZERO, |p| p.as_vec3());
    let orientation = info.and_then(|f| f.orientation).or(first.orientation);
    let direction = orientation.map_or(Mat3::IDENTITY, |(row, col)| {
        let (r, c) = (row.normalize_or_zero().as_vec3(), col.normalize_or_zero().as_vec3());
        Mat3::from_cols(r, c, r.cross(c).normalize_or_zero())
    });
    Geometry { origin, direction }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    /// Every stored voxel keeps its value at its patient position, and the
    /// canonical grid runs along `+x`, `+y`, `+z`.
    fn check_canonical(row: Vec3, col: Vec3) {
        let dims = Dims3::new(3, 4, 5);
        let spacing = Vec3::new(0.5, 0.75, 2.0);
        let geometry = Geometry {
            origin: Vec3::new(-12.0, 30.0, 7.0),
            direction: Mat3::from_cols(row, col, row.cross(col).normalize()),
        };
        let data: Vec<u16> = (0..dims.voxel_count() as u16).collect();
        let (out, odims, osp, og) = to_canonical(data.clone(), dims, spacing, geometry);
        assert_eq!(odims.voxel_count(), dims.voxel_count());
        for (a, axis) in [og.direction.x_axis, og.direction.y_axis, og.direction.z_axis].into_iter().enumerate() {
            assert!(axis[a] > 0.9, "axis {a} of {row} {col}: {axis}");
        }
        let inv = og.direction.inverse();
        for k in 0..dims.z {
            for j in 0..dims.y {
                for i in 0..dims.x {
                    let p = geometry.voxel_to_patient(Vec3::new(i as f32, j as f32, k as f32), spacing);
                    let o = (inv * (p - og.origin)) / osp;
                    let r = o.round();
                    assert!((o - r).abs().max_element() < 1e-3, "{row} {col}: voxel {i},{j},{k} lands at {o}");
                    let idx = odims.index(r.x as u32, r.y as u32, r.z as u32);
                    assert_eq!(out[idx], data[dims.index(i, j, k)], "{row} {col}: voxel {i},{j},{k}");
                }
            }
        }
    }

    #[test]
    fn dicom_grids_are_brought_into_lps() {
        // covers 2.8-a
        let deg = 20f32.to_radians();
        for (row, col) in [
            (Vec3::X, Vec3::Y),                                                // axial: unchanged
            (Vec3::Y, Vec3::NEG_Z),                                            // sagittal, normal −x
            (Vec3::X, Vec3::NEG_Z),                                            // coronal, normal +y
            (Vec3::NEG_X, Vec3::Y),                                            // rows towards the right, normal −z
            (Vec3::new(deg.cos(), deg.sin(), 0.0), Vec3::new(0.0, 0.0, -1.0)), // oblique coronal
        ] {
            check_canonical(row, col);
        }
        // the axial grid is left as it is
        let (_, d, sp, g) = to_canonical(vec![0; 60], Dims3::new(3, 4, 5), Vec3::ONE, Geometry::default());
        assert_eq!((d, sp, g), (Dims3::new(3, 4, 5), Vec3::ONE, Geometry::default()));
    }

    #[test]
    fn a_sagittal_stack_becomes_x_y_z() {
        // covers 2.8-a: 4 sagittal slices of 6 × 5 pixels → 4 × 6 × 5 voxels
        let g = Geometry { origin: Vec3::ZERO, direction: Mat3::from_cols(Vec3::Y, Vec3::NEG_Z, Vec3::NEG_X) };
        let (_, d, sp, og) = to_canonical(vec![0; 120], Dims3::new(6, 5, 4), Vec3::new(0.5, 0.6, 3.0), g);
        assert_eq!(d, Dims3::new(4, 6, 5));
        assert_eq!(sp, Vec3::new(3.0, 0.5, 0.6));
        assert_eq!(og.direction, Mat3::IDENTITY);
        // the last slice (k = 3, x = −9 mm) and the bottom row (j = 4, z = −2.4 mm) become the origin
        assert_eq!(og.origin, Vec3::new(-9.0, 0.0, -2.4));
    }

    #[test]
    fn geometry_comes_from_the_first_plane() {
        let mut h = crate::dicom::series::tests::header("1", Some(-40.0), None);
        h.position = Some(DVec3::new(-120.0, -90.0, -40.0));
        h.orientation = Some((DVec3::X, DVec3::new(0.0, 0.0, -2.0)));
        let g = plane_geometry(&h, 0);
        assert_eq!(g.origin, Vec3::new(-120.0, -90.0, -40.0));
        assert_eq!(g.direction, Mat3::from_cols(Vec3::X, Vec3::NEG_Z, Vec3::Y));
        h.position = None;
        h.orientation = None;
        assert_eq!(plane_geometry(&h, 0), Geometry::default());
    }
}
