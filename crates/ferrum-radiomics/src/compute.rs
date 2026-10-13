//! The entry point: features of a segment under a profile.

use ferrum_domain::{MaskRegion, Volume};

use crate::diameter::max_diameter_mm;
use crate::distribution::{distribution, hu_range_shares, ivh_curve, Distribution, HuRangeShares, IvhCurve, IDS};
use crate::error::MetricsError;
use crate::intensity::{first_order_sorted, region_values, FirstOrder};
use crate::params::{Params, ValueUnit};
use crate::profile::Profile;
use crate::result::{unit_label, FeatureValue, MetricsResult, Value};
use crate::shape::{shape3d, HullStats, Shape3d};
use crate::size::{size, Size};
use crate::topology::{topology, warnings, Topology};

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
    topology: Option<Topology>,
    diameter: Option<f64>,
    voxel_mm3: f64,
    values: Option<Vec<f32>>,
    intensity: Option<FirstOrder>,
    distribution: Option<Distribution>,
}

impl Measured {
    fn new(region: &MaskRegion, volume: &Volume, profile: &Profile, params: &Params) -> Self {
        let has = |prefix: &str| profile.features.iter().any(|id| id.starts_with(prefix));
        let wants_values = has("intensity.") || profile.ivh_curve || profile.hu_ranges;
        let values = wants_values.then(|| {
            let mut v = region_values(region, volume);
            v.sort_by(f32::total_cmp);
            v
        });
        let intensity = values.as_deref().and_then(first_order_sorted);
        let wants_distribution = profile.features.iter().any(|id| IDS.contains(id));
        let distribution = match (&values, &intensity) {
            (Some(v), Some(f)) if wants_distribution => Some(distribution(v, f, params.bin_width)),
            _ => None,
        };
        Self {
            size: has("size.").then(|| size(region, volume)),
            shape: has("shape.").then(|| {
                shape3d(region, volume, has("shape.hull_") || has("shape.solidity") || has("shape.convexity"))
            }),
            topology: has("topology.").then(|| topology(region, volume.dims())),
            diameter: has("size.max_diameter_mm").then(|| max_diameter_mm(region, volume.spacing())),
            voxel_mm3: voxel_mm3(volume),
            values,
            intensity,
            distribution,
        }
    }

    fn value(&self, id: &str) -> Value {
        let missing = || Value::Null { reason: "not computed".to_owned() };
        let Some((family, _)) = id.split_once('.') else { return missing() };
        match family {
            "size" if id == "size.max_diameter_mm" => {
                self.diameter.map_or_else(missing, |d| Value::number(d, "not a finite number"))
            }
            "size" => self.size.as_ref().map_or_else(missing, |s| size_value(s, id)),
            "topology" => self.topology.as_ref().map_or_else(missing, |t| topology_value(t, self.voxel_mm3, id)),
            "shape" => self.shape.as_ref().map_or_else(missing, |s| shape_value(s, id)),
            "intensity" => self.intensity.as_ref().map_or_else(
                || Value::Null { reason: "the segment has no voxel values".to_owned() },
                |f| intensity_value(f, self.distribution.as_ref(), id),
            ),
            _ => missing(),
        }
    }
}

fn voxel_mm3(volume: &Volume) -> f64 {
    let s = volume.spacing();
    f64::from(s.x) * f64::from(s.y) * f64::from(s.z)
}

fn topology_value(t: &Topology, voxel_mm3: f64, id: &str) -> Value {
    let number = |x: f64| Value::number(x, "not a finite number");
    match id {
        "topology.components" => number(t.components as f64),
        "topology.largest_component_share" => number(t.largest_share),
        "topology.holes" => number(t.holes as f64),
        "topology.hole_volume_mm3" => number(t.hole_voxels as f64 * voxel_mm3),
        "topology.euler_number" => number(t.euler as f64),
        "topology.border_sides" => number(f64::from(t.border_sides)),
        _ => Value::Null { reason: "not computed".to_owned() },
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
    let no_hull = "the surface is flat or a single point, so it has no convex hull";
    let hull = |f: fn(&HullStats) -> f64| number(s.hull.as_ref().map(f), no_hull);
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
        "shape.hull_volume_ml" => hull(|h| h.volume_mm3 / 1000.0),
        "shape.hull_area_mm2" => hull(|h| h.area_mm2),
        "shape.solidity" => number(s.solidity(), no_hull),
        "shape.convexity" => number(s.convexity(), no_hull),
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

fn intensity_value(f: &FirstOrder, d: Option<&Distribution>, id: &str) -> Value {
    let number = |x: f64| Value::number(x, "not a finite number");
    let flat = "all voxel values are equal, so the spread is zero";
    let moment = |x: Option<f64>| x.map_or_else(|| Value::Null { reason: flat.to_owned() }, number);
    if let Some(d) = d {
        match id {
            "intensity.skewness" => return moment(d.skewness),
            "intensity.kurtosis" => return moment(d.kurtosis),
            "intensity.energy" => return number(d.energy),
            "intensity.rms" => return number(d.rms),
            "intensity.mad" => return number(d.mad),
            "intensity.rmad" => return number(d.rmad),
            "intensity.entropy" => return number(d.entropy),
            "intensity.uniformity" => return number(d.uniformity),
            "intensity.ivh_v10" => return number(d.ivh_v10),
            "intensity.ivh_v90" => return number(d.ivh_v90),
            "intensity.ivh_i10" => return number(d.ivh_i10),
            "intensity.ivh_i90" => return number(d.ivh_i90),
            _ => {}
        }
    }
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

/// The intensity-volume histogram and the HU-range shares, for the profiles
/// that carry them.
fn curves(measured: &Measured, profile: &Profile, params: &Params) -> (Option<IvhCurve>, Option<HuRangeShares>) {
    let Some(values) = measured.values.as_deref().filter(|v| !v.is_empty()) else { return (None, None) };
    let (ranges, is_default) = params.ranges();
    (
        profile.ivh_curve.then(|| ivh_curve(values)),
        profile.hu_ranges.then(|| hu_range_shares(values, &ranges, is_default)),
    )
}

/// Computes the features of `profile` for the segment in `input`.
///
/// Fails if the profile needs the unit of the voxel values and it is not
/// known, or if the region is empty. Same input and parameters always give
/// the same result, bit for bit.
pub fn compute(input: &Input, profile: &Profile, params: &Params) -> Result<MetricsResult, MetricsError> {
    params.validate()?;
    check_unit(&input.unit, profile)?;
    let region = input.region.cropped().ok_or(MetricsError::EmptyRegion)?;
    let measured = Measured::new(&region, input.volume, profile, params);
    let (ivh, hu_ranges) = curves(&measured, profile, params);
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
        params: params.clone(),
        bin_width: profile.features.contains(&"intensity.entropy").then_some(params.bin_width),
        features,
        ivh,
        hu_ranges,
        warnings: measured.topology.as_ref().map_or_else(Vec::new, warnings),
    })
}
