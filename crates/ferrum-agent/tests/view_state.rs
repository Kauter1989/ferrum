//! Contract tests of `view set`, `view state`, `view events` (Stage 28, #70):
//! the artifact round trip, the source-hash binding, the event log, CLI = MCP,
//! and that no command creates a decision.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{Dims3, Geometry, SliceAxis, ViewEvent, Volume};
use glam::{Mat3, Vec3};
use serde_json::{json, Value};

fn phantom() -> Volume {
    let dims = Dims3::new(40, 40, 30);
    let mut values = vec![-1000.0f32; dims.voxel_count()];
    for k in 0..30u32 {
        for j in 0..40u32 {
            for i in 0..40u32 {
                let d = Vec3::new(i as f32 - 14.0, j as f32 - 20.0, (k as f32 - 15.0) * 2.0);
                if d.length() <= 8.0 {
                    values[dims.index(i, j, k)] = 100.0;
                }
            }
        }
    }
    let v = Volume::from_physical(dims, Vec3::new(1.0, 1.0, 2.0), &values).unwrap();
    v.with_geometry(Geometry { origin: Vec3::new(-20.0, -20.0, 100.0), direction: Mat3::IDENTITY })
}

struct Fixture {
    _dir: tempfile::TempDir,
    data: PathBuf,
    ws: String,
    agent: Agent,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    ferrum_io::write_nifti(&phantom(), &data.join("phantom.nii.gz")).unwrap();
    let ws = dir.path().join("ws/ct1").to_string_lossy().into_owned();
    Fixture { _dir: dir, data, ws, agent: Agent::new(AgentConfig::default()) }
}

impl Fixture {
    fn run(&self, command: &str, mut params: Value) -> Value {
        if params.get("workspace").is_none() {
            params["workspace"] = json!(self.ws);
        }
        self.agent.run(command, &params)
    }
    fn ok(&self, command: &str, params: Value) -> Value {
        let env = self.run(command, params);
        assert_eq!(env["ok"], true, "{command}: {env:#}");
        env["data"].clone()
    }
    fn err(&self, command: &str, params: Value) -> String {
        let env = self.run(command, params);
        assert_eq!(env["ok"], false, "{command} should fail: {env:#}");
        env["error"]["code"].as_str().unwrap().to_owned()
    }
    fn open(&self) {
        self.ok("study open", json!({ "path": self.data.join("phantom.nii.gz") }));
    }
}

// covers 28.3-a
#[test]
fn view_state_defaults_then_set_round_trips() {
    let f = fixture();
    f.open();
    let before = f.ok("view state", json!({}));
    assert_eq!(before["state"]["component"], "slice");
    assert_eq!(before["revision"], 0);
    assert!(before["source_hash"].as_str().unwrap().len() >= 64);

    let set = f.ok(
        "view set",
        json!({ "state": { "plane": "coronal", "slice_index": 5, "window": { "center": -600, "width": 1500 }, "tool": "probe" } }),
    );
    assert_eq!(set["revision"], 1);
    assert_eq!(set["state"]["plane"], "coronal");
    assert_eq!(set["state"]["slice_index"], 5);
    assert_eq!(set["state"]["tool"], "probe");

    let after = f.ok("view state", json!({}));
    assert_eq!(after["revision"], 1);
    assert_eq!(after["state"], set["state"]);
    assert_eq!(after["source_hash"], before["source_hash"]);
}

// covers 28.4-a
#[test]
fn a_stale_source_hash_is_refused() {
    let f = fixture();
    f.open();
    let code = f.err("view set", json!({ "source_hash": "stale", "state": { "slice_index": 3 } }));
    assert_eq!(code, "source_changed");
    // nothing was applied
    assert_eq!(f.ok("view state", json!({}))["revision"], 0);
}

// covers 28.3-a
#[test]
fn an_unknown_state_field_is_refused() {
    let f = fixture();
    f.open();
    assert_eq!(f.err("view set", json!({ "state": { "bogus": 1 } })), "bad_request");
    assert_eq!(f.err("view set", json!({ "state": { "component": "3d" } })), "bad_request");
}

// covers 28.3-b
#[test]
fn view_events_reads_the_log_after_a_sequence() {
    let f = fixture();
    f.open();
    let ws = ferrum_io::Workspace::open(std::path::Path::new(&f.ws)).unwrap();
    ws.save_view_events(
        &[
            (1, ViewEvent::SliceChanged { plane: SliceAxis::Axial, slice_index: 4 }),
            (2, ViewEvent::WindowChanged { center: -600.0, width: 1500.0 }),
            (3, ViewEvent::RoiDrawn { item: ferrum_domain::RoiItem::Segment(2) }),
        ],
        3,
        0,
    )
    .unwrap();

    let all = f.ok("view events", json!({}));
    assert_eq!(all["events"].as_array().unwrap().len(), 3);
    assert_eq!(all["next"], 3);
    assert_eq!(all["events"][2]["kind"], "roi_drawn");

    let after = f.ok("view events", json!({ "since": 1 }));
    assert_eq!(after["events"].as_array().unwrap().len(), 2);
    assert_eq!(after["events"][0]["seq"], 2);
}

// covers 28.3-c
#[test]
fn view_set_creates_no_decision() {
    let f = fixture();
    f.open();
    f.ok("segment threshold", json!({ "seed": { "voxel": [14, 20, 15] }, "min": 50, "max": 150, "name": "Sphere" }));
    let before = f.ok("review list", json!({}));
    f.ok("view set", json!({ "state": { "selected_segment": 1, "overlays": { "segments_shown": true } } }));
    let after = f.ok("review list", json!({}));
    assert_eq!(before, after, "view set must not confirm or reject anything");
    let seg = &f.ok("segment list", json!({}))["segments"][0]["provenance"];
    assert_eq!(seg["status"], "proposed");
}

// covers 28.3-b
#[test]
fn cli_and_mcp_give_identical_json() {
    let f = fixture();
    f.open();
    let cli = f.ok("view state", json!({}));

    let mut mcp = ferrum_agent::mcp::McpServer::new(Agent::new(AgentConfig::default()));
    mcp.handle(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "ferrum_study_open", "arguments": { "workspace": f.ws, "path": f.data.join("phantom.nii.gz") } }
    }));
    let resp = mcp
        .handle(&json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "ferrum_view_state", "arguments": { "workspace": f.ws } }
        }))
        .unwrap();
    let mcp_data = &resp["result"]["structuredContent"]["data"];
    assert_eq!(&cli, mcp_data, "CLI and MCP must give identical JSON");
}
