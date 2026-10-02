//! `segment list|threshold|rename|delete`: segments saved in the
//! workspace (label map + `ferrum-segments` sidecar).

use std::collections::VecDeque;

use ferrum_domain::{Author, SegmentationSet, Volume, VoxelBox};
use ferrum_io::provenance::provenance_json;
use glam::UVec3;
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

/// 6-connected region of unlabelled voxels with values in `[lo, hi]`,
/// grown from `seed`. Returns the region's bounding box and its mask, or
/// `None` once it exceeds `max_voxels`.
pub fn grow(
    v: &Volume,
    labels: &SegmentationSet,
    seed: [u32; 3],
    lo: f32,
    hi: f32,
    max_voxels: u64,
) -> Option<(VoxelBox, Vec<u8>)> {
    let d = v.dims();
    let (nx, ny, nz) = (d.x as usize, d.y as usize, d.z as usize);
    let accept = |i: u32, j: u32, k: u32| {
        labels.labels().label(i, j, k) == Some(0) && v.physical(i, j, k).is_some_and(|x| (lo..=hi).contains(&x))
    };
    let mut inside = vec![false; nx * ny * nz];
    let mut queue = VecDeque::from([seed]);
    let at = |i: u32, j: u32, k: u32| i as usize + j as usize * nx + k as usize * nx * ny;
    inside[at(seed[0], seed[1], seed[2])] = true;
    let (mut lo_c, mut hi_c, mut count) = (UVec3::from(seed), UVec3::from(seed), 0u64);
    while let Some([i, j, k]) = queue.pop_front() {
        count += 1;
        if count > max_voxels {
            return None;
        }
        lo_c = lo_c.min(UVec3::new(i, j, k));
        hi_c = hi_c.max(UVec3::new(i, j, k));
        let neighbours = [
            (i.checked_sub(1), Some(j), Some(k)),
            ((i + 1 < d.x).then_some(i + 1), Some(j), Some(k)),
            (Some(i), j.checked_sub(1), Some(k)),
            (Some(i), (j + 1 < d.y).then_some(j + 1), Some(k)),
            (Some(i), Some(j), k.checked_sub(1)),
            (Some(i), Some(j), (k + 1 < d.z).then_some(k + 1)),
        ];
        for (a, b, c) in neighbours {
            let (Some(a), Some(b), Some(c)) = (a, b, c) else { continue };
            if !inside[at(a, b, c)] && accept(a, b, c) {
                inside[at(a, b, c)] = true;
                queue.push_back([a, b, c]);
            }
        }
    }
    let bx = VoxelBox::new(lo_c, hi_c + UVec3::ONE);
    let mut mask = Vec::with_capacity(bx.voxel_count());
    for k in bx.min.z..bx.max.z {
        for j in bx.min.y..bx.max.y {
            for i in bx.min.x..bx.max.x {
                mask.push(u8::from(inside[at(i, j, k)]));
            }
        }
    }
    Some((bx, mask))
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
    let (bx, mask) = grow(&study.volume, &study.segments, seed, lo, hi, max_voxels).ok_or_else(|| {
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
