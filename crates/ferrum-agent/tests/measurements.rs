//! Measurements that staging relies on (17.8): the distance from a segment
//! to the surface of another, and the solid part of a part-solid lesion,
//! on a synthetic volume with exactly known geometry.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use ferrum_agent::mcp::McpServer;
use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{Dims3, Volume};
use glam::Vec3;
use serde_json::{json, Value};

const SPACING: Vec3 = Vec3::new(0.8, 0.8, 2.0);

/// 40 × 30 × 20 voxels of air (−1000) with:
/// - a lesion x 5..=14, y 10..=19, z 5..=9 at −500, its solid core
///   x 8..=11, y 13..=16, z 6..=8 at +50;
/// - a block x 20..=24, y 10..=19, z 5..=9 at +300 (6 voxels from the lesion along x).
fn write_volume(path: &std::path::Path) {
    let dims = Dims3::new(40, 30, 20);
    let mut values = vec![-1000.0f32; dims.voxel_count()];
    let inside = |v: u32, lo: u32, hi: u32| (lo..=hi).contains(&v);
    for k in 0..20u32 {
        for j in 0..30u32 {
            for i in 0..40u32 {
                let n = (i + 40 * (j + 30 * k)) as usize;
                if inside(i, 5, 14) && inside(j, 10, 19) && inside(k, 5, 9) {
                    values[n] = -500.0;
                }
                if inside(i, 8, 11) && inside(j, 13, 16) && inside(k, 6, 8) {
                    values[n] = 50.0;
                }
                if inside(i, 20, 24) && inside(j, 10, 19) && inside(k, 5, 9) {
                    values[n] = 300.0;
                }
            }
        }
    }
    let volume = Volume::from_physical(dims, SPACING, &values).unwrap();
    ferrum_io::write_nifti(&volume, path).unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    ws: String,
    agent: Agent,
    lesion: u64,
    block: u64,
}

fn ok(agent: &Agent, command: &str, params: Value) -> Value {
    let e = agent.run(command, &params);
    assert_eq!(e["ok"], true, "{command}: {e:#}");
    e
}

impl Fixture {
    fn new(modality: Option<&str>) -> Self {
        Self::with(modality, false)
    }

    /// With `core_first`, the solid core is a segment of its own, made
    /// first, so the lesion's segment has a hole where the core is.
    fn with(modality: Option<&str>, core_first: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("lesion.nii.gz");
        write_volume(&src);
        let ws = dir.path().join("ws").to_string_lossy().into_owned();
        let agent = Agent::new(AgentConfig { is_default: false, ..AgentConfig::default() });
        let mut open = json!({ "workspace": ws, "path": src });
        if let Some(m) = modality {
            open["modality"] = json!(m);
        }
        ok(&agent, "study open", open);
        let threshold = |seed: [u32; 3], min: f64, max: f64, name: &str| {
            let p = json!({ "workspace": ws, "seed": { "voxel": seed }, "min": min, "max": max, "name": name, "agent": "a" });
            ok(&agent, "segment threshold", p)["data"]["segment"]["label"].as_u64().unwrap()
        };
        if core_first {
            threshold([9, 14, 7], 0.0, 100.0, "core");
        }
        let lesion = threshold([6, 11, 6], -700.0, 200.0, "lesion");
        let block = threshold([22, 15, 7], 250.0, 400.0, "block");
        Self { _dir: dir, ws, agent, lesion, block }
    }

    fn params(&self, extra: Value) -> Value {
        let mut p = json!({ "workspace": self.ws });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    }
}

fn mm(v: &Value) -> f64 {
    v["mm"].as_f64().unwrap()
}

// covers 17.8-a, 17.8-d
#[test]
fn distance_to_a_surface_at_a_known_gap() {
    let f = Fixture::new(Some("CT"));
    let d = ok(&f.agent, "segment distance", f.params(json!({ "segment": f.lesion, "to": f.block })));
    let data = &d["data"];
    // voxel centres x = 14 and x = 20: 6 × 0.8 mm
    assert!((mm(&data["distance"]) - 4.8).abs() < 1e-6, "{data:#}");
    assert_eq!(data["distance"]["from"]["voxel_index"][0], 14);
    assert_eq!(data["distance"]["to"]["voxel_index"][0], 20);
    assert_eq!(data["distance"]["uncertainty_mm"], 2.0, "the largest spacing");
    assert_eq!(data["inside"], false);
    assert_eq!(data["segment"]["label"].as_u64(), Some(f.lesion));
    assert_eq!(data["to"]["label"].as_u64(), Some(f.block));
    let back = ok(&f.agent, "segment distance", f.params(json!({ "segment": f.block, "to": f.lesion })));
    assert!((mm(&back["data"]["distance"]) - 4.8).abs() < 1e-6);

    // the same JSON over MCP
    let mut server = McpServer::new(Agent::new(AgentConfig { is_default: false, ..AgentConfig::default() }));
    let call = json!({ "name": "ferrum_segment_distance", "arguments": f.params(json!({ "segment": f.lesion, "to": f.block })) });
    let reply = server.handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": call })).unwrap();
    assert_eq!(reply["result"]["structuredContent"]["data"], *data);
}

