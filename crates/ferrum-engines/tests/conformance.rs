//! `ferrum-engine/1` conformance suite.
//!
//! Runs against the reference server with the mock engine, and against any
//! other engine when `FERRUM_ENGINE_URL` (and optionally
//! `FERRUM_ENGINE_TOKEN`) is set:
//!
//! ```text
//! FERRUM_ENGINE_URL=http://127.0.0.1:8765 cargo test -p ferrum-engines --test conformance
//! ```
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrum_domain::{
    Dims3, EngineError, EngineInfo, InteractiveSession, JobState, Prompt, PromptKind, SegmentationEngine, Volume,
    VoxelBox,
};
use ferrum_engines::{EngineServer, HttpConfig, HttpEngine, MockEngine};
use glam::{UVec3, Vec3};
use serde_json::{json, Value};

/// CT-like phantom: a bright sphere (400 HU) in air (−1000 HU).
fn phantom() -> Volume {
    let dims = Dims3::new(40, 40, 24);
    let c = Vec3::new(20.0, 20.0, 12.0);
    let mut vals = Vec::with_capacity(dims.voxel_count());
    for k in 0..dims.z {
        for j in 0..dims.y {
            for i in 0..dims.x {
                let inside = (Vec3::new(i as f32, j as f32, k as f32) - c).length() < 8.0;
                vals.push(if inside { 400.0 } else { -1000.0 });
            }
        }
    }
    Volume::from_physical(dims, Vec3::new(1.0, 1.0, 2.0), &vals).unwrap()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().into()
}

fn bearer(token: &Option<String>) -> String {
    token.as_ref().map(|t| format!("Bearer {t}")).unwrap_or_default()
}

/// Raw request returning status, error code (if any) and body bytes.
fn raw(method: &str, url: &str, token: &Option<String>, body: Option<&Value>) -> (u16, String, Vec<u8>) {
    let a = agent();
    let resp = match (method, body) {
        ("GET", _) => a.get(url).header("Authorization", bearer(token)).call(),
        ("DELETE", _) => a.delete(url).header("Authorization", bearer(token)).call(),
        (_, Some(b)) => a
            .post(url)
            .header("Authorization", bearer(token))
            .header("Content-Type", "application/json")
            .send(b.to_string()),
        _ => a.post(url).header("Authorization", bearer(token)).send("{}"),
    };
    let mut resp = resp.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.body_mut().read_to_vec().unwrap();
    let code = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_owned))
        .unwrap_or_default();
    (status, code, bytes)
}

/// The suite: everything a conforming engine must do, by capability.
fn conformance(base: &str, token: Option<String>) {
    let engine = HttpEngine::new(HttpConfig { token: token.clone(), ..HttpConfig::new(base) });
    let info: EngineInfo = engine.info().expect("GET /v1/info");
    assert_eq!(info.protocol, "ferrum-engine/1");
    let caps = &info.capabilities;
    assert!(caps.interactive || caps.automatic, "an engine is interactive, automatic or both");
    session_errors(base, &token, &info);
    let volume = phantom();
    let mut session: Box<dyn InteractiveSession> = engine.open_session(&volume, "CT").expect("session + upload");
    if caps.interactive {
        interactive_round(session.as_mut(), &info, &volume);
    }
    if caps.automatic {
        automatic_round(session.as_mut(), &info, &volume);
    }
    drop(session);
    if caps.interactive && caps.deterministic && info.supports(PromptKind::Point) {
        replay_round(&engine, &volume);
    }
}

