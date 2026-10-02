//! Segmentation engines in the skill, against FERRUM's reference server
//! with the mock engine (region growing, intensity bands; no model).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{EngineError, EngineInfo, InteractiveSession, SegmentationEngine, Volume};
use ferrum_engines::{EngineServer, MockEngine};

/// The mock engine, declared research-only (as nnInteractive's weights are).
struct ResearchOnly(MockEngine);

impl SegmentationEngine for ResearchOnly {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        Ok(EngineInfo { research_only: true, license: "CC BY-NC-SA 4.0 (test)".into(), ..self.0.info()? })
    }
    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        self.0.open_session(volume, modality)
    }
}
use serde_json::{json, Value};

struct Fixture {
    _dir: tempfile::TempDir,
    _server: EngineServer,
    url: String,
    ws: String,
    src: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let src = ferrum_agent::evals::write_phantoms(&dir.path().join("in")).unwrap().remove(0);
    let server = EngineServer::start(Arc::new(ResearchOnly(MockEngine::default())), "127.0.0.1:0", None).unwrap();
    let url = server.url();
    let ws = dir.path().join("ws").to_string_lossy().into_owned();
    Fixture { _dir: dir, _server: server, url, ws, src }
}

fn ok(agent: &Agent, command: &str, params: Value) -> Value {
    let e = agent.run(command, &params);
    assert_eq!(e["ok"], true, "{command}: {e:#}");
    e
}

