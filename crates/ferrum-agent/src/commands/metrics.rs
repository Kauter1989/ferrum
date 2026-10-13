//! `segment metrics`: the quantitative profile of a segment (size,
//! first-order HU statistics; more families with later profiles), computed
//! by `ferrum-radiomics` from the label map and the voxel values.

use ferrum_radiomics::{compute, Input, MetricsError, MetricsResult, Profile, Value, ValueUnit};
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
        MetricsError::EmptyRegion => AgentError::not_found("the segment has no voxels"),
    }
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
    let result = compute(&input, profile, &Default::default()).map_err(metrics_error)?;
    let mut out = Output::new(json!({
        "segment": segment_json(study, label),
        "profile": { "name": result.profile, "version": result.profile_version },
        "unit": result.unit.label(),
        "params": { "preprocessing": result.params.preprocessing.name() },
        "features": features_json(&result, describe),
        "method": "from the label map and the voxel values; extents are voxel counts × spacing, the centroid is the \
                   mean of the voxel centres in LPS millimetres, percentiles are nearest rank, std is the population \
                   standard deviation; shape: area and volume of a marching-cubes surface mesh, principal axes from \
                   the population covariance of the voxel centres in millimetres, convex hull of the mesh vertices; \
                   maximum 3D diameter between voxel centres; connectivity: 26-connected pieces, 6-connected \
                   cavities, Euler characteristic of the closed voxels; six significant digits",
    }));
    for w in &result.warnings {
        out = out.warn(w.clone());
    }
    Ok(out)
}
