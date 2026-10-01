//! Min/max brick grid for empty-space skipping.
//!
//! The volume is partitioned into cubic bricks of `brick_size³` voxels. For
//! each brick the minimum and maximum normalised value is stored, including
//! a one-voxel apron so that trilinear samples taken anywhere inside the
//! brick are bounded by the stored range. When the transfer function (or
//! threshold) maps the whole `[min, max]` range of a brick to zero opacity,
//! the ray marcher may jump over the brick without sampling it.

use ferrum_domain::{Dims3, Volume};
use glam::{UVec3, Vec3};
use rayon::prelude::*;

/// Per-brick value range of a volume.
#[derive(Debug, Clone, PartialEq)]
pub struct BrickGrid {
    brick_size: u32,
    grid: Dims3,
    min: Vec<u16>,
    max: Vec<u16>,
}

impl BrickGrid {
    /// Default brick edge length in voxels.
    pub const DEFAULT_BRICK_SIZE: u32 = 8;

    /// Computes the grid for `volume` with bricks of `brick_size` voxels.
    pub fn compute(volume: &Volume, brick_size: u32) -> Self {
        let b = brick_size.max(1);
        let d = volume.dims();
        let grid = Dims3::new(d.x.div_ceil(b), d.y.div_ceil(b), d.z.div_ceil(b));
        let data = volume.data();
        let ranges: Vec<(u16, u16)> = (0..grid.voxel_count())
            .into_par_iter()
            .map(|idx| {
                let gi = (idx % grid.x as usize) as u32;
                let gj = ((idx / grid.x as usize) % grid.y as usize) as u32;
                let gk = (idx / grid.slice_len()) as u32;
                let lo = UVec3::new(gi, gj, gk) * b;
                let lo = lo.saturating_sub(UVec3::ONE);
                let hi = ((UVec3::new(gi, gj, gk) + UVec3::ONE) * b).min(d.as_uvec3() - UVec3::ONE);
                let (mut mn, mut mx) = (u16::MAX, u16::MIN);
                for k in lo.z..=hi.z {
                    for j in lo.y..=hi.y {
                        let row = d.index(lo.x, j, k);
                        for &v in &data[row..=row + (hi.x - lo.x) as usize] {
                            mn = mn.min(v);
                            mx = mx.max(v);
                        }
                    }
                }
                (mn, mx)
            })
            .collect();
        let (min, max) = ranges.into_iter().unzip();
        Self { brick_size: b, grid, min, max }
    }

    /// Brick edge length in voxels.
    pub fn brick_size(&self) -> u32 {
        self.brick_size
    }

    /// Number of bricks along each axis.
    pub fn grid_dims(&self) -> Dims3 {
        self.grid
    }

    /// Normalised `(min, max)` of brick `(i, j, k)`.
    pub fn range(&self, i: u32, j: u32, k: u32) -> (f32, f32) {
        let idx = self.grid.index(i, j, k);
        let s = f32::from(u16::MAX);
        (f32::from(self.min[idx]) / s, f32::from(self.max[idx]) / s)
    }

    /// Brick containing texture coordinate `t` of a volume of `dims`.
    pub fn brick_of(&self, t: Vec3, dims: Dims3) -> UVec3 {
        let v = (t * dims.as_vec3()).floor().max(Vec3::ZERO).as_uvec3();
        (v / self.brick_size).min(self.grid.as_uvec3() - UVec3::ONE)
    }

    /// Classifies every brick with `visible(min, max)` and returns an
    /// occupancy map (`255` = must be sampled, `0` = empty) in grid order.
    pub fn occupancy<F>(&self, visible: F) -> Vec<u8>
    where
        F: Fn(f32, f32) -> bool + Sync,
    {
        let s = f32::from(u16::MAX);
        self.min
            .par_iter()
            .zip(self.max.par_iter())
            .map(|(&mn, &mx)| if visible(f32::from(mn) / s, f32::from(mx) / s) { 255 } else { 0 })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::IntensityRange;

    fn volume_with_hot_voxel(n: u32, hot: UVec3) -> Volume {
        let dims = Dims3::new(n, n, n);
        let mut data = vec![0u16; dims.voxel_count()];
        data[dims.index(hot.x, hot.y, hot.z)] = u16::MAX;
        Volume::new(dims, Vec3::ONE, IntensityRange::new(0.0, 1.0).unwrap(), data).unwrap()
    }

    #[test]
    fn grid_dims_round_up() {
        let v = volume_with_hot_voxel(17, UVec3::ZERO);
        let g = BrickGrid::compute(&v, 8);
        assert_eq!(g.grid_dims(), Dims3::new(3, 3, 3));
    }

    #[test]
    fn hot_voxel_marks_its_brick_and_apron_neighbours() {
        let v = volume_with_hot_voxel(16, UVec3::new(8, 3, 3));
        let g = BrickGrid::compute(&v, 8);
        // Voxel 8 belongs to brick 1 and lies in the apron of brick 0.
        assert_eq!(g.range(1, 0, 0).1, 1.0);
        assert_eq!(g.range(0, 0, 0).1, 1.0);
        assert_eq!(g.range(0, 1, 0).1, 0.0);
        assert_eq!(g.range(1, 1, 1).1, 0.0);
        let occ = g.occupancy(|_, mx| mx > 0.5);
        assert_eq!(occ.iter().filter(|&&o| o == 255).count(), 2);
    }

    #[test]
    fn brick_of_clamps() {
        let v = volume_with_hot_voxel(16, UVec3::ZERO);
        let g = BrickGrid::compute(&v, 8);
        let d = v.dims();
        assert_eq!(g.brick_of(Vec3::ZERO, d), UVec3::ZERO);
        assert_eq!(g.brick_of(Vec3::splat(0.99), d), UVec3::ONE);
        assert_eq!(g.brick_of(Vec3::splat(1.5), d), UVec3::ONE);
        assert_eq!(g.brick_of(Vec3::splat(-0.5), d), UVec3::ZERO);
    }

    #[test]
    fn trilinear_samples_are_bounded_by_brick_range() {
        // Random-ish field; check that samples inside each brick are within range.
        let dims = Dims3::new(20, 12, 9);
        let data: Vec<u16> = (0..dims.voxel_count()).map(|i| ((i * 7919) % 65536) as u16).collect();
        let v = Volume::new(dims, Vec3::ONE, IntensityRange::new(0.0, 1.0).unwrap(), data).unwrap();
        let g = BrickGrid::compute(&v, 4);
        for s in 0..2000u32 {
            let t =
                Vec3::new((s * 37 % 997) as f32 / 997.0, (s * 91 % 991) as f32 / 991.0, (s * 53 % 983) as f32 / 983.0);
            let b = g.brick_of(t, dims);
            let (mn, mx) = g.range(b.x, b.y, b.z);
            let val = v.sample(t);
            assert!(val >= mn - 1e-5 && val <= mx + 1e-5, "{t:?}: {val} not in [{mn},{mx}]");
        }
    }
}
