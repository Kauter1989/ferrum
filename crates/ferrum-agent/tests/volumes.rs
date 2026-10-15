//! Contract tests of multi-volume series (Stage 20.3): one scan entry per
//! volume, opening and switching volumes in one workspace, and the volume
//! named in every value's provenance.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{Dims3, Volume};
use glam::Vec3;
use serde_json::{json, Value};

/// 16×16×8 voxels: −1000 with a block of `block` (voxels 4..12 in i and j,
/// 2..6 in k) and one voxel of 1000, so that both volumes share their range
/// and so the NIfTI header.
fn phantom(block: f32) -> Volume {
    let dims = Dims3::new(16, 16, 8);
    let mut values = vec![-1000.0f32; dims.voxel_count()];
    for k in 2..6 {
        for j in 4..12 {
            for i in 4..12 {
                values[dims.index(i, j, k)] = block;
            }
        }
    }
    values[dims.index(0, 0, 0)] = 1000.0;
    Volume::from_physical(dims, Vec3::new(1.0, 1.0, 2.0), &values).unwrap()
}

/// A 4D NIfTI of a b = 0 volume (block 100) and a b = 1000 volume
/// (block 500), with its BIDS `.bval`.
fn dwi(dir: &Path) -> PathBuf {
    let path = dir.join("sub-01_dwi.nii");
    let tmp = dir.join("second.nii");
    ferrum_io::write_nifti(&phantom(100.0), &path).unwrap();
    ferrum_io::write_nifti(&phantom(500.0), &tmp).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let other = std::fs::read(&tmp).unwrap();
    std::fs::remove_file(&tmp).unwrap();
    assert_eq!(bytes[..352], other[..352], "same header");
    bytes[40..42].copy_from_slice(&4i16.to_le_bytes());
    bytes[48..50].copy_from_slice(&2i16.to_le_bytes());
    bytes.extend_from_slice(&other[352..]);
    std::fs::write(&path, bytes).unwrap();
    std::fs::write(dir.join("sub-01_dwi.bval"), "0 1000\n").unwrap();
    path
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    src: PathBuf,
    ws: String,
    agent: Agent,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let src = dwi(&data);
    let ws = root.join("ws").to_string_lossy().into_owned();
    Fixture { _dir: dir, root, src, ws, agent: Agent::new(AgentConfig::default()) }
}

impl Fixture {
    fn run(&self, command: &str, mut params: Value) -> Value {
        if params.get("workspace").is_none() && command != "study scan" {
            params["workspace"] = json!(self.ws);
        }
        self.agent.run(command, &params)
    }

    fn ok(&self, command: &str, params: Value) -> Value {
        let env = self.run(command, params);
        assert_eq!(env["ok"], true, "{command}: {env:#}");
        env
    }

    fn open(&self, series: Option<&str>) -> Value {
        let mut params = json!({ "path": self.src });
        if let Some(s) = series {
            params["series"] = json!(s);
        }
        self.ok("study open", params)
    }

    fn probe(&self) -> Value {
        self.ok("probe", json!({ "point": { "voxel": [8, 8, 3] } }))
    }
}

