//! GPU groups shared with a process outside FERRUM (Stage 23,
//! `docs/gpu-sharing.md`): a model server holds part of the card, and the
//! agent's engine calls wait for room and then run.
//!
//! The external holder is this test binary run again with
//! `FERRUM_AGENT_TEST_GPU_HOLDER=<lock dir>`: its `external_holder` test
//! holds 6 000 MiB of group `card` until its stdin closes.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ferrum_agent::mcp::McpServer;
use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{EngineError, EngineInfo, InteractiveSession, SegmentationEngine, Volume};
use ferrum_engines::gpu::{Kind, LockDir, Request};
use ferrum_engines::{EngineServer, MockEngine};
use serde_json::{json, Value};

const HOLDER_ENV: &str = "FERRUM_AGENT_TEST_GPU_HOLDER";
const CAPACITY: u64 = 12_000;

#[test]
fn external_holder() {
    let Some(dir) = std::env::var_os(HOLDER_ENV) else { return };
    let req = Request {
        group: "card",
        holder: "llama-server",
        kind: Kind::Resident,
        lease_mib: Some(6000),
        capacity_mib: Some(CAPACITY),
    };
    let lease = LockDir::at(PathBuf::from(dir)).unwrap().acquire(&req, Duration::ZERO, &mut |_| true).unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    drop(lease);
}

/// The mock engine; it copies the lock records it can see when a session
/// opens (the agent holds its lease by then).
struct Peeking(MockEngine, PathBuf, Arc<Mutex<Vec<String>>>);

impl SegmentationEngine for Peeking {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        self.0.info()
    }
    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        if let Ok(entries) = std::fs::read_dir(self.1.join("card")) {
            for e in entries.flatten() {
                if e.path().extension().is_some_and(|x| x == "json") {
                    self.2.lock().unwrap().push(std::fs::read_to_string(e.path()).unwrap_or_default());
                }
            }
        }
        self.0.open_session(volume, modality)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    _server: EngineServer,
    url: String,
    ws: String,
    src: PathBuf,
    locks: PathBuf,
    records: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let src = ferrum_agent::evals::write_phantoms(&dir.path().join("in")).unwrap().remove(0);
        let locks = dir.path().join("gpu");
        let records = Arc::new(Mutex::new(Vec::new()));
        let engine = Peeking(MockEngine::default(), locks.clone(), records.clone());
        let server = EngineServer::start(Arc::new(engine), "127.0.0.1:0", None).unwrap();
        let url = server.url();
        let ws = dir.path().join("ws").to_string_lossy().into_owned();
        Self { _dir: dir, _server: server, url, ws, src, locks, records }
    }

    /// An agent whose engine needs 7 000 MiB of the 12 000 MiB group `card`.
    fn agent(&self, timeout_s: u64) -> Agent {
        let mut c = AgentConfig {
            engines: vec![self.url.clone()],
            engine_job_timeout_s: timeout_s,
            is_default: false,
            ..AgentConfig::default()
        };
        c.gpu.groups = vec![("card".into(), vec![self.url.clone()])];
        c.gpu.capacity_mib = vec![("card".into(), CAPACITY)];
        c.gpu.peaks_mib = vec![(self.url.clone(), 7000)];
        c.gpu.lock_dir = Some(self.locks.clone());
        Agent::new(c)
    }

    fn holder(&self) -> Child {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "external_holder", "--test-threads=1", "--nocapture"])
            .env(HOLDER_ENV, &self.locks)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let dir = LockDir::at(&self.locks).unwrap();
        let start = Instant::now();
        while !dir.holders("card").unwrap().iter().any(|h| h.pid == child.id()) {
            assert!(start.elapsed() < Duration::from_secs(20), "the holder never took its lease");
            std::thread::sleep(Duration::from_millis(20));
        }
        child
    }
}

fn release(mut child: Child) -> Instant {
    child.stdin.take().unwrap().write_all(b"release\n").unwrap();
    child.wait().unwrap();
    Instant::now()
}

