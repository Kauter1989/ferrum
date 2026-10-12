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
    /// Shape of the segment: surface, compactness, principal axes.
    Shape,
    /// Connectivity of the segment: pieces, cavities, Euler number.
    Topology,
    /// First-order statistics of the voxel values.
    Intensity,
}

impl Family {
    /// Name used in ids and results.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Shape => "shape",
            Self::Topology => "topology",
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
    /// Short name for people, as the desktop app shows it.
    pub label: &'static str,
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

const fn feature(
    id: &'static str,
    label: &'static str,
    family: Family,
    unit: Unit,
    definition: &'static str,
    needs_unit: bool,
) -> Feature {
    Feature { id, label, family, unit, definition, since: "stage 18", needs_unit }
}

const fn size(id: &'static str, label: &'static str, unit: &'static str, definition: &'static str) -> Feature {
    feature(id, label, Family::Size, Unit::Fixed(unit), definition, false)
}

const fn shape(id: &'static str, label: &'static str, unit: &'static str, definition: &'static str) -> Feature {
    feature(id, label, Family::Shape, Unit::Fixed(unit), definition, false)
}

const fn topology(id: &'static str, label: &'static str, unit: &'static str, definition: &'static str) -> Feature {
    feature(id, label, Family::Topology, Unit::Fixed(unit), definition, false)
}

const fn intensity(id: &'static str, label: &'static str, definition: &'static str) -> Feature {
    feature(id, label, Family::Intensity, Unit::Value, definition, true)
}

/// Every registered feature, in the order profiles list them.
pub const FEATURES: &[Feature] = &[
    size("size.voxels", "Voxels", "", "Number of voxels labelled with the segment."),
    size(
        "size.volume_mm3",
        "Volume (mm³)",
        "mm3",
        "Voxel count times the volume of one voxel (spacing x times y times z), in cubic millimetres.",
    ),
    size(
        "size.volume_ml",
        "Volume",
        "mL",
        "size.volume_mm3 divided by 1000; one millilitre is 1000 cubic millimetres.",
    ),
    size(
        "size.extent_i_mm",
        "Extent along i",
        "mm",
        "Extent of the bounding box along grid axis i (columns): voxels from the first to the last labelled \
         voxel times the spacing along i, in mm.",
    ),
    size(
        "size.extent_j_mm",
        "Extent along j",
        "mm",
        "Extent of the bounding box along grid axis j (rows): voxels from the first to the last labelled voxel \
         times the spacing along j, in mm.",
    ),
    size(
        "size.extent_k_mm",
        "Extent along k",
        "mm",
        "Extent of the bounding box along grid axis k (slices): voxels from the first to the last labelled \
         voxel times the spacing along k, in mm.",
    ),
    size(
        "size.centroid_x_mm",
        "Centroid x (left +)",
        "mm",
        "x of the centroid (the mean of the voxel centres) in patient LPS coordinates, in mm; +x is the \
         patient's left.",
    ),
    size(
        "size.centroid_y_mm",
        "Centroid y (posterior +)",
        "mm",
        "y of the centroid (the mean of the voxel centres) in patient LPS coordinates, in mm; +y is posterior.",
    ),
    size(
        "size.centroid_z_mm",
        "Centroid z (superior +)",
        "mm",
        "z of the centroid (the mean of the voxel centres) in patient LPS coordinates, in mm; +z is superior.",
    ),
    size(
        "size.long_axis_mm",
        "Long axis (axial)",
        "mm",
        "Longest distance between two voxel centres on the axial slice with the most voxels of the segment, in \
         mm. Uncertain by one in-plane voxel spacing.",
    ),
    size(
        "size.short_axis_mm",
        "Short axis (axial)",
        "mm",
        "Longest extent perpendicular to size.long_axis_mm on the same axial slice, in mm.",
    ),
    size(
        "size.max_diameter_mm",
        "Maximum 3D diameter",
        "mm",
        "Largest distance between two voxel centres of the segment, in mm, in any direction (not only on one axial slice). Voxel centres, not mesh vertices, so it can differ slightly from other tools.",
    ),
    size(
        "size.largest_slice_axial",
        "Largest axial slice",
        "",
        "1-based number of the axial slice (along k) with the most voxels of the segment.",
    ),
    size(
        "size.largest_slice_coronal",
        "Largest coronal slice",
        "",
        "1-based number of the coronal slice (along j) with the most voxels of the segment.",
    ),
    size(
        "size.largest_slice_sagittal",
        "Largest sagittal slice",
        "",
        "1-based number of the sagittal slice (along i) with the most voxels of the segment.",
    ),
    shape(
        "shape.mesh_volume_ml",
        "Volume of the surface",
        "mL",
        "Volume enclosed by the surface mesh (marching cubes at half the distance between a voxel inside and one outside), in mL. Slightly smaller than size.volume_ml because the mesh cuts the corners of the voxels.",
    ),
    shape(
        "shape.surface_area_mm2",
        "Surface area",
        "mm2",
        "Area of the surface mesh in mm². A mesh of a binary mask is rougher than a smooth surface: a ball comes out about 7 % larger than 4 pi r squared, at any voxel size.",
    ),
    shape(
        "shape.surface_to_volume_per_mm",
        "Surface to volume ratio",
        "1/mm",
        "Surface area divided by the volume of the surface mesh, in 1/mm. Larger for small, thin or irregular segments.",
    ),
    shape(
        "shape.compactness_1",
        "Compactness 1",
        "",
        "V / (sqrt(pi) * A^(3/2)) with the mesh volume V and the surface area A. About 0.053 (1 / 6 pi) for a perfect sphere, smaller for other shapes.",
    ),
    shape(
        "shape.compactness_2",
        "Compactness 2",
        "",
        "36 pi V^2 / A^3 with the mesh volume V and the surface area A: 1 for a perfect sphere, smaller for other shapes (the cube of the sphericity).",
    ),
    shape(
        "shape.sphericity",
        "Sphericity",
        "",
        "(36 pi V^2)^(1/3) / A with the mesh volume V and the surface area A: 1 for a perfect sphere, smaller for any other shape. A mesh of a binary mask gives about 0.93 even for a ball.",
    ),
    shape(
        "shape.spherical_disproportion",
        "Spherical disproportion",
        "",
        "A / (4 pi R^2) with the surface area A and R = (3 V / 4 pi)^(1/3) from the mesh volume V: 1 for a perfect sphere, larger for other shapes (the inverse of the sphericity).",
    ),
    shape(
        "shape.major_axis_mm",
        "Major axis",
        "mm",
        "4 sqrt(l1) where l1 is the largest eigenvalue of the covariance matrix of the voxel centres in patient millimetres (population covariance, divisor N). For a solid ellipsoid with semi-axis a this is 4 a / sqrt(5), about 1.79 a.",
    ),
    shape(
        "shape.minor_axis_mm",
        "Minor axis",
        "mm",
        "4 sqrt(l2) where l2 is the second eigenvalue of the covariance matrix of the voxel centres in patient millimetres (population covariance).",
    ),
    shape(
        "shape.least_axis_mm",
        "Least axis",
        "mm",
        "4 sqrt(l3) where l3 is the smallest eigenvalue of the covariance matrix of the voxel centres in patient millimetres (population covariance). Zero for a segment one voxel thick.",
    ),
    shape(
        "shape.elongation",
        "Elongation",
        "",
        "sqrt(l2 / l1): the minor axis over the major axis. 1 for a round cross-section, towards 0 for a long thin segment. Undefined for a single voxel.",
    ),
    shape(
        "shape.flatness",
        "Flatness",
        "",
        "sqrt(l3 / l1): the least axis over the major axis. 1 for a ball, towards 0 for a flat segment. Undefined for a single voxel.",
    ),
    shape(
        "shape.major_axis_dir_x",
        "Major axis direction x (left +)",
        "",
        "x component of the unit vector along the major axis in patient LPS coordinates (left +), the sign chosen so that the largest component of the vector is positive. Undefined when the axis is not unique: two axes are equally long, or the segment has no extent along it.",
    ),
    shape(
        "shape.major_axis_dir_y",
        "Major axis direction y (posterior +)",
        "",
        "y component of the unit vector along the major axis in patient LPS coordinates (posterior +), the sign chosen so that the largest component of the vector is positive. Undefined when the axis is not unique: two axes are equally long, or the segment has no extent along it.",
    ),
    shape(
        "shape.major_axis_dir_z",
        "Major axis direction z (superior +)",
        "",
        "z component of the unit vector along the major axis in patient LPS coordinates (superior +), the sign chosen so that the largest component of the vector is positive. Undefined when the axis is not unique: two axes are equally long, or the segment has no extent along it.",
    ),
    shape(
        "shape.minor_axis_dir_x",
        "Minor axis direction x (left +)",
        "",
        "x component of the unit vector along the minor axis in patient LPS coordinates (left +), the sign chosen so that the largest component of the vector is positive. Undefined when the axis is not unique: two axes are equally long, or the segment has no extent along it.",
    ),
    shape(
        "shape.minor_axis_dir_y",
        "Minor axis direction y (posterior +)",
        "",
        "y component of the unit vector along the minor axis in patient LPS coordinates (posterior +), the sign chosen so that the largest component of the vector is positive. Undefined when the axis is not unique: two axes are equally long, or the segment has no extent along it.",
    ),
    shape(
        "shape.minor_axis_dir_z",
        "Minor axis direction z (superior +)",
        "",
        "z component of the unit vector along the minor axis in patient LPS coordinates (superior +), the sign chosen so that the largest component of the vector is positive. Undefined when the axis is not unique: two axes are equally long, or the segment has no extent along it.",
    ),
    shape(
        "shape.least_axis_dir_x",
        "Least axis direction x (left +)",
        "",
        "x component of the unit vector along the least axis in patient LPS coordinates (left +), the sign chosen so that the largest component of the vector is positive. Undefined when the axis is not unique: two axes are equally long, or the segment has no extent along it.",
    ),
    shape(
        "shape.least_axis_dir_y",
        "Least axis direction y (posterior +)",
        "",
        "y component of the unit vector along the least axis in patient LPS coordinates (posterior +), the sign chosen so that the largest component of the vector is positive. Undefined when the axis is not unique: two axes are equally long, or the segment has no extent along it.",
    ),
    shape(
        "shape.least_axis_dir_z",
        "Least axis direction z (superior +)",
        "",
        "z component of the unit vector along the least axis in patient LPS coordinates (superior +), the sign chosen so that the largest component of the vector is positive. Undefined when the axis is not unique: two axes are equally long, or the segment has no extent along it.",
    ),
    shape(
        "shape.hull_volume_ml",
        "Convex hull volume",
        "mL",
        "Volume of the smallest convex solid that contains the surface mesh (the convex hull of the mesh vertices), in mL. Undefined when the mesh is flat.",
    ),
    shape(
        "shape.hull_area_mm2",
        "Convex hull area",
        "mm2",
        "Surface area of the convex hull of the mesh vertices, in mm². Undefined when the mesh is flat.",
    ),
    shape(
        "shape.solidity",
        "Solidity",
        "",
        "shape.mesh_volume_ml divided by shape.hull_volume_ml: 1 for a convex body, smaller the more the segment is dented, hollowed or split into pieces.",
    ),
    shape(
        "shape.convexity",
        "Convexity",
        "",
        "Area of the convex hull divided by the surface area of the mesh: 1 for a convex body; for a connected body it is at most 1 and drops as the surface gets more folded, for separate pieces it can exceed 1.",
    ),
    topology(
        "topology.components",
        "Pieces",
        "",
        "Number of 26-connected pieces (voxels that touch by a face, an edge or a corner are one piece). Warning `fragmented` when the largest piece holds less than 95 % of the voxels.",
    ),
    topology(
        "topology.largest_component_share",
        "Largest piece share",
        "",
        "Share of the voxels that belong to the largest piece, 0 to 1. Warning `fragmented` below 0.95.",
    ),
    topology(
        "topology.holes",
        "Cavities",
        "",
        "Number of cavities: background enclosed by the segment and not connected to the outside (6-connected background). A tunnel that opens to the outside is not a cavity. Warning `has_holes` when there is at least one.",
    ),
    topology(
        "topology.hole_volume_mm3",
        "Cavity volume (mm³)",
        "mm3",
        "Volume of all cavities together: their voxel count times the volume of one voxel, in cubic millimetres.",
    ),
    topology(
        "topology.euler_number",
        "Euler number",
        "",
        "Euler characteristic of the segment as a union of closed voxels (26-connected): pieces minus tunnels plus cavities. 1 for a solid body, 2 for a body with one cavity, 0 for a ring.",
    ),
    topology(
        "topology.border_sides",
        "Image sides touched",
        "",
        "How many of the six sides of the image box the segment touches (0 to 6). Warning `touches_border` when it is above 0: the segment may be cut off by the field of view.",
    ),
    intensity("intensity.mean", "Mean", "Mean of the voxel values inside the segment."),
    intensity(
        "intensity.std",
        "Standard deviation",
        "Population standard deviation (divisor N, not N - 1) of the voxel values inside the segment.",
    ),
    intensity("intensity.min", "Minimum", "Lowest voxel value inside the segment."),
    intensity("intensity.max", "Maximum", "Highest voxel value inside the segment."),
    intensity("intensity.range", "Range", "intensity.max minus intensity.min."),
    intensity(
        "intensity.p5",
        "5th percentile",
        "5th percentile of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.05).",
    ),
    intensity(
        "intensity.p25",
        "25th percentile",
        "25th percentile of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.25).",
    ),
    intensity(
        "intensity.p50",
        "Median",
        "Median (50th percentile) of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.5).",
    ),
    intensity(
        "intensity.p75",
        "75th percentile",
        "75th percentile of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.75).",
    ),
    intensity(
        "intensity.p95",
        "95th percentile",
        "95th percentile of the voxel values, nearest rank: the sorted value at index round((N - 1) * 0.95).",
    ),
    intensity(
        "intensity.iqr",
        "Interquartile range",
        "Interquartile range: intensity.p75 minus intensity.p25 (nearest-rank percentiles).",
    ),
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
            assert!(!f.label.is_empty(), "{} has no label", f.id);
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
    fn labels_are_unique_within_a_family() {
        let mut seen = HashSet::new();
        for f in FEATURES {
            assert!(seen.insert((f.family, f.label)), "two {} features are called {:?}", f.family.name(), f.label);
        }
    }

    #[test]
    fn only_intensity_features_need_a_unit() {
        for f in FEATURES {
            assert_eq!(f.needs_unit, f.family == Family::Intensity, "{}", f.id);
            assert_eq!(matches!(f.unit, Unit::Value), f.family == Family::Intensity, "{}", f.id);
        }
    }
}
