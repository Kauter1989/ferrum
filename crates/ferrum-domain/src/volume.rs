//! The scalar volume entity.

use glam::{Mat3, UVec3, Vec3};
use thiserror::Error;

use crate::geometry::{Aabb, Dims3};

/// Errors raised when constructing a [`Volume`].
#[derive(Debug, Error, PartialEq)]
pub enum VolumeError {
    /// One of the grid dimensions is zero.
    #[error("volume dimensions must be non-zero, got {0:?}")]
    EmptyDimensions(Dims3),
    /// The voxel buffer length does not match the dimensions.
    #[error("voxel buffer has {actual} elements, expected {expected}")]
    DataLengthMismatch {
        /// Expected number of voxels.
        expected: usize,
        /// Actual buffer length.
        actual: usize,
    },
    /// Spacing contains a non-positive or non-finite component.
    #[error("voxel spacing must be positive and finite, got {0:?}")]
    InvalidSpacing(Vec3),
    /// The intensity range is empty or not finite.
    #[error("invalid intensity range [{min}, {max}]")]
    InvalidRange {
        /// Lower bound.
        min: f32,
        /// Upper bound.
        max: f32,
    },
}

/// Physical intensity range mapped onto the normalised `[0, 1]` storage.
///
/// Voxels are stored as `u16` where `0 ↦ min` and `65535 ↦ max`. For CT the
/// physical unit is Hounsfield; for MR it is the scanner's arbitrary unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntensityRange {
    /// Physical value of normalised `0`.
    pub min: f32,
    /// Physical value of normalised `1`.
    pub max: f32,
}

impl IntensityRange {
    /// Creates a validated range. A degenerate range (`min == max`) is widened
    /// by one unit so normalisation never divides by zero.
    pub fn new(min: f32, max: f32) -> Result<Self, VolumeError> {
        if !min.is_finite() || !max.is_finite() || max < min {
            return Err(VolumeError::InvalidRange { min, max });
        }
        let max = if max == min { min + 1.0 } else { max };
        Ok(Self { min, max })
    }

    /// Width of the range (always > 0).
    pub fn span(&self) -> f32 {
        self.max - self.min
    }

    /// Physical value → normalised `[0, 1]` (clamped).
    pub fn normalize(&self, value: f32) -> f32 {
        ((value - self.min) / self.span()).clamp(0.0, 1.0)
    }

    /// Normalised value → physical value.
    pub fn denormalize(&self, n: f32) -> f32 {
        self.min + n * self.span()
    }

    /// Physical value → `u16` storage.
    pub fn to_storage(&self, value: f32) -> u16 {
        // normalize() clamps into [0,1], so the product fits u16.
        (self.normalize(value) * f32::from(u16::MAX)).round() as u16
    }

    /// Physical value of a (possibly fractional) stored value, e.g. a mean
    /// of stored values.
    pub fn from_storage_f64(&self, stored: f64) -> f32 {
        self.denormalize((stored / f64::from(u16::MAX)) as f32)
    }

    /// `u16` storage → physical value.
    pub fn from_storage(&self, stored: u16) -> f32 {
        self.denormalize(f32::from(stored) / f32::from(u16::MAX))
    }
}

/// Placement of the voxel grid in patient space (LPS, millimetres).
///
/// The patient position of voxel `(i, j, k)` is
/// `origin + direction · ((i, j, k) ⊙ spacing)`, where the columns of
/// `direction` are the unit vectors of the `i`, `j` and `k` axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// Patient position of the centre of voxel `(0, 0, 0)`.
    pub origin: Vec3,
    /// Unit axis vectors of the grid as matrix columns.
    pub direction: Mat3,
}

impl Default for Geometry {
    fn default() -> Self {
        Self { origin: Vec3::ZERO, direction: Mat3::IDENTITY }
    }
}

impl Geometry {
    /// Patient position of the (fractional) voxel index `v` with `spacing`.
    pub fn voxel_to_patient(&self, v: Vec3, spacing: Vec3) -> Vec3 {
        self.origin + self.direction * (v * spacing)
    }

