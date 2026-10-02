//! Contract tests: every command on a synthetic phantom with known answers.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{Dims3, Geometry, Volume};
use glam::{Mat3, Vec3};
use serde_json::{json, Value};

/// 40×40×30 voxels of 1×1×2 mm: air (−1000) with a sphere of 100 (radius
/// 8 mm, centre voxel (14, 20, 15)) and a cube of 1000 (voxels 28..34 in
/// i and j, 12..18 in k).
fn phantom() -> Volume {
    let dims = Dims3::new(40, 40, 30);
    let mut values = vec![-1000.0f32; dims.voxel_count()];
    for k in 0..30u32 {
        for j in 0..40u32 {
            for i in 0..40u32 {
                let d = Vec3::new(i as f32 - 14.0, j as f32 - 20.0, (k as f32 - 15.0) * 2.0);
                let v = &mut values[dims.index(i, j, k)];
                if d.length() <= 8.0 {
                    *v = 100.0;
                }
                if (28..34).contains(&i) && (28..34).contains(&j) && (12..18).contains(&k) {
                    *v = 1000.0;
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

fn fixture(config: AgentConfig) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    ferrum_io::write_nifti(&phantom(), &data.join("phantom.nii.gz")).unwrap();
    let ws = dir.path().join("ws/ct1").to_string_lossy().into_owned();
    Fixture { _dir: dir, data, ws, agent: Agent::new(config) }
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
        assert_eq!(env["api"], "ferrum-agent/1");
        env["data"].clone()
    }

    fn err(&self, command: &str, params: Value) -> String {
        let env = self.run(command, params);
        assert_eq!(env["ok"], false, "{command} should fail: {env:#}");
        env["error"]["code"].as_str().unwrap().to_owned()
    }

    fn open(&self) -> Value {
        self.ok("study open", json!({ "path": self.data.join("phantom.nii.gz") }))
    }
}

fn close(v: &Value, expected: f64, tol: f64) {
    let x = v.as_f64().unwrap_or(f64::NAN);
    assert!((x - expected).abs() <= tol, "{x} != {expected} ± {tol}");
}

#[test]
fn study_scan_open_info() {
    let f = fixture(AgentConfig::default());
    let scan = f.ok("study scan", json!({ "paths": [f.data] }));
    assert_eq!(scan["series"][0]["dims"], json!([40, 40, 30]));
    assert!(scan["series"][0]["series"].as_str().unwrap().starts_with("anon-"), "series ids are pseudonymised");
    assert_eq!(f.err("study info", json!({})), "no_study");
    let info = f.open();
    assert_eq!(info["dims"], json!([40, 40, 30]));
    assert_eq!(info["spacing_mm"], json!([1.0, 1.0, 2.0]));
    assert_eq!(info["origin_mm"], json!([-20.0, -20.0, 100.0]));
    assert_eq!(info["slice_counts"]["axial"], 30);
    assert!(info["window_presets"].as_array().unwrap().iter().any(|w| w["name"] == "lung"));
    let env = f.run("study info", json!({}));
    assert!(env["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains("no operator configuration")));
    assert_eq!(env["provenance"]["source_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(env["data"]["annotations"]["count"], 0);
    // reopening the same series reuses the workspace; another source does not fit
    f.open();
    std::fs::copy(f.data.join("phantom.nii.gz"), f.data.join("other.nii.gz")).unwrap();
    assert_eq!(f.err("study open", json!({ "path": f.data.join("other.nii.gz") })), "bad_request");
    assert_eq!(f.err("study open", json!({ "path": f.data.join("missing.nii") })), "not_found");
    assert_eq!(f.err("study scan", json!({ "paths": [f.data.join("missing")] })), "not_found");
    assert_eq!(f.err("study open", json!({ "path": f.data, "series": "anon-0000" })), "not_found");
}

#[test]
fn probe_stats_and_measurements_have_known_answers() {
    let f = fixture(AgentConfig::default());
    f.open();
    let p = f.ok("probe", json!({ "point": { "voxel": [14, 20, 15] } }));
    close(&p["value"], 100.0, 0.1);
    assert_eq!(p["patient_mm"], json!([-6.0, 0.0, 130.0]));
    let p = f.ok("probe", json!({ "point": { "patient_mm": [10.0, 10.0, 130.0] } }));
    close(&p["value"], 1000.0, 0.1);
    assert_eq!(p["voxel_index"], json!([30, 30, 15]));
    assert_eq!(f.err("probe", json!({ "point": { "voxel": [40, 0, 0] } })), "out_of_volume");
    assert_eq!(f.err("probe", json!({ "point": [1, 2, 3] })), "bad_request");

    let cube = f.ok("stats", json!({ "box": { "min": { "voxel": [28, 28, 12] }, "max": { "voxel": [33, 33, 17] } } }));
    assert_eq!(cube["voxels"], 216);
    close(&cube["mean"], 1000.0, 0.1);
    close(&cube["volume_ml"], 0.432, 1e-6);
    let sphere = f.ok("stats", json!({ "sphere": { "center": { "voxel": [14, 20, 15] }, "radius_mm": 6.0 } }));
    close(&sphere["min"], 100.0, 0.1);
    close(&sphere["max"], 100.0, 0.1);
    assert_eq!(f.err("stats", json!({})), "bad_request");
    assert_eq!(f.err("stats", json!({ "segment": 3 })), "not_found");
    assert_eq!(
        f.err("stats", json!({ "sphere": { "center": { "voxel": [1, 1, 1] }, "radius_mm": -1 } })),
        "bad_request"
    );

    let d = f.ok("measure distance", json!({ "points": [{ "voxel": [0, 0, 0] }, { "voxel": [3, 4, 0] }] }));
    close(&d["value"], 5.0, 1e-9);
    assert_eq!((d["unit"].as_str(), d["uncertainty_mm"].as_f64()), (Some("mm"), Some(2.0)));
    let d = f.ok("measure distance", json!({ "points": [{ "voxel": [0, 0, 0] }, { "voxel": [0, 0, 2] }] }));
    close(&d["value"], 4.0, 1e-9);
    let a = f.ok(
        "measure angle",
        json!({ "points": [{ "voxel": [1, 0, 0] }, { "voxel": [0, 0, 0] }, { "voxel": [0, 1, 0] }] }),
    );
    close(&a["value"], 90.0, 1e-9);
    let area = f.ok("measure area", json!({ "points": [{ "voxel": [0, 0, 0] }, { "voxel": [4, 0, 0] }, { "voxel": [4, 0, 3] }, { "voxel": [0, 0, 3] }] }));
    close(&area["value"], 24.0, 1e-9);
    assert_eq!(area["unit"], "mm2");
    assert_eq!(f.err("measure distance", json!({ "points": [{ "voxel": [0, 0, 0] }] })), "bad_request");
    assert_eq!(
        f.err(
            "measure angle",
            json!({ "points": [{ "voxel": [0, 0, 0] }, { "voxel": [0, 0, 0] }, { "voxel": [1, 0, 0] }] })
        ),
        "bad_request"
    );
    assert_eq!(
        f.err("measure area", json!({ "points": [{ "voxel": [0, 0, 0] }, { "voxel": [1, 0, 0] }] })),
        "bad_request"
    );
}

#[test]
fn slice_renders_map_pixels_back_to_voxels() {
    let f = fixture(AgentConfig::default());
    f.open();
    let r = f.ok("view slice", json!({ "plane": "axial", "slice_number": 16, "window": "bone", "size": 80 }));
    assert_eq!((r["render"].as_str(), r["size"].clone()), (Some("r-0001"), json!([80, 80])));
    assert_eq!((r["pixel_mm"].as_f64(), r["slice_index"].as_u64()), (Some(0.5), Some(15)));
    assert_eq!(r["orientation"], json!({ "left": "R", "right": "L", "top": "A", "bottom": "P" }));
    let png = image::open(r["image"].as_str().unwrap()).unwrap().to_rgb8();
    assert_eq!(png.dimensions(), (80, 80));
    // pixel (61, 61) shows voxel (30, 30), the cube: 1000 HU in the bone window (400 ± 900) is grey 213
    assert_eq!(png.get_pixel(61, 61)[0], 213);
    assert_eq!(png.get_pixel(2, 2)[0], 0);
    // the agent points at the pixel and gets the voxel back
    let p = f.ok("probe", json!({ "point": { "render": "r-0001", "pixel": [61, 61] } }));
    assert_eq!(p["voxel_index"], json!([30, 30, 15]));
    close(&p["value"], 1000.0, 0.1);
    let s = f.ok(
        "view slice",
        json!({ "plane": "coronal", "at": { "voxel": [14, 20, 15] }, "window": { "center": 0, "width": 2000 } }),
    );
    assert_eq!((s["render"].as_str(), s["slice_number"].as_u64()), (Some("r-0002"), Some(21)));
    assert_eq!(s["size"], json!([512, 768]), "40 mm wide, 60 mm tall at the default size");
    // coronal: top of the image is superior; pixel (0, 0) is the last k
    let p = f.ok("probe", json!({ "point": { "render": "r-0002", "pixel": [0.5, 0.5] } }));
    assert_eq!(p["voxel_index"], json!([0, 20, 29]));
    assert!(Path::new(s["sidecar"].as_str().unwrap()).exists());
    assert_eq!(f.err("view slice", json!({ "plane": "axial", "slice_number": 31 })), "out_of_volume");
    assert_eq!(f.err("view slice", json!({ "plane": "axial", "slice_number": 0 })), "bad_request");
    assert_eq!(f.err("view slice", json!({ "plane": "axial" })), "bad_request");
    assert_eq!(f.err("view slice", json!({ "plane": "axial", "slice_number": 1, "window": "neon" })), "bad_request");
    assert_eq!(
        f.err("view slice", json!({ "plane": "axial", "slice_number": 1, "overlays": ["labels"] })),
        "bad_request"
    );
    assert_eq!(f.err("probe", json!({ "point": { "render": "r-0009", "pixel": [1, 1] } })), "not_found");
}

#[test]
fn annotations_are_agent_proposals() {
    let f = fixture(AgentConfig::default());
    f.open();
    let a = f.ok(
        "annotate add",
        json!({ "kind": "distance", "plane": "axial", "name": "Sphere diameter", "agent": "run-1",
                "points": [{ "voxel": [6, 20, 15] }, { "voxel": [22, 20, 15] }] }),
    )["annotation"]
        .clone();
    assert_eq!((a["type"].as_str(), a["slice_number"].as_u64()), (Some("Distance"), Some(16)));
    close(&a["value"], 16.0, 1e-3);
    assert_eq!(a["provenance"]["author"], json!({ "kind": "agent", "id": "run-1" }));
    assert_eq!(a["provenance"]["status"], "proposed");
    let id = a["id"].as_u64().unwrap();
    let rect = f.ok(
        "annotate add",
        json!({ "kind": "rectangle", "plane": "axial", "points": [{ "voxel": [28, 28, 15] }, { "voxel": [33, 33, 15] }] }),
    );
    let rid = rect["annotation"]["id"].as_u64().unwrap();
    // the rectangle's corners are voxel centres: 5 × 5 voxel centres lie strictly inside
    let roi = f.ok("stats", json!({ "annotation": rid }));
    close(&roi["mean"], 1000.0, 0.1);
    assert_eq!(f.err("stats", json!({ "annotation": id })), "bad_request");
    f.ok(
        "annotate add",
        json!({ "kind": "text", "plane": "coronal", "text": "see cube", "points": [{ "voxel": [30, 30, 15] }] }),
    );
    assert_eq!(
        f.err(
            "annotate add",
            json!({ "kind": "distance", "plane": "axial", "points": [{ "voxel": [0, 0, 1] }, { "voxel": [0, 0, 2] }] })
        ),
        "bad_request"
    );
    assert_eq!(
        f.err("annotate add", json!({ "kind": "ellipse", "plane": "axial", "points": [{ "voxel": [0, 0, 1] }] })),
        "bad_request"
    );
    let list = f.ok("annotate list", json!({}));
    assert_eq!(list["annotations"].as_array().unwrap().len(), 3);
    f.ok("annotate rename", json!({ "id": id, "name": "Sphere" }));
    assert_eq!(f.ok("annotate list", json!({}))["annotations"][0]["name"], "Sphere");
    f.ok("annotate delete", json!({ "id": rid }));
    assert_eq!(f.err("annotate delete", json!({ "id": rid })), "not_found");
    assert_eq!(f.ok("study info", json!({}))["annotations"], json!({ "count": 2, "pending_review": 2 }));
    // the workspace file holds what the desktop app reads
    let saved = ferrum_io::read_annotations(&Path::new(&f.ws).join("annotations.json"), Some([40, 40, 30])).unwrap();
    assert_eq!(saved.pending(), 2);
}

#[test]
fn threshold_segments_and_review() {
    let f = fixture(AgentConfig::default());
    f.open();
    let s = f
        .ok("segment threshold", json!({ "seed": { "voxel": [14, 20, 15] }, "min": 50, "max": 150, "name": "Sphere" }));
    let seg = &s["segment"];
    assert_eq!((seg["label"].as_u64(), seg["name"].as_str()), (Some(1), Some("Sphere")));
    // a sphere of radius 8 mm is about 2.14 ml; on this grid the voxel count gives the exact volume
    let ml = seg["volume_ml"].as_f64().unwrap();
    assert!((2.0..2.3).contains(&ml), "{ml}");
    assert_eq!(seg["provenance"]["status"], "proposed");
    let st = f.ok("stats", json!({ "segment": 1 }));
    close(&st["mean"], 100.0, 0.1);
    close(&st["volume_ml"], ml, 1e-6);
    assert_eq!(
        f.err("segment threshold", json!({ "seed": { "voxel": [14, 20, 15] }, "min": 50, "max": 150 })),
        "bad_request"
    );
    assert_eq!(
        f.err("segment threshold", json!({ "seed": { "voxel": [0, 0, 0] }, "min": 50, "max": 150 })),
        "bad_request"
    );
    assert_eq!(
        f.err("segment threshold", json!({ "seed": { "voxel": [0, 0, 0] }, "min": -1100, "max": -900, "max_ml": 1 })),
        "limit"
    );
    assert_eq!(
        f.err("segment threshold", json!({ "seed": { "voxel": [0, 0, 0] }, "min": 5, "max": -5 })),
        "bad_request"
    );
    let r = f.ok("view slice", json!({ "plane": "axial", "slice_number": 16, "size": 40, "overlays": ["segments"] }));
    let png = image::open(r["image"].as_str().unwrap()).unwrap().to_rgb8();
    let red = png.pixels().filter(|p| p.0 == [230, 85, 75]).count();
    assert!(red > 10, "the segment outline is drawn ({red} pixels)");
    f.ok("segment rename", json!({ "label": 1, "name": "Ball" }));
    assert_eq!(f.ok("segment list", json!({}))["segments"][0]["name"], "Ball");
    let pending = f.ok("review list", json!({}));
    assert_eq!(pending["segments"].as_array().unwrap().len(), 1);
    assert_eq!(f.err("review confirm", json!({ "segment": 1, "by": "dr.k" })), "forbidden");
    f.ok("segment delete", json!({ "label": 1 }));
    assert_eq!(f.err("segment delete", json!({ "label": 1 })), "not_found");
    // the audit log has one line per call on the workspace
    let log = std::fs::read_to_string(Path::new(&f.ws).join("audit.jsonl")).unwrap();
    assert!(log.lines().count() >= 10);
    assert!(log.lines().any(|l| l.contains("\"segment threshold\"") && l.contains("\"ok\":true")));
}

#[test]
fn harness_review_needs_operator_permission() {
    let config = AgentConfig { allow_harness_confirmation: true, ..AgentConfig::default() };
    let f = fixture(config);
    f.open();
    f.ok("segment threshold", json!({ "seed": { "voxel": [30, 30, 15] }, "min": 900, "max": 1100 }));
    let a = f.ok(
        "annotate add",
        json!({ "kind": "text", "plane": "axial", "text": "x", "points": [{ "voxel": [1, 1, 1] }] }),
    );
    let id = a["annotation"]["id"].as_u64().unwrap();
    assert_eq!(f.err("review confirm", json!({ "segment": 1 })), "bad_request");
    assert_eq!(f.err("review confirm", json!({ "segment": 1, "by": " " })), "bad_request");
    assert_eq!(f.err("review confirm", json!({ "by": "dr.k" })), "bad_request");
    f.ok("review confirm", json!({ "segment": 1, "by": "dr.k" }));
    f.ok("review reject", json!({ "annotation": id, "by": "dr.k" }));
    assert_eq!(f.err("review confirm", json!({ "segment": 9, "by": "dr.k" })), "not_found");
    assert_eq!(f.err("review confirm", json!({ "annotation": 99, "by": "dr.k" })), "not_found");
    let list = f.ok("review list", json!({}));
    assert_eq!(list, json!({ "annotations": [], "segments": [] }));
    let seg = &f.ok("segment list", json!({}))["segments"][0]["provenance"];
    assert_eq!((seg["status"].as_str(), seg["reviewed_by"].as_str()), (Some("confirmed"), Some("dr.k")));
}

#[test]
fn people_and_engines_keep_their_items() {
    let f = fixture(AgentConfig::default());
    f.open();
    // a person drew an annotation in the viewer (default provenance)
    let ws = ferrum_io::Workspace::open(Path::new(&f.ws)).unwrap();
    let mut set = ferrum_domain::AnnotationSet::default();
    set.add(
        ferrum_domain::SliceKey::new(ferrum_domain::SliceAxis::Axial, 2),
        ferrum_domain::Annotation::Text { pos: glam::Vec2::ONE, text: "radiologist".into() },
    );
    let report = ferrum_domain::AnnotationReport::build(PathBuf::new(), Default::default(), &phantom(), &set);
    ws.save_annotations(&report, "test").unwrap();
    assert_eq!(f.err("annotate delete", json!({ "id": 0 })), "forbidden");
    assert_eq!(f.err("annotate rename", json!({ "id": 0, "name": "x" })), "forbidden");
}

#[test]
fn operator_rules_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        read_roots: vec![dir.path().join("allowed")],
        workspace_root: Some(dir.path().join("ws")),
        ..AgentConfig::default()
    };
    let f = fixture(config);
    assert_eq!(f.err("study scan", json!({ "paths": [f.data] })), "forbidden");
    assert_eq!(f.err("study open", json!({ "workspace": "ct1", "path": f.data.join("phantom.nii.gz") })), "forbidden");
    assert_eq!(f.err("study info", json!({ "workspace": "../escape" })), "forbidden");
    assert_eq!(f.err("study info", json!({ "workspace": "ct1" })), "no_study");
    let limited = fixture(AgentConfig { max_voxels: 1000, ..AgentConfig::default() });
    assert_eq!(limited.err("study open", json!({ "path": limited.data.join("phantom.nii.gz") })), "limit");
    let small = fixture(AgentConfig { max_render_px: 64, ..AgentConfig::default() });
    small.open();
    let r = small.ok("view slice", json!({ "plane": "axial", "slice_number": 1, "size": 4000 }));
    assert_eq!(r["size"], json!([64, 64]));
}

#[test]
fn changed_sources_and_bad_calls_are_reported() {
    let f = fixture(AgentConfig::default());
    f.open();
    assert_eq!(f.err("no such command", json!({})), "bad_request");
    assert_eq!(f.agent.run("probe", &json!([1]))["error"]["code"], "bad_request");
    assert_eq!(f.err("study info", json!({ "workspace": 5 })), "bad_request");
    // overwrite the source: results no longer belong to it
    let mut v = phantom();
    v = Volume::from_physical(v.dims(), v.spacing(), &vec![0.0; v.dims().voxel_count()]).unwrap();
    ferrum_io::write_nifti(&v, &f.data.join("phantom.nii.gz")).unwrap();
    let env = f.run("study info", json!({}));
    assert_eq!(env["error"]["code"], "source_changed");
    assert!(env["error"]["hint"].as_str().is_some());
    assert_eq!(Agent::commands().count(), 28);
}

#[test]
fn montage_and_mpr_tiles_map_back_to_voxels() {
    let f = fixture(AgentConfig::default());
    f.open();
    let m = f.ok(
        "view montage",
        json!({ "plane": "axial", "from": 14, "to": 18, "step": 2, "window": "bone", "size": 300 }),
    );
    assert_eq!(m["slice_numbers"], json!([14, 16, 18]));
    let tiles = m["tiles"].as_array().unwrap();
    assert_eq!(tiles.len(), 3);
    assert_eq!(tiles[1]["slice_number"], 16);
    // a pixel in the second tile resolves through that tile's map
    let (ox, oy) = (tiles[1]["origin"][0].as_f64().unwrap(), tiles[1]["origin"][1].as_f64().unwrap());
    let px = tiles[1]["pixel_mm"].as_f64().unwrap();
    let pixel = [ox + (30.5 / px), oy + (30.5 / px)];
    let p = f.ok("probe", json!({ "point": { "render": m["render"], "pixel": pixel } }));
    assert_eq!(p["voxel_index"], json!([30, 30, 15]));
    close(&p["value"], 1000.0, 0.1);
    // three tiles on a 2 × 2 grid: the bottom-right cell is empty
    let (w, h) = (m["size"][0].as_f64().unwrap(), m["size"][1].as_f64().unwrap());
    assert_eq!(
        f.err("probe", json!({ "point": { "render": m["render"], "pixel": [w - 1.0, h - 1.0] } })),
        "out_of_volume"
    );
    assert_eq!(f.err("view montage", json!({ "plane": "axial", "from": 20, "to": 10 })), "out_of_volume");
    assert_eq!(f.err("view montage", json!({ "plane": "axial", "to": 31 })), "out_of_volume");
    let all = f.ok("view montage", json!({ "plane": "coronal", "size": 200 }));
    assert_eq!(all["slice_numbers"].as_array().unwrap().len(), 14, "40 slices at the default step of 3");

    let mpr = f.ok("view mpr", json!({ "at": { "voxel": [14, 20, 15] }, "size": 300, "overlays": ["segments"] }));
    let tiles = mpr["tiles"].as_array().unwrap();
    let planes: Vec<_> =
        tiles.iter().map(|t| (t["plane"].as_str().unwrap().to_owned(), t["slice_number"].as_u64().unwrap())).collect();
    assert_eq!(planes, [("axial".to_owned(), 16), ("coronal".to_owned(), 21), ("sagittal".to_owned(), 15)]);
    assert_eq!(mpr["point"]["voxel_index"], json!([14, 20, 15]));
    let png = image::open(mpr["image"].as_str().unwrap()).unwrap().to_rgb8();
    assert!(png.pixels().any(|p| p.0 == [255, 214, 10]), "crosshair and labels are drawn");
    // the centre of the coronal tile's crosshair is the point again
    let t = &tiles[1];
    let m = &t["pixel_to_voxel"];
    let x = (14.0 - m[0][2].as_f64().unwrap()) / m[0][0].as_f64().unwrap();
    let y = (15.0 - m[2][2].as_f64().unwrap()) / m[2][1].as_f64().unwrap();
    let pixel = [t["origin"][0].as_f64().unwrap() + x, y];
    let p = f.ok("probe", json!({ "point": { "render": mpr["render"], "pixel": pixel } }));
    assert_eq!(p["voxel_index"], json!([14, 20, 15]));
}

#[test]
fn profile_crosses_the_sphere() {
    let f = fixture(AgentConfig::default());
    f.open();
    let pr = f.ok("profile", json!({ "from": { "voxel": [0, 20, 15] }, "to": { "voxel": [39, 20, 15] } }));
    let samples = pr["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 40, "one sample per millimetre");
    close(&pr["length_mm"], 39.0, 1e-9);
    close(&samples[14]["value"], 100.0, 0.1);
    close(&samples[14]["distance_mm"], 14.0, 1e-9);
    close(&pr["min"], -1000.0, 0.1);
    close(&pr["max"], 100.0, 0.1);
    // the sphere spans 17 samples (radius 8 mm around i = 14)
    let inside = samples.iter().filter(|s| s["value"].as_f64().unwrap() > 0.0).count();
    assert_eq!(inside, 17);
    let few = f.ok("profile", json!({ "from": { "voxel": [0, 0, 0] }, "to": { "voxel": [0, 0, 29] }, "samples": 3 }));
    assert_eq!(few["samples"][1]["voxel"], json!([0.0, 0.0, 14.5]));
    assert_eq!(
        f.err("profile", json!({ "from": { "voxel": [0, 0, 0] }, "to": { "voxel": [1, 0, 0] }, "samples": 1 })),
        "bad_request"
    );
}

#[test]
fn export_bundle_marks_unconfirmed_items() {
    let f = fixture(AgentConfig::default());
    f.open();
    let empty = f.ok("export bundle", json!({}));
    assert_eq!(empty["files"].as_array().unwrap().len(), 1, "only the report");
    f.ok("segment threshold", json!({ "seed": { "voxel": [14, 20, 15] }, "min": 50, "max": 150, "name": "Sphere" }));
    f.ok("annotate add", json!({ "kind": "distance", "plane": "axial", "points": [{ "voxel": [6, 20, 15] }, { "voxel": [22, 20, 15] }] }));
    let env = f.run("export bundle", json!({}));
    assert!(env["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains("2 item(s) are unconfirmed")));
    let b = &env["data"];
    assert_eq!(b["unconfirmed_items"], 2);
    let names: Vec<String> = b["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| Path::new(f["path"].as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["report.json", "annotations.json", "segments.nii.gz", "segments.json"]);
    for file in b["files"].as_array().unwrap() {
        let (sha, bytes) = ferrum_io::sha256_file(Path::new(file["path"].as_str().unwrap())).unwrap();
        assert_eq!((file["sha256"].as_str().unwrap(), file["bytes"].as_u64().unwrap()), (sha.as_str(), bytes));
    }
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(Path::new(&f.ws).join("export/report.json")).unwrap()).unwrap();
    assert_eq!(report["format"], "ferrum-report");
    assert!(report["disclaimer"].as_str().unwrap().contains("not a medical device"));
    assert_eq!(report["segments"][0]["confirmed"], false);
    assert_eq!(report["measurements"][0]["confirmed"], false);
    close(&report["measurements"][0]["value"], 16.0, 1e-3);
    assert_eq!(report["study"]["dims"], json!([40, 40, 30]));
    // the exported label map lies on the source grid
    let labels = ferrum_io::read_label_nifti(&Path::new(&f.ws).join("export/segments.nii.gz"), &phantom()).unwrap();
    assert!(labels.data().contains(&1));
}

#[test]
fn volume_renders_from_standard_views() {
    let f = fixture(AgentConfig::default());
    f.open();
    let iso = f.ok("view volume", json!({ "mode": "isosurface", "threshold": 500, "view": "anterior", "size": 96 }));
    assert_eq!(
        (iso["kind"].as_str(), iso["view"].as_str(), iso["size"].clone()),
        (Some("volume"), Some("anterior"), json!([96, 96]))
    );
    let png = image::open(iso["image"].as_str().unwrap()).unwrap().to_rgb8();
    let lit = png.pixels().filter(|p| p.0.iter().any(|c| *c > 40)).count();
    // the 6 mm cube spans about 5 % of the 96 px frame
    assert!(lit > 15, "the cube's surface is visible ({lit} pixels)");
    // the sphere (100) lies below the threshold 500, the cube (1000) above: from the left the image differs
    let left = f.ok("view volume", json!({ "mode": "isosurface", "threshold": 500, "view": "left", "size": 96 }));
    assert_ne!(
        std::fs::read(iso["image"].as_str().unwrap()).unwrap(),
        std::fs::read(left["image"].as_str().unwrap()).unwrap()
    );
    let mip = f.ok("view volume", json!({ "view": "superior", "size": 64 }));
    assert_eq!(mip["rendering"]["mode"], "mip");
    f.ok("segment threshold", json!({ "seed": { "voxel": [14, 20, 15] }, "min": 50, "max": 150 }));
    let env = f.run(
        "view volume",
        json!({ "mode": "transfer_function", "preset": "bone", "size": 64, "overlays": ["segments"] }),
    );
    assert_eq!(env["ok"], true);
    assert!(
        env["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("assume CT")),
        "NIfTI has no modality"
    );
    assert_eq!(f.err("view volume", json!({ "mode": "isosurface" })), "bad_request");
    assert_eq!(f.err("view volume", json!({ "mode": "xray" })), "bad_request");
    assert_eq!(f.err("view volume", json!({ "view": "oblique" })), "bad_request");
    // a 3D render has no pixel mapping: pointing into it is refused
    assert_eq!(f.err("probe", json!({ "point": { "render": iso["render"], "pixel": [48, 48] } })), "bad_request");
}