fn auto_over_mcp(server: &mut McpServer, f: &Fixture) -> (Value, Vec<Value>) {
    let args = json!({ "workspace": f.ws, "name_prefix": "auto/" });
    let params = json!({ "name": "ferrum_segment_auto", "arguments": args, "_meta": { "progressToken": "t" } });
    let mut notes = Vec::new();
    let reply = server
        .handle_with(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params }), &mut |n| {
            notes.push(n)
        })
        .unwrap();
    (reply["result"].clone(), notes)
}

// covers 23.1-d, 23.1-e
#[test]
fn segment_auto_waits_for_an_external_holder_then_runs() {
    let f = Fixture::new();
    let agent = f.agent(30);
    assert_eq!(agent.run("study open", &json!({ "workspace": f.ws, "path": f.src }))["ok"], true);
    let holder = f.holder();
    let pid = holder.id();
    let mut server = McpServer::new(agent);
    let call = std::thread::scope(|s| {
        let call = s.spawn(|| {
            let (result, notes) = auto_over_mcp(&mut server, &f);
            (result, notes, Instant::now())
        });
        std::thread::sleep(Duration::from_millis(800));
        let released = release(holder);
        let (result, notes, done) = call.join().unwrap();
        (result, notes, done.saturating_duration_since(released))
    });
    let (result, notes, after_release) = call;
    assert_eq!(result["isError"], false, "{result:#}");
    let segments = result["structuredContent"]["data"]["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 2, "the job ran once the holder left");
    assert!(after_release < Duration::from_secs(1), "{after_release:?} after the release");
    let waiting =
        format!("waiting for GPU group card: llama-server (pid {pid}) holds 6000 MiB; this call needs 7000 MiB");
    let messages: Vec<&str> = notes.iter().filter_map(|n| n["params"]["message"].as_str()).collect();
    assert!(messages.iter().filter(|m| **m == waiting).count() >= 2, "{messages:?}");
}

// covers 23.1-e
#[test]
fn a_full_group_times_out_naming_its_holder() {
    let f = Fixture::new();
    let agent = f.agent(1);
    assert_eq!(agent.run("study open", &json!({ "workspace": f.ws, "path": f.src }))["ok"], true);
    let holder = f.holder();
    let pid = holder.id();
    let e = agent.run("segment auto", &json!({ "workspace": f.ws }));
    assert_eq!(e["ok"], false);
    assert_eq!(e["error"]["code"], "engine_unavailable");
    assert_eq!(
        e["error"]["message"],
        format!("GPU group card stayed full for 1 s: llama-server (pid {pid}) holds 6000 MiB")
    );
    assert!(e["error"]["hint"].as_str().unwrap().contains("network.gpu_capacity_mib"));
    let interactive = agent.run(
        "segment interactive",
        &json!({ "workspace": f.ws, "prompts": [{ "type": "point", "point": { "voxel": [14, 20, 15] }, "positive": true }] }),
    );
    assert_eq!(interactive["error"]["code"], "engine_unavailable", "interactive calls take the lease too");
    release(holder);
}

// covers 23.2-e
#[test]
fn lock_records_name_no_study() {
    let f = Fixture::new();
    let agent = f.agent(30);
    assert_eq!(agent.run("study open", &json!({ "workspace": f.ws, "path": f.src }))["ok"], true);
    let e = agent.run(
        "segment interactive",
        &json!({ "workspace": f.ws, "prompts": [{ "type": "point", "point": { "voxel": [14, 20, 15] }, "positive": true }] }),
    );
    assert_eq!(e["ok"], true, "{e:#}");
    let records = f.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1, "the agent held its lease while the session was open");
    let r = &records[0];
    assert!(r.contains("\"holder\":\"ferrum-agent FERRUM mock engine\""), "{r}");
    let leaks = [f.ws.as_str(), &f.src.to_string_lossy(), &file_name(&f.src)];
    assert!(leaks.iter().all(|l| !r.contains(*l)), "{r}");
    assert!(LockDir::at(&f.locks).unwrap().holders("card").unwrap().is_empty(), "released after the call");
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}
