//! `study scan`, `study open`, `study info`.

use std::path::PathBuf;

use ferrum_domain::{SeriesMetadata, SliceAxis, WindowPreset};
use serde_json::{json, Value};

use super::Ctx;
use crate::config::AgentConfig;
use crate::envelope::{AgentError, Output};
use crate::params::Params;
use crate::study::{scan as scan_series, series_key, Study};

/// `study scan`: lists the series in files and folders (no identifiers).
pub fn scan(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let paths = p.list("paths")?.ok_or_else(|| AgentError::bad_request("paths is required"))?;
    let paths = paths
        .iter()
        .map(|v| {
            let s = v.as_str().ok_or_else(|| AgentError::bad_request("paths must be a list of paths"))?;
            ctx.config.check_source(std::path::Path::new(s))
        })
        .collect::<Result<Vec<PathBuf>, _>>()?;
    let series: Vec<Value> = scan_series(&paths)?
        .iter()
        .map(|s| {
            json!({
                "series": series_key(ctx.config, &s.id),
                "format": s.format,
                "modality": s.modality,
                "dims": [s.dims.x, s.dims.y, s.dims.z],
                "description": s.description,
                "files": s.sources.len(),
            })
        })
        .collect();
    Ok(Output::new(json!({ "series": series })))
}

/// `study open`: loads a series into a workspace (created if needed).
pub fn open(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let root = ctx.workspace_path(p)?;
    let source = PathBuf::from(p.req_str("path")?);
    let study = Study::open(ctx.config, &root, &source, p.str("series")?, ctx.cache.as_deref_mut())?;
    let out = describe(ctx.config, &study);
    ctx.study = Some(study);
    Ok(out)
}

/// `study info`: describes the open study, its segments and annotations.
pub fn info(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let config = ctx.config;
    let study = ctx.study(p)?;
    let mut out = describe(config, study);
    out.data["annotations"] =
        json!({ "count": study.annotations.len(), "pending_review": study.annotations.pending() });
    out.data["segments"] =
        json!({ "count": study.segments.segments().len(), "pending_review": study.segments.pending() });
    Ok(out)
}

/// What an agent needs to know about a study, without identifiers unless
/// the operator allows them.
fn describe(config: &AgentConfig, study: &Study) -> Output {
    let v = &study.volume;
    let d = v.dims();
    let s = v.spacing();
    let g = v.geometry();
    let m = &study.metadata;
    let windows: Vec<Value> = WindowPreset::ALL
        .iter()
        .map(|w| {
            let wl = w.window(v.range());
            json!({ "name": w.label().to_lowercase(), "center": wl.center, "width": wl.width })
        })
        .collect();
    let slices: serde_json::Map<String, Value> =
        SliceAxis::ALL.iter().map(|a| (a.label().to_lowercase(), json!(a.slice_count(v)))).collect();
    let mut data = json!({
        "series": series_key(config, &study.workspace.manifest().source.series_id),
        "modality": m.modality,
        "description": m.description,
        "dims": [d.x, d.y, d.z],
        "spacing_mm": [s.x, s.y, s.z],
        "origin_mm": [g.origin.x, g.origin.y, g.origin.z],
        "direction": g.direction_rows(),
        "frame": "LPS: voxel i, j, k map to patient millimetres through origin_mm and direction rows",
        "value_unit": study.value_unit(),
        "value_range": [v.range().min, v.range().max],
        "slice_counts": slices,
        "window_presets": windows,
        "source_sha256": study.source_sha256,
    });
    data["study"] = study_ids(config, m);
    let mut out = Output::new(data);
    if s.max_element() > 3.0 * s.min_element() {
        out = out.warn(format!("strongly anisotropic voxels ({:.2} × {:.2} × {:.2} mm)", s.x, s.y, s.z));
    }
    if !m.modality.eq_ignore_ascii_case("CT") {
        out = out.warn("values are not in Hounsfield units (not CT)");
    }
    out
}

fn study_ids(config: &AgentConfig, m: &SeriesMetadata) -> Value {
    let s = &m.study;
    let uid = |id: &str| if id.is_empty() { String::new() } else { series_key(config, id) };
    let mut ids = json!({
        "study_instance_uid": uid(&s.study_instance_uid),
        "series_instance_uid": uid(&s.series_instance_uid),
        "series_number": s.series_number,
        "study_description": s.study_description,
    });
    if config.expose_dates {
        ids["study_date"] = json!(s.study_date);
        ids["study_time"] = json!(s.study_time);
    }
    if config.expose_identifiers {
        ids["accession_number"] = json!(s.accession_number);
    }
    ids
}
