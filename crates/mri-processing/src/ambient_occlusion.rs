//! Volumetric ambient occlusion ("obscurance") volume.
//!
//! Instead of tracing occlusion rays per shaded point (expensive, noisy) the
//! occlusion is precomputed once per threshold as the local density of
//! "solid" material around each point:
//!
//! 1. The volume is thresholded and downsampled to at most `max_dim` voxels
//!    per axis, storing the solid fraction per cell.
//! 2. A separable box filter of radius `radius` cells averages the solid
//!    fraction in the neighbourhood — `O(N)` regardless of radius.
//! 3. A point on a flat surface sees 50 % solid neighbourhood, a point in a
//!    crevice more; this excess is converted into darkening.
//!
//! The renderer samples the result with trilinear filtering and multiplies
//! the ambient/diffuse lighting by it.

use glam::Vec3;
use mri_domain::{Dims3, Volume};
use rayon::prelude::*;

/// Parameters of the ambient occlusion computation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AoParams {
    /// Normalised iso threshold separating solid from empty.
    pub threshold: f32,
    /// Neighbourhood radius in AO cells.
    pub radius: u32,
    /// Darkening strength, `1.0` = full.
    pub strength: f32,
    /// Maximum AO grid resolution per axis.
    pub max_dim: u32,
}

impl Default for AoParams {
    fn default() -> Self {
        Self { threshold: 0.46, radius: 3, strength: 1.0, max_dim: 128 }
    }
}

/// Ambient occlusion factors in `[0, 255]` (`255` = unoccluded).
#[derive(Debug, Clone, PartialEq)]
pub struct AmbientOcclusion {
    /// AO grid dimensions.
    pub dims: Dims3,
    /// Factor applied to source texture coordinates to obtain AO texture
    /// coordinates (accounts for the partial last cell).
    pub tex_scale: Vec3,
    /// Occlusion factors, `x` fastest.
    pub data: Vec<u8>,
}

impl AmbientOcclusion {
    /// Computes the AO volume.
    pub fn compute(volume: &Volume, params: AoParams) -> Self {
        let src = volume.dims();
        let factor = src.x.max(src.y).max(src.z).div_ceil(params.max_dim.max(1)).max(1);
        let dims = Dims3::new(src.x.div_ceil(factor), src.y.div_ceil(factor), src.z.div_ceil(factor));
        let tex_scale = src.as_vec3() / (dims.as_vec3() * factor as f32);
        let threshold = (params.threshold.clamp(0.0, 1.0) * f32::from(u16::MAX)) as u16;

        // 1. solid fraction per cell
        let data = volume.data();
        let mut solid: Vec<f32> = (0..dims.voxel_count())
            .into_par_iter()
            .map(|idx| {
                let ci = (idx % dims.x as usize) as u32;
                let cj = ((idx / dims.x as usize) % dims.y as usize) as u32;
                let ck = (idx / dims.slice_len()) as u32;
                let (x0, y0, z0) = (ci * factor, cj * factor, ck * factor);
                let (x1, y1, z1) = ((x0 + factor).min(src.x), (y0 + factor).min(src.y), (z0 + factor).min(src.z));
                let mut hits = 0u32;
                for k in z0..z1 {
                    for j in y0..y1 {
                        let row = src.index(0, j, k);
                        hits += data[row + x0 as usize..row + x1 as usize].iter().filter(|&&v| v > threshold).count()
                            as u32;
                    }
                }
                hits as f32 / ((x1 - x0) * (y1 - y0) * (z1 - z0)) as f32
            })
            .collect();

        // 2. separable box blur
        let r = params.radius as usize;
        for axis in 0..3 {
            solid = box_blur_axis(&solid, dims, axis, r);
        }

        // 3. occlusion factor
        let strength = params.strength.max(0.0);
        let data = solid
            .par_iter()
            .map(|&occ| {
                let ao = 1.0 - strength * ((occ - 0.5) * 2.0).max(0.0);
                mri_domain::color::unit_to_u8(ao)
            })
            .collect();
        Self { dims, tex_scale, data }
    }
}

