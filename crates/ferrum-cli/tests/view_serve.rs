//! `view serve` resolves its workspace through the operator configuration,
//! like every agent command: relative names live below `workspace_root`,
//! and a workspace outside the root is refused.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

fn cli() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ferrum-cli"));
    c.env_remove("FERRUM_AGENT_CONFIG");
    c
}

fn config_with_root(dir: &std::path::Path) -> std::path::PathBuf {
    let root = dir.join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let config = dir.join("agent.toml");
    std::fs::write(&config, format!("[data]\nworkspace_root = {root:?}\n")).unwrap();
    config
}

// covers 25.1-a: the workspace opens like the desktop's, through the config
#[test]
fn a_relative_workspace_is_resolved_below_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_root(dir.path());
    let root = dir.path().join("ws");
    let out = cli()
        .args(["--config", config.to_str().unwrap(), "view", "serve", "-w", "missing", "--port", "0"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "a missing workspace must fail");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(root.to_str().unwrap()),
        "the error must name the path resolved below the root, not a cwd-relative one: {err}"
    );
}

// covers 25.1-a: the operator's root is a boundary, not a suggestion
#[test]
fn a_workspace_outside_the_root_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_root(dir.path());
    let out = cli()
        .args(["--config", config.to_str().unwrap(), "view", "serve", "-w", "../outside", "--port", "0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "forbidden", "{text}");
}

// covers 25.1-a: an absolute path outside the root is refused as well
#[test]
fn an_absolute_workspace_outside_the_root_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_root(dir.path());
    let outside = dir.path().join("elsewhere");
    let out = cli()
        .args(["--config", config.to_str().unwrap(), "view", "serve", "-w", outside.to_str().unwrap(), "--port", "0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope["error"]["code"], "forbidden", "{text}");
}

// covers 25.1-a: a lexical escape through the root normalises away and stays refused
#[test]
fn a_lexical_escape_through_the_root_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_root(dir.path());
    let out = cli()
        .args(["--config", config.to_str().unwrap(), "view", "serve", "-w", "ws/../../outside", "--port", "0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope["error"]["code"], "forbidden", "{text}");
}
