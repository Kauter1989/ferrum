//! Workspace change notifications (16.8): changes made through the agent
//! API reach an in-process subscriber before the call returns; changes
//! made by another process are found by polling; no change names anything.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use ferrum_agent::decisions::{record, revision, Decision, Item, Verdict};
use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::ReviewStatus;
use ferrum_io::workspace::changes::{subscribe, WorkspaceChange as C, WorkspaceWatcher};
use ferrum_io::workspace::Workspace;
use serde_json::{json, Value};

struct Fixture {
    _dir: tempfile::TempDir,
    ws: PathBuf,
    agent: Agent,
    config: AgentConfig,
}

fn ok(agent: &Agent, command: &str, params: Value) -> Value {
    let e = agent.run(command, &params);
    assert_eq!(e["ok"], true, "{command}: {e:#}");
    e
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let src = ferrum_agent::evals::write_phantoms(&dir.path().join("in")).unwrap().remove(0);
        let ws = dir.path().join("ws");
        let config = AgentConfig { is_default: false, ..AgentConfig::default() };
        let agent = Agent::new(config.clone());
        ok(&agent, "study open", json!({ "workspace": ws, "path": src }));
        Self { _dir: dir, ws, agent, config }
    }

    fn run(&self, command: &str, mut params: Value) -> Value {
        params["workspace"] = json!(self.ws);
        ok(&self.agent, command, params)
    }

    fn threshold(&self, name: &str) -> u8 {
        self.threshold_at(name, [14, 20, 15])
    }

    fn threshold_at(&self, name: &str, seed: [u32; 3]) -> u8 {
        let t = self.run(
            "segment threshold",
            json!({ "seed": { "voxel": seed }, "min": 0, "max": 200, "name": name, "agent": "a" }),
        );
        u8::try_from(t["data"]["segment"]["label"].as_u64().unwrap()).unwrap()
    }

    fn annotate(&self) -> u64 {
        let a = self.run(
            "annotate add",
            json!({ "kind": "distance", "plane": "axial", "agent": "a",
                    "points": [{ "voxel": [6, 20, 15] }, { "voxel": [22, 20, 15] }], "name": "Nodule of Ivanov" }),
        );
        a["data"]["annotation"]["id"].as_u64().unwrap()
    }
}

/// What the subscriber has now, without waiting: the change must already be there.
fn pending(rx: &Receiver<C>) -> Vec<C> {
    rx.try_iter().collect()
}

// covers 16.8-a, 16.8-b
#[test]
fn changes_through_the_agent_api_arrive_before_the_call_returns() {
    let f = Fixture::new();
    let rx = subscribe(&f.ws).unwrap();
    assert!(pending(&rx).is_empty(), "opening the study before subscribing is not a change");

    let label = f.threshold("sphere");
    assert_eq!(pending(&rx), vec![C::SegmentAdded { label }]);
    let id = f.annotate();
    assert_eq!(pending(&rx), vec![C::AnnotationAdded { id }]);

    f.run("annotate rename", json!({ "id": id, "name": "renamed" }));
    assert_eq!(pending(&rx), vec![C::AnnotationRevised { id }]);
    let slab = json!({ "min": { "voxel": [0, 0, 15] }, "max": { "voxel": [39, 39, 15] } });
    f.run("segment edit", json!({ "segment": label, "op": "restrict_to_box", "box": slab }));
    assert_eq!(pending(&rx), vec![C::SegmentRevised { label }], "a mask edit revises the segment");

    // a host's decision (16.7) is a review change, not a revision
    let item = Item::Segment(label);
    let rev = revision(&f.config, &f.ws, item, None).unwrap();
    let shown = "ab".repeat(32);
    let d =
        Decision { item, verdict: Verdict::Confirm, person: "Dr. K. Example", revision: &rev, shown_sha256: &shown };
    record(&f.config, &f.ws, &d, None).unwrap();
    assert_eq!(pending(&rx), vec![C::SegmentReviewed { label, status: ReviewStatus::Confirmed }]);

    f.run("annotate delete", json!({ "id": id }));
    assert_eq!(pending(&rx), vec![C::AnnotationRemoved { id }]);
    // the first segment was cut to slice 15: the rest of the sphere is free
    let other = f.threshold_at("second", [14, 20, 18]);
    assert_eq!(pending(&rx), vec![C::SegmentAdded { label: other }]);

    // reading changes nothing
    f.run("segment list", json!({}));
    f.run("annotate list", json!({}));
    assert!(pending(&rx).is_empty());

    // a second subscriber of the same workspace gets the same, and dropping one unsubscribes
    let rx2 = subscribe(&f.ws).unwrap();
    drop(rx);
    f.run("segment delete", json!({ "label": other }));
    assert_eq!(pending(&rx2), vec![C::SegmentRemoved { label: other }]);
}

