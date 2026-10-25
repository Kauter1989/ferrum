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
    assert_eq!((code, env["commands"].as_array().map(Vec::len)), (0, Some(36)));
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
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 36);
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
    assert_eq!(serde_json::from_slice::<Value>(&all.stdout).unwrap().as_object().unwrap().len(), 36);
    assert_eq!(
        Command::new(env!("CARGO_BIN_EXE_ferrum-cli")).args(["schema", "nope"]).status().unwrap().code(),
        Some(2)
    );
}

fn gpu_config(dir: &Path) -> std::path::PathBuf {
    let config = dir.join("ferrum-agent.toml");
    let text = format!(
        "[network]\ngpu_groups = {{ card = [\"http://127.0.0.1:1\"] }}\ngpu_capacity_mib = {{ card = 12000 }}\ngpu_lock_dir = {:?}\n",
        dir.join("gpu")
    );
    std::fs::write(&config, text).unwrap();
    config
}

fn records(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join("gpu/card"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .map(|e| std::fs::read_to_string(e.path()).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

// covers 23.5-d
#[cfg(unix)]
#[test]
fn gpu_hold_runs_command_and_releases() {
    let dir = tempfile::tempdir().unwrap();
    let config = gpu_config(dir.path());
    // the command sees its own lease record, then exits with its own code
    let out = Command::new(env!("CARGO_BIN_EXE_ferrum-cli"))
        .args(["gpu", "hold", "--group", "card", "--lease-mib", "6000", "--name", "sidecar", "--"])
        .args(["sh", "-c", "cat \"$0\"/card/*.json; exit 3"])
        .arg(dir.path().join("gpu"))
        .env("FERRUM_AGENT_CONFIG", &config)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "the command's exit code: {stderr}");
    let record: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!((record["holder"].as_str(), record["lease_mib"].as_u64()), (Some("sidecar"), Some(6000)), "{record}");
    assert_eq!(record["kind"], "resident");
    assert!(stderr.contains("holding 6000 MiB of GPU group card for sidecar"), "{stderr}");
    assert!(stderr.contains("released GPU group card"), "{stderr}");
    assert!(records(dir.path()).is_empty(), "the lease is released when the command exits");

    // a group the operator did not configure is refused before anything runs
    let (code, env) = cli(&["gpu", "hold", "--group", "other", "--", "sh", "-c", "exit 0"], Some(&config));
    assert_eq!((code, env["error"]["code"].as_str()), (1, Some("bad_request")), "{env}");
    // nothing to run is a usage error
    let (code, _) = cli(&["gpu", "hold", "--group", "card"], Some(&config));
    assert_eq!(code, 2);
}

// covers 23.5-d
#[test]
fn gpu_hold_not_an_mcp_tool() {
    let (code, env) = cli(&["commands"], None);
    assert_eq!(code, 0);
    let names: Vec<&str> = env["commands"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
    assert!(names.iter().all(|n| !n.starts_with("gpu")), "{names:?}");
    let schemas =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills/ferrum/schemas/commands.json"))
            .unwrap();
    let schemas: Value = serde_json::from_str(&schemas).unwrap();
    assert!(schemas.as_object().unwrap().keys().all(|k| !k.starts_with("gpu")), "not in commands.json");
    let mut mcp = Command::new(env!("CARGO_BIN_EXE_ferrum-cli"))
        .arg("mcp")
        .env_remove("FERRUM_AGENT_CONFIG")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let mut stdin = mcp.stdin.take().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"t","version":"0"}}}}}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    }
    let out = mcp.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let list: Value = text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).find(|v| v["id"] == 2).unwrap();
    let tools: Vec<&str> =
        list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(tools.len() > 20, "{tools:?}");
    assert!(tools.iter().all(|t| !t.contains("gpu")), "{tools:?}");
}

