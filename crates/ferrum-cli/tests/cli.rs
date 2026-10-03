//! The binary: a scripted session, exit codes and the operator config.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

use ferrum_domain::{Dims3, Volume};
use glam::Vec3;
use serde_json::Value;

fn cli(args: &[&str], config: Option<&Path>) -> (i32, Value) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ferrum-cli"));
    c.args(args).env_remove("FERRUM_AGENT_CONFIG");
    if let Some(p) = config {
        c.env("FERRUM_AGENT_CONFIG", p);
    }
    let out = c.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    (out.status.code().unwrap(), serde_json::from_str(&stdout).unwrap_or(Value::Null))
}

fn phantom(path: &Path) {
    let dims = Dims3::new(20, 20, 10);
    let values: Vec<f32> = (0..dims.voxel_count()).map(|i| if i % 20 >= 10 { 500.0 } else { -500.0 }).collect();
    ferrum_io::write_nifti(&Volume::from_physical(dims, Vec3::ONE, &values).unwrap(), path).unwrap();
}

#[test]
fn scripted_session() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in/p.nii.gz");
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    phantom(&src);
    let config = dir.path().join("ferrum-agent.toml");
    std::fs::write(
        &config,
        format!("[data]\nread_roots = [{:?}]\nworkspace_root = {:?}\n", dir.path().join("in"), dir.path().join("ws")),
    )
    .unwrap();
    let c = Some(config.as_path());
    let (code, env) = cli(&["study", "open", "-w", "s1", src.to_str().unwrap()], c);
    assert_eq!((code, env["ok"].as_bool()), (0, Some(true)), "{env}");
    assert!(env["warnings"].as_array().unwrap().iter().all(|w| !w.as_str().unwrap().contains("operator")));
    let (code, env) =
        cli(&["view", "slice", "-w", "s1", "--plane", "axial", "--slice-number", "5", "--window", "0,1000"], c);
    assert_eq!(code, 0, "{env}");
    assert!(Path::new(env["data"]["image"].as_str().unwrap()).starts_with(dir.path().join("ws/s1/renders")));
    let (_, a) = cli(&["probe", "-w", "s1", "v:15,3,4"], c);
    let (_, b) = cli(&["run", "probe", "--params", r#"{"workspace": "s1", "point": {"voxel": [15, 3, 4]}}"#], c);
    assert_eq!(a["data"], b["data"], "the subcommand and the JSON call give the same data");
    assert!((a["data"]["value"].as_f64().unwrap() - 500.0).abs() < 0.1);
    let (code, env) = cli(&["measure", "distance", "-w", "s1", "v:0,0,0", "mm:3,4,0"], c);
    assert_eq!((code, env["data"]["value"].as_f64()), (0, Some(5.0)));
    // command errors exit 1 with an error envelope; usage errors exit 2
    let (code, env) = cli(&["probe", "-w", "s1", "v:99,0,0"], c);
    assert_eq!((code, env["error"]["code"].as_str()), (1, Some("out_of_volume")));
    let (code, env) = cli(&["study", "scan", dir.path().to_str().unwrap()], c);
    assert_eq!((code, env["error"]["code"].as_str()), (1, Some("forbidden")));
    assert_eq!(cli(&["probe", "-w", "s1", "15,3,4"], c).0, 2);
    assert_eq!(cli(&["frobnicate"], c).0, 2);
    let (code, env) = cli(&["commands"], None);
    assert_eq!((code, env["commands"].as_array().map(Vec::len)), (0, Some(33)));
    // a broken configuration is an error envelope, never a silent default
    std::fs::write(&config, "[data]\nread_root = []\n").unwrap();
    let (code, env) = cli(&["study", "info", "-w", "s1"], c);
    assert_eq!((code, env["error"]["code"].as_str()), (1, Some("bad_request")));
    let (code, env) = cli(&["--config", "/nonexistent.toml", "study", "info", "-w", "s1"], None);
    assert_eq!((code, env["ok"].as_bool()), (1, Some(false)));
}

#[test]
fn mcp_over_stdio_and_schemas() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("p.nii.gz");
    phantom(&src);
    let mut child = Command::new(env!("CARGO_BIN_EXE_ferrum-cli"))
        .args(["mcp", "--workspace-root", dir.path().join("ws").to_str().unwrap()])
        .env_remove("FERRUM_AGENT_CONFIG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    // a notification gets no reply, so it travels in front of the next request
    let mut ask = |msg: Value, notify_first: bool| -> Value {
        if notify_first {
            writeln!(stdin, "{}", serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .unwrap();
        }
        writeln!(stdin, "{msg}").unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let init = ask(
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } }),
        false,
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "ferrum");
    let tools = ask(serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }), true);
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 33);
    let open = ask(
        serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "ferrum_study_open", "arguments": { "workspace": "a", "path": src } } }),
        false,
    );
    assert_eq!(open["result"]["isError"], false, "{open}");
    assert!(dir.path().join("ws/a/workspace.json").exists(), "relative workspaces go below --workspace-root");
    drop(stdin);
    assert!(child.wait().unwrap().success());

    let out = Command::new(env!("CARGO_BIN_EXE_ferrum-cli")).args(["schema", "probe"]).output().unwrap();
    let schema: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(schema["parameters"]["required"], serde_json::json!(["workspace", "point"]));
    let all = Command::new(env!("CARGO_BIN_EXE_ferrum-cli")).arg("schema").output().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&all.stdout).unwrap().as_object().unwrap().len(), 33);
    assert_eq!(
        Command::new(env!("CARGO_BIN_EXE_ferrum-cli")).args(["schema", "nope"]).status().unwrap().code(),
        Some(2)
    );
}
