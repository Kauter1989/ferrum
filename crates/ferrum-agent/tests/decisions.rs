//! Review decisions through the library API, bound to what was shown (16.7):
//! the host reads an item's revision when it shows it, and records a
//! person's decision with that revision and a hash of what it showed.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use ferrum_agent::decisions::{record, revision, Decision, Item, Verdict};
use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::ReviewStatus;
use serde_json::{json, Value};

const SHOWN: &str = "5f2c8a1e9d3b7c6a4e0f1b2d3c4a5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f";

struct Fixture {
    _dir: tempfile::TempDir,
    ws: PathBuf,
    agent: Agent,
    config: AgentConfig,
    segment: u8,
    annotation: u64,
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
        let t = ok(
            &agent,
            "segment threshold",
            json!({ "workspace": ws, "seed": { "voxel": [14, 20, 15] }, "min": 0, "max": 200, "name": "sphere", "agent": "a" }),
        );
        let segment = u8::try_from(t["data"]["segment"]["label"].as_u64().unwrap()).unwrap();
        let a = ok(
            &agent,
            "annotate add",
            json!({ "workspace": ws, "kind": "distance", "plane": "axial", "agent": "a",
                    "points": [{ "voxel": [6, 20, 15] }, { "voxel": [22, 20, 15] }] }),
        );
        let annotation = a["data"]["annotation"]["id"].as_u64().unwrap();
        Self { _dir: dir, ws, agent, config, segment, annotation }
    }

    fn decide<'a>(&self, item: Item, verdict: Verdict, revision: &'a str) -> Decision<'a> {
        Decision { item, verdict, person: "Dr. K. Example", revision, shown_sha256: SHOWN }
    }

    fn status(&self, item: Item) -> String {
        let list = ok(&self.agent, "segment list", json!({ "workspace": self.ws }));
        match item {
            Item::Segment(label) => {
                list["data"]["segments"].as_array().unwrap().iter().find(|s| s["label"] == json!(label)).unwrap()
                    ["provenance"]["status"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            }
            Item::Annotation(id) => {
                let a = ok(&self.agent, "annotate list", json!({ "workspace": self.ws }));
                a["data"]["annotations"].as_array().unwrap().iter().find(|x| x["id"] == json!(id)).unwrap()
                    ["provenance"]["status"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            }
        }
    }
}

/// The audit entries the host's decisions wrote.
fn audit(ws: &Path) -> Vec<Value> {
    std::fs::read_to_string(ws.join("audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|e| e["source"] == "host")
        .collect()
}

// covers 16.7-a, 16.7-b
#[test]
fn a_decision_on_what_was_shown_is_recorded_with_both_hashes() {
    let f = Fixture::new();
    let item = Item::Segment(f.segment);
    let rev = revision(&f.config, &f.ws, item, None).unwrap();
    assert_eq!(rev.len(), 64);
    assert_eq!(rev, revision(&f.config, &f.ws, item, None).unwrap(), "stable while nothing changes");
    assert_eq!(f.status(item), "proposed");

    let done = record(&f.config, &f.ws, &f.decide(item, Verdict::Confirm, &rev), None).unwrap();
    assert_eq!(
        (done.status, done.person.as_str(), done.shown_sha256.as_str()),
        (ReviewStatus::Confirmed, "Dr. K. Example", SHOWN)
    );
    assert_eq!(f.status(item), "confirmed");
    let last = audit(&f.ws).pop().unwrap();
    assert_eq!(last["command"], "review confirm");
    assert_eq!(last["source"], "host");
    assert_eq!(last["params"]["segment"], json!(f.segment));
    assert_eq!(last["params"]["by"], "Dr. K. Example");
    assert_eq!(last["params"]["revision"], rev.as_str());
    assert_eq!(last["params"]["shown_sha256"], SHOWN);
    assert_eq!(last["time"], done.time.to_rfc3339(), "one time for the entry and the decision");
    assert_eq!(revision(&f.config, &f.ws, item, None).unwrap(), rev, "the decision is not content");

    let a = Item::Annotation(f.annotation);
    let arev = revision(&f.config, &f.ws, a, None).unwrap();
    assert_ne!(arev, rev);
    record(&f.config, &f.ws, &f.decide(a, Verdict::Reject, &arev), None).unwrap();
    assert_eq!(f.status(a), "rejected");
    let last = audit(&f.ws).pop().unwrap();
    assert_eq!(last["command"], "review reject");
    assert_eq!(last["params"]["source_sha256"].as_str().map(str::len), Some(64), "tied to the data");
}

// covers 16.7-c
#[test]
fn a_decided_item_is_not_decided_again() {
    let f = Fixture::new();
    let item = Item::Segment(f.segment);
    let rev = revision(&f.config, &f.ws, item, None).unwrap();
    let a =
        record(&f.config, &f.ws, &Decision { person: "Dr. B", ..f.decide(item, Verdict::Reject, &rev) }, None).unwrap();
    assert_eq!(a.status, ReviewStatus::Rejected);
    // another host showed the same proposed item and decides with the same revision
    let e = record(&f.config, &f.ws, &f.decide(item, Verdict::Confirm, &rev), None).unwrap_err();
    assert_eq!(e.code.as_str(), "forbidden");
    assert!(e.message.contains("already rejected by Dr. B"), "{}", e.message);
    assert_eq!(f.status(item), "rejected", "the first decision stands");
    assert_eq!(audit(&f.ws).len(), 1, "the refused decision is not logged as made");
}

// covers 16.7-a, 16.7-c
#[test]
fn a_changed_mask_changes_the_revision_without_a_rename() {
    let f = Fixture::new();
    let item = Item::Segment(f.segment);
    let shown = revision(&f.config, &f.ws, item, None).unwrap();
    // cut the sphere to one slab: same name, fewer voxels
    let slab = json!({ "min": { "voxel": [0, 0, 15] }, "max": { "voxel": [39, 39, 15] } });
    ok(
        &f.agent,
        "segment edit",
        json!({ "workspace": f.ws, "segment": f.segment, "op": "restrict_to_box", "box": slab }),
    );
    assert_ne!(revision(&f.config, &f.ws, item, None).unwrap(), shown);
    let e = record(&f.config, &f.ws, &f.decide(item, Verdict::Confirm, &shown), None).unwrap_err();
    assert_eq!(e.code.as_str(), "source_changed");
}

// covers 16.7-c
#[test]
fn a_stale_revision_records_nothing() {
    let f = Fixture::new();
    let item = Item::Segment(f.segment);
    let shown = revision(&f.config, &f.ws, item, None).unwrap();
    ok(&f.agent, "segment rename", json!({ "workspace": f.ws, "label": f.segment, "name": "renamed" }));
    let before = audit(&f.ws).len();
    let e = record(&f.config, &f.ws, &f.decide(item, Verdict::Confirm, &shown), None).unwrap_err();
    assert_eq!(e.code.as_str(), "source_changed");
    assert!(e.message.contains(&format!("segment {} changed since it was shown", f.segment)), "{}", e.message);
    assert_eq!(f.status(item), "proposed", "nothing was recorded");
    assert_eq!(audit(&f.ws).len(), before);
    // a changed mask changes the revision as well
    let now = revision(&f.config, &f.ws, item, None).unwrap();
    assert_ne!(now, shown);
    let ann = Item::Annotation(f.annotation);
    let shown_ann = revision(&f.config, &f.ws, ann, None).unwrap();
    ok(&f.agent, "segment delete", json!({ "workspace": f.ws, "label": f.segment }));
    assert_eq!(revision(&f.config, &f.ws, ann, None).unwrap(), shown_ann, "other items keep their revision");
}

// covers 16.7-c
#[test]
fn bad_input_is_refused() {
    let f = Fixture::new();
    let item = Item::Segment(f.segment);
    let rev = revision(&f.config, &f.ws, item, None).unwrap();
    let code = |d: Decision| record(&f.config, &f.ws, &d, None).unwrap_err().code.as_str().to_owned();
    assert_eq!(code(Decision { person: "  ", ..f.decide(item, Verdict::Confirm, &rev) }), "bad_request");
    assert_eq!(code(Decision { person: "a\nb", ..f.decide(item, Verdict::Confirm, &rev) }), "bad_request");
    assert_eq!(code(Decision { person: "Dr. \u{202e}evil", ..f.decide(item, Verdict::Confirm, &rev) }), "bad_request");
    let outside = AgentConfig { workspace_root: Some(f.ws.join("elsewhere")), ..f.config.clone() };
    assert_eq!(
        record(&outside, &f.ws, &f.decide(item, Verdict::Confirm, &rev), None).unwrap_err().code.as_str(),
        "forbidden"
    );
    assert_eq!(code(Decision { shown_sha256: "abc", ..f.decide(item, Verdict::Confirm, &rev) }), "bad_request");
    assert_eq!(code(f.decide(item, Verdict::Confirm, "zz")), "bad_request");
    assert_eq!(code(f.decide(Item::Segment(200), Verdict::Confirm, &rev)), "not_found");
    assert_eq!(code(f.decide(Item::Annotation(999), Verdict::Confirm, &rev)), "not_found");
    assert_eq!(f.status(item), "proposed");
    // an uppercase hash is the same hash
    let upper = rev.to_ascii_uppercase();
    assert_eq!(record(&f.config, &f.ws, &f.decide(item, Verdict::Confirm, &upper), None).unwrap().revision, rev);
}

// covers 16.7-d
#[test]
fn the_agent_command_set_does_not_change() {
    let names: Vec<&str> = Agent::commands().collect();
    assert!(names.iter().all(|n| !n.contains("decision") && !n.contains("record")), "{names:?}");
    for n in &names {
        let schema = ferrum_agent::schema::command_schema(n).unwrap().1.to_string();
        assert!(!schema.contains("shown_sha256"), "{n} must not take a host's decision");
    }
    // the command-line review path still needs the operator's switch
    let f = Fixture::new();
    let e = f.agent.run("review confirm", &json!({ "workspace": f.ws, "segment": f.segment, "by": "x" }));
    assert_eq!(e["error"]["code"], "forbidden");
}