    /// Direction rows as written by the engine protocol (`i`, `j`, `k`).
    pub fn direction_rows(&self) -> [[f32; 3]; 3] {
        [self.direction.x_axis.to_array(), self.direction.y_axis.to_array(), self.direction.z_axis.to_array()]
    }
}

/// A regular scalar volume with physical spacing.
///
/// This is the central entity of the viewer. Voxel data is immutable once
/// constructed; editing (e.g. the 3D eraser) is expressed through a separate
/// [`crate::mask::VoxelMask`].
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    dims: Dims3,
    spacing: Vec3,
    range: IntensityRange,
    data: Vec<u16>,
    geometry: Geometry,
}

impl Volume {
    /// Creates a volume from normalised `u16` storage.
    pub fn new(dims: Dims3, spacing: Vec3, range: IntensityRange, data: Vec<u16>) -> Result<Self, VolumeError> {
        if dims.is_empty() {
            return Err(VolumeError::EmptyDimensions(dims));
        }
        if !(spacing.is_finite() && spacing.cmpgt(Vec3::ZERO).all()) {
            return Err(VolumeError::InvalidSpacing(spacing));
        }
        if data.len() != dims.voxel_count() {
            return Err(VolumeError::DataLengthMismatch { expected: dims.voxel_count(), actual: data.len() });
        }
        Ok(Self { dims, spacing, range, data, geometry: Geometry::default() })
    }

    /// Creates a volume from physical values, computing the range from data.
    pub fn from_physical(dims: Dims3, spacing: Vec3, values: &[f32]) -> Result<Self, VolumeError> {
        let (min, max) = values
            .iter()
            .filter(|v| v.is_finite())
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        let range = if min.is_finite() { IntensityRange::new(min, max)? } else { IntensityRange::new(0.0, 1.0)? };
        let data = values.iter().map(|&v| range.to_storage(v)).collect();
        Self::new(dims, spacing, range, data)
    }

    /// Returns the volume placed at `geometry` in patient space.
    #[must_use]
    pub fn with_geometry(mut self, geometry: Geometry) -> Self {
        self.geometry = geometry;
        self
    }

    /// Placement of the grid in patient space.
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// Patient position (LPS, mm) of the centre of voxel `v`.
    pub fn voxel_to_patient(&self, v: Vec3) -> Vec3 {
        self.geometry.voxel_to_patient(v, self.spacing)
    }

    /// Grid dimensions.
    pub fn dims(&self) -> Dims3 {
        self.dims
    }

    /// Voxel spacing in millimetres `(column, row, slice)`.
    pub fn spacing(&self) -> Vec3 {
        self.spacing
    }

    /// Physical intensity range of the normalised storage.
    pub fn range(&self) -> IntensityRange {
        self.range
    }

    /// Raw normalised storage (`x` fastest).
    pub fn data(&self) -> &[u16] {
        &self.data
    }

    /// Physical size of the volume in millimetres.
    pub fn physical_size(&self) -> Vec3 {
        self.dims.as_vec3() * self.spacing
    }

    /// Size of the model-space box: physical size scaled so that the longest
    /// side equals `1`.
    pub fn model_extent(&self) -> Vec3 {
        let size = self.physical_size();
        size / size.max_element()
    }

    /// Model-space bounding box, centred at the origin.
    pub fn model_bounds(&self) -> Aabb {
        Aabb::centered(self.model_extent())
    }

    /// Model-space point → texture coordinate.
    pub fn model_to_tex(&self, p: Vec3) -> Vec3 {
        p / self.model_extent() + Vec3::splat(0.5)
    }

    /// Texture coordinate → model-space point.
    pub fn tex_to_model(&self, t: Vec3) -> Vec3 {
        (t - Vec3::splat(0.5)) * self.model_extent()
    }

    /// Stored value at `(i, j, k)`; `None` if out of bounds.
    pub fn voxel(&self, i: u32, j: u32, k: u32) -> Option<u16> {
        (i < self.dims.x && j < self.dims.y && k < self.dims.z).then(|| self.data[self.dims.index(i, j, k)])
    }

