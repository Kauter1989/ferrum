//! `segment list|threshold|rename|delete`: segments saved in the
//! workspace (label map + `ferrum-segments` sidecar).

use ferrum_domain::{grow_region, Author};
use ferrum_io::provenance::provenance_json;
use serde_json::{json, Value};

use super::annotate::agent_provenance;
use super::inspect::point;
use super::Ctx;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::points::{describe, nearest_voxel};
use crate::study::Study;

/// One segment as JSON (`null` for an unknown label).
pub fn segment_json(study: &Study, label: u8) -> Value {
    let set = &study.segments;
    let Some(s) = set.segment(label) else { return Value::Null };
    json!({
        "label": s.label,
        "name": s.name,
        "color": s.color,
        "voxels": set.voxel_count(label),
        "volume_ml": (set.volume_ml(label, study.volume.spacing()) * 1000.0).round() / 1000.0,
        "provenance": provenance_json(&s.provenance),
    })
}

/// `segment list`: every segment with its volume and provenance.
pub fn list(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let items: Vec<Value> = study.segments.segments().iter().map(|s| segment_json(study, s.label)).collect();
    Ok(Output::new(json!({ "segments": items })))
}

/// `segment threshold`: region growing from a seed within a value range;
/// the result is a new segment proposed by the agent.
pub fn threshold(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let provenance = agent_provenance(p)?;
    let study = ctx.study(p)?;
    let seed_point = point(study, p, "seed")?;
    let seed = nearest_voxel(&study.volume, seed_point);
    let (lo, hi) = (p.req_f64("min")? as f32, p.req_f64("max")? as f32);
    if lo > hi {
        return Err(AgentError::bad_request("min must not exceed max"));
    }
    let value = study.volume.physical(seed[0], seed[1], seed[2]).unwrap_or(f32::NAN);
    if !(lo..=hi).contains(&value) {
        return Err(AgentError::bad_request(format!("the seed value {value:.1} is outside [{lo}, {hi}]"))
            .hint("probe the seed first and choose a range that contains it"));
    }
    if study.segments.labels().label(seed[0], seed[1], seed[2]) != Some(0) {
        return Err(AgentError::bad_request("the seed lies in an existing segment"));
    }
    let s = study.volume.spacing();
    let voxel_ml = f64::from(s.x) * f64::from(s.y) * f64::from(s.z) / 1000.0;
    let max_ml = p.f64("max_ml")?.unwrap_or(f64::INFINITY);
    let max_voxels = if max_ml.is_finite() { (max_ml / voxel_ml).floor() as u64 } else { u64::MAX };
    let seed = glam::UVec3::from(seed);
    let (bx, mask) = grow_region(&study.volume, &study.segments, seed, lo, hi, max_voxels).ok_or_else(|| {
        AgentError::new(ErrorCode::Limit, format!("the region grows beyond {max_ml} ml"))
            .hint("narrow the value range or move the seed; the region may leak into neighbouring tissue")
    })?;
    let name = p.str("name")?.unwrap_or("Threshold region");
    let label = study.segments.add_segment(name).map_err(|e| AgentError::new(ErrorCode::Limit, e.to_string()))?;
    let set = &mut study.segments;
    set.set_provenance(label, provenance).map_err(|e| AgentError::internal(e.to_string()))?;
    set.apply_mask(label, bx, &mask, false).map_err(|e| AgentError::internal(e.to_string()))?;
    study.save_segments()?;
    let mut data = json!({ "segment": segment_json(study, label) });
    data["seed"] = describe(&study.volume, seed_point);
    data["method"] =
        json!(format!("6-connected region growing within [{lo}, {hi}] {}", study.value_unit()).trim_end().to_owned());
    Ok(Output::new(data))
}

fn own_segment(study: &Study, p: &Params) -> Result<u8, AgentError> {
    let label = p.u64("label")?.ok_or_else(|| AgentError::bad_request("label is required"))?;
    let seg = u8::try_from(label).ok().and_then(|l| study.segments.segment(l));
    let seg = seg.ok_or_else(|| AgentError::not_found(format!("unknown segment {label}")).hint("see segment list"))?;
    if !matches!(seg.provenance.author, Author::Agent { .. }) {
        return Err(AgentError::new(
            ErrorCode::Forbidden,
            format!(
                "segment {label} was created by a {}; agents may change only their own segments",
                seg.provenance.author.kind()
            ),
        ));
    }
    Ok(seg.label)
}

/// `segment rename`: renames a segment the agent created.
pub fn rename(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let label = own_segment(study, p)?;
    if !study.segments.rename(label, p.req_str("name")?) {
        return Err(AgentError::bad_request("the name must not be empty"));
    }
    study.save_segments()?;
    Ok(Output::new(json!({ "segment": segment_json(study, label) })))
}

/// `segment delete`: deletes a segment the agent created.
pub fn delete(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let label = own_segment(study, p)?;
    study.segments.remove_segment(label).map_err(|e| AgentError::internal(e.to_string()))?;
    study.save_segments()?;
    Ok(Output::new(json!({ "deleted": label })))
}
