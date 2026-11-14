//! Security contract: the read-root boundary holds against symlinks and
//! the source walk is bounded (#82, Stage F2). Synthetic data only,
//! generated at run time.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{Dims3, Volume};
use glam::Vec3;
use serde_json::{json, Value};

/// A 4×4×4 NIfTI phantom, written at run time (never committed).
fn write_phantom(path: &Path) {
    let dims = Dims3::new(4, 4, 4);
    let v = Volume::from_physical(dims, Vec3::ONE, &vec![100.0f32; dims.voxel_count()]).unwrap();
    ferrum_io::write_nifti(&v, path).unwrap();
}

/// Creates a symlink; on Windows this needs a privilege the test runner
/// may not hold, so a failed creation reports and skips the case. CI runs
/// Linux, where every case runs.
fn symlink(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    let r = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let r = if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    match r {
        Ok(()) => true,
        Err(e) => {
            eprintln!("skipping a symlink case: {e}");
            false
        }
    }
}

/// A read root with one real phantom, and an outside folder with a
/// different phantom (2×2×2, so a followed link is visible in the scan).
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
    agent: Agent,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    write_phantom(&root.join("phantom.nii.gz"));
    let dims = Dims3::new(2, 2, 2);
    let v = Volume::from_physical(dims, Vec3::ONE, &vec![-100.0f32; dims.voxel_count()]).unwrap();
    ferrum_io::write_nifti(&v, &outside.join("secret.nii.gz")).unwrap();
    let config = AgentConfig { read_roots: vec![root.clone()], ..AgentConfig::default() };
    Fixture { _dir: dir, root, outside, agent: Agent::new(config) }
}

impl Fixture {
    fn run(&self, command: &str, params: Value) -> Value {
        self.agent.run(command, &params)
    }

    fn err(&self, command: &str, params: Value) -> String {
        let env = self.run(command, params);
        assert_eq!(env["ok"], false, "{command} should fail: {env:#}");
        env["error"]["code"].as_str().unwrap().to_owned()
    }
}

#[test]
fn scan_refuses_a_symlink_to_a_file_outside_the_root() {
    // covers F2.1-a: the link is inside the root, its target is not.
    let f = fixture();
    let link = f.root.join("link.nii.gz");
    if !symlink(&f.outside.join("secret.nii.gz"), &link) {
        return;
    }
    assert_eq!(f.err("study scan", json!({ "paths": [link] })), "forbidden");
}

#[test]
fn open_refuses_a_symlink_to_a_file_outside_the_root() {
    // covers F2.1-a: the same boundary on the command that loads voxels.
    let f = fixture();
    let link = f.root.join("link.nii.gz");
    if !symlink(&f.outside.join("secret.nii.gz"), &link) {
        return;
    }
    let ws = f._dir.path().join("ws");
    assert_eq!(f.err("study open", json!({ "path": link, "workspace": ws })), "forbidden");
}

#[test]
fn open_refuses_a_symlinked_folder_outside_the_root() {
    // covers F2.1-a: a folder link is refused as a whole.
    let f = fixture();
    let link = f.root.join("link-dir");
    if !symlink(&f.outside, &link) {
        return;
    }
    let ws = f._dir.path().join("ws");
    assert_eq!(f.err("study open", json!({ "path": link, "workspace": ws })), "forbidden");
}

#[test]
fn a_folder_holding_an_escape_link_scans_only_its_real_files() {
    // covers F2.2-a: the walk skips the link, so the outside phantom
    // (2×2×2) never appears beside the real one (4×4×4).
    let f = fixture();
    let sub = f.root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    if !symlink(&f.outside.join("secret.nii.gz"), &sub.join("link.nii.gz")) {
        return;
    }
    assert!(symlink(&f.outside, &sub.join("link-dir")));
    let env = f.run("study scan", json!({ "paths": [f.root] }));
    assert_eq!(env["ok"], true, "{env:#}");
    let series = env["data"]["series"].as_array().unwrap();
    assert_eq!(series.len(), 1, "only the real phantom is found: {env:#}");
    assert_eq!(series[0]["dims"], json!([4, 4, 4]));
}

