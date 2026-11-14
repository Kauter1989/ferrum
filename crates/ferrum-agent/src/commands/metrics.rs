//! `segment metrics`: the quantitative profile of a segment (size,
//! first-order HU statistics; more families with later profiles), computed
//! by `ferrum-radiomics` from the label map and the voxel values.

use ferrum_radiomics::{
    compute, HuRange, Input, MetricsError, MetricsResult, Params as MetricsParams, Profile, Value, ValueUnit,
};
use serde_json::{json, Map, Value as Json};

use super::masks::{region, segment_label};
use super::segment::segment_json;
use super::Ctx;
use crate::envelope::{AgentError, Output};
use crate::params::Params;

/// Significant digits kept in the envelope; whole numbers up to a million
/// are unaffected, and small figures (a compactness of 0.0412) keep theirs.
const SIGNIFICANT: i32 = 6;

fn round(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let decimals = (SIGNIFICANT - 1 - x.abs().log10().floor() as i32).clamp(0, 15);
    let f = 10f64.powi(decimals);
    (x * f).round() / f
}

fn value_json(v: &Value) -> Json {
    match v {
        Value::Number(x) => json!(round(*x)),
        Value::Null { .. } => Json::Null,
    }
}

/// The features of `result` as an object keyed by id, in profile order.
fn features_json(result: &MetricsResult, describe: bool) -> Json {
    let mut out = Map::new();
    for f in &result.features {
        let mut item = Map::new();
        item.insert("value".into(), value_json(&f.value));
        item.insert("unit".into(), json!(f.unit));
        if let Value::Null { reason } = &f.value {
            item.insert("reason".into(), json!(reason));
        }
        if describe {
            item.insert("definition".into(), json!(f.feature.definition));
        }
        out.insert(f.feature.id.to_owned(), Json::Object(item));
    }
    Json::Object(out)
}

/// The error of a failed computation as the agent sees it.
fn metrics_error(e: MetricsError) -> AgentError {
    match &e {
        MetricsError::UnknownProfile { known, .. } => {
            AgentError::bad_request(e.to_string()).hint(format!("profile is one of: {}", known.join(", ")))
        }
        MetricsError::UnitUnknown { modality, .. } if modality.is_empty() => AgentError::bad_request(
            "the unit of the voxel values is unknown: the study carries no modality (NIfTI has none)",
        )
        .hint("declare it: study open --modality CT"),
        MetricsError::UnitUnknown { modality, .. } => AgentError::bad_request(format!(
            "the unit of the voxel values is unknown: intensity features need a CT study in HU, this one is {modality}"
        ))
        .hint("use segment shape for size and shape, which do not need the unit"),
        MetricsError::BadParameter(what) => AgentError::bad_request(what.clone())
            .hint("bin_width is a positive number of HU; hu_ranges is a list of [min, max] pairs, half-open and not overlapping"),
        MetricsError::EmptyRegion => AgentError::not_found("the segment has no voxels"),
    }
}

/// `[min, max]` pairs of finite numbers.
fn hu_ranges(p: &Params) -> Result<Option<Vec<HuRange>>, AgentError> {
    let bad = || {
        AgentError::bad_request("hu_ranges must be a list of [min, max] pairs of numbers, e.g. [[-100, 0], [0, 100]]")
    };
    p.list("hu_ranges")?
        .map(|list| {
            list.iter()
                .map(|v| match v.as_array().map(Vec::as_slice) {
                    Some([lo, hi]) => match (lo.as_f64(), hi.as_f64()) {
                        (Some(lo), Some(hi)) if lo.is_finite() && hi.is_finite() => Ok(HuRange::new(lo, hi)),
                        _ => Err(bad()),
                    },
                    _ => Err(bad()),
                })
                .collect()
        })
        .transpose()
}

