//! The feature registry.
//!
//! Every feature has a stable id, a family, a unit, a definition text and
//! the stage it appeared in. The definition text is the single source of
//! the wording shown by the desktop app, the command line and MCP.

/// Family of a feature: the group a result and the interface show it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Family {
    /// Size of the segment: volume, extent, centroid, axes.
    Size,
    /// First-order statistics of the voxel values.
    Intensity,
}

impl Family {
    /// Name used in ids and results.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Intensity => "intensity",
        }
    }
}

/// Unit of a feature value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// A fixed unit (`mm`, `mL`, empty for a count).
    Fixed(&'static str),
    /// The unit of the voxel values of the study (`HU` for CT).
    Value,
}

/// One quantitative feature of a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    /// Stable id, `family.name`; never reused for another meaning.
    pub id: &'static str,
    /// Group of the feature.
    pub family: Family,
    /// Unit of its value.
    pub unit: Unit,
    /// What it is and how it is computed, in plain words.
    pub definition: &'static str,
    /// Where it first shipped.
    pub since: &'static str,
    /// The feature needs a known unit of the voxel values (a CT study in
    /// HU); without one it is refused instead of guessed.
    pub needs_unit: bool,
}

const fn feature(id: &'static str, family: Family, unit: Unit, definition: &'static str, needs_unit: bool) -> Feature {
    Feature { id, family, unit, definition, since: "stage 18", needs_unit }
}

const fn size(id: &'static str, unit: &'static str, definition: &'static str) -> Feature {
    feature(id, Family::Size, Unit::Fixed(unit), definition, false)
}

const fn intensity(id: &'static str, definition: &'static str) -> Feature {
    feature(id, Family::Intensity, Unit::Value, definition, true)
}

/// Every registered feature, in the order profiles list them.
pub const FEATURES: &[Feature] = &[
    size("size.voxels", "", "Number of voxels labelled with the segment."),
    size(
        "size.volume_mm3",
        "mm3",
        "Voxel count times the volume of one voxel (spacing x times y times z), in cubic millimetres.",
    ),
    size("size.volume_ml", "mL", "size.volume_mm3 divided by 1000; one millilitre is 1000 cubic millimetres."),
    size(
        "size.extent_i_mm",
        "mm",
        "Extent of the bounding box along grid axis i (columns): voxels from the first to the last labelled \
         voxel times the spacing along i, in mm.",
    ),
    size(
        "size.extent_j_mm",
        "mm",
        "Extent of the bounding box along grid axis j (rows): voxels from the first to the last labelled voxel \
         times the spacing along j, in mm.",
    ),
    size(
        "size.extent_k_mm",
        "mm",
        "Extent of the bounding box along grid axis k (slices): voxels from the first to the last labelled \
         voxel times the spacing along k, in mm.",
    ),
    size(
        "size.centroid_x_mm",
        "mm",
        "x of the centroid (the mean of the voxel centres) in patient LPS coordinates, in mm; +x is the \
         patient's left.",
    ),
    size(
        "size.centroid_y_mm",
        "mm",
        "y of the centroid (the mean of the voxel centres) in patient LPS coordinates, in mm; +y is posterior.",
    ),
    size(
        "size.centroid_z_mm",
        "mm",
        "z of the centroid (the mean of the voxel centres) in patient LPS coordinates, in mm; +z is superior.",
    ),
    size(
        "size.long_axis_mm",
        "mm",
        "Longest distance between two voxel centres on the axial slice with the most voxels of the segment, in \
         mm. Uncertain by one in-plane voxel spacing.",
    ),
    size(
        "size.short_axis_mm",
        "mm",
        "Longest extent perpendicular to size.long_axis_mm on the same axial slice, in mm.",
    ),
    size(
        "size.largest_slice_axial",
        "",
        "1-based number of the axial slice (along k) with the most voxels of the segment.",
    ),
    size(
        "size.largest_slice_coronal",
        "",
        "1-based number of the coronal slice (along j) with the most voxels of the segment.",
    ),
    size(
        "size.largest_slice_sagittal",
        "",
        "1-based number of the sagittal slice (along i) with the most voxels of the segment.",
    ),
    intensity("intensity.mean", "Mean of the voxel values inside the segment."),
    intensity(
        "intensity.std",
        "Population standard deviation (divisor N, not N - 1) of the voxel values inside the segment.",
    ),
    intensity("intensity.min", "Lowest voxel value inside the segment."),
    intensity("intensity.max", "Highest voxel value inside the segment."),
    intensity("intensity.range", "intensity.max minus intensity.min."),
    intensity(
        "intensity.p5",
        "5th percentile of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.05).",
    ),
    intensity(
        "intensity.p25",
        "25th percentile of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.25).",
    ),
    intensity(
        "intensity.p50",
        "Median (50th percentile) of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.5).",
    ),
    intensity(
        "intensity.p75",
        "75th percentile of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.75).",
    ),
    intensity(
        "intensity.p95",
        "95th percentile of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.95).",
    ),
    intensity("intensity.iqr", "Interquartile range: intensity.p75 minus intensity.p25 (nearest-rank percentiles)."),
];

/// The registered feature with `id`.
pub fn feature_by_id(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|f| f.id == id)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    // covers 18.1-a
    #[test]
    fn ids_are_unique_and_documented() {
        let mut seen = HashSet::new();
        for f in FEATURES {
            assert!(seen.insert(f.id), "duplicate id {}", f.id);
            let (family, name) = f.id.split_once('.').expect("id is family.name");
            assert_eq!(family, f.family.name(), "{} sits in the wrong family", f.id);
            assert!(!name.is_empty());
            assert!(f.definition.len() > 20, "{} has no real definition", f.id);
            assert!(!f.since.is_empty(), "{} has no origin", f.id);
        }
    }

    // covers 18.3-d
    #[test]
    fn definitions_name_the_std_divisor_and_the_percentile_method() {
        let std = feature_by_id("intensity.std").unwrap();
        assert!(std.definition.contains("Population") && std.definition.contains("divisor N"));
        for id in ["intensity.p5", "intensity.p25", "intensity.p50", "intensity.p75", "intensity.p95"] {
            assert!(feature_by_id(id).unwrap().definition.contains("nearest rank"), "{id}");
        }
        assert!(feature_by_id("intensity.iqr").unwrap().definition.contains("nearest-rank"));
    }

    #[test]
    fn only_intensity_features_need_a_unit() {
        for f in FEATURES {
            assert_eq!(f.needs_unit, f.family == Family::Intensity, "{}", f.id);
            assert_eq!(matches!(f.unit, Unit::Value), f.family == Family::Intensity, "{}", f.id);
        }
    }
}