    /// Normalised value at `(i, j, k)` with coordinates clamped to the grid.
    #[inline]
    pub fn normalized_clamped(&self, i: i64, j: i64, k: i64) -> f32 {
        let i = i.clamp(0, i64::from(self.dims.x) - 1) as u32;
        let j = j.clamp(0, i64::from(self.dims.y) - 1) as u32;
        let k = k.clamp(0, i64::from(self.dims.z) - 1) as u32;
        f32::from(self.data[self.dims.index(i, j, k)]) / f32::from(u16::MAX)
    }

    /// Physical value at `(i, j, k)`; `None` if out of bounds.
    pub fn physical(&self, i: u32, j: u32, k: u32) -> Option<f32> {
        self.voxel(i, j, k).map(|v| self.range.from_storage(v))
    }

    /// Trilinear sample of the normalised value at texture coordinate `t`,
    /// replicating GPU `ClampToEdge` + linear filtering.
    pub fn sample(&self, t: Vec3) -> f32 {
        let g = t * self.dims.as_vec3() - Vec3::splat(0.5);
        let base = g.floor();
        let f = g - base;
        let (i, j, k) = (base.x as i64, base.y as i64, base.z as i64);
        let c000 = self.normalized_clamped(i, j, k);
        let c100 = self.normalized_clamped(i + 1, j, k);
        let c010 = self.normalized_clamped(i, j + 1, k);
        let c110 = self.normalized_clamped(i + 1, j + 1, k);
        let c001 = self.normalized_clamped(i, j, k + 1);
        let c101 = self.normalized_clamped(i + 1, j, k + 1);
        let c011 = self.normalized_clamped(i, j + 1, k + 1);
        let c111 = self.normalized_clamped(i + 1, j + 1, k + 1);
        let x00 = c000 + (c100 - c000) * f.x;
        let x10 = c010 + (c110 - c010) * f.x;
        let x01 = c001 + (c101 - c001) * f.x;
        let x11 = c011 + (c111 - c011) * f.x;
        let y0 = x00 + (x10 - x00) * f.y;
        let y1 = x01 + (x11 - x01) * f.y;
        y0 + (y1 - y0) * f.z
    }

    /// Voxel containing texture coordinate `t`, or `None` outside `[0, 1)³`.
    pub fn tex_to_voxel(&self, t: Vec3) -> Option<UVec3> {
        let g = (t * self.dims.as_vec3()).floor();
        let d = self.dims;
        self.dims
            .contains(g.x as i64, g.y as i64, g.z as i64)
            .then(|| UVec3::new(g.x as u32, g.y as u32, g.z as u32))
            .filter(|v| v.x < d.x && v.y < d.y && v.z < d.z)
    }

    /// Texture coordinate of the centre of voxel `v`.
    pub fn voxel_to_tex(&self, v: UVec3) -> Vec3 {
        (v.as_vec3() + Vec3::splat(0.5)) / self.dims.as_vec3()
    }