/// The parameters of the computation that the caller gave.
fn metrics_params(p: &Params) -> Result<MetricsParams, AgentError> {
    let mut params = MetricsParams::default();
    if let Some(w) = p.f64("bin_width")? {
        params.bin_width = w;
    }
    params.hu_ranges = hu_ranges(p)?;
    Ok(params)
}

/// The parameters the numbers depend on, as recorded in the envelope.
fn params_json(result: &MetricsResult) -> Json {
    let mut out = Map::new();
    out.insert("preprocessing".into(), json!(result.params.preprocessing.name()));
    if let Some(w) = result.bin_width {
        out.insert("bin_width".into(), json!(w));
        out.insert("bin_width_unit".into(), json!(result.unit.label()));
    }
    Json::Object(out)
}

/// The intensity-volume histogram and the HU-range shares, when the profile has them.
fn curves_json(result: &MetricsResult) -> Map<String, Json> {
    let mut out = Map::new();
    if let Some(c) = &result.ivh {
        let round_all = |v: &[f64]| v.iter().map(|x| round(*x)).collect::<Vec<_>>();
        out.insert(
            "ivh".into(),
            json!({
                "points": "101 samples, at 0, 1, … 100 % of the way from the lowest to the highest value",
                "intensity": round_all(&c.intensity),
                "volume_fraction": round_all(&c.volume_fraction),
            }),
        );
    }
    if let Some(s) = &result.hu_ranges {
        let ranges: Vec<Json> = s
            .ranges
            .iter()
            .map(|r| json!({ "min": r.range.lo, "max": r.range.hi, "voxels": r.voxels, "fraction": round(r.fraction) }))
            .collect();
        let note = if s.is_default {
            "default ranges for orientation, not clinical thresholds; pass hu_ranges to choose your own"
        } else {
            "ranges as given"
        };
        out.insert(
            "hu_ranges".into(),
            json!({ "ranges": ranges, "outside": round(s.outside), "default": s.is_default, "note": note,
                    "definition": "share of the segment's voxels with min <= value < max; outside is the share in none of the ranges" }),
        );
    }
    out
}

/// `segment metrics`: the features of a profile for one segment.
pub fn metrics(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let profile = Profile::by_name(p.str("profile")?.unwrap_or("quick")).map_err(metrics_error)?;
    let describe = p.bool("describe")?.unwrap_or(false);
    let study = ctx.study(p)?;
    let label = segment_label(study, p, "segment")?;
    let r = region(&study.segments, label).ok_or_else(|| {
        AgentError::not_found(format!("segment {label} has no voxels"))
            .hint("see segment list for the volume of each segment")
    })?;
    let unit = ValueUnit::from_modality(&study.metadata.modality);
    let input = Input { region: &r, volume: &study.volume, unit };
    let params = metrics_params(p)?;
    let result = compute(&input, profile, &params).map_err(metrics_error)?;
    let mut data = json!({
        "segment": segment_json(study, label),
        "profile": { "name": result.profile, "version": result.profile_version },
        "unit": result.unit.label(),
        "params": params_json(&result),
        "features": features_json(&result, describe),
        "method": "from the label map and the voxel values; extents are voxel counts × spacing, the centroid is the \
                   mean of the voxel centres in LPS millimetres, percentiles are nearest rank, std is the population \
                   standard deviation; shape: area and volume of a marching-cubes surface mesh, principal axes from \
                   the population covariance of the voxel centres in millimetres, convex hull of the mesh vertices; \
                   maximum 3D diameter between voxel centres; connectivity: 26-connected pieces, 6-connected \
                   cavities, Euler characteristic of the closed voxels; distribution: moments and deviations of the voxel values, \
                   histogram entropy and uniformity on bins of bin_width anchored at 0 HU, the intensity-volume histogram on the \
                   voxel values; six significant digits",
    });
    if let Some(map) = data.as_object_mut() {
        map.extend(curves_json(&result));
    }
    let mut out = Output::new(data);
    for w in &result.warnings {
        out = out.warn(w.clone());
    }
    Ok(out)
}
