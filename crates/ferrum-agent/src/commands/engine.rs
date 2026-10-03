//! Segmentation engines in the skill (`ferrum-engine/1`): `engine info`,
//! `engine list` and `segment auto` (`segment interactive` lives in
//! [`super::interactive`]). Results become segments
//! proposed by the engine (author `engine`, with its `research_only`
//! flag), never confirmed findings.

use std::time::{Duration, Instant};

use ferrum_domain::{
    Author, EngineError, EngineInfo, InteractiveSession, JobState, JobStatus, Provenance, SegmentationEngine, Timestamp,
};
use ferrum_engines::{HttpConfig, HttpEngine};
use serde_json::{json, Value};

use super::masks::{checks, requesting_agent, CheckInput};
use super::Ctx;
use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::sessions::GpuLock;

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

/// The engine client of `url`, with the operator's token.
pub fn engine(url: &str) -> HttpEngine {
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
        EngineError::Busy { retry_after_s } => AgentError::new(ErrorCode::EngineUnavailable, msg)
            .hint(format!("the engine is busy: wait {retry_after_s} s and retry; do not switch engines silently")),
        EngineError::Unauthorized => AgentError::new(ErrorCode::EngineUnavailable, msg)
            .hint(format!("the operator sets the engine token in {TOKEN_ENV}")),
        EngineError::TooLarge(_) => AgentError::new(ErrorCode::Limit, msg)
            .hint("segment interactive uploads a region of interest; leave out whole_volume or give a smaller roi"),
        EngineError::BadRequest(_) | EngineError::Unsupported(_) => AgentError::bad_request(msg),
        EngineError::Protocol(_) | EngineError::NotFound | EngineError::Internal(_) => AgentError::internal(msg),
    }
}

/// Engine description as JSON.
pub fn info_json(config: &AgentConfig, url: &str, i: &EngineInfo) -> Value {
    json!({
        "engine": url,
        "name": i.name,
        "version": i.version,
        "vendor": i.vendor,
        "device": i.device,
        "interactive": i.capabilities.interactive,
        "automatic": i.capabilities.automatic,
        "prompts": i.capabilities.prompts.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        "planar_boxes_only": i.capabilities.planar_boxes_only,
        "undo": i.capabilities.undo,
        "deterministic": i.capabilities.deterministic,
        "labels": i.labels.iter().map(|l| json!({ "value": l.value, "name": l.name })).collect::<Vec<_>>(),
        "modalities": i.modalities,
        "research_only": i.research_only,
        "license": i.license,
        "gpu_group": config.gpu_group(url),
    })
}

/// Adds the research-use warning of a research-only engine.
pub fn research_warning(out: Output, i: &EngineInfo) -> Output {
    if i.research_only {
        out.warn(format!("{} is for research use only ({})", i.name, i.license))
    } else {
        out
    }
}

/// Refuses research-only engines when the operator does not allow them.
pub fn research_allowed(config: &AgentConfig, i: &EngineInfo) -> Result<(), AgentError> {
    if i.research_only && !config.allow_research_only {
        return Err(AgentError::new(ErrorCode::Forbidden, format!("{} is for research use only", i.name))
            .hint("the operator allows only engines whose licence permits this use (limits.allow_research_only)"));
    }
    Ok(())
}

/// `engine info`: the engine's capabilities, labels and licence.
pub fn info(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let url = engine_url(ctx.config, p)?;
    let i = engine(&url).info().map_err(engine_error)?;
    Ok(research_warning(Output::new(info_json(ctx.config, &url, &i)), &i))
}

/// `engine list`: every engine the agent may use, whether it answers, and
/// what it is.
pub fn list(ctx: &mut Ctx, _p: &Params) -> Result<Output, AgentError> {
    let config = ctx.config;
    let mut urls = config.engines.clone();
    if urls.is_empty() {
        urls.extend(std::env::var("FERRUM_ENGINE_URL").ok().map(|u| u.trim_end_matches('/').to_owned()));
    }
    let engines: Vec<Value> = urls
        .iter()
        .map(|url| match engine(url).info() {
            Ok(i) => {
                let mut v = info_json(config, url, &i);
                v["reachable"] = json!(true);
                v["labels"] = json!(i.labels.len());
                v
            }
            Err(e) => {
                json!({ "engine": url, "reachable": false, "error": e.to_string(), "gpu_group": config.gpu_group(url) })
            }
        })
        .collect();
    let mut out = Output::new(json!({ "engines": engines }));
    if engines.is_empty() {
        out = out.warn("no engine is configured (network.engines or FERRUM_ENGINE_URL)");
    }
    Ok(out)
}

fn provenance(i: &EngineInfo, agent: Author) -> Provenance {
    Provenance::engine(&i.name, &i.version, i.research_only, Timestamp::now()).requested_by(agent)
}

