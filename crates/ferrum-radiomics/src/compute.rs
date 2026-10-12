//! The entry point: features of a segment under a profile.

use ferrum_domain::{MaskRegion, Volume};

use crate::error::MetricsError;
use crate::intensity::{first_order, region_values, FirstOrder};
use crate::params::{Params, ValueUnit};
use crate::profile::Profile;
use crate::result::{unit_label, FeatureValue, MetricsResult, Value};
use crate::shape::{shape3d, Shape3d};
use crate::size::{size, Size};

/// What is measured.
#[derive(Debug, Clone)]
pub struct Input<'a> {
    /// The voxels of the segment.
    pub region: &'a MaskRegion,
    /// The image the segment lies on.
    pub volume: &'a Volume,
    /// Unit of the voxel values.
    pub unit: ValueUnit,
}

/// The measures a profile needs, computed once.
struct Measured {
    size: Option<Size>,
    shape: Option<Shape3d>,
    intensity: Option<FirstOrder>,
}

impl Measured {
    fn new(region: &MaskRegion, volume: &Volume, profile: &Profile) -> Self {
        let has = |prefix: &str| profile.features.iter().any(|id| id.starts_with(prefix));
        Self {
            size: has("size.").then(|| size(region, volume)),
            shape: has("shape.").then(|| shape3d(region, volume)),
            intensity: if has("intensity.") { first_order(region_values(region, volume)) } else { None },
        }
    }

    fn value(&self, id: &str) -> Value {
        let missing = || Value::Null { reason: "not computed".to_owned() };
        let Some((family, _)) = id.split_once('.') else { return missing() };
        match family {
            "size" => self.size.as_ref().map_or_else(missing, |s| size_value(s, id)),
            "shape" => self.shape.as_ref().map_or_else(missing, |s| shape_value(s, id)),
            "intensity" => self.intensity.as_ref().map_or_else(
                || Value::Null { reason: "the segment has no voxel values".to_owned() },
                |f| intensity_value(f, id),
            ),
            _ => missing(),
        }
    }
}

fn axis_value(axis: Option<f64>) -> Value {
    axis.map_or_else(
        || Value::Null { reason: "the segment has no axial slice to measure on".to_owned() },
        Value::Number,
    )
}

fn size_value(s: &Size, id: &str) -> Value {
    let number = |x: f64| Value::number(x, "not a finite number");
    match id {
        "size.voxels" => number(s.voxels as f64),
        "size.volume_mm3" => number(s.volume_mm3),
        "size.volume_ml" => number(s.volume_mm3 / 1000.0),
        "size.extent_i_mm" => number(s.extent_mm[0]),
        "size.extent_j_mm" => number(s.extent_mm[1]),
        "size.extent_k_mm" => number(s.extent_mm[2]),
        "size.centroid_x_mm" => number(s.centroid_mm.x),
        "size.centroid_y_mm" => number(s.centroid_mm.y),
        "size.centroid_z_mm" => number(s.centroid_mm.z),
        "size.long_axis_mm" => axis_value(s.long_axis_mm),
        "size.short_axis_mm" => axis_value(s.short_axis_mm),
        "size.largest_slice_axial" => number(f64::from(s.largest_slice[0])),
        "size.largest_slice_coronal" => number(f64::from(s.largest_slice[1])),
        "size.largest_slice_sagittal" => number(f64::from(s.largest_slice[2])),
        _ => Value::Null { reason: "not computed".to_owned() },
    }
}