fn warned(env: &Value, text: &str) -> bool {
    env["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains(text))
}

#[test]
fn scan_lists_one_entry_per_volume() {
    // covers 20.3-a
    let f = fixture();
    let scan = f.ok("study scan", json!({ "paths": [f.src] }));
    let entries = scan["data"]["series"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{scan:#}");
    let labels: Vec<_> = entries.iter().map(|e| e["volume"]["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["b = 0", "b = 1000"]);
    assert_eq!(entries[1]["volume"]["number"], 2);
    assert_eq!(entries[1]["volume"]["count"], 2);
    assert_eq!(entries[1]["volume"]["b_value"], 1000.0);
    assert_eq!(entries[1]["volume"]["complete"], true);
    assert!(entries[1]["volume"].get("echo_time_ms").is_none(), "unknown keys are left out");
    assert_eq!(entries[0]["group"], entries[1]["group"]);
    assert_ne!(entries[0]["series"], entries[1]["series"]);
    for e in entries {
        assert!(e["series"].as_str().unwrap().starts_with("anon-"), "volume ids are pseudonymised");
        assert!(e["group"].as_str().unwrap().starts_with("anon-"), "group ids are pseudonymised");
        assert_eq!(e["dims"], json!([16, 16, 8]));
    }

    // a single-volume series is listed as before
    let flat = f.root.join("data/t1.nii.gz");
    ferrum_io::write_nifti(&phantom(100.0), &flat).unwrap();
    let scan = f.ok("study scan", json!({ "paths": [flat] }));
    let entry = &scan["data"]["series"][0];
    assert!(entry.get("volume").is_none() && entry.get("group").is_none(), "{entry:#}");
}

#[test]
fn volumes_open_and_switch_in_one_workspace() {
    // covers 20.3-b, 20.3-c, 20.3-d
    let f = fixture();
    let scan = f.ok("study scan", json!({ "paths": [f.src] }));
    let ids: Vec<String> =
        scan["data"]["series"].as_array().unwrap().iter().map(|e| e["series"].as_str().unwrap().to_owned()).collect();
    let group = scan["data"]["series"][0]["group"].as_str().unwrap().to_owned();

    // no series: volume 1, with a warning naming the count and how to choose
    let env = f.open(None);
    assert!(warned(&env, "the series holds 2 volumes and volume 1 is open"), "{env:#}");
    assert!(warned(&env, "volume 1 of 2 is shown (b = 0)"), "{env:#}");
    assert_eq!(env["data"]["volume"]["label"], "b = 0");
    assert_eq!(env["data"]["volume"]["series"], ids[0].as_str());
    assert_eq!(env["data"]["series"], group.as_str(), "the study is the series");
    let probe = f.probe();
    assert_eq!(probe["data"]["value"], 100.0);
    assert_eq!(probe["provenance"]["volume"], json!({ "series": ids[0], "label": "b = 0" }));

    // study info lists the volumes; the workspace keeps a segment
    f.ok("segment threshold", json!({ "seed": { "voxel": [8, 8, 3] }, "min": 50, "max": 150 }));
    let info = f.ok("study info", json!({}));
    assert_eq!(info["data"]["volumes"].as_array().unwrap().len(), 2);
    assert_eq!(
        info["data"]["volumes"][1],
        json!({ "series": ids[1], "number": 2, "label": "b = 1000", "complete": true })
    );
    assert!(warned(&info, "volume 1 of 2 is shown"), "study info repeats which volume is shown");
    assert!(!warned(&info, "no volume was named"), "the default note belongs to study open");

    // the other volume switches the workspace and keeps the segment
    let env = f.open(Some(&ids[1]));
    assert_eq!(env["data"]["volume"]["label"], "b = 1000");
    assert!(!warned(&env, "no volume was named"), "{env:#}");
    let probe = f.probe();
    assert_eq!(probe["data"]["value"], 500.0, "values come from the open volume");
    assert_eq!(probe["provenance"]["volume"]["label"], "b = 1000");
    let info = f.ok("study info", json!({}));
    assert_eq!(info["data"]["segments"]["count"], 1);
    let stats = f.ok("stats", json!({ "sphere": { "center": { "voxel": [8, 8, 3] }, "radius_mm": 2 } }));
    assert_eq!(stats["provenance"]["volume"]["label"], "b = 1000");
    let render = f.ok("view slice", json!({ "plane": "axial", "slice_number": 4, "size": 32 }));
    assert_eq!(render["provenance"]["volume"]["label"], "b = 1000");

    // reopening without series keeps the workspace's volume
    let env = f.open(None);
    assert_eq!(env["data"]["volume"]["label"], "b = 1000");
    assert!(warned(&env, "volume 2 is open"), "{env:#}");
    // naming the group keeps it too
    let env = f.open(Some(&group));
    assert_eq!(env["data"]["volume"]["label"], "b = 1000", "a named group keeps the open volume");

    // the manifest is version 2 and names the volume; the audit names it too
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(Path::new(&f.ws).join("workspace.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], 2);
    assert!(manifest["source"]["volume"].as_str().unwrap().contains("#v="));
    let audit = std::fs::read_to_string(Path::new(&f.ws).join("audit.jsonl")).unwrap();
    assert!(audit.lines().any(|l| l.contains("\"b = 1000\"")), "the audit names the volume");

    // another series is refused
    let other = f.root.join("data/other.nii.gz");
    ferrum_io::write_nifti(&phantom(100.0), &other).unwrap();
    let env = f.run("study open", json!({ "path": other }));
    assert_eq!(env["error"]["code"], "bad_request", "{env:#}");
    let env = f.run("study open", json!({ "path": f.src, "series": "anon-0000" }));
    assert_eq!(env["error"]["code"], "not_found");
}

#[test]
fn a_volume_opens_in_a_new_workspace_by_its_id() {
    // covers 20.3-b
    let f = fixture();
    let scan = f.ok("study scan", json!({ "paths": [f.src] }));
    let id = scan["data"]["series"][1]["series"].as_str().unwrap().to_owned();
    let env = f.open(Some(&id));
    assert!(!warned(&env, "no volume was named"), "{env:#}");
    assert_eq!(env["data"]["volume"]["number"], 2);
    assert_eq!(f.probe()["data"]["value"], 500.0);
}

#[test]
fn single_volume_studies_are_unchanged() {
    // covers 20.3-a, 20.3-f
    let f = fixture();
    let flat = f.root.join("data/t1.nii.gz");
    ferrum_io::write_nifti(&phantom(100.0), &flat).unwrap();
    let env = f.ok("study open", json!({ "path": flat }));
    assert!(env["data"].get("volume").is_none() && env["data"].get("volumes").is_none(), "{env:#}");
    assert!(f.probe()["provenance"].get("volume").is_none());
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(Path::new(&f.ws).join("workspace.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], 1);
    assert!(manifest["source"].get("volume").is_none());
}

#[test]
fn a_version_1_workspace_opens_as_volume_1() {
    // covers 20.3-f
    let f = fixture();
    f.open(None);
    f.ok("segment threshold", json!({ "seed": { "voxel": [8, 8, 3] }, "min": 50, "max": 150 }));
    // as an earlier FERRUM wrote it: no volume, version 1
    let path = Path::new(&f.ws).join("workspace.json");
    let mut manifest: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    manifest["version"] = json!(1);
    manifest["source"].as_object_mut().unwrap().remove("volume");
    std::fs::write(&path, manifest.to_string()).unwrap();
    assert_eq!(f.probe()["data"]["value"], 100.0, "the first image at every position");
    let env = f.open(None);
    assert_eq!(env["data"]["volume"]["number"], 1);
    let info = f.ok("study info", json!({}));
    assert_eq!(info["data"]["segments"]["count"], 1, "segments stay");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(manifest["version"], 2, "now names its volume");
}
