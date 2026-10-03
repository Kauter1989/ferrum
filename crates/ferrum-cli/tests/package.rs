//! The skill package stays in sync with the code: schemas, command
//! references, manifests, and the evaluation commands.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cli(args: &[&str]) -> (i32, String) {
    let out =
        Command::new(env!("CARGO_BIN_EXE_ferrum-cli")).args(args).env_remove("FERRUM_AGENT_CONFIG").output().unwrap();
    (out.status.code().unwrap(), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn schemas_file_matches_the_code() {
    let (code, generated) = cli(&["schema"]);
    assert_eq!(code, 0);
    let file = std::fs::read_to_string(root().join("skills/ferrum/schemas/commands.json")).unwrap();
    let (a, b): (Value, Value) = (serde_json::from_str(&generated).unwrap(), serde_json::from_str(&file).unwrap());
    assert!(
        a == b,
        "skills/ferrum/schemas/commands.json is stale: run `ferrum-cli schema > skills/ferrum/schemas/commands.json`"
    );
}

#[test]
fn references_name_every_command() {
    let (_, out) = cli(&["commands"]);
    let commands: Value = serde_json::from_str(&out).unwrap();
    let skill_ref = std::fs::read_to_string(root().join("skills/ferrum/reference/commands.md")).unwrap();
    let cli_doc = std::fs::read_to_string(root().join("docs/agent-cli.md")).unwrap();
    for c in commands["commands"].as_array().unwrap() {
        let c = c.as_str().unwrap();
        let first = c.split(' ').next().unwrap();
        assert!(skill_ref.contains(&format!("`{c}`")) || skill_ref.contains(c), "reference/commands.md lacks {c}");
        assert!(cli_doc.contains(first), "docs/agent-cli.md lacks {c}");
    }
    let skill = std::fs::read_to_string(root().join("skills/ferrum/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: ferrum-imaging\ndescription: "), "SKILL.md front matter");
    for page in ["commands", "coordinates", "outputs", "safety", "segmentation"] {
        assert!(skill.contains(&format!("reference/{page}.md")));
        assert!(root().join(format!("skills/ferrum/reference/{page}.md")).exists());
    }
}

#[test]
fn plugin_manifests() {
    let plugin: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("skills/plugin/plugin.json")).unwrap()).unwrap();
    assert_eq!(plugin["name"], "ferrum");
    assert_eq!(plugin["version"], env!("CARGO_PKG_VERSION"), "keep the plugin version with the crates");
    let mcp: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("skills/plugin/mcp.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["ferrum"]["args"], serde_json::json!(["mcp"]));
    assert!(mcp["mcpServers"]["ferrum"]["command"].as_str().unwrap().contains("__EXE__"));
}

#[test]
fn eval_commands() {
    let dir = tempfile::tempdir().unwrap();
    let (code, out) = cli(&["eval", "tasks"]);
    assert_eq!(code, 0);
    let tasks = serde_json::from_str::<Value>(&out).unwrap()["tasks"].as_array().unwrap().clone();
    assert_eq!(tasks.len(), 8);
    assert_eq!(tasks.iter().filter(|t| t["engine"] == true).count(), 3);
    let (code, out) = cli(&["eval", "phantoms", dir.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(dir.path().join("sphere_cube.nii.gz").exists());
    let t = dir.path().join("t.json");
    std::fs::write(&t, r#"{ "calls": [ { "command": "probe", "result": { "data": { "value": 1000.0 } } } ], "answer": "The mean is 1000.0." }"#).unwrap();
    assert_eq!(cli(&["eval", "grade", "--task", "cube_mean", t.to_str().unwrap()]).0, 0);
    std::fs::write(&t, r#"{ "calls": [], "answer": "About 1000." }"#).unwrap();
    let (code, out) = cli(&["eval", "grade", "--task", "cube_mean", t.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap()["from_tools"], false);
    assert_eq!(cli(&["eval", "grade", "--task", "nope", t.to_str().unwrap()]).0, 2);
}

#[test]
fn eval_engine_serves_the_mock_engine() {
    use std::io::{BufRead, Read, Write};
    let mut child = Command::new(env!("CARGO_BIN_EXE_ferrum-cli"))
        .args(["eval", "engine", "127.0.0.1:0"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let url = serde_json::from_str::<Value>(&line).unwrap()["engine"].as_str().unwrap().to_owned();
    let addr = url.trim_start_matches("http://");
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "GET /v1/info HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").unwrap();
    let mut reply = String::new();
    s.read_to_string(&mut reply).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(reply.contains("ferrum-engine/1") && reply.contains("mock"), "{reply}");
}