    /// Central-difference gradient of the normalised field at `t`, in
    /// texture space units (one texel step per axis).
    pub fn gradient(&self, t: Vec3) -> Vec3 {
        let d = Vec3::ONE / self.dims.as_vec3();
        Vec3::new(
            self.sample(t + Vec3::new(d.x, 0.0, 0.0)) - self.sample(t - Vec3::new(d.x, 0.0, 0.0)),
            self.sample(t + Vec3::new(0.0, d.y, 0.0)) - self.sample(t - Vec3::new(0.0, d.y, 0.0)),
            self.sample(t + Vec3::new(0.0, 0.0, d.z)) - self.sample(t - Vec3::new(0.0, 0.0, d.z)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: u32) -> Volume {
        let dims = Dims3::new(n, n, n);
        let values: Vec<f32> = (0..dims.voxel_count()).map(|idx| (idx as u32 % n) as f32).collect();
        Volume::from_physical(dims, Vec3::ONE, &values).unwrap()
    }

    #[test]
    fn rejects_bad_input() {
        let r = IntensityRange::new(0.0, 1.0).unwrap();
        assert!(matches!(Volume::new(Dims3::new(0, 1, 1), Vec3::ONE, r, vec![]), Err(VolumeError::EmptyDimensions(_))));
        assert!(matches!(
            Volume::new(Dims3::new(2, 1, 1), Vec3::ONE, r, vec![0]),
            Err(VolumeError::DataLengthMismatch { expected: 2, actual: 1 })
        ));
        assert!(matches!(
            Volume::new(Dims3::new(1, 1, 1), Vec3::new(1.0, 0.0, 1.0), r, vec![0]),
            Err(VolumeError::InvalidSpacing(_))
        ));
        assert!(IntensityRange::new(2.0, 1.0).is_err());
        assert!(IntensityRange::new(f32::NAN, 1.0).is_err());
    }

    #[test]
    fn geometry_places_voxels_in_patient_space() {
        let v = ramp(4);
        assert_eq!(v.geometry(), Geometry::default());
        assert_eq!(v.voxel_to_patient(Vec3::new(1.0, 2.0, 3.0)), Vec3::new(1.0, 2.0, 3.0));
        let flip = Mat3::from_cols(Vec3::X, -Vec3::Y, Vec3::Z);
        let g = Geometry { origin: Vec3::new(-10.0, 5.0, 2.0), direction: flip };
        let v = Volume::new(v.dims(), Vec3::new(0.5, 2.0, 1.0), v.range(), v.data().to_vec()).unwrap().with_geometry(g);
        assert_eq!(v.voxel_to_patient(Vec3::new(2.0, 1.0, 3.0)), Vec3::new(-9.0, 3.0, 5.0));
        assert_eq!(g.direction_rows(), [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]]);
    }

    #[test]
    fn degenerate_range_is_widened() {
        let r = IntensityRange::new(5.0, 5.0).unwrap();
        assert_eq!(r.span(), 1.0);
    }

    #[test]
    fn storage_roundtrip_is_accurate() {
        let r = IntensityRange::new(-1024.0, 3071.0).unwrap();
        for v in [-1024.0, -500.0, 0.0, 40.0, 3071.0] {
            let back = r.from_storage(r.to_storage(v));
            assert!((back - v).abs() < 0.05, "{v} -> {back}");
        }
    }

    #[test]
    fn model_extent_longest_side_is_one() {
        let dims = Dims3::new(100, 50, 20);
        let v = Volume::new(
            dims,
            Vec3::new(1.0, 1.0, 5.0),
            IntensityRange::new(0.0, 1.0).unwrap(),
            vec![0; dims.voxel_count()],
        )
        .unwrap();
        let e = v.model_extent();
        assert_eq!(e.max_element(), 1.0);
        assert_eq!(e, Vec3::new(1.0, 0.5, 1.0));
        let p = Vec3::new(0.1, -0.2, 0.3);
        assert!((v.tex_to_model(v.model_to_tex(p)) - p).length() < 1e-6);
    }

    #[test]
    fn trilinear_sample_hits_voxel_centres() {
        let v = ramp(4);
        for i in 0..4 {
            let t = v.voxel_to_tex(UVec3::new(i, 1, 2));
            let expected = i as f32 / 3.0;
            assert!((v.sample(t) - expected).abs() < 1e-4);
        }
        // halfway between voxel 1 and 2
        let t = Vec3::new(0.5, 0.5, 0.5);
        assert!((v.sample(t) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn sample_clamps_to_edge() {
        let v = ramp(4);
        assert!((v.sample(Vec3::new(-1.0, 0.5, 0.5)) - 0.0).abs() < 1e-6);
        assert!((v.sample(Vec3::new(2.0, 0.5, 0.5)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn gradient_points_along_ramp() {
        let v = ramp(8);
        let g = v.gradient(Vec3::splat(0.5));
        assert!(g.x > 0.0);
        assert!(g.y.abs() < 1e-6 && g.z.abs() < 1e-6);
    }

    #[test]
    fn tex_to_voxel_bounds() {
        let v = ramp(4);
        assert_eq!(v.tex_to_voxel(Vec3::splat(0.0)), Some(UVec3::ZERO));
        assert_eq!(v.tex_to_voxel(Vec3::splat(0.99)), Some(UVec3::splat(3)));
        assert_eq!(v.tex_to_voxel(Vec3::splat(1.0)), None);
        assert_eq!(v.tex_to_voxel(Vec3::splat(-0.01)), None);
    }
}