fn code(agent: &Agent, command: &str, params: Value) -> String {
    let e = agent.run(command, &params);
    assert_eq!(e["ok"], false, "{command} should fail: {e:#}");
    e["error"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn interactive_and_automatic_engines_propose_segments() {
    let f = fixture();
    let agent = Agent::default(); // no configuration: loopback engines only
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let info = ok(&agent, "engine info", json!({ "engine": f.url }));
    assert_eq!((info["data"]["interactive"].as_bool(), info["data"]["automatic"].as_bool()), (Some(true), Some(true)));
    assert!(info["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("research use only")));

    // a point in the sphere grows the sphere; the box prompt is planar-free in the mock
    let s = ok(
        &agent,
        "segment interactive",
        json!({
            "workspace": f.ws, "engine": f.url, "name": "Sphere",
            "prompts": [{ "type": "point", "point": { "voxel": [14, 20, 15] } }],
        }),
    );
    let seg = &s["data"]["segment"];
    assert_eq!(seg["name"], "Sphere");
    assert_eq!(seg["provenance"]["author"]["kind"], "engine");
    assert_eq!(seg["provenance"]["author"]["research_only"], true);
    assert_eq!(seg["provenance"]["status"], "proposed");
    let ml = seg["volume_ml"].as_f64().unwrap();
    assert!((1.9..2.4).contains(&ml), "{ml}");
    // nothing there: no segment, a warning
    let none = ok(
        &agent,
        "segment interactive",
        json!({
            "workspace": f.ws, "engine": f.url,
            "prompts": [{ "type": "point", "point": { "voxel": [14, 20, 15] }, "positive": false }],
        }),
    );
    assert_eq!(none["data"]["segment"], Value::Null);

    // automatic: the mock labels intensity bands; existing segments are kept
    let auto = ok(&agent, "segment auto", json!({ "workspace": f.ws, "engine": f.url }));
    let names: Vec<&str> =
        auto["data"]["segments"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["intermediate", "bright"]);
    assert_eq!(auto["data"]["segments"][0]["voxels"], 0, "the sphere already belongs to the interactive segment");
    let cube = auto["data"]["segments"][1]["volume_ml"].as_f64().unwrap();
    assert!((cube - 0.432).abs() < 1e-6, "{cube}");
    let list = ok(&agent, "segment list", json!({ "workspace": f.ws }));
    assert_eq!(list["data"]["segments"].as_array().unwrap().len(), 3);
    assert_eq!(
        ok(&agent, "review list", json!({ "workspace": f.ws }))["data"]["segments"].as_array().unwrap().len(),
        3
    );
    // a subset that is absent yields no segment
    let none = ok(&agent, "segment auto", json!({ "workspace": f.ws, "engine": f.url, "labels": ["intermediate"] }));
    assert_eq!(none["data"]["segments"][0]["voxels"], 0, "the band exists but its voxels are taken");
}

#[test]
fn engines_follow_the_operator_rules() {
    let f = fixture();
    let agent = Agent::default();
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    // only allow-listed engines; without configuration only loopback ones
    assert_eq!(code(&agent, "engine info", json!({ "engine": "http://10.1.2.3:8765" })), "forbidden");
    let listed = Agent::new(AgentConfig {
        engines: vec!["http://127.0.0.1:1".into()],
        is_default: false,
        ..AgentConfig::default()
    });
    assert_eq!(code(&listed, "engine info", json!({ "engine": f.url })), "forbidden");
    assert_eq!(code(&listed, "engine info", json!({})), "engine_unavailable", "the listed engine is not running");
    let configured =
        Agent::new(AgentConfig { engines: vec![f.url.clone()], is_default: false, ..AgentConfig::default() });
    ok(&configured, "engine info", json!({}));
    let unconfigured = Agent::new(AgentConfig { is_default: false, ..AgentConfig::default() });
    if std::env::var("FERRUM_ENGINE_URL").is_err() {
        assert_eq!(code(&unconfigured, "engine info", json!({})), "engine_unavailable");
    }
    assert_eq!(
        code(
            &agent,
            "segment interactive",
            json!({ "workspace": f.ws, "engine": f.url, "prompts": [{ "type": "scribble" }] })
        ),
        "bad_request"
    );
    assert_eq!(
        code(
            &agent,
            "segment interactive",
            json!({ "workspace": f.ws, "engine": f.url, "prompts": [{ "type": "point", "point": { "voxel": [99, 0, 0] } }] })
        ),
        "out_of_volume"
    );
}

#[test]
fn engine_failures_are_reported() {
    use ferrum_domain::{JobState, JobStatus, Prompt, PromptResult, VoxelBox};

    /// Accepts the volume; every job fails, every prompt is out of order.
    struct Broken;
    struct BrokenSession;
    impl SegmentationEngine for Broken {
        fn info(&self) -> Result<EngineInfo, EngineError> {
            let mut i = MockEngine::describe();
            i.capabilities.automatic = true;
            Ok(i)
        }
        fn open_session(&self, _: &Volume, _: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
            Ok(Box::new(BrokenSession))
        }
    }
    impl InteractiveSession for BrokenSession {
        fn prompt(&mut self, _: &Prompt) -> Result<PromptResult, EngineError> {
            Err(EngineError::Internal("CUDA out of memory".into()))
        }
        fn mask(&mut self, _: VoxelBox) -> Result<Vec<u8>, EngineError> {
            Err(EngineError::NotFound)
        }
        fn undo(&mut self) -> Result<PromptResult, EngineError> {
            Err(EngineError::Unsupported("undo".into()))
        }
        fn reset(&mut self) -> Result<(), EngineError> {
            Ok(())
        }
        fn start_job(&mut self, _: Option<&[String]>) -> Result<String, EngineError> {
            Ok("j1".into())
        }
        fn job_status(&mut self, _: &str) -> Result<JobStatus, EngineError> {
            Ok(JobStatus { state: JobState::Failed, progress: 0.1, message: "model crashed".into() })
        }
    }

    let f = fixture();
    let server = EngineServer::start(Arc::new(Broken), "127.0.0.1:0", None).unwrap();
    let agent = Agent::default();
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let e = agent.run("segment auto", &json!({ "workspace": f.ws, "engine": server.url() }));
    assert_eq!(e["error"]["code"], "engine_unavailable");
    assert!(e["error"]["message"].as_str().unwrap().contains("model crashed"));
    let e = agent.run("segment interactive", &json!({ "workspace": f.ws, "engine": server.url(), "prompts": [{ "type": "box", "min": { "voxel": [1, 1, 1] }, "max": { "voxel": [5, 5, 5] } }] }));
    assert_eq!(e["error"]["code"], "internal");
    // nothing listens on the port
    let unreachable = Agent::default().run("engine info", &json!({ "engine": "http://127.0.0.1:9" }));
    assert_eq!(unreachable["error"]["code"], "engine_unavailable");
    assert!(unreachable["error"]["hint"].as_str().unwrap().contains("bridge"));
}