/// Engines that report `deterministic` (the default) give the same mask
/// when the same prompts are replayed on a new session: FERRUM's agent
/// refines objects that way (`docs/agent-segmentation.md` §5.3).
fn replay_round(engine: &HttpEngine, volume: &Volume) {
    let prompts = [
        Prompt::Point { positive: true, voxel: UVec3::new(20, 20, 12) },
        Prompt::Point { positive: true, voxel: UVec3::new(22, 20, 13) },
    ];
    let run = || {
        let mut s = engine.open_session(volume, "CT").expect("session + upload");
        for p in &prompts {
            s.prompt(p).expect("prompt");
        }
        s.mask(VoxelBox::full(volume.dims())).expect("mask")
    };
    let (first, second) = (run(), run());
    assert!(first.contains(&1));
    assert!(first == second, "replayed prompts must give the same mask, or info must report deterministic: false");
}

/// Errors before and without a volume, and the endpoints an engine lacks.
fn session_errors(base: &str, token: &Option<String>, info: &EngineInfo) {
    let v1 = format!("{base}/v1");
    assert_eq!(raw("GET", &format!("{v1}/sessions/does-not-exist/mask"), token, None).0, 404);
    assert_eq!(raw("GET", &format!("{v1}/jobs/does-not-exist"), token, None).0, 404);
    let created = raw(
        "POST",
        &format!("{v1}/sessions"),
        token,
        Some(&json!({"dims": [4, 4, 4], "dtype": "int16", "spacing": [1, 1, 1]})),
    );
    assert_eq!(created.0, 201);
    let id = serde_json::from_slice::<Value>(&created.2).unwrap()["session_id"].as_str().unwrap().to_owned();
    let early = if info.capabilities.interactive {
        raw("POST", &format!("{v1}/sessions/{id}/prompts"), token, Some(&json!({"type": "point", "voxel": [1, 1, 1]})))
    } else {
        raw("POST", &format!("{v1}/sessions/{id}/segment"), token, Some(&json!({"labels": null})))
    };
    assert_eq!((early.0, early.1.as_str()), (409, "no_volume"));
    if !info.capabilities.automatic {
        let (status, _, _) = raw("POST", &format!("{v1}/sessions/{id}/segment"), token, Some(&json!({})));
        assert_eq!(status, 404, "interactive-only engines answer 404 to automatic endpoints");
    }
    assert_eq!(raw("DELETE", &format!("{v1}/sessions/{id}"), token, None).0, 204);
    assert_eq!(raw("DELETE", &format!("{v1}/sessions/{id}"), token, None).0, 404);
    let (status, code, _) = raw("POST", &format!("{v1}/sessions"), token, Some(&json!({"dims": [0, 1, 1]})));
    assert_eq!((status, code.as_str()), (400, "bad_request"));
}

fn interactive_round(session: &mut dyn InteractiveSession, info: &EngineInfo, volume: &Volume) {
    let centre = UVec3::new(20, 20, 12);
    if info.supports(PromptKind::Point) {
        let r = session.prompt(&Prompt::Point { positive: true, voxel: centre }).unwrap();
        assert!(r.revision > 0 && !r.empty);
        let changed = r.changed.expect("a positive point on the sphere changes the mask");
        let m = session.mask(changed).unwrap();
        assert_eq!(m.len(), changed.voxel_count());
        assert!(m.iter().all(|&v| v <= 1));
        assert!(m.contains(&1));
        let seed = session.mask(VoxelBox::new(centre, centre + UVec3::ONE)).unwrap();
        assert_eq!(seed, vec![1], "the seed voxel is part of the object");
        if info.capabilities.undo {
            let u = session.undo().unwrap();
            assert!(u.empty, "undoing the only prompt empties the mask");
        }
    }
    if info.supports(PromptKind::Box) {
        let bx = VoxelBox::new(UVec3::new(10, 10, 12), UVec3::new(31, 31, 13));
        let r = session.prompt(&Prompt::Box { positive: true, bx }).unwrap();
        assert!(!r.empty);
    }
    session.reset().unwrap();
    let all = session.mask(VoxelBox::full(volume.dims())).unwrap();
    assert!(all.iter().all(|&v| v == 0), "reset clears the mask");
    let outside = VoxelBox::new(UVec3::ZERO, UVec3::new(41, 1, 1));
    assert!(matches!(session.mask(outside), Err(EngineError::BadRequest(_))));
}

