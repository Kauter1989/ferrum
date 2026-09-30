//! Resampling of volumes that exceed GPU texture limits or memory budgets.

use glam::{UVec3, Vec3};
use mri_domain::{Dims3, Volume, VolumeError};
use rayon::prelude::*;

/// Integer box-downsampling factors per axis so that every dimension is at
/// most `max_dim` and the voxel count is at most `max_voxels`.
pub fn fit_factors(dims: Dims3, max_dim: u32, max_voxels: usize) -> UVec3 {
    let max_dim = max_dim.max(1);
    let mut f =
        UVec3::new(dims.x.div_ceil(max_dim).max(1), dims.y.div_ceil(max_dim).max(1), dims.z.div_ceil(max_dim).max(1));
    let count =
        |f: UVec3| dims.x.div_ceil(f.x) as usize * dims.y.div_ceil(f.y) as usize * dims.z.div_ceil(f.z) as usize;
    while count(f) > max_voxels.max(1) {
        // grow the factor of the axis with the most remaining samples
        let r = UVec3::new(dims.x.div_ceil(f.x), dims.y.div_ceil(f.y), dims.z.div_ceil(f.z));
        if r.x >= r.y && r.x >= r.z {
            f.x += 1;
        } else if r.y >= r.z {
            f.y += 1;
        } else {
            f.z += 1;
        }
    }
    f
}

/// Box-filter downsampling by integer `factors`. Spacing is scaled so that
/// the physical extent of the covered region is preserved.
pub fn downsample(volume: &Volume, factors: UVec3) -> Result<Volume, VolumeError> {
    let f = factors.max(UVec3::ONE);
    if f == UVec3::ONE {
        return Ok(volume.clone());
    }
    let src = volume.dims();
    let dst = Dims3::new(src.x.div_ceil(f.x), src.y.div_ceil(f.y), src.z.div_ceil(f.z));
    let data = volume.data();
    let out: Vec<u16> = (0..dst.voxel_count())
        .into_par_iter()
        .map(|idx| {
            let i = (idx % dst.x as usize) as u32;
            let j = ((idx / dst.x as usize) % dst.y as usize) as u32;
            let k = (idx / dst.slice_len()) as u32;
            let lo = UVec3::new(i, j, k) * f;
            let hi = (lo + f).min(src.as_uvec3());
            let mut sum = 0u64;
            for z in lo.z..hi.z {
                for y in lo.y..hi.y {
                    let row = src.index(lo.x, y, z);
                    sum += data[row..row + (hi.x - lo.x) as usize].iter().map(|&v| u64::from(v)).sum::<u64>();
                }
            }
            let n = u64::from((hi.x - lo.x) * (hi.y - lo.y) * (hi.z - lo.z));
            ((sum + n / 2) / n) as u16
        })
        .collect();
    let spacing = volume.physical_size() / dst.as_vec3();
    Volume::new(dst, spacing.max(Vec3::splat(f32::MIN_POSITIVE)), volume.range(), out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mri_domain::IntensityRange;

    #[test]
    fn factors_respect_limits() {
        let d = Dims3::new(512, 512, 900);
        let f = fit_factors(d, 512, usize::MAX);
        assert_eq!(f, UVec3::new(1, 1, 2));
        let f = fit_factors(d, 2048, 512 * 512 * 100);
        let n = d.x.div_ceil(f.x) as usize * d.y.div_ceil(f.y) as usize * d.z.div_ceil(f.z) as usize;
        assert!(n <= 512 * 512 * 100);
        assert_eq!(fit_factors(Dims3::new(10, 10, 10), 100, 10_000), UVec3::ONE);
    }

    #[test]
    fn downsample_averages_blocks_and_keeps_extent() {
        let dims = Dims3::new(4, 2, 2);
        let data: Vec<u16> = (0..16).map(|i| i as u16 * 100).collect();
        let v = Volume::new(dims, Vec3::new(1.0, 2.0, 3.0), IntensityRange::new(0.0, 1.0).unwrap(), data).unwrap();
        let d = downsample(&v, UVec3::new(2, 2, 2)).unwrap();
        assert_eq!(d.dims(), Dims3::new(2, 1, 1));
        // block 0: indices 0,1,4,5,8,9,12,13 -> mean 6.5*100
        assert_eq!(d.data()[0], 650);
        assert_eq!(d.physical_size(), v.physical_size());
    }

    #[test]
    fn identity_factor_is_clone() {
        let v = Volume::from_physical(Dims3::new(2, 2, 2), Vec3::ONE, &[0.0; 8]).unwrap();
        assert_eq!(downsample(&v, UVec3::ONE).unwrap(), v);
    }
}
