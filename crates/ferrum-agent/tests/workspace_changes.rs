//! Workspace change notifications (16.8): changes made through the agent
//! API reach an in-process subscriber before the call returns; changes
//! made by another process are found by polling; no change names anything.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::{Path, PathBuf};

use ferrum_agent::decisions::{record, revision, Decision, Item, Verdict};
use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::ReviewStatus;
use ferrum_io::workspace::changes::{subscribe, Subscription, WorkspaceChange as C, WorkspaceWatcher};
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
fn pending(rx: &Subscription) -> Vec<C> {
    rx.try_iter().collect()
}

// covers 16.8-a, 16.8-b
#[test]
fn changes_through_the_agent_api_arrive_before_the_call_returns() {
    let f = Fixture::new();
    let rx = subscribe(&f.ws);
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
    let rx2 = subscribe(&f.ws);
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
    let mut watcher = WorkspaceWatcher::new(&f.ws);
    assert!(watcher.poll().is_empty(), "nothing changed since it started");

    // another program confirms the annotation by editing the file
    edit_json(&f.ws.join("annotations.json"), |v| {
        v["annotations"][0]["provenance"]["status"] = json!("confirmed");
    });
    assert_eq!(watcher.poll(), vec![C::AnnotationReviewed { id, status: ReviewStatus::Confirmed }]);
    assert!(watcher.poll().is_empty(), "each change is reported once");

    // another process (here an agent with its own state) edits the mask
    let other = Agent::new(f.config.clone());
    let slab = json!({ "min": { "voxel": [0, 0, 15] }, "max": { "voxel": [39, 39, 15] } });
    ok(&other, "segment edit", json!({ "workspace": f.ws, "segment": label, "op": "restrict_to_box", "box": slab }));
    assert_eq!(watcher.poll(), vec![C::SegmentRevised { label }]);

    // the open volume of a multi-volume series switches: only its key is given
    let mut ws = Workspace::open(&f.ws).unwrap();
    ws.set_volume(Some("1.2.840.99999.1#v=b1000")).unwrap();
    assert_eq!(watcher.poll(), vec![C::VolumeSwitched { key: Some("b1000".into()) }]);

    // a file removed by hand removes its items
    std::fs::remove_file(f.ws.join("annotations.json")).unwrap();
    assert_eq!(watcher.poll(), vec![C::AnnotationRemoved { id }]);
}

// covers 16.8-d
#[test]
fn changes_name_nothing() {
    let f = Fixture::new();
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(f.ws.join("workspace.json")).unwrap()).unwrap();
    let rx = subscribe(&f.ws);
    let mut watcher = WorkspaceWatcher::new(&f.ws);
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
    // a crafted manifest whose volume "key" is the series UID or a name
    let mut ws = Workspace::open(&f.ws).unwrap();
    ws.set_volume(Some(&format!("x#v={}", manifest["source"]["series_id"].as_str().unwrap()))).unwrap();
    ws.set_volume(Some("x#v=Ivanov^Petr")).unwrap();
    let mut seen: Vec<C> = pending(&rx);
    seen.extend(watcher.poll());
    assert!(seen.contains(&C::VolumeSwitched { key: Some("other".into()) }), "{seen:?}");
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

// covers 16.8-b
#[test]
fn a_subscription_made_before_the_workspace_exists_receives_its_changes() {
    let dir = tempfile::tempdir().unwrap();
    let src = ferrum_agent::evals::write_phantoms(&dir.path().join("in")).unwrap().remove(0);
    let ws = dir.path().join("later").join("ws");
    let rx = subscribe(&ws);
    let agent = Agent::new(AgentConfig { is_default: false, ..AgentConfig::default() });
    ok(&agent, "study open", json!({ "workspace": ws, "path": src }));
    ok(
        &agent,
        "segment threshold",
        json!({ "workspace": ws, "seed": { "voxel": [14, 20, 15] }, "min": 0, "max": 200, "agent": "a" }),
    );
    assert!(pending(&rx).iter().any(|c| matches!(c, C::SegmentAdded { .. })));
}