/// Runs a job for all labels and checks the label map. Real engines may take
/// minutes: `FERRUM_ENGINE_JOB_TIMEOUT_S` (default 900) bounds the wait.
fn automatic_round(session: &mut dyn InteractiveSession, info: &EngineInfo, volume: &Volume) {
    assert!(!info.labels.is_empty(), "automatic engines list their labels");
    let timeout: u64 = std::env::var("FERRUM_ENGINE_JOB_TIMEOUT_S").ok().and_then(|s| s.parse().ok()).unwrap_or(900);
    let job = session.start_job(None).expect("POST …/segment");
    let start = std::time::Instant::now();
    let status = loop {
        let s = session.job_status(&job).expect("GET /v1/jobs/{id}");
        assert!((0.0..=1.0).contains(&s.progress));
        if s.state.is_final() {
            break s;
        }
        assert!(start.elapsed().as_secs() < timeout, "job did not finish within {timeout} s");
        std::thread::sleep(std::time::Duration::from_millis(250));
    };
    assert_eq!(status.state, JobState::Done, "{}", status.message);
    let values = session.label_map().expect("GET …/labelmap");
    assert_eq!(values.len(), volume.dims().voxel_count());
    let known: Vec<u16> = info.labels.iter().map(|l| l.value).collect();
    assert!(values.iter().all(|v| *v == 0 || known.contains(v)), "label values refer to info.labels");
}

fn mock_server(token: Option<String>) -> EngineServer {
    EngineServer::start(Arc::new(MockEngine::default()), "127.0.0.1:0", token).unwrap()
}

#[test]
fn reference_server_with_the_mock_engine_conforms() {
    let server = mock_server(None);
    conformance(&server.url(), None);
}

#[test]
fn token_is_enforced() {
    let server = mock_server(Some("s3cret".into()));
    conformance(&server.url(), Some("s3cret".into()));
    let wrong = HttpEngine::new(HttpConfig { token: Some("nope".into()), ..HttpConfig::new(server.url()) });
    assert_eq!(wrong.info(), Err(EngineError::Unauthorized));
}

#[test]
fn external_engine_conforms_if_configured() {
    if let Ok(url) = std::env::var("FERRUM_ENGINE_URL") {
        conformance(&url, std::env::var("FERRUM_ENGINE_TOKEN").ok());
    }
}

#[test]
fn mask_reports_its_revision_and_raw_bodies_work() {
    let server = mock_server(None);
    let v1 = format!("{}/v1", server.url());
    let engine = HttpEngine::new(HttpConfig::new(server.url()));
    let mut s = engine.open_session(&phantom(), "CT").unwrap();
    let r = s.prompt(&Prompt::Point { positive: true, voxel: UVec3::new(20, 20, 12) }).unwrap();
    // a second session driven through the raw API with a plain upload
    drop(s);
    let created = raw(
        "POST",
        &format!("{v1}/sessions"),
        &None,
        Some(&json!({"dims": [40, 40, 24], "dtype": "int16", "spacing": [1, 1, 2]})),
    );
    let id = serde_json::from_slice::<Value>(&created.2).unwrap()["session_id"].as_str().unwrap().to_owned();
    let volume = phantom();
    let range = volume.range();
    let bytes: Vec<u8> =
        volume.data().iter().flat_map(|&v| (range.from_storage(v).round() as i16).to_le_bytes()).collect();
    let resp = agent().put(&format!("{v1}/sessions/{id}/volume")).send(&bytes[..]).unwrap();
    assert_eq!(resp.status().as_u16(), 204, "plain (not gzip) uploads are accepted");
    raw("POST", &format!("{v1}/sessions/{id}/prompts"), &None, Some(&json!({"type": "point", "voxel": [20, 20, 12]})));
    let mut resp = agent().get(&format!("{v1}/sessions/{id}/mask?box=20,20,12,21,21,13")).call().unwrap();
    assert_eq!(resp.headers().get("x-ferrum-revision").unwrap().to_str().unwrap(), r.revision.to_string());
    assert_eq!(resp.body_mut().read_to_vec().unwrap(), vec![1]);
    let (status, code, _) = raw("GET", &format!("{v1}/sessions/{id}/mask?box=1,2,3"), &None, None);
    assert_eq!((status, code.as_str()), (400, "bad_request"));
    let (status, code, _) = raw("GET", &format!("{v1}/nothing"), &None, None);
    assert_eq!((status, code.as_str()), (404, "not_found"));
}

