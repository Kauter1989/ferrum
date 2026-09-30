//! Volume filters available to the user: Gaussian smoothing (noise
//! reduction before 3D rendering) and Sobel gradient magnitude (edge
//! enhancement).

use mri_domain::{Dims3, Volume, VolumeError};
use rayon::prelude::*;

/// Separable 1D convolution along `axis` with clamped borders.
fn convolve_axis(src: &[f32], dims: Dims3, axis: usize, kernel: &[f32]) -> Vec<f32> {
    let r = (kernel.len() / 2) as i64;
    let d = [dims.x as i64, dims.y as i64, dims.z as i64];
    let stride = [1i64, d[0], d[0] * d[1]][axis];
    let len = d[axis];
    let mut out = vec![0.0f32; src.len()];
    out.par_chunks_mut(dims.slice_len()).enumerate().for_each(|(k, slab)| {
        for (local, o) in slab.iter_mut().enumerate() {
            let idx = (k * dims.slice_len() + local) as i64;
            let coord = [idx % d[0], (idx / d[0]) % d[1], k as i64][axis];
            let mut acc = 0.0;
            for (t, w) in kernel.iter().enumerate() {
                let c = (coord + t as i64 - r).clamp(0, len - 1);
                acc += w * src[(idx + (c - coord) * stride) as usize];
            }
            *o = acc;
        }
    });
    out
}

fn to_f32(volume: &Volume) -> Vec<f32> {
    volume.data().par_iter().map(|&v| f32::from(v)).collect()
}

fn from_f32(volume: &Volume, values: Vec<f32>) -> Result<Volume, VolumeError> {
    let data = values.into_par_iter().map(|v| v.round().clamp(0.0, 65535.0) as u16).collect();
    Volume::new(volume.dims(), volume.spacing(), volume.range(), data)
}

/// Normalised Gaussian kernel with standard deviation `sigma` voxels.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let sigma = sigma.max(1e-3);
    let r = (3.0 * sigma).ceil() as i32;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = k.iter().sum();
    k.into_iter().map(|v| v / s).collect()
}

/// Gaussian smoothing with standard deviation `sigma` voxels.
pub fn gaussian_smooth(volume: &Volume, sigma: f32) -> Result<Volume, VolumeError> {
    let kernel = gaussian_kernel(sigma);
    let mut v = to_f32(volume);
    for axis in 0..3 {
        v = convolve_axis(&v, volume.dims(), axis, &kernel);
    }
    from_f32(volume, v)
}

/// Sobel gradient magnitude, rescaled so that the strongest edge maps to the
/// top of the intensity range. The physical range is preserved so that
/// windowing presets remain meaningful.
pub fn sobel_magnitude(volume: &Volume) -> Result<Volume, VolumeError> {
    let dims = volume.dims();
    let src = to_f32(volume);
    let smooth = [1.0f32, 2.0, 1.0];
    let diff = [-1.0f32, 0.0, 1.0];
    let mut gradients = Vec::with_capacity(3);
    for axis in 0..3 {
        let mut g = src.clone();
        for a in 0..3 {
            g = convolve_axis(&g, dims, a, if a == axis { &diff } else { &smooth });
        }
        gradients.push(g);
    }
    let mag: Vec<f32> = (0..src.len())
        .into_par_iter()
        .map(|i| (gradients[0][i].powi(2) + gradients[1][i].powi(2) + gradients[2][i].powi(2)).sqrt())
        .collect();
    let max = mag.par_iter().copied().reduce(|| 0.0, f32::max).max(1e-6);
    let scaled = mag.into_par_iter().map(|m| m / max * 65535.0).collect();
    from_f32(volume, scaled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use mri_domain::IntensityRange;

    fn vol(dims: Dims3, f: impl Fn(u32, u32, u32) -> u16) -> Volume {
        let mut data = Vec::with_capacity(dims.voxel_count());
        for k in 0..dims.z {
            for j in 0..dims.y {
                for i in 0..dims.x {
                    data.push(f(i, j, k));
                }
            }
        }
        Volume::new(dims, Vec3::ONE, IntensityRange::new(0.0, 100.0).unwrap(), data).unwrap()
    }

    #[test]
    fn kernel_is_normalised_and_symmetric() {
        let k = gaussian_kernel(1.5);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert_eq!(k.len() % 2, 1);
        assert!((k[0] - k[k.len() - 1]).abs() < 1e-7);
    }

    #[test]
    fn smoothing_keeps_constant_volume() {
        let v = vol(Dims3::new(6, 5, 4), |_, _, _| 1234);
        let s = gaussian_smooth(&v, 1.0).unwrap();
        assert_eq!(s.data(), v.data());
        assert_eq!(s.range(), v.range());
    }

    #[test]
    fn smoothing_spreads_an_impulse_and_preserves_mass() {
        let dims = Dims3::new(9, 9, 9);
        let v = vol(dims, |i, j, k| if (i, j, k) == (4, 4, 4) { 60000 } else { 0 });
        let s = gaussian_smooth(&v, 1.0).unwrap();
        let centre = s.data()[dims.index(4, 4, 4)];
        let neighbour = s.data()[dims.index(5, 4, 4)];
        assert!(centre < 60000 && neighbour > 0 && neighbour < centre);
        let mass: u64 = s.data().iter().map(|&x| u64::from(x)).sum();
        assert!((mass as f64 - 60000.0).abs() < 60000.0 * 0.02, "{mass}");
    }

    #[test]
    fn sobel_detects_step_edge() {
        let dims = Dims3::new(8, 4, 4);
        let v = vol(dims, |i, _, _| if i < 4 { 0 } else { 50000 });
        let s = sobel_magnitude(&v).unwrap();
        assert_eq!(s.data()[dims.index(0, 2, 2)], 0);
        assert!(s.data()[dims.index(3, 2, 2)] > 60000);
        assert!(s.data()[dims.index(4, 2, 2)] > 60000);
        assert_eq!(s.data()[dims.index(7, 2, 2)], 0);
    }
}
