//! The scalar volume entity.

use glam::{DVec3, Mat3, UVec3, Vec3};
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

    /// The storage step of a range of data `min..=max` that lets values on
    /// a grid be stored exactly: the smallest power of two not below
    /// `span / 65535`. `None` if the range is empty or not finite.
    ///
    /// A power of two, so that integers (CT in HU) and halves, quarters and
    /// so on are whole multiples of it when it is at most 1.
    pub fn grid_step(min: f32, max: f32) -> Option<f64> {
        let span = f64::from(max) - f64::from(min);
        (span.is_finite() && span > 0.0).then(|| 2f64.powi((span / f64::from(u16::MAX)).log2().ceil() as i32))
    }

    /// `true` if `value` lies on the grid of `step` that starts at `min`
    /// (`min + n * step`); a value that is not finite does not matter.
    pub fn is_on_grid(value: f32, min: f32, step: f64) -> bool {
        !value.is_finite() || ((f64::from(value) - f64::from(min)) / step).fract() == 0.0
    }

    /// The range that stores values on the grid of `step` from `min` exactly:
    /// `65535` steps wide, so a stored `n` is `min + n * step` to the last bit.
    /// It reaches up to twice as far above the data as the data are wide.
    pub fn on_grid(min: f32, step: f64) -> Result<Self, VolumeError> {
        let max = (f64::from(min) + step * f64::from(u16::MAX)) as f32;
        Self::new(min, max)
    }

    /// The range for data `min..=max`, with `all_on_grid(min, step)` saying
    /// whether every value is a multiple of the step above `min`. Data on a
    /// grid are stored exactly; any other data (float images, spans wider
    /// than 65 535 steps) use the data range, as before.
    pub fn for_data(min: f32, max: f32, all_on_grid: impl FnOnce(f32, f64) -> bool) -> Result<Self, VolumeError> {
        match Self::grid_step(min, max) {
            Some(step) if all_on_grid(min, step) => Self::on_grid(min, step),
            _ => Self::new(min, max),
        }
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

    /// The physical width of one storage step, in double precision.
    fn step(&self) -> f64 {
        (f64::from(self.max) - f64::from(self.min)) / f64::from(u16::MAX)
    }

    /// Physical value → `u16` storage (in double precision, so a value on
    /// the grid of an [`on_grid`](Self::on_grid) range lands on its step).
    pub fn to_storage(&self, value: f32) -> u16 {
        let n = (f64::from(value) - f64::from(self.min)) / self.step();
        n.round().clamp(0.0, f64::from(u16::MAX)) as u16
    }

    /// Physical value of a (possibly fractional) stored value, e.g. a mean
    /// of stored values.
    pub fn from_storage_f64(&self, stored: f64) -> f32 {
        (f64::from(self.min) + stored * self.step()) as f32
    }

    /// `u16` storage → physical value, exact for an on-grid range.
    pub fn from_storage(&self, stored: u16) -> f32 {
        self.from_storage_f64(f64::from(stored))
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

    /// Creates a volume from physical values, computing the range from data
    /// (see [`IntensityRange::for_data`]: values on a grid, such as CT in HU,
    /// are stored exactly).
    pub fn from_physical(dims: Dims3, spacing: Vec3, values: &[f32]) -> Result<Self, VolumeError> {
        let (min, max) = values
            .iter()
            .filter(|v| v.is_finite())
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        let range = if min.is_finite() {
            IntensityRange::for_data(min, max, |min, step| {
                values.iter().all(|v| IntensityRange::is_on_grid(*v, min, step))
            })?
        } else {
            IntensityRange::new(0.0, 1.0)?
        };
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

    /// Patient position (LPS, mm) of the continuous voxel index `v`, in
    /// double precision (for measurements; integers are voxel centres).
    pub fn voxel_to_patient_f64(&self, v: DVec3) -> DVec3 {
        let g = self.geometry;
        g.origin.as_dvec3() + g.direction.as_dmat3() * (v * self.spacing.as_dvec3())
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

    /// A ramp along `x` from 0 to 1 in normalised storage (the range is the data range, so
    /// the sampling tests do not depend on how `from_physical` picks its storage step).
    fn ramp(n: u32) -> Volume {
        let dims = Dims3::new(n, n, n);
        let range = IntensityRange::new(0.0, (n - 1) as f32).unwrap();
        let data: Vec<u16> = (0..dims.voxel_count()).map(|idx| range.to_storage((idx as u32 % n) as f32)).collect();
        Volume::new(dims, Vec3::ONE, range, data).unwrap()
    }

    fn volume_of(values: &[f32]) -> Volume {
        Volume::from_physical(Dims3::new(values.len() as u32, 1, 1), Vec3::ONE, values).unwrap()
    }

    // covers 18.13-a
    #[test]
    fn every_whole_hounsfield_unit_of_a_ct_range_is_stored_and_read_back_exactly() {
        // -1024 to 3071 HU, the usual range of a CT, as 4096 voxels
        let values: Vec<f32> = (-1024..=3071).map(|v| v as f32).collect();
        let v = volume_of(&values);
        for (i, expected) in values.iter().enumerate() {
            assert_eq!(v.physical(i as u32, 0, 0), Some(*expected), "voxel {i}");
        }
        // the step is a power of two, so the range is a little wider than the data (here 4095.9375 for 4095)
        assert_eq!(v.range().min, -1024.0);
        assert_eq!(v.range().max, -1024.0 + 65535.0 / 16.0);
    }

    // covers 18.13-a
    #[test]
    fn halves_and_a_range_that_does_not_start_at_zero_are_exact_too() {
        let values = [-3.5, -3.0, 0.0, 0.5, 1.0, 41.5, 100.0, 1999.5];
        let v = volume_of(&values);
        for (i, expected) in values.iter().enumerate() {
            assert_eq!(v.physical(i as u32, 0, 0), Some(*expected), "voxel {i}");
        }
        // a wide span: 6 000 HU still fits in steps of 1/8
        let wide: Vec<f32> = (-1000..=5000).step_by(7).map(|v| v as f32).collect();
        let v = volume_of(&wide);
        for (i, expected) in wide.iter().enumerate() {
            assert_eq!(v.physical(i as u32, 0, 0), Some(*expected), "voxel {i}");
        }
    }

    // covers 18.13-b
    #[test]
    fn data_that_are_not_on_a_grid_keep_the_data_range_and_the_old_precision() {
        let values = [0.1f32, 0.2, 0.3, 0.7, 1.9];
        let v = volume_of(&values);
        assert_eq!((v.range().min, v.range().max), (0.1, 1.9), "the range of the data");
        for (i, expected) in values.iter().enumerate() {
            let got = v.physical(i as u32, 0, 0).unwrap();
            assert!((got - expected).abs() <= 1.8 / 65535.0, "voxel {i}: {got} != {expected}");
        }
        // a span of more than 65 535 whole units cannot be stored exactly either way
        let huge = [0.0f32, 1.0, 70_000.0];
        let v = volume_of(&huge);
        assert_eq!((v.range().min, v.range().max), (0.0, 70_000.0));
        // one fractional value among whole ones makes the data off the grid
        let mixed = [0.0f32, 1.0, 2.0, 3.3333];
        assert_eq!(volume_of(&mixed).range().max, 3.3333);
    }

    // covers 18.13-b
    #[test]
    fn a_constant_image_or_one_without_finite_values_still_has_a_range() {
        let v = volume_of(&[42.0; 4]);
        assert_eq!((v.range().min, v.range().max), (42.0, 43.0));
        assert_eq!(v.physical(2, 0, 0), Some(42.0));
        let v = volume_of(&[f32::NAN, f32::INFINITY]);
        assert_eq!((v.range().min, v.range().max), (0.0, 1.0));
        assert_eq!(IntensityRange::grid_step(1.0, 1.0), None);
        assert_eq!(IntensityRange::grid_step(0.0, f32::NAN), None);
    }

    // covers 18.13-c
    #[test]
    fn the_range_is_at_most_twice_as_wide_as_the_data_and_never_narrower() {
        for (min, max) in
            [(0.0f32, 255.0), (-1024.0, 3071.0), (-1000.0, 1500.0), (0.0, 4095.0), (0.0, 4096.0), (0.0, 60_000.0)]
        {
            let step = IntensityRange::grid_step(min, max).unwrap();
            let r = IntensityRange::on_grid(min, step).unwrap();
            assert!(r.max >= max, "{min}..{max}: {}", r.max);
            assert!(f64::from(r.max - r.min) < 2.0 * f64::from(max - min) + 1.0, "{min}..{max}: {}", r.max);
            assert!(step.log2().fract() == 0.0, "{step} is a power of two");
        }
    }

    // covers 18.13-a
    #[test]
    fn storing_is_monotonic_and_clamps_and_the_old_ranges_still_round_trip_within_a_step() {
        let r = IntensityRange::new(-1000.0, 3000.0).unwrap();
        let step = 4000.0 / 65535.0;
        let mut last = 0;
        for i in 0..=400 {
            let value = -1000.0 + 10.0 * i as f32;
            let stored = r.to_storage(value);
            assert!(stored >= last);
            last = stored;
            assert!((f64::from(r.from_storage(stored)) - f64::from(value)).abs() <= step / 2.0 + 1e-4, "{value}");
        }
        assert_eq!((r.to_storage(-5000.0), r.to_storage(5000.0)), (0, u16::MAX));
        assert_eq!(r.from_storage(0), -1000.0);
        assert_eq!(r.from_storage(u16::MAX), 3000.0);
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