/// Waits up to 10 s for `child`, killing it on timeout: a regression fails
/// instead of hanging the suite.
#[cfg(unix)]
fn wait_or_kill(child: &mut std::process::Child) -> Option<i32> {
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        if start.elapsed().as_secs() >= 10 {
            let _ = child.kill();
            panic!("gpu hold did not exit");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// `gpu hold` running `script` in `sh`, once the script has touched
/// `$READY` (after setting its traps). Scripts loop at most about 20 s, so a
/// failing test leaves no process behind that holds the test's output open.
#[cfg(unix)]
fn held(dir: &Path, script: &str) -> std::process::Child {
    let config = gpu_config(dir);
    let ready = dir.join("ready");
    let hold = Command::new(env!("CARGO_BIN_EXE_ferrum-cli"))
        .args(["gpu", "hold", "--group", "card", "--lease-mib", "6000", "--", "sh", "-c", script])
        .env("FERRUM_AGENT_CONFIG", &config)
        .env("READY", &ready)
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while !ready.exists() {
        assert!(start.elapsed().as_secs() < 10, "the command never started");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    hold
}

/// Whether group `card` has a live holder, as another process sees it.
#[cfg(unix)]
fn group_held(dir: &Path) -> bool {
    use ferrum_engines::gpu::LockDir;
    !LockDir::at(dir.join("gpu")).unwrap().holders("card").unwrap().is_empty()
}

/// Ctrl-C at a terminal reaches `gpu hold` only: the command has its own process group.
#[cfg(unix)]
fn ctrl_c(hold: &std::process::Child) {
    assert!(Command::new("kill").args(["-INT", &hold.id().to_string()]).status().unwrap().success());
}

// covers 23.5-d
#[cfg(unix)]
#[test]
fn gpu_hold_forwards_ctrl_c_and_releases_after_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let done = dir.path().join("done");
    let script = format!(
        "trap 'sleep 0.3; touch {}; exit 7' INT; touch \"$READY\"; i=0; while [ $i -lt 400 ]; do i=$((i+1)); sleep 0.05; done",
        done.display()
    );
    let mut hold = held(dir.path(), &script);
    assert_eq!(records(dir.path()).len(), 1);
    ctrl_c(&hold);
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(!done.exists() && group_held(dir.path()), "still held while the command finishes");
    assert_eq!(wait_or_kill(&mut hold), Some(7), "the command got SIGINT and its exit code is returned");
    assert!(records(dir.path()).is_empty());
}

// covers 23.5-d
#[cfg(unix)]
#[test]
fn gpu_hold_interrupts_a_wrapper_and_its_server() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("server-got-sigint");
    // a wrapper that does not exec: only the group signal reaches the "server" below it
    let script = format!(
        "sh -c 'trap \"touch {}; exit 0\" INT; touch \"$READY\"; i=0; while [ $i -lt 400 ]; do i=$((i+1)); sleep 0.05; done'; exit 5",
        flag.display()
    );
    let mut hold = held(dir.path(), &script);
    ctrl_c(&hold);
    let code = wait_or_kill(&mut hold);
    assert!(flag.exists(), "the server under the wrapper got SIGINT");
    assert!(code == Some(5) || code == Some(130), "the wrapper's exit: {code:?}");
    assert!(!group_held(dir.path()));
}

// covers 23.5-d
#[cfg(unix)]
#[test]
fn a_killed_gpu_hold_keeps_the_lease_while_its_command_runs() {
    let dir = tempfile::tempdir().unwrap();
    let go = dir.path().join("go");
    let script = format!(
        "touch \"$READY\"; i=0; while [ ! -e {} ] && [ $i -lt 400 ]; do i=$((i+1)); sleep 0.05; done",
        go.display()
    );
    let mut hold = held(dir.path(), &script);
    hold.kill().unwrap(); // SIGKILL: no handler runs
    hold.wait().unwrap();
    assert!(group_held(dir.path()), "the command still runs and still holds the lease");
    std::fs::write(&go, b"").unwrap(); // the command exits
    let start = std::time::Instant::now();
    while group_held(dir.path()) {
        assert!(start.elapsed().as_secs() < 10, "the lease outlived its command");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
