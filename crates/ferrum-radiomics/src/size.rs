//! Size of a segment: volume, extent, centroid and the axial axes.
//!
//! The measures that `ferrum-domain` already has (extent, largest slices,
//! long and short axis) are taken from [`ferrum_domain::analysis::shape`],
//! so `segment shape` and the metrics give the same numbers from one
//! implementation.

use ferrum_domain::{analysis, MaskRegion, Volume};
use glam::DVec3;

/// Size measures of a segment.
#[derive(Debug, Clone, PartialEq)]
pub struct Size {
    /// Number of voxels.
    pub voxels: u64,
    /// Volume in cubic millimetres.
    pub volume_mm3: f64,
    /// Extent of the bounding box along the grid axes `i`, `j`, `k`, in mm.
    pub extent_mm: [f64; 3],
    /// Centroid in patient LPS coordinates, in mm.
    pub centroid_mm: DVec3,
    /// Longest distance between voxel centres on the largest axial slice.
    pub long_axis_mm: Option<f64>,
    /// Longest extent perpendicular to the long axis on that slice.
    pub short_axis_mm: Option<f64>,
    /// 1-based number of the largest slice: axial (`k`), coronal (`j`),
    /// sagittal (`i`).
    pub largest_slice: [u32; 3],
}

/// Size of `region`, which must be non-empty, tight (cropped to its
/// bounding box) and inside `volume`.
pub fn size(region: &MaskRegion, volume: &Volume) -> Size {
    let spacing = volume.spacing();
    let s = analysis::shape(region, volume.dims(), spacing);
    let voxel_mm3 = f64::from(spacing.x) * f64::from(spacing.y) * f64::from(spacing.z);
    Size {
        voxels: s.voxels,
        volume_mm3: s.voxels as f64 * voxel_mm3,
        extent_mm: s.extent_mm,
        centroid_mm: volume.voxel_to_patient_f64(s.centroid),
        long_axis_mm: s.long_axis.map(|d| d.mm),
        short_axis_mm: s.short_axis.map(|d| d.mm),
        largest_slice: s.largest_slices.map(|l| l.index + 1),
    }
}

#[cfg(test)]
mod tests {
    use ferrum_domain::{Dims3, Geometry, IntensityRange, Volume};
    use glam::{Mat3, UVec3, Vec3};

    use super::*;

    fn volume(dims: [u32; 3], spacing: [f32; 3]) -> Volume {
        let d = Dims3 { x: dims[0], y: dims[1], z: dims[2] };
        let range = IntensityRange::new(-1000.0, 1000.0).unwrap();
        Volume::new(d, Vec3::from(spacing), range, vec![0; d.voxel_count()]).unwrap()
    }

    /// A box `lo..hi` of a grid of `dims`, as a tight region.
    fn boxed(dims: [u32; 3], lo: [u32; 3], hi: [u32; 3]) -> MaskRegion {
        let d = Dims3 { x: dims[0], y: dims[1], z: dims[2] };
        let (lo, hi) = (UVec3::from(lo), UVec3::from(hi));
        MaskRegion::from_grid(d, |n| {
            let (i, j, k) = (n as u32 % d.x, (n as u32 / d.x) % d.y, n as u32 / (d.x * d.y));
            let p = UVec3::new(i, j, k);
            p.cmpge(lo).all() && p.cmplt(hi).all()
        })
        .unwrap()
    }

    // covers 18.2-a
    #[test]
    fn volume_of_a_box_with_anisotropic_spacing() {
        // 4 x 5 x 6 voxels of 0.7 x 0.7 x 2.5 mm
        let v = volume([10, 10, 10], [0.7, 0.7, 2.5]);
        let s = size(&boxed([10, 10, 10], [1, 2, 3], [5, 7, 9]), &v);
        assert_eq!(s.voxels, 120);
        let voxel = f64::from(0.7_f32) * f64::from(0.7_f32) * 2.5;
        assert!((s.volume_mm3 - 120.0 * voxel).abs() < 1e-9, "{}", s.volume_mm3);
        assert!((s.volume_mm3 - 147.0).abs() < 0.01, "{}", s.volume_mm3);
        assert!((s.volume_mm3 / 1000.0 - 0.147).abs() < 1e-5);
    }

    // covers 18.2-b
    #[test]
    fn extent_and_centroid_in_patient_mm_with_direction_and_origin() {
        // axes swapped and flipped: i -> +y, j -> -x, k -> +z, origin (10, 20, 30)
        let direction = Mat3::from_cols(Vec3::Y, -Vec3::X, Vec3::Z);
        let g = Geometry { origin: Vec3::new(10.0, 20.0, 30.0), direction };
        let v = volume([8, 8, 8], [0.5, 2.0, 3.0]).with_geometry(g);
        // voxels i 2..4, j 1..4, k 5..6: centroid index (2.5, 2.0, 5.0)
        let s = size(&boxed([8, 8, 8], [2, 1, 5], [5, 4, 6]), &v);
        let expected_extent = [3.0 * 0.5, 3.0 * 2.0, 1.0 * 3.0];
        assert_eq!(s.extent_mm, expected_extent);
        // index (3.0, 2.0, 5.0) -> spacing (1.5, 4.0, 15.0) -> x = 10 - 4, y = 20 + 1.5, z = 30 + 15
        assert!((s.centroid_mm - DVec3::new(6.0, 21.5, 45.0)).length() < 1e-9, "{:?}", s.centroid_mm);
    }

    // covers 18.2-e
    #[test]
    fn a_dimension_of_one_on_each_axis_still_measures() {
        for axis in 0..3 {
            let mut dims = [6, 7, 8];
            dims[axis] = 1;
            let v = volume(dims, [1.0, 2.0, 3.0]);
            let hi = dims;
            let s = size(&boxed(dims, [0, 0, 0], hi), &v);
            assert_eq!(s.voxels, u64::from(dims[0] * dims[1] * dims[2]));
            assert!(s.volume_mm3 > 0.0 && s.centroid_mm.is_finite());
            assert!(s.extent_mm.iter().all(|e| *e > 0.0));
            assert!(s.largest_slice.iter().all(|n| *n >= 1));
        }
    }

    // covers 18.2-d
    #[test]
    fn axes_and_slices_come_from_the_same_analysis_as_segment_shape() {
        let v = volume([12, 12, 12], [1.0, 1.0, 2.0]);
        let r = boxed([12, 12, 12], [2, 3, 4], [8, 6, 9]);
        let s = size(&r, &v);
        let shape = analysis::shape(&r, v.dims(), v.spacing());
        assert_eq!(s.long_axis_mm, shape.long_axis.map(|d| d.mm));
        assert_eq!(s.short_axis_mm, shape.short_axis.map(|d| d.mm));
        assert_eq!(s.extent_mm, shape.extent_mm);
        assert_eq!(s.largest_slice, shape.largest_slices.map(|l| l.index + 1));
    }
}
