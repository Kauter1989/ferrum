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
use ferrum_domain::{
    EngineError, EngineInfo, InteractiveSession, JobStatus, Prompt, PromptResult, SegmentationEngine, Volume, VoxelBox,
};
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
/// opens (the agent holds its lease by then), and notes when a session
/// closes whether a lease was still held (it must be: the engine's memory is
/// freed before the lease is given back).
struct Peeking(MockEngine, PathBuf, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<bool>>>);

fn records(locks: &Path) -> Vec<String> {
    std::fs::read_dir(locks.join("card"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

/// A session that reports, when dropped, whether a lease was held.
struct Watched(Box<dyn InteractiveSession>, PathBuf, Arc<Mutex<Vec<bool>>>);

impl Drop for Watched {
    fn drop(&mut self) {
        let held = records(&self.1).iter().any(|r| r.contains("ferrum-agent"));
        self.2.lock().unwrap().push(held);
    }
}

impl InteractiveSession for Watched {
    fn prompt(&mut self, prompt: &Prompt) -> Result<PromptResult, EngineError> {
        self.0.prompt(prompt)
    }
    fn mask(&mut self, bx: VoxelBox) -> Result<Vec<u8>, EngineError> {
        self.0.mask(bx)
    }
    fn undo(&mut self) -> Result<PromptResult, EngineError> {
        self.0.undo()
    }
    fn reset(&mut self) -> Result<(), EngineError> {
        self.0.reset()
    }
    fn start_job(&mut self, labels: Option<&[String]>) -> Result<String, EngineError> {
        self.0.start_job(labels)
    }
    fn job_status(&mut self, job: &str) -> Result<JobStatus, EngineError> {
        self.0.job_status(job)
    }
    fn cancel_job(&mut self, job: &str) -> Result<(), EngineError> {
        self.0.cancel_job(job)
    }
    fn label_map(&mut self) -> Result<Vec<u16>, EngineError> {
        self.0.label_map()
    }
}

impl SegmentationEngine for Peeking {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        self.0.info()
    }
    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        self.2.lock().unwrap().extend(records(&self.1));
        let s = self.0.open_session(volume, modality)?;
        Ok(Box::new(Watched(s, self.1.clone(), self.3.clone())))
    }
    fn unload(&self) -> Result<(), EngineError> {
        self.0.unload()
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
    closed_with_lease: Arc<Mutex<Vec<bool>>>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let src = ferrum_agent::evals::write_phantoms(&dir.path().join("in")).unwrap().remove(0);
        let locks = dir.path().join("gpu");
        let records = Arc::new(Mutex::new(Vec::new()));
        let closed_with_lease = Arc::new(Mutex::new(Vec::new()));
        let engine = Peeking(MockEngine::default(), locks.clone(), records.clone(), closed_with_lease.clone());
        let server = EngineServer::start(Arc::new(engine), "127.0.0.1:0", None).unwrap();
        let url = server.url();
        let ws = dir.path().join("ws").to_string_lossy().into_owned();
        Self { _dir: dir, _server: server, url, ws, src, locks, records, closed_with_lease }
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
    assert!(r.contains("\"holder\":\"ferrum-agent: FERRUM mock engine\""), "{r}");
    let leaks = [f.ws.as_str(), &f.src.to_string_lossy(), &file_name(&f.src)];
    assert!(leaks.iter().all(|l| !r.contains(*l)), "{r}");
    assert!(LockDir::at(&f.locks).unwrap().holders("card").unwrap().is_empty(), "released after the call");
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn engine_sessions_close_before_the_lease_is_released() {
    let f = Fixture::new();
    let agent = f.agent(30);
    assert_eq!(agent.run("study open", &json!({ "workspace": f.ws, "path": f.src }))["ok"], true);
    let auto = agent.run("segment auto", &json!({ "workspace": f.ws }));
    assert_eq!(auto["ok"], true, "{auto:#}");
    let point = json!({ "type": "point", "point": { "voxel": [14, 20, 15] }, "positive": true });
    let mut server = McpServer::new(agent);
    let call = json!({ "name": "ferrum_segment_interactive", "arguments": { "workspace": f.ws, "prompts": [point] } });
    let reply = server.handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": call })).unwrap();
    assert_eq!(reply["result"]["isError"], false, "{reply:#}");
    // the server drops a session when the client deletes it; give it a moment
    let start = Instant::now();
    while f.closed_with_lease.lock().unwrap().len() < 2 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        *f.closed_with_lease.lock().unwrap(),
        [true, true],
        "auto, then interactive (MCP): both closed under the lease"
    );
}

/// The mock engine without unload (an engine from before Stage 23).
struct NoUnload(MockEngine);

impl SegmentationEngine for NoUnload {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        let mut i = self.0.info()?;
        i.capabilities.unload = false;
        i.resident_mib = Some(2100);
        i.expected_peak_mib = Some(5400);
        Ok(i)
    }
    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        self.0.open_session(volume, modality)
    }
}

