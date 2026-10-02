//! Segmentation engines in the skill (`ferrum-engine/1`): `engine info`,
//! `segment interactive` and `segment auto`. Results become segments
//! proposed by the engine (author `engine`, with its `research_only`
//! flag), never confirmed findings.

use std::time::{Duration, Instant};

use ferrum_domain::{EngineError, EngineInfo, Prompt, Provenance, SegmentationEngine, Timestamp, VoxelBox};
use ferrum_engines::{HttpConfig, HttpEngine};
use glam::UVec3;
use serde_json::{json, Value};

use super::inspect::point;
use super::Ctx;
use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::points::nearest_voxel;
use crate::study::Study;

/// Environment variable with the engine's bearer token (never in files).
pub const TOKEN_ENV: &str = "FERRUM_ENGINE_TOKEN";

fn is_loopback(url: &str) -> bool {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")).unwrap_or("");
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host =
        host.rsplit_once(':').map_or(host, |(h, port)| if port.chars().all(|c| c.is_ascii_digit()) { h } else { host });
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// The engine URL of a call: `engine`, else the first allow-listed one,
/// else `FERRUM_ENGINE_URL`; checked against the operator's allow-list.
pub fn engine_url(config: &AgentConfig, p: &Params) -> Result<String, AgentError> {
    let url = match p.str("engine")? {
        Some(u) => u.trim_end_matches('/').to_owned(),
        None => config
            .engines
            .first()
            .cloned()
            .or_else(|| std::env::var("FERRUM_ENGINE_URL").ok().map(|u| u.trim_end_matches('/').to_owned()))
            .ok_or_else(|| {
                AgentError::new(ErrorCode::EngineUnavailable, "no segmentation engine is configured")
                    .hint("the operator lists engines in network.engines; or pass engine (a URL)")
            })?,
    };
    let allowed =
        if config.engines.is_empty() { config.is_default && is_loopback(&url) } else { config.engines.contains(&url) };
    if !allowed {
        return Err(AgentError::new(ErrorCode::Forbidden, format!("engine {url} is not allowed"))
            .hint("the operator configuration lists the engines FERRUM may contact (network.engines)"));
    }
    Ok(url)
}

fn engine(url: &str) -> HttpEngine {
    let mut cfg = HttpConfig::new(url);
    cfg.token = std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty());
    HttpEngine::new(cfg)
}

/// Maps engine errors to envelope errors.
pub fn engine_error(e: EngineError) -> AgentError {
    let msg = e.to_string();
    match e {
        EngineError::Unreachable(_) => AgentError::new(ErrorCode::EngineUnavailable, msg)
            .hint("start the engine bridge or check the URL (see FERRUM's docs/ai-demo.md)"),
        EngineError::Busy { retry_after_s } => {
            AgentError::new(ErrorCode::EngineUnavailable, msg).hint(format!("retry in {retry_after_s} s"))
        }
        EngineError::Unauthorized => AgentError::new(ErrorCode::EngineUnavailable, msg)
            .hint(format!("the operator sets the engine token in {TOKEN_ENV}")),
        EngineError::TooLarge(_) => AgentError::new(ErrorCode::Limit, msg),
        EngineError::BadRequest(_) | EngineError::Unsupported(_) => AgentError::bad_request(msg),
        EngineError::Protocol(_) | EngineError::NotFound | EngineError::Internal(_) => AgentError::internal(msg),
    }
}

fn info_json(url: &str, i: &EngineInfo) -> Value {
    json!({
        "engine": url,
        "name": i.name,
        "version": i.version,
        "vendor": i.vendor,
        "device": i.device,
        "interactive": i.capabilities.interactive,
        "automatic": i.capabilities.automatic,
        "prompts": i.capabilities.prompts.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        "undo": i.capabilities.undo,
        "labels": i.labels.iter().map(|l| json!({ "value": l.value, "name": l.name })).collect::<Vec<_>>(),
        "modalities": i.modalities,
        "research_only": i.research_only,
        "license": i.license,
    })
}