fn wait_for_job(ctx: &mut Ctx, session: &mut dyn InteractiveSession, job: &str) -> Result<JobStatus, AgentError> {
    let timeout = Duration::from_secs(ctx.config.engine_job_timeout_s);
    let start = Instant::now();
    loop {
        let s = session.job_status(job).map_err(engine_error)?;
        ctx.report(f64::from(s.progress), &s.message);
        if s.state.is_final() {
            return Ok(s);
        }
        if start.elapsed() > timeout {
            let _ = session.cancel_job(job);
            return Err(AgentError::new(ErrorCode::Limit, format!("the job ran longer than {} s", timeout.as_secs()))
                .hint("the operator sets limits.engine_job_timeout_s"));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Runs the job and returns the engine's label map.
fn run_job(ctx: &mut Ctx, p: &Params, url: &str, labels: Option<&[String]>) -> Result<Vec<u16>, AgentError> {
    let modality = match p.str("modality")? {
        Some(m) => m.to_owned(),
        None => ctx.study(p)?.metadata.modality.clone(),
    };
    let volume = ctx.study(p)?.volume.clone();
    let _gpu = GpuLock::acquire(ctx.config, url, Duration::from_secs(ctx.config.engine_job_timeout_s))?;
    let mut session = engine(url).open_session(&volume, &modality).map_err(engine_error)?;
    let job = session.start_job(labels).map_err(engine_error)?;
    let status = wait_for_job(ctx, session.as_mut(), &job)?;
    if status.state != JobState::Done {
        return Err(AgentError::new(
            ErrorCode::EngineUnavailable,
            format!("the job {}: {}", status.state.as_str(), status.message),
        ));
    }
    session.label_map().map_err(engine_error)
}

/// Engine values present in `values`, with the number of their voxels that
/// already belong to a segment.
fn present_values(values: &[u16], labels: &[u8]) -> Vec<(u16, u64)> {
    let mut counts: std::collections::BTreeMap<u16, u64> = std::collections::BTreeMap::new();
    for (v, l) in values.iter().zip(labels) {
        if *v != 0 {
            *counts.entry(*v).or_default() += u64::from(*l != 0);
        }
    }
    counts.into_iter().collect()
}

/// `segment auto`: an automatic engine job; each structure found becomes a
/// segment proposed by the engine, requested by the agent. Voxels of
/// existing segments are kept; every segment carries its checks.
pub fn auto(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let url = engine_url(ctx.config, p)?;
    let agent = requesting_agent(p)?;
    let prefix = p.str("name_prefix")?.unwrap_or_default().to_owned();
    let labels: Option<Vec<String>> =
        p.list("labels")?.map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect());
    let info = engine(&url).info().map_err(engine_error)?;
    research_allowed(ctx.config, &info)?;
    if !info.capabilities.automatic {
        return Err(AgentError::bad_request(format!("{} has no automatic segmentation", info.name)));
    }
    let values = run_job(ctx, p, &url, labels.as_deref())?;
    let config = ctx.config;
    let study = ctx.study(p)?;
    let present = present_values(&values, study.segments.labels().data());
    let free = 255 - study.segments.segments().len();
    if present.len() > free {
        return Err(AgentError::new(
            ErrorCode::Limit,
            format!("{} structures found, {free} segment labels free", present.len()),
        )
        .hint("choose the structures with labels, or use a new workspace"));
    }
    let set = &mut study.segments;
    let mut mapping = Vec::new();
    for (value, _) in &present {
        let known = info.labels.iter().find(|l| l.value == *value);
        let name = known.map_or_else(|| format!("Label {value}"), |l| l.name.clone());
        let label = set
            .add_segment(&format!("{prefix}{name}"))
            .map_err(|e| AgentError::new(ErrorCode::Limit, e.to_string()))?;
        set.set_provenance(label, provenance(&info, agent.clone())).map_err(|e| AgentError::internal(e.to_string()))?;
        if let Some(c) = known.and_then(|l| l.color) {
            set.set_color(label, c).map_err(|e| AgentError::internal(e.to_string()))?;
        }
        mapping.push((*value, label));
    }
    set.apply_label_values(&values, &mapping, false).map_err(|e| AgentError::internal(e.to_string()))?;
    study.save_segments()?;
    let mut warnings = Vec::new();
    let segments: Vec<Value> = mapping
        .iter()
        .zip(&present)
        .map(|((_, l), (_, taken))| {
            let input =
                CheckInput { overlap_voxels: *taken, research_only: info.research_only, ..CheckInput::default() };
            let (c, w) = checks(study, *l, &input);
            warnings.extend(w);
            let mut v = super::segment::segment_json(study, *l);
            v["checks"] = c;
            v
        })
        .collect();
    let mut out = Output::new(json!({ "segments": segments, "engine": info_json(config, &url, &info) }));
    if segments.is_empty() {
        out = out.warn("the engine found none of the requested structures");
    }
    for w in warnings {
        out = out.warn(w);
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