// covers 23.4-c, 23.4-d
#[test]
fn idle_engines_unload_busy_ones_keep_their_memory() {
    let f = Fixture::new();
    let old = EngineServer::start(Arc::new(NoUnload(MockEngine::default())), "127.0.0.1:0", None).unwrap();
    let mut c = AgentConfig { engines: vec![f.url.clone(), old.url()], is_default: false, ..AgentConfig::default() };
    c.gpu.groups = vec![("card".into(), vec![f.url.clone(), old.url()])];
    c.gpu.lock_dir = Some(f.locks.clone());
    let agent = Agent::new(c);

    let list = agent.run("engine list", &json!({}));
    let engines = list["data"]["engines"].as_array().unwrap();
    assert_eq!((engines[0]["unload"].as_bool(), engines[1]["unload"].as_bool()), (Some(true), Some(false)));
    assert_eq!(
        (engines[1]["resident_mib"].as_u64(), engines[1]["expected_peak_mib"].as_u64()),
        (Some(2100), Some(5400))
    );
    assert_eq!(engines[0]["resident_mib"], Value::Null);

    let group = agent.run("engine unload", &json!({ "group": "card" }));
    assert_eq!(group["ok"], true, "{group:#}");
    let results: Vec<&str> =
        group["data"]["engines"].as_array().unwrap().iter().map(|e| e["result"].as_str().unwrap()).collect();
    assert_eq!(results, ["unloaded", "unsupported"]);

    // an open session keeps the engine busy; nothing is stopped
    let engine = ferrum_engines::HttpEngine::new(ferrum_engines::HttpConfig::new(&f.url));
    let session = engine.open_session(&ferrum_agent::evals::sphere_cube().unwrap(), "CT").unwrap();
    let busy = agent.run("engine unload", &json!({ "engine": f.url }));
    assert_eq!(busy["data"]["engines"][0]["result"], "busy", "{busy:#}");
    assert!(busy["data"]["engines"][0]["retry_after_s"].as_u64().is_some());
    assert!(busy["warnings"].to_string().contains("busy engine keeps its memory"));
    drop(session);
    assert_eq!(agent.run("engine unload", &json!({}))["data"]["engines"][0]["result"], "unloaded", "default engine");

    let code = |p: Value| agent.run("engine unload", &p)["error"]["code"].as_str().unwrap_or("").to_owned();
    assert_eq!(code(json!({ "group": "card", "engine": f.url })), "bad_request");
    assert_eq!(code(json!({ "group": "other" })), "not_found");
    assert_eq!(code(json!({ "engine": "http://127.0.0.1:9" })), "forbidden");
}

/// The mock engine expecting 4 000 MiB per call.
struct Small(MockEngine);

impl SegmentationEngine for Small {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        Ok(EngineInfo { expected_peak_mib: Some(4000), ..self.0.info()? })
    }
    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        self.0.open_session(volume, modality)
    }
}

// covers 23.2-b
#[test]
fn the_engines_peak_raises_the_operators_never_lowers_it() {
    let f = Fixture::new();
    let small = EngineServer::start(Arc::new(Small(MockEngine::default())), "127.0.0.1:0", None).unwrap();
    let agent = |operator_peak: Option<u64>| {
        let mut c = AgentConfig {
            engines: vec![small.url()],
            engine_job_timeout_s: 1,
            is_default: false,
            ..AgentConfig::default()
        };
        c.gpu.groups = vec![("card".into(), vec![small.url()])];
        c.gpu.capacity_mib = vec![("card".into(), CAPACITY)];
        c.gpu.peaks_mib = operator_peak.map(|p| (small.url(), p)).into_iter().collect();
        c.gpu.lock_dir = Some(f.locks.clone());
        Agent::new(c)
    };
    // the holder takes 6 000 of 12 000 MiB
    let holder = f.holder();
    // no operator value: the engine's 4 000 MiB fit next to the holder (without it the call would need the whole group)
    let a = agent(None);
    assert_eq!(a.run("study open", &json!({ "workspace": f.ws, "path": f.src }))["ok"], true);
    let ran = a.run("segment auto", &json!({ "workspace": f.ws, "name_prefix": "a/" }));
    assert_eq!(ran["ok"], true, "{ran:#}");
    // the operator's 7 000 MiB stand although the engine says 4 000
    let refused = agent(Some(7000)).run("segment auto", &json!({ "workspace": f.ws, "name_prefix": "b/" }));
    assert_eq!(refused["error"]["code"], "engine_unavailable", "{refused:#}");
    release(holder);
}

// covers 23.4-c
#[test]
fn unload_closes_this_servers_own_cached_sessions_first() {
    let f = Fixture::new();
    // outside a GPU group the MCP server keeps engine sessions open between calls
    let agent = Agent::new(AgentConfig { engines: vec![f.url.clone()], is_default: false, ..AgentConfig::default() });
    assert_eq!(agent.run("study open", &json!({ "workspace": f.ws, "path": f.src }))["ok"], true);
    let mut server = McpServer::new(agent);
    let point = json!({ "type": "point", "point": { "voxel": [14, 20, 15] }, "positive": true });
    let call = |name: &str, args: Value| json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
    let r =
        server.handle(&call("ferrum_segment_interactive", json!({ "workspace": f.ws, "prompts": [point] }))).unwrap();
    assert_eq!(r["result"]["isError"], false, "{r:#}");
    assert_eq!(server.cached_engine_sessions(), 1);
    let u = server.handle(&call("ferrum_engine_unload", json!({}))).unwrap();
    assert_eq!(u["result"]["structuredContent"]["data"]["engines"][0]["result"], "unloaded", "{u:#}");
    assert_eq!(server.cached_engine_sessions(), 0);
}
