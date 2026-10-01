//! Basic geometric primitives: grid dimensions, rays and axis-aligned boxes.

use glam::{UVec3, Vec3};

/// Dimensions of a regular 3D voxel grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dims3 {
    /// Number of columns.
    pub x: u32,
    /// Number of rows.
    pub y: u32,
    /// Number of slices.
    pub z: u32,
}

impl Dims3 {
    /// Creates new dimensions.
    pub const fn new(x: u32, y: u32, z: u32) -> Self {
        Self { x, y, z }
    }

    /// Total number of voxels.
    pub fn voxel_count(&self) -> usize {
        self.x as usize * self.y as usize * self.z as usize
    }

    /// Number of voxels in one XY slice.
    pub fn slice_len(&self) -> usize {
        self.x as usize * self.y as usize
    }

    /// Returns `true` if any dimension is zero.
    pub fn is_empty(&self) -> bool {
        self.x == 0 || self.y == 0 || self.z == 0
    }

    /// Linear index of voxel `(i, j, k)`. The caller guarantees bounds.
    #[inline]
    pub fn index(&self, i: u32, j: u32, k: u32) -> usize {
        i as usize + self.x as usize * (j as usize + self.y as usize * k as usize)
    }

    /// Returns `true` if `(i, j, k)` lies inside the grid.
    #[inline]
    pub fn contains(&self, i: i64, j: i64, k: i64) -> bool {
        i >= 0 && j >= 0 && k >= 0 && i < self.x as i64 && j < self.y as i64 && k < self.z as i64
    }

    /// Dimensions as a float vector.
    pub fn as_vec3(&self) -> Vec3 {
        Vec3::new(self.x as f32, self.y as f32, self.z as f32)
    }

    /// Dimensions as an unsigned vector.
    pub fn as_uvec3(&self) -> UVec3 {
        UVec3::new(self.x, self.y, self.z)
    }
}

/// A half-line `origin + t * direction`, `t >= 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// Ray start point.
    pub origin: Vec3,
    /// Ray direction (normalised by constructors of this crate).
    pub direction: Vec3,
}

impl Ray {
    /// Creates a ray and normalises its direction.
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        Self { origin, direction: direction.normalize_or_zero() }
    }

    /// Point at parameter `t`.
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }
}

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl Aabb {
    /// Creates a box from its corners.
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// Box centred at the origin with the given full extent.
    pub fn centered(extent: Vec3) -> Self {
        Self::new(-extent * 0.5, extent * 0.5)
    }

    /// Box size.
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    /// Returns `true` if `p` is inside (inclusive).
    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }

    /// Slab-method ray/box intersection.
    ///
    /// Returns the parametric interval `(t_near, t_far)` clamped to `t >= 0`,
    /// or `None` if the ray misses the box. Works for rays starting inside.
    pub fn intersect(&self, ray: &Ray) -> Option<(f32, f32)> {
        let inv = ray.direction.recip();
        let t0 = (self.min - ray.origin) * inv;
        let t1 = (self.max - ray.origin) * inv;
        let tmin = t0.min(t1);
        let tmax = t0.max(t1);
        // NaN (0 * inf) components come from axis-parallel rays exactly on a
        // slab boundary; `max_element`/`min_element` ignore NaN in glam.
        let near = tmin.max_element().max(0.0);
        let far = tmax.min_element();
        (near <= far).then_some((near, far))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_is_x_fastest() {
        let d = Dims3::new(4, 3, 2);
        assert_eq!(d.index(0, 0, 0), 0);
        assert_eq!(d.index(1, 0, 0), 1);
        assert_eq!(d.index(0, 1, 0), 4);
        assert_eq!(d.index(0, 0, 1), 12);
        assert_eq!(d.index(3, 2, 1), 23);
        assert_eq!(d.voxel_count(), 24);
    }

    #[test]
    fn ray_hits_box_from_outside() {
        let b = Aabb::centered(Vec3::ONE);
        let r = Ray::new(Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 0.0, -1.0));
        let (n, f) = b.intersect(&r).expect("hit");
        assert!((n - 1.5).abs() < 1e-6);
        assert!((f - 2.5).abs() < 1e-6);
    }

    #[test]
    fn ray_inside_box_starts_at_zero() {
        let b = Aabb::centered(Vec3::ONE);
        let r = Ray::new(Vec3::ZERO, Vec3::X);
        let (n, f) = b.intersect(&r).expect("hit");
        assert_eq!(n, 0.0);
        assert!((f - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ray_misses_box() {
        let b = Aabb::centered(Vec3::ONE);
        let r = Ray::new(Vec3::new(2.0, 2.0, 2.0), Vec3::X);
        assert!(b.intersect(&r).is_none());
        let behind = Ray::new(Vec3::new(0.0, 0.0, 2.0), Vec3::Z);
        assert!(b.intersect(&behind).is_none());
    }
}