/// Rewrites `annotations.json` as another program would (no FERRUM save).
fn edit_json(path: &Path, f: impl FnOnce(&mut Value)) {
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    f(&mut v);
    std::fs::write(path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

// covers 16.8-a, 16.8-c
#[test]
fn a_watcher_finds_changes_made_by_another_process() {
    let f = Fixture::new();
    let id = f.annotate();
    let label = f.threshold("sphere");
    let mut watcher = WorkspaceWatcher::new(&f.ws).unwrap();
    assert!(watcher.poll().unwrap().is_empty(), "nothing changed since it started");

    // another program confirms the annotation by editing the file
    edit_json(&f.ws.join("annotations.json"), |v| {
        v["annotations"][0]["provenance"]["status"] = json!("confirmed");
    });
    assert_eq!(watcher.poll().unwrap(), vec![C::AnnotationReviewed { id, status: ReviewStatus::Confirmed }]);
    assert!(watcher.poll().unwrap().is_empty(), "each change is reported once");

    // another process (here an agent with its own state) edits the mask
    let other = Agent::new(f.config.clone());
    let slab = json!({ "min": { "voxel": [0, 0, 15] }, "max": { "voxel": [39, 39, 15] } });
    ok(&other, "segment edit", json!({ "workspace": f.ws, "segment": label, "op": "restrict_to_box", "box": slab }));
    assert_eq!(watcher.poll().unwrap(), vec![C::SegmentRevised { label }]);

    // the open volume of a multi-volume series switches: only its key is given
    let mut ws = Workspace::open(&f.ws).unwrap();
    ws.set_volume(Some("1.2.840.99999.1#v=b1000")).unwrap();
    assert_eq!(watcher.poll().unwrap(), vec![C::VolumeSwitched { key: Some("b1000".into()) }]);

    // a file removed by hand removes its items
    std::fs::remove_file(f.ws.join("annotations.json")).unwrap();
    assert_eq!(watcher.poll().unwrap(), vec![C::AnnotationRemoved { id }]);
}

// covers 16.8-d
#[test]
fn changes_name_nothing() {
    let f = Fixture::new();
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(f.ws.join("workspace.json")).unwrap()).unwrap();
    let rx = subscribe(&f.ws).unwrap();
    let mut watcher = WorkspaceWatcher::new(&f.ws).unwrap();
    let label = f.threshold("Ivanov liver");
    let id = f.annotate();
    let rev = revision(&f.config, &f.ws, Item::Annotation(id), None).unwrap();
    let shown = "cd".repeat(32);
    let d = Decision {
        item: Item::Annotation(id),
        verdict: Verdict::Reject,
        person: "Dr. Petrova",
        revision: &rev,
        shown_sha256: &shown,
    };
    record(&f.config, &f.ws, &d, None).unwrap();
    let mut seen: Vec<C> = pending(&rx);
    seen.extend(watcher.poll().unwrap());
    assert!(seen.contains(&C::SegmentAdded { label }) && seen.contains(&C::AnnotationAdded { id }), "{seen:?}");
    let text = format!("{seen:?}");
    let series = manifest["source"]["series_id"].as_str().unwrap();
    let path = manifest["source"]["path"].as_str().unwrap();
    for secret in [series, path, "Ivanov", "Petrova", "Nodule", "liver"] {
        assert!(!text.contains(secret), "{secret:?} in {text}");
    }
}

// covers 16.8-e
#[test]
fn the_agent_command_set_does_not_change() {
    let names: Vec<&str> = Agent::commands().collect();
    assert!(
        names.iter().all(|n| !n.contains("watch") && !n.contains("subscribe") && !n.contains("change")),
        "{names:?}"
    );
}