#[test]
fn a_source_tree_deeper_than_the_limit_is_reported() {
    // covers F2.2-b: nine levels of folders below the root exceed the
    // depth bound of eight, and the refusal is a limit, not a silent cut.
    let f = fixture();
    let mut deep = f.root.clone();
    for i in 1..=9 {
        deep = deep.join(format!("d{i}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    write_phantom(&deep.join("phantom.nii.gz"));
    assert_eq!(f.err("study scan", json!({ "paths": [f.root] })), "limit");
}

#[test]
fn a_depth_changing_link_cannot_split_the_check_from_the_scan() {
    // covers F2.1-a (security-review regression): the link points deeper
    // than its spelling; `..` behind it is lexical, and the lexical
    // target lies outside the root, so the scan is refused.
    let f = fixture();
    let deep = f.root.join("x/y");
    std::fs::create_dir_all(&deep).unwrap();
    if !symlink(&deep, &f.root.join("link")) {
        return;
    }
    let escape = f.root.join("link/../../outside/secret.nii.gz");
    assert_eq!(f.err("study scan", json!({ "paths": [escape] })), "forbidden");
    // One `..` less lands on the real phantom inside the root: allowed.
    let env = f.run("study scan", json!({ "paths": [f.root.join("link/../phantom.nii.gz")] }));
    assert_eq!(env["ok"], true, "{env:#}");
    assert_eq!(env["data"]["series"][0]["dims"], json!([4, 4, 4]));
}

#[test]
fn a_crafted_manifest_cannot_point_the_load_outside_the_root() {
    // covers F2.1-c: Study::load checks every recorded file path before
    // hashing or scanning it; a rewritten workspace.json cannot turn the
    // load into a read of the outside phantom.
    let f = fixture();
    let ws = f._dir.path().join("ws");
    let env = f.run("study open", json!({ "path": f.root.join("phantom.nii.gz"), "workspace": ws }));
    assert_eq!(env["ok"], true, "{env:#}");
    let manifest_path = ws.join("workspace.json");
    let mut m: Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    m["source"]["files"][0]["path"] = json!(f.outside.join("secret.nii.gz"));
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&m).unwrap()).unwrap();
    assert_eq!(f.err("study info", json!({ "workspace": ws })), "forbidden");
}

#[test]
fn a_crafted_manifest_with_parent_components_is_refused() {
    // covers F2.1-c (code-review regression): the re-check normalises
    // `..` lexically, but verify_sources opens the raw spelling, where
    // the OS resolves it behind the link — outside the root. A manifest
    // path with a `..` is refused outright, in the source and the files.
    let f = fixture();
    let ws = f._dir.path().join("ws");
    let env = f.run("study open", json!({ "path": f.root.join("phantom.nii.gz"), "workspace": ws }));
    assert_eq!(env["ok"], true, "{env:#}");
    if !symlink(&f.outside, &f.root.join("link")) {
        return;
    }
    let manifest_path = ws.join("workspace.json");
    let mut m: Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    // Relative file path under a folder source: lexically
    // root/secret.nii.gz (inside), in OS order outside/secret.nii.gz
    // (outside) — the readers open the raw spelling.
    m["source"]["path"] = json!(f.root);
    m["source"]["files"][0]["path"] = json!("link/../secret.nii.gz");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&m).unwrap()).unwrap();
    assert_eq!(f.err("study info", json!({ "workspace": ws })), "forbidden");
    // The same trick in the source path itself.
    let mut m: Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    m["source"]["files"][0]["path"] = json!("phantom.nii.gz");
    m["source"]["path"] = json!(f.root.join("link/.."));
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&m).unwrap()).unwrap();
    assert_eq!(f.err("study info", json!({ "workspace": ws })), "forbidden");
}
