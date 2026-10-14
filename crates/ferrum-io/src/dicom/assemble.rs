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
    Ok(Volume::new(dims, spacing, range, data)?.with_geometry(geometry))
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
