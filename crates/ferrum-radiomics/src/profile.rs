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
}

impl Profile {
    /// The features of the profile, in order.
    pub fn features(&self) -> impl Iterator<Item = &'static Feature> + '_ {
        self.features.iter().filter_map(|id| feature_by_id(id))
    }

    /// `true` if any feature of the profile needs a known unit.
    pub fn needs_unit(&self) -> bool {
        self.features().any(|f| f.needs_unit)
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
};

/// Every registered profile.
pub const PROFILES: &[Profile] = &[QUICK];

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
                assert_eq!(known, vec!["quick"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn quick_needs_a_unit_because_it_has_intensity_features() {
        assert!(QUICK.needs_unit());
    }
}
