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
    let rev = revision(&f.config, &f.ws, item).unwrap();
    assert_eq!(rev.len(), 64);
    assert_eq!(rev, revision(&f.config, &f.ws, item).unwrap(), "stable while nothing changes");
    assert_eq!(f.status(item), "proposed");

    let done = record(&f.config, &f.ws, &f.decide(item, Verdict::Confirm, &rev)).unwrap();
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
    assert_eq!(revision(&f.config, &f.ws, item).unwrap(), rev, "the decision is not content");

    let a = Item::Annotation(f.annotation);
    let arev = revision(&f.config, &f.ws, a).unwrap();
    assert_ne!(arev, rev);
    record(&f.config, &f.ws, &f.decide(a, Verdict::Reject, &arev)).unwrap();
    assert_eq!(f.status(a), "rejected");
    assert_eq!(audit(&f.ws).pop().unwrap()["command"], "review reject");
}

// covers 16.7-c
#[test]
fn a_stale_revision_records_nothing() {
    let f = Fixture::new();
    let item = Item::Segment(f.segment);
    let shown = revision(&f.config, &f.ws, item).unwrap();
    ok(&f.agent, "segment rename", json!({ "workspace": f.ws, "label": f.segment, "name": "renamed" }));
    let before = audit(&f.ws).len();
    let e = record(&f.config, &f.ws, &f.decide(item, Verdict::Confirm, &shown)).unwrap_err();
    assert_eq!(e.code.as_str(), "source_changed");
    assert!(e.message.contains(&format!("segment {} changed since it was shown", f.segment)), "{}", e.message);
    assert_eq!(f.status(item), "proposed", "nothing was recorded");
    assert_eq!(audit(&f.ws).len(), before);
    // a changed mask changes the revision as well
    let now = revision(&f.config, &f.ws, item).unwrap();
    assert_ne!(now, shown);
    let ann = Item::Annotation(f.annotation);
    let shown_ann = revision(&f.config, &f.ws, ann).unwrap();
    ok(&f.agent, "segment delete", json!({ "workspace": f.ws, "label": f.segment }));
    assert_eq!(revision(&f.config, &f.ws, ann).unwrap(), shown_ann, "other items keep their revision");
}

// covers 16.7-c
#[test]
fn bad_input_is_refused() {
    let f = Fixture::new();
    let item = Item::Segment(f.segment);
    let rev = revision(&f.config, &f.ws, item).unwrap();
    let code = |d: Decision| record(&f.config, &f.ws, &d).unwrap_err().code.as_str().to_owned();
    assert_eq!(code(Decision { person: "  ", ..f.decide(item, Verdict::Confirm, &rev) }), "bad_request");
    assert_eq!(code(Decision { person: "a\nb", ..f.decide(item, Verdict::Confirm, &rev) }), "bad_request");
    assert_eq!(code(Decision { shown_sha256: "abc", ..f.decide(item, Verdict::Confirm, &rev) }), "bad_request");
    assert_eq!(code(f.decide(item, Verdict::Confirm, "zz")), "bad_request");
    assert_eq!(code(f.decide(Item::Segment(200), Verdict::Confirm, &rev)), "not_found");
    assert_eq!(code(f.decide(Item::Annotation(999), Verdict::Confirm, &rev)), "not_found");
    assert_eq!(f.status(item), "proposed");
}

// covers 16.7-d
#[test]
fn the_agent_command_set_does_not_change() {
    let names: Vec<&str> = Agent::commands().collect();
    assert!(names.iter().all(|n| !n.contains("decision") && !n.contains("record")), "{names:?}");
    // the command-line review path still needs the operator's switch
    let f = Fixture::new();
    let e = f.agent.run("review confirm", &json!({ "workspace": f.ws, "segment": f.segment, "by": "x" }));
    assert_eq!(e["error"]["code"], "forbidden");
}
