//! The MCP transport: same JSON as the command line, renders as images,
//! series kept in memory but never stale.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use ferrum_agent::mcp::McpServer;
use ferrum_agent::Agent;
use ferrum_domain::{Dims3, Volume};
use glam::Vec3;
use serde_json::{json, Value};

fn phantom(path: &Path, value: f32) {
    let dims = Dims3::new(24, 24, 12);
    let values: Vec<f32> = (0..dims.voxel_count()).map(|i| if i % 24 >= 12 { value } else { -1000.0 }).collect();
    ferrum_io::write_nifti(&Volume::from_physical(dims, Vec3::new(1.0, 1.0, 2.0), &values).unwrap(), path).unwrap();
}

/// The session both transports run.
fn script(ws: &str, src: &Path) -> Vec<(&'static str, Value)> {
    vec![
        ("study open", json!({ "workspace": ws, "path": src })),
        ("study info", json!({ "workspace": ws })),
        ("view slice", json!({ "workspace": ws, "plane": "axial", "slice_number": 6, "window": "bone", "size": 48 })),
        ("probe", json!({ "workspace": ws, "point": { "render": "r-0001", "pixel": [40, 10] } })),
        (
            "stats",
            json!({ "workspace": ws, "box": { "min": { "voxel": [12, 0, 0] }, "max": { "voxel": [23, 3, 3] } } }),
        ),
        (
            "measure distance",
            json!({ "workspace": ws, "points": [{ "voxel": [0, 0, 0] }, { "patient_mm": [3, 4, 0] }] }),
        ),
        (
            "segment threshold",
            json!({ "workspace": ws, "seed": { "voxel": [20, 5, 5] }, "min": 0, "max": 900, "agent": "t" }),
        ),
        (
            "annotate add",
            json!({ "workspace": ws, "kind": "distance", "plane": "axial", "points": [{ "voxel": [1, 1, 2] }, { "voxel": [5, 1, 2] }] }),
        ),
        ("review list", json!({ "workspace": ws })),
        ("probe", json!({ "workspace": ws, "point": { "voxel": [99, 0, 0] } })),
        ("study info", json!({ "workspace": ws, "typo": 1 })),
    ]
}

/// Data without what legitimately differs: times, workspace paths.
fn comparable(envelope: &Value, ws_root: &str) -> Value {
    let mut v = envelope.clone();
    if let Some(p) = v.get_mut("provenance") {
        p["time"] = Value::Null;
        p["params"] = Value::Null;
    }
    let text = v.to_string().replace(ws_root, "<ws>");
    let mut v: Value = serde_json::from_str(&text).unwrap();
    walk(&mut v);
    v
}

/// Provenance `created` times differ between runs.
fn walk(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "created" || k == "reviewed" {
                    *x = Value::Null;
                } else {
                    walk(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(walk),
        _ => {}
    }
}

fn call(server: &mut McpServer, id: u64, command: &str, args: &Value) -> Value {
    let name = format!("ferrum_{}", command.replace(' ', "_"));
    let msg =
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } });
    server.handle(&msg).unwrap()["result"].clone()
}

#[test]
fn mcp_and_command_line_give_the_same_json() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("p.nii.gz");
    phantom(&src, 400.0);
    let (cli_ws, mcp_ws) = (dir.path().join("cli"), dir.path().join("mcp"));
    let agent = Agent::default();
    let mut server = McpServer::new(Agent::default());
    let cli_root = cli_ws.to_string_lossy().into_owned();
    let mcp_root = mcp_ws.to_string_lossy().into_owned();
    for (i, ((command, a), (_, b))) in script(&cli_root, &src).into_iter().zip(script(&mcp_root, &src)).enumerate() {
        let direct = agent.run(command, &a);
        let result = call(&mut server, i as u64, command, &b);
        let via_mcp = &result["structuredContent"];
        assert_eq!(comparable(&direct, &cli_root), comparable(via_mcp, &mcp_root), "{command}");
        assert_eq!(result["isError"], direct["ok"] != true);
        let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(&text, via_mcp, "text and structured content agree");
        if command == "view slice" {
            let image = &result["content"][1];
            assert_eq!(image["mimeType"], "image/png");
            assert!(image["data"].as_str().unwrap().starts_with("iVBORw0KGgo"), "a base64 PNG");
        }
    }
    assert_eq!(server.cached_series(), 1, "the series stays in memory");
}

#[test]
fn cached_series_are_never_stale() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("p.nii.gz");
    phantom(&src, 400.0);
    let ws = dir.path().join("ws").to_string_lossy().into_owned();
    let mut server = McpServer::new(Agent::default());
    assert_eq!(call(&mut server, 1, "study open", &json!({ "workspace": ws, "path": src }))["isError"], false);
    let probe = json!({ "workspace": ws, "point": { "voxel": [20, 0, 0] } });
    let v = call(&mut server, 2, "probe", &probe);
    assert!((v["structuredContent"]["data"]["value"].as_f64().unwrap() - 400.0).abs() < 0.1);

    // a person annotates in the desktop app meanwhile: the next call sees it
    let workspace = ferrum_io::Workspace::open(Path::new(&ws)).unwrap();
    let mut set = ferrum_domain::AnnotationSet::default();
    let key = ferrum_domain::SliceKey::new(ferrum_domain::SliceAxis::Axial, 1);
    set.add(key, ferrum_domain::Annotation::Text { pos: glam::Vec2::ONE, text: "radiologist".into() });
    let volume =
        Volume::from_physical(Dims3::new(24, 24, 12), Vec3::new(1.0, 1.0, 2.0), &vec![0.0; 24 * 24 * 12]).unwrap();
    workspace
        .save_annotations(
            &ferrum_domain::AnnotationReport::build(PathBuf::new(), Default::default(), &volume, &set),
            "t",
        )
        .unwrap();
    let list = call(&mut server, 3, "annotate list", &json!({ "workspace": ws }));
    assert_eq!(list["structuredContent"]["data"]["annotations"][0]["text"], "radiologist");

    // the source changes: the cached series is not used
    std::thread::sleep(std::time::Duration::from_millis(20));
    phantom(&src, 700.0);
    let v = call(&mut server, 4, "probe", &probe);
    assert_eq!(v["isError"], true);
    assert_eq!(v["structuredContent"]["error"]["code"], "source_changed");
}
