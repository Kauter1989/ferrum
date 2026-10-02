//! Parallel decoding of pixel data and assembly of the volume.

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

    let planes: Vec<Vec<f32>> = if headers.len() == 1 {
        if progress.is_cancelled() {
            return Err(IoError::Cancelled);
        }
        let frames: Vec<u32> = ordered.planes.iter().map(|p| p.frame).collect();
        let out = decode_frames(&first.path, &frames)?;
        tick(n);
        out
    } else {
        ordered
            .planes
            .par_iter()
            .map(|p| {
                if progress.is_cancelled() {
                    return Err(IoError::Cancelled);
                }
                let hdr = &headers[p.header];
                let mut v = decode_frames(&hdr.path, &[p.frame])?;
                tick(1);
                v.pop().ok_or_else(|| IoError::invalid(&hdr.path, "no frame decoded"))
            })
            .collect::<Result<_, _>>()?
    };

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
    let range = IntensityRange::new(min, max)?;
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
    let geometry = plane_geometry(&headers[ordered.planes[0].header]);
    Ok(Volume::new(dims, spacing, range, data)?.with_geometry(geometry))
}

/// Patient-space placement of a stack whose first plane is `first`: the
/// origin is its Image Position, the `i`/`j` axes are its row/column
/// directions and `k` is their cross product (planes are ordered along it).
pub fn plane_geometry(first: &SliceHeader) -> Geometry {
    let origin = first.position.map_or(Vec3::ZERO, |p| p.as_vec3());
    let direction = first.orientation.map_or(Mat3::IDENTITY, |(row, col)| {
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
        let g = plane_geometry(&h);
        assert_eq!(g.origin, Vec3::new(-120.0, -90.0, -40.0));
        assert_eq!(g.direction, Mat3::from_cols(Vec3::X, Vec3::NEG_Z, Vec3::Y));
        h.position = None;
        h.orientation = None;
        assert_eq!(plane_geometry(&h), Geometry::default());
    }
}