// covers 16.8-c
#[test]
fn the_label_map_is_read_again_only_when_its_stamp_changes() {
    let f = Fixture::new();
    f.threshold("sphere");
    let mut watcher = WorkspaceWatcher::new(&f.ws);
    let map = f.ws.join("segments.nii.gz");
    let mtime = std::fs::metadata(&map).unwrap().modified().unwrap();
    let len = std::fs::metadata(&map).unwrap().len() as usize;
    // garbage of the same length, in place (same inode), with the old time
    std::fs::OpenOptions::new().write(true).open(&map).unwrap().write_all(&vec![0u8; len]).unwrap();
    std::fs::File::options().write(true).open(&map).unwrap().set_modified(mtime).unwrap();
    assert!(watcher.poll().is_empty(), "same size, time and inode: not decoded again");
    std::fs::OpenOptions::new().append(true).open(&map).unwrap().write_all(b"x").unwrap();
    assert_eq!(watcher.poll(), vec![C::FileUnreadable { file: "segments.nii.gz" }], "a new size is read");
    assert!(watcher.poll().is_empty(), "reported once");
}

// covers 16.8-c
#[test]
fn a_save_caught_between_its_two_renames_reports_nothing_false() {
    let f = Fixture::new();
    let label = f.threshold("sphere");
    let slab = json!({ "min": { "voxel": [0, 0, 15] }, "max": { "voxel": [39, 39, 15] } });
    f.run("segment edit", json!({ "segment": label, "op": "restrict_to_box", "box": slab }));
    let mut watcher = WorkspaceWatcher::new(&f.ws);
    let (map, meta) = (f.ws.join("segments.nii.gz"), f.ws.join("segments.json"));
    let meta_before = std::fs::read(&meta).unwrap();
    let other = f.threshold_at("second", [14, 20, 18]);
    let meta_after = std::fs::read(&meta).unwrap();
    // the state between the renames: the new label map, the old metadata
    std::fs::write(&meta, &meta_before).unwrap();
    let mtime = std::fs::metadata(&map).unwrap().modified().unwrap();
    std::fs::File::options()
        .write(true)
        .open(&meta)
        .unwrap()
        .set_modified(mtime - std::time::Duration::from_secs(5))
        .unwrap();
    let mid = watcher.poll();
    assert!(mid.is_empty(), "held back for one poll: {mid:?}");
    std::fs::write(&meta, &meta_after).unwrap();
    assert_eq!(watcher.poll(), vec![C::SegmentAdded { label: other }]);
    assert_ne!(label, other);
}

// covers 16.8-c
#[cfg(unix)]
#[test]
fn a_planted_fifo_is_refused_without_blocking() {
    let f = Fixture::new();
    let id = f.annotate();
    let rx = subscribe(&f.ws);
    let mut watcher = WorkspaceWatcher::new(&f.ws);
    let path = f.ws.join("annotations.json");
    std::fs::remove_file(&path).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(&path).status().unwrap().success());
    assert_eq!(watcher.poll(), vec![C::FileUnreadable { file: "annotations.json" }], "the FIFO is not opened");
    // the hub of this process does not block either, and other files still count (a save that
    // does not load the study: the study loader itself still opens annotations.json)
    Workspace::open(&f.ws).unwrap().set_volume(Some("x#v=b1000")).unwrap();
    std::fs::remove_file(&path).unwrap(); // the hub read the manifest only; nothing else touched the FIFO
    let seen = pending(&rx);
    assert_eq!(
        seen,
        vec![C::VolumeSwitched { key: Some("b1000".into()) }, C::FileUnreadable { file: "annotations.json" }],
        "the changed annotations file is refused, not opened"
    );
    assert_eq!(
        watcher.poll(),
        vec![C::AnnotationRemoved { id }, C::VolumeSwitched { key: Some("b1000".into()) }],
        "the file is gone now; the watcher sees the switch too"
    );
}