/// One pass of a box filter of radius `r` along `axis` with clamped borders.
fn box_blur_axis(src: &[f32], dims: Dims3, axis: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return src.to_vec();
    }
    let d = [dims.x as usize, dims.y as usize, dims.z as usize];
    let stride = [1, d[0], d[0] * d[1]][axis];
    let len = d[axis];
    // Line starts: all positions with coordinate 0 along `axis`.
    let (a, b) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let strides = [1, d[0], d[0] * d[1]];
    let starts: Vec<usize> = (0..d[b]).flat_map(|q| (0..d[a]).map(move |p| p * strides[a] + q * strides[b])).collect();
    let mut out = vec![0.0f32; src.len()];
    let results: Vec<(usize, Vec<f32>)> = starts
        .par_iter()
        .map(|&start| {
            let line: Vec<f32> = (0..len).map(|p| src[start + p * stride]).collect();
            // prefix sums for O(1) window averages
            let mut prefix = vec![0.0f64; len + 1];
            for (p, &v) in line.iter().enumerate() {
                prefix[p + 1] = prefix[p] + f64::from(v);
            }
            let blurred = (0..len)
                .map(|p| {
                    let lo = p.saturating_sub(r);
                    let hi = (p + r + 1).min(len);
                    ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32
                })
                .collect();
            (start, blurred)
        })
        .collect();
    for (start, line) in results {
        for (p, v) in line.into_iter().enumerate() {
            out[start + p * stride] = v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mri_domain::IntensityRange;

    fn half_space_volume(n: u32) -> Volume {
        // solid for z >= n/2
        let dims = Dims3::new(n, n, n);
        let data = (0..dims.voxel_count())
            .map(|i| if (i / dims.slice_len()) as u32 >= n / 2 { u16::MAX } else { 0 })
            .collect();
        Volume::new(dims, Vec3::ONE, IntensityRange::new(0.0, 1.0).unwrap(), data).unwrap()
    }

    #[test]
    fn empty_space_is_unoccluded_and_deep_solid_is_dark() {
        let v = half_space_volume(32);
        let ao = AmbientOcclusion::compute(&v, AoParams { threshold: 0.5, radius: 2, strength: 1.0, max_dim: 32 });
        assert_eq!(ao.dims, Dims3::new(32, 32, 32));
        assert_eq!(ao.data[ao.dims.index(16, 16, 2)], 255);
        assert_eq!(ao.data[ao.dims.index(16, 16, 30)], 0);
    }

    #[test]
    fn flat_surface_is_nearly_unoccluded() {
        let v = half_space_volume(32);
        let ao = AmbientOcclusion::compute(&v, AoParams { threshold: 0.5, radius: 3, strength: 1.0, max_dim: 32 });
        // cell just above the surface sees < 50 % solid
        assert!(ao.data[ao.dims.index(16, 16, 15)] > 200);
    }

    #[test]
    fn downsampling_respects_max_dim() {
        let v = half_space_volume(40);
        let ao = AmbientOcclusion::compute(&v, AoParams { max_dim: 16, ..AoParams::default() });
        assert_eq!(ao.dims, Dims3::new(14, 14, 14));
        assert!(ao.tex_scale.cmple(Vec3::ONE).all());
        assert_eq!(ao.data.len(), ao.dims.voxel_count());
    }

    #[test]
    fn box_blur_preserves_constant_field() {
        let dims = Dims3::new(5, 4, 3);
        let src = vec![0.25f32; dims.voxel_count()];
        for axis in 0..3 {
            assert!(box_blur_axis(&src, dims, axis, 2).iter().all(|&v| (v - 0.25).abs() < 1e-6));
        }
    }

    #[test]
    fn box_blur_averages_along_axis_only() {
        let dims = Dims3::new(3, 2, 1);
        let src = vec![0.0, 3.0, 0.0, 9.0, 9.0, 9.0];
        let out = box_blur_axis(&src, dims, 0, 1);
        assert_eq!(out, vec![1.5, 1.0, 1.5, 9.0, 9.0, 9.0]);
    }
}