fn shape_value(s: &Shape3d, id: &str) -> Value {
    let number = |x: Option<f64>, why: &str| match x {
        Some(x) => Value::number(x, why),
        None => Value::Null { reason: why.to_owned() },
    };
    let no_surface = "the surface encloses no volume";
    let no_extent = "the segment has no extent";
    let not_unique =
        "the axis direction is not unique: two axes are equally long, or the segment has no extent along it";
    let direction = |axis: usize, component: usize| number(s.axes.directions[axis].map(|d| d[component]), not_unique);
    match id {
        "shape.mesh_volume_ml" => number(Some(s.mesh_volume_mm3 / 1000.0), no_surface),
        "shape.surface_area_mm2" => number(Some(s.area_mm2), no_surface),
        "shape.surface_to_volume_per_mm" => number(s.surface_to_volume(), no_surface),
        "shape.compactness_1" => number(s.compactness_1(), no_surface),
        "shape.compactness_2" => number(s.compactness_2(), no_surface),
        "shape.sphericity" => number(s.sphericity(), no_surface),
        "shape.spherical_disproportion" => number(s.spherical_disproportion(), no_surface),
        "shape.major_axis_mm" => number(Some(s.axes.lengths_mm[0]), no_extent),
        "shape.minor_axis_mm" => number(Some(s.axes.lengths_mm[1]), no_extent),
        "shape.least_axis_mm" => number(Some(s.axes.lengths_mm[2]), no_extent),
        "shape.elongation" => number(s.axes.elongation(), no_extent),
        "shape.flatness" => number(s.axes.flatness(), no_extent),
        "shape.major_axis_dir_x" => direction(0, 0),
        "shape.major_axis_dir_y" => direction(0, 1),
        "shape.major_axis_dir_z" => direction(0, 2),
        "shape.minor_axis_dir_x" => direction(1, 0),
        "shape.minor_axis_dir_y" => direction(1, 1),
        "shape.minor_axis_dir_z" => direction(1, 2),
        "shape.least_axis_dir_x" => direction(2, 0),
        "shape.least_axis_dir_y" => direction(2, 1),
        "shape.least_axis_dir_z" => direction(2, 2),
        _ => Value::Null { reason: "not computed".to_owned() },
    }
}

fn intensity_value(f: &FirstOrder, id: &str) -> Value {
    let number = |x: f64| Value::number(x, "not a finite number");
    match id {
        "intensity.mean" => number(f.mean),
        "intensity.std" => number(f.std),
        "intensity.min" => number(f.min),
        "intensity.max" => number(f.max),
        "intensity.range" => number(f.range()),
        "intensity.p5" => number(f.p5),
        "intensity.p25" => number(f.p25),
        "intensity.p50" => number(f.p50),
        "intensity.p75" => number(f.p75),
        "intensity.p95" => number(f.p95),
        "intensity.iqr" => number(f.iqr()),
        _ => Value::Null { reason: "not computed".to_owned() },
    }
}

/// Refuses a profile with intensity features when the unit is unknown.
fn check_unit(unit: &ValueUnit, profile: &Profile) -> Result<(), MetricsError> {
    if unit.is_known() {
        return Ok(());
    }
    let features: Vec<&'static str> = profile.features().filter(|f| f.needs_unit).map(|f| f.id).collect();
    if features.is_empty() {
        return Ok(());
    }
    let modality = match unit {
        ValueUnit::Unknown { modality } => modality.clone(),
        ValueUnit::Hu => String::new(),
    };
    Err(MetricsError::UnitUnknown { modality, features })
}

/// Computes the features of `profile` for the segment in `input`.
///
/// Fails if the profile needs the unit of the voxel values and it is not
/// known, or if the region is empty. Same input and parameters always give
/// the same result, bit for bit.
pub fn compute(input: &Input, profile: &Profile, params: &Params) -> Result<MetricsResult, MetricsError> {
    check_unit(&input.unit, profile)?;
    let region = input.region.cropped().ok_or(MetricsError::EmptyRegion)?;
    let measured = Measured::new(&region, input.volume, profile);
    let features = profile
        .features()
        .map(|feature| FeatureValue {
            feature,
            value: measured.value(feature.id),
            unit: unit_label(feature, &input.unit),
        })
        .collect();
    Ok(MetricsResult {
        profile: profile.name,
        profile_version: profile.version,
        unit: input.unit.clone(),
        params: *params,
        features,
        warnings: Vec::new(),
    })
}