fn research_warning(out: Output, i: &EngineInfo) -> Output {
    if i.research_only {
        out.warn(format!("{} is for research use only ({})", i.name, i.license))
    } else {
        out
    }
}

/// `engine info`: the engine's capabilities, labels and licence.
pub fn info(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let url = engine_url(ctx.config, p)?;
    let i = engine(&url).info().map_err(engine_error)?;
    Ok(research_warning(Output::new(info_json(&url, &i)), &i))
}

fn provenance(i: &EngineInfo) -> Provenance {
    Provenance::engine(&i.name, &i.version, i.research_only, Timestamp::now())
}

fn prompt(study: &Study, v: &Value) -> Result<Prompt, AgentError> {
    let p = Params::new(v)?;
    let positive = p.bool("positive")?.unwrap_or(true);
    match p.req_str("type")? {
        "point" => {
            Ok(Prompt::Point { positive, voxel: UVec3::from(nearest_voxel(&study.volume, point(study, &p, "point")?)) })
        }
        "box" => {
            let (a, b) = (
                nearest_voxel(&study.volume, point(study, &p, "min")?),
                nearest_voxel(&study.volume, point(study, &p, "max")?),
            );
            let lo = UVec3::new(a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2]));
            let hi = UVec3::new(a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])) + UVec3::ONE;
            Ok(Prompt::Box { positive, bx: VoxelBox::new(lo, hi) })
        }
        other => {
            Err(AgentError::bad_request(format!("unknown prompt type {other:?}")).hint("prompt types: point, box"))
        }
    }
}

/// `segment interactive`: point and box prompts to an interactive engine;
/// the resulting object becomes a new segment proposed by the engine.
pub fn interactive(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let url = engine_url(ctx.config, p)?;
    let study = ctx.study(p)?;
    let prompts = p.list("prompts")?.ok_or_else(|| AgentError::bad_request("prompts is required"))?;
    let prompts: Vec<Prompt> = prompts.iter().map(|v| prompt(study, v)).collect::<Result<_, _>>()?;
    let eng = engine(&url);
    let info = eng.info().map_err(engine_error)?;
    if let Some(k) = prompts.iter().map(Prompt::kind).find(|k| !info.supports(*k)) {
        return Err(AgentError::bad_request(format!("{} does not take {} prompts", info.name, k.as_str()))
            .hint("see engine info: prompts"));
    }
    let mut session = eng.open_session(&study.volume, &study.metadata.modality).map_err(engine_error)?;
    let mut changed: Option<VoxelBox> = None;
    let mut empty = true;
    for pr in &prompts {
        let r = session.prompt(pr).map_err(engine_error)?;
        empty = r.empty;
        if let Some(bx) = r.changed {
            changed = Some(changed.map_or(bx, |c| c.union(bx)));
        }
    }
    let Some(bx) = changed.filter(|_| !empty) else {
        return Ok(Output::new(json!({ "segment": null, "engine": info_json(&url, &info) }))
            .warn("the engine found no object for these prompts"));
    };
    let mask = session.mask(bx).map_err(engine_error)?;
    let name = p.str("name")?.map_or_else(|| format!("{} object", info.name), str::to_owned);
    let set = &mut study.segments;
    let label = set.add_segment(&name).map_err(|e| AgentError::new(ErrorCode::Limit, e.to_string()))?;
    set.set_provenance(label, provenance(&info)).map_err(|e| AgentError::internal(e.to_string()))?;
    set.apply_mask(label, bx, &mask, false).map_err(|e| AgentError::internal(e.to_string()))?;
    study.save_segments()?;
    let out = Output::new(
        json!({ "segment": super::segment::segment_json(study, label), "prompts": prompts.len(), "engine": info_json(&url, &info) }),
    );
    Ok(research_warning(out, &info))
}