// covers 17.8-b
#[test]
fn distance_refuses_unknown_empty_or_identical_segments() {
    let f = Fixture::new(Some("CT"));
    let code = |extra: Value| {
        let e = f.agent.run("segment distance", &f.params(extra));
        assert_eq!(e["ok"], false, "{e:#}");
        e["error"]["code"].as_str().unwrap().to_owned()
    };
    assert_eq!(code(json!({ "segment": f.lesion, "to": f.lesion })), "bad_request");
    assert_eq!(code(json!({ "segment": f.lesion, "to": 99 })), "not_found");
    assert_eq!(code(json!({ "segment": f.lesion })), "bad_request");
}

// covers 17.8-c
#[test]
fn the_solid_part_of_a_lesion_is_measured_at_the_given_threshold() {
    let f = Fixture::new(Some("CT"));
    let s = ok(&f.agent, "segment shape", f.params(json!({ "segment": f.lesion, "solid_min": -100 })));
    let total = &s["data"]["shape"];
    let solid = &s["data"]["solid"];
    assert_eq!(total["voxels"], 500);
    assert_eq!(solid["min_value"], -100.0);
    assert_eq!(solid["voxels"], 48, "4 × 4 × 3 core voxels");
    assert_eq!(solid["share"], 0.096);
    assert_eq!(solid["volume_ml"], 0.061);
    // the diagonal between the centres of a 4 × 4 square at 0.8 mm
    assert_eq!(solid["long_axis"]["mm"], 3.39);
    assert!(mm(&total["long_axis"]) > mm(&solid["long_axis"]));
    assert!(solid["method"].as_str().unwrap().contains("not a classification"));
    assert!(s["warnings"].as_array().unwrap().is_empty(), "CT values are HU: {s:#}");

    let none = ok(&f.agent, "segment shape", f.params(json!({ "segment": f.lesion, "solid_min": 1000 })));
    assert_eq!(none["data"]["solid"]["voxels"], 0);
    assert_eq!(none["data"]["solid"]["long_axis"], Value::Null);
    let plain = ok(&f.agent, "segment shape", f.params(json!({ "segment": f.lesion })));
    assert!(plain["data"].get("solid").is_none(), "no threshold, no solid part");
}

// covers 17.8-c
#[test]
fn a_threshold_on_values_not_known_as_hu_warns() {
    let f = Fixture::new(None);
    let s = ok(&f.agent, "segment shape", f.params(json!({ "segment": f.lesion, "solid_min": -100 })));
    assert_eq!(s["data"]["solid"]["voxels"], 48);
    let w = s["warnings"].as_array().unwrap();
    assert!(w.iter().any(|w| w.as_str().unwrap().contains("not known to be HU")), "{w:?}");
}

// covers 17.8-a
#[test]
fn a_segment_in_a_hole_of_another_is_measured_to_its_outer_surface() {
    // as a nodule inside a lung segment: the core is segment 1, the lesion around it has a hole there
    let f = Fixture::with(Some("CT"), true);
    let core = 1;
    let within = ok(&f.agent, "segment distance", f.params(json!({ "segment": core, "to": f.lesion })));
    let d = &within["data"];
    assert_eq!(d["inside"], true, "{d:#}");
    // core z 6..=8, lesion z 5..=9 at 2 mm: one slice to the outer surface (x and y are 3 × 0.8 mm away)
    assert!((mm(&d["distance"]) - 2.0).abs() < 1e-6, "{d:#}");
    assert!(d["method"].as_str().unwrap().contains("holes filled"));
    let around = ok(&f.agent, "segment distance", f.params(json!({ "segment": f.lesion, "to": core })));
    assert_eq!(around["data"]["inside"], false);
    assert!((mm(&around["data"]["distance"]) - 0.8).abs() < 1e-6, "neighbouring voxels: one spacing");
}