/// An engine that only accepts points and small volumes.
struct Limited(MockEngine);

impl SegmentationEngine for Limited {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        let mut i = MockEngine::describe();
        i.capabilities.prompts = vec![PromptKind::Point];
        i.capabilities.undo = false;
        i.capabilities.automatic = false;
        i.max_voxels = 50_000;
        i.research_only = true;
        Ok(i)
    }

    fn open_session(&self, v: &Volume, m: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        self.0.open_session(v, m)
    }
}

#[test]
fn capabilities_and_limits_are_enforced() {
    let server = EngineServer::start(Arc::new(Limited(MockEngine::default())), "127.0.0.1:0", None).unwrap();
    conformance(&server.url(), None);
    let engine = HttpEngine::new(HttpConfig::new(server.url()));
    assert!(engine.info().unwrap().research_only);
    let mut s = engine.open_session(&phantom(), "CT").unwrap();
    let bx = VoxelBox::new(UVec3::ZERO, UVec3::ONE);
    assert!(matches!(s.prompt(&Prompt::Lasso { positive: true, bx, mask: vec![1] }), Err(EngineError::Unsupported(_))));
    let big = Volume::from_physical(Dims3::new(40, 40, 40), Vec3::ONE, &vec![0.0; 64_000]).unwrap();
    assert!(matches!(engine.open_session(&big, "CT"), Err(EngineError::TooLarge(_))));
}

#[test]
fn unreachable_engines_are_reported() {
    let server = mock_server(None);
    let url = server.url();
    drop(server);
    let engine = HttpEngine::new(HttpConfig::new(url));
    assert!(matches!(engine.info(), Err(EngineError::Unreachable(_))));
}

/// An automatic-only engine (like TotalSegmentator).
struct AutomaticOnly(MockEngine);

impl SegmentationEngine for AutomaticOnly {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        let mut i = MockEngine::describe();
        i.capabilities.interactive = false;
        i.capabilities.prompts.clear();
        Ok(i)
    }

    fn open_session(&self, v: &Volume, m: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        self.0.open_session(v, m)
    }
}

#[test]
fn automatic_only_engines_conform() {
    let server = EngineServer::start(Arc::new(AutomaticOnly(MockEngine::default())), "127.0.0.1:0", None).unwrap();
    conformance(&server.url(), None);
    let engine = HttpEngine::new(HttpConfig::new(server.url()));
    let mut s = engine.open_session(&phantom(), "CT").unwrap();
    let job = s.start_job(Some(&["bright".to_string()])).unwrap();
    assert_eq!(s.job_status(&job).unwrap().state, JobState::Done);
    let bright = s.label_map().unwrap().iter().filter(|&&v| v == 2).count();
    assert!(bright > 1000, "{bright}");
    s.cancel_job(&job).unwrap();
    assert!(matches!(s.start_job(Some(&["liver".to_string()])), Err(EngineError::BadRequest(_))));
    assert!(matches!(s.job_status("nope"), Err(EngineError::NotFound)));
    assert!(matches!(
        s.prompt(&Prompt::Point { positive: true, voxel: UVec3::ZERO }),
        Err(EngineError::Unsupported(_))
    ));
}