/// `segment auto`: an automatic engine job; each structure found becomes a
/// segment proposed by the engine. Voxels of existing segments are kept.
pub fn auto(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let url = engine_url(ctx.config, p)?;
    let timeout = Duration::from_secs(ctx.config.engine_job_timeout_s);
    let study = ctx.study(p)?;
    let labels: Option<Vec<String>> =
        p.list("labels")?.map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect());
    let eng = engine(&url);
    let info = eng.info().map_err(engine_error)?;
    if !info.capabilities.automatic {
        return Err(AgentError::bad_request(format!("{} has no automatic segmentation", info.name)));
    }
    let mut session = eng.open_session(&study.volume, &study.metadata.modality).map_err(engine_error)?;
    let job = session.start_job(labels.as_deref()).map_err(engine_error)?;
    let start = Instant::now();
    let status = loop {
        let s = session.job_status(&job).map_err(engine_error)?;
        if s.state.is_final() {
            break s;
        }
        if start.elapsed() > timeout {
            let _ = session.cancel_job(&job);
            return Err(AgentError::new(ErrorCode::Limit, format!("the job ran longer than {} s", timeout.as_secs()))
                .hint("the operator sets limits.engine_job_timeout_s"));
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    if status.state != ferrum_domain::JobState::Done {
        return Err(AgentError::new(
            ErrorCode::EngineUnavailable,
            format!("the job {}: {}", status.state.as_str(), status.message),
        ));
    }
    let values = session.label_map().map_err(engine_error)?;
    let mut present: Vec<u16> = values.iter().copied().filter(|v| *v != 0).collect();
    present.sort_unstable();
    present.dedup();
    let set = &mut study.segments;
    let mut mapping = Vec::new();
    for value in present {
        let known = info.labels.iter().find(|l| l.value == value);
        let name = known.map_or_else(|| format!("Label {value}"), |l| l.name.clone());
        let label = set.add_segment(&name).map_err(|e| AgentError::new(ErrorCode::Limit, e.to_string()))?;
        set.set_provenance(label, provenance(&info)).map_err(|e| AgentError::internal(e.to_string()))?;
        if let Some(c) = known.and_then(|l| l.color) {
            set.set_color(label, c).map_err(|e| AgentError::internal(e.to_string()))?;
        }
        mapping.push((value, label));
    }
    set.apply_label_values(&values, &mapping, false).map_err(|e| AgentError::internal(e.to_string()))?;
    study.save_segments()?;
    let segments: Vec<Value> = mapping.iter().map(|(_, l)| super::segment::segment_json(study, *l)).collect();
    let mut out = Output::new(json!({ "segments": segments, "engine": info_json(&url, &info) }));
    if segments.is_empty() {
        out = out.warn("the engine found none of the requested structures");
    }
    Ok(research_warning(out, &info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_errors_map_to_envelope_codes() {
        let cases = [
            (EngineError::Unreachable("x".into()), ErrorCode::EngineUnavailable),
            (EngineError::Busy { retry_after_s: 5 }, ErrorCode::EngineUnavailable),
            (EngineError::Unauthorized, ErrorCode::EngineUnavailable),
            (EngineError::TooLarge("x".into()), ErrorCode::Limit),
            (EngineError::BadRequest("x".into()), ErrorCode::BadRequest),
            (EngineError::Unsupported("x".into()), ErrorCode::BadRequest),
            (EngineError::Protocol("x".into()), ErrorCode::Internal),
            (EngineError::NotFound, ErrorCode::Internal),
            (EngineError::Internal("x".into()), ErrorCode::Internal),
        ];
        for (e, code) in cases {
            assert_eq!(engine_error(e).code, code);
        }
        assert!(engine_error(EngineError::Busy { retry_after_s: 5 }).hint.unwrap().contains("5 s"));
        assert!(engine_error(EngineError::Unauthorized).hint.unwrap().contains(TOKEN_ENV));
    }

    #[test]
    fn loopback_detection() {
        for u in ["http://127.0.0.1:8765", "http://localhost", "https://[::1]:9/x"] {
            assert!(is_loopback(u), "{u}");
        }
        for u in [
            "http://10.0.0.5:8765",
            "http://evil.example/127.0.0.1",
            "ftp://127.0.0.1",
            "http://127.0.0.1.evil.example",
        ] {
            assert!(!is_loopback(u), "{u}");
        }
    }
}
