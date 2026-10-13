//! Profiles: versioned lists of features.
//!
//! A profile is what a person or an agent asks for. A result names the
//! profile and its version, so numbers stay comparable. A profile is
//! `0.x` while its stage is in progress and `1` when released; adding or
//! changing a feature is a version change.

use crate::error::MetricsError;
use crate::feature::{feature_by_id, Feature};

/// A named, versioned list of feature ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// Name used on the command line and in results.
    pub name: &'static str,
    /// Version of the list.
    pub version: &'static str,
    /// What the profile is for, in one sentence.
    pub summary: &'static str,
    /// Ids of its features, in result order.
    pub features: &'static [&'static str],
    /// The result carries the intensity-volume histogram curve.
    pub ivh_curve: bool,
    /// The result carries the shares of voxels in HU ranges.
    pub hu_ranges: bool,
}

impl Profile {
    /// The features of the profile, in order.
    pub fn features(&self) -> impl Iterator<Item = &'static Feature> + '_ {
        self.features.iter().filter_map(|id| feature_by_id(id))
    }

    /// `true` if any feature of the profile needs a known unit.
    pub fn needs_unit(&self) -> bool {
        self.ivh_curve || self.hu_ranges || self.features().any(|f| f.needs_unit)
    }

    /// The profile called `name`, or an error that lists the known ones.
    pub fn by_name(name: &str) -> Result<&'static Profile, MetricsError> {
        PROFILES.iter().find(|p| p.name == name).ok_or_else(|| MetricsError::UnknownProfile {
            given: name.to_owned(),
            known: PROFILES.iter().map(|p| p.name).collect(),
        })
    }
}

/// Size and first-order statistics: the quick look at a segment.
pub const QUICK: Profile = Profile {
    name: "quick",
    version: "0.1",
    summary: "Size (volume, extent, centroid, axial axes) and first-order statistics of the voxel values.",
    features: &[
        "size.voxels",
        "size.volume_mm3",
        "size.volume_ml",
        "size.extent_i_mm",
        "size.extent_j_mm",
        "size.extent_k_mm",
        "size.centroid_x_mm",
        "size.centroid_y_mm",
        "size.centroid_z_mm",
        "size.long_axis_mm",
        "size.short_axis_mm",
        "size.largest_slice_axial",
        "size.largest_slice_coronal",
        "size.largest_slice_sagittal",
        "intensity.mean",
        "intensity.std",
        "intensity.min",
        "intensity.max",
        "intensity.range",
        "intensity.p5",
        "intensity.p25",
        "intensity.p50",
        "intensity.p75",
        "intensity.p95",
        "intensity.iqr",
    ],
    ivh_curve: false,
    hu_ranges: false,
};

/// Size, shape and first-order statistics: what a radiologist reads from a
/// segment.
pub const CLINICAL: Profile = Profile {
    name: "clinical",
    version: "0.3",
    summary: "Size, shape (surface, compactness, principal axes, convex hull), connectivity (pieces, cavities, Euler number), first-order statistics and distribution of the voxel values (skewness, kurtosis, energy, deviations, histogram entropy, intensity-volume histogram) and shares of voxels in HU ranges.",
    features: &[
        "size.voxels",
        "size.volume_mm3",
        "size.volume_ml",
        "size.extent_i_mm",
        "size.extent_j_mm",
        "size.extent_k_mm",
        "size.centroid_x_mm",
        "size.centroid_y_mm",
        "size.centroid_z_mm",
        "size.long_axis_mm",
        "size.short_axis_mm",
        "size.max_diameter_mm",
        "size.largest_slice_axial",
        "size.largest_slice_coronal",
        "size.largest_slice_sagittal",
        "shape.mesh_volume_ml",
        "shape.surface_area_mm2",
        "shape.surface_to_volume_per_mm",
        "shape.compactness_1",
        "shape.compactness_2",
        "shape.sphericity",
        "shape.spherical_disproportion",
        "shape.major_axis_mm",
        "shape.minor_axis_mm",
        "shape.least_axis_mm",
        "shape.elongation",
        "shape.flatness",
        "shape.major_axis_dir_x",
        "shape.major_axis_dir_y",
        "shape.major_axis_dir_z",
        "shape.minor_axis_dir_x",
        "shape.minor_axis_dir_y",
        "shape.minor_axis_dir_z",
        "shape.least_axis_dir_x",
        "shape.least_axis_dir_y",
        "shape.least_axis_dir_z",
        "shape.hull_volume_ml",
        "shape.hull_area_mm2",
        "shape.solidity",
        "shape.convexity",
        "topology.components",
        "topology.largest_component_share",
        "topology.holes",
        "topology.hole_volume_mm3",
        "topology.euler_number",
        "topology.border_sides",
        "intensity.mean",
        "intensity.std",
        "intensity.min",
        "intensity.max",
        "intensity.range",
        "intensity.p5",
        "intensity.p25",
        "intensity.p50",
        "intensity.p75",
        "intensity.p95",
        "intensity.iqr",
        "intensity.skewness",
        "intensity.kurtosis",
        "intensity.energy",
        "intensity.rms",
        "intensity.mad",
        "intensity.rmad",
        "intensity.entropy",
        "intensity.uniformity",
        "intensity.ivh_v10",
        "intensity.ivh_v90",
        "intensity.ivh_i10",
        "intensity.ivh_i90",
    ],
    ivh_curve: true,
    hu_ranges: true,
};

/// Every registered profile.
pub const PROFILES: &[Profile] = &[QUICK, CLINICAL];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn profiles_list_only_registered_features_once() {
        for p in PROFILES {
            let mut seen = HashSet::new();
            for id in p.features {
                assert!(feature_by_id(id).is_some(), "{} lists unknown {id}", p.name);
                assert!(seen.insert(*id), "{} lists {id} twice", p.name);
            }
            assert_eq!(p.features().count(), p.features.len());
            assert!(!p.version.is_empty() && !p.summary.is_empty());
        }
    }

    // covers 18.1-b
    #[test]
    fn unknown_profile_lists_known_ones() {
        assert_eq!(Profile::by_name("quick").unwrap().name, "quick");
        match Profile::by_name("nope") {
            Err(MetricsError::UnknownProfile { given, known }) => {
                assert_eq!(given, "nope");
                assert_eq!(known, vec!["quick", "clinical"]);
            }
            other => panic!("{other:?}"),
        }
    }

    // covers 18.1-b
    #[test]
    fn clinical_contains_every_quick_feature_in_the_same_order() {
        let mut rest = CLINICAL.features.iter();
        for id in QUICK.features {
            assert!(rest.any(|c| c == id), "clinical lacks {id} or lists it out of order");
        }
        assert!(CLINICAL.features.len() > QUICK.features.len());
        assert!(CLINICAL.features().any(|f| f.family == crate::feature::Family::Shape));
        assert_eq!(Profile::by_name("clinical").unwrap().version, "0.3");
    }

    #[test]
    fn quick_needs_a_unit_because_it_has_intensity_features() {
        assert!(QUICK.needs_unit());
    }
}
