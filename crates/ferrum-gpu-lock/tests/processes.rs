//! Leases across real processes and real lock files.
//!
//! A second process is this test binary run again with
//! `FERRUM_GPU_LOCK_TEST_HOLDER=<dir>`: its `holder_child` test then holds
//! group `g` until its stdin closes.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ferrum_gpu_lock::{Kind, LockDir, LockError, Request};

const CHILD_ENV: &str = "FERRUM_GPU_LOCK_TEST_HOLDER";

/// A fresh lock directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "ferrum-gpu-lock-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn req(kind: Kind, lease_mib: Option<u64>, capacity_mib: Option<u64>) -> Request<'static> {
    Request { group: "g", holder: "test", kind, lease_mib, capacity_mib }
}

fn try_once(dir: &LockDir, r: &Request) -> Result<ferrum_gpu_lock::Lease, LockError> {
    dir.acquire(r, Duration::ZERO, &mut |_| true)
}

#[test]
fn holder_child() {
    let Some(dir) = std::env::var_os(CHILD_ENV) else { return };
    let dir = LockDir::at(PathBuf::from(dir)).unwrap();
    let r = Request { group: "g", holder: "child", kind: Kind::Resident, lease_mib: None, capacity_mib: None };
    let lease = try_once(&dir, &r).unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    drop(lease);
}

fn spawn_holder(dir: &Path) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "holder_child", "--test-threads=1", "--nocapture"])
        .env(CHILD_ENV, dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_until_held_by(dir: &LockDir, pid: u32) {
    let start = Instant::now();
    while !dir.holders("g").unwrap().iter().any(|h| h.pid == pid) {
        assert!(start.elapsed() < Duration::from_secs(20), "the child never took the lease");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// covers 23.1-d
#[test]
fn killed_holder_is_stale() {
    let tmp = TempDir::new();
    let dir = LockDir::at(&tmp.0).unwrap();
    let mut child = spawn_holder(&tmp.0);
    wait_until_held_by(&dir, child.id());
    assert!(matches!(try_once(&dir, &req(Kind::Engine, Some(1), Some(100))), Err(LockError::Timeout { .. })));
    child.kill().unwrap(); // SIGKILL: the child cannot clean up
    child.wait().unwrap();
    let killed = Instant::now();
    let lease = dir.acquire(&req(Kind::Engine, None, None), Duration::from_secs(5), &mut |_| true).unwrap();
    assert!(killed.elapsed() < Duration::from_secs(1), "{:?}", killed.elapsed());
    assert_eq!(dir.holders("g").unwrap().len(), 1, "only the new lease is live");
    let files = std::fs::read_dir(tmp.0.join("g")).unwrap().count();
    assert_eq!(files, 3, "admission.lock and the new pair; the dead pair was removed");
    drop(lease);
    assert!(dir.holders("g").unwrap().is_empty());
}

// covers 23.1-d
#[test]
fn waiter_is_admitted_within_a_second_after_release() {
    let tmp = TempDir::new();
    let dir = LockDir::at(&tmp.0).unwrap();
    let mut child = spawn_holder(&tmp.0);
    wait_until_held_by(&dir, child.id());
    let waiter = {
        let dir = dir.clone();
        std::thread::spawn(move || {
            let mut waits = 0;
            let lease = dir.acquire(&req(Kind::Engine, None, None), Duration::from_secs(20), &mut |w| {
                assert_eq!(w.held.len(), 1);
                assert_eq!(w.held[0].holder, "child");
                waits += 1;
                true
            });
            (lease.map(|_| Instant::now()), waits)
        })
    };
    std::thread::sleep(Duration::from_millis(600));
    child.stdin.take().unwrap().write_all(b"release\n").unwrap();
    child.wait().unwrap();
    let released = Instant::now();
    let (admitted, waits) = waiter.join().unwrap();
    let admitted = admitted.unwrap();
    assert!(waits >= 2, "it waited ({waits} attempts)");
    assert!(admitted.saturating_duration_since(released) < Duration::from_secs(1));
}

// covers 23.2-c
#[test]
fn leases_share_the_capacity_through_files() {
    let tmp = TempDir::new();
    let dir = LockDir::at(&tmp.0).unwrap();
    let model = try_once(&dir, &req(Kind::Resident, Some(6000), Some(12_000))).unwrap();
    match try_once(&dir, &req(Kind::Engine, Some(7000), Some(12_000))) {
        Err(LockError::Timeout { group, held }) => {
            assert_eq!(group, "g");
            assert_eq!(held.len(), 1);
            assert_eq!(held[0].lease_mib, Some(6000));
            assert_eq!(held[0].pid, std::process::id());
        }
        other => panic!("{other:?}"),
    }
    let small = try_once(&dir, &req(Kind::Engine, Some(4000), Some(12_000))).unwrap();
    assert_eq!(dir.holders("g").unwrap().len(), 2);
    drop(small);
    drop(model);
    let big = try_once(&dir, &req(Kind::Engine, Some(7000), Some(12_000))).unwrap();
    assert!(big.warning().is_none());
}

// covers 23.1-e
#[test]
fn cancel_stops_waiting() {
    let tmp = TempDir::new();
    let dir = LockDir::at(&tmp.0).unwrap();
    let _held = try_once(&dir, &req(Kind::Resident, None, None)).unwrap();
    let start = Instant::now();
    let r = dir.acquire(&req(Kind::Engine, None, None), Duration::from_secs(30), &mut |_| false);
    assert!(matches!(r, Err(LockError::Cancelled)));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn an_unreadable_live_record_holds_the_whole_group() {
    let tmp = TempDir::new();
    let dir = LockDir::at(&tmp.0).unwrap();
    let held = try_once(&dir, &req(Kind::Resident, Some(10), Some(12_000))).unwrap();
    let record = std::fs::read_dir(tmp.0.join("g"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    std::fs::write(&record, "not json").unwrap();
    let holders = dir.holders("g").unwrap();
    assert_eq!(holders[0].lease_mib, None);
    assert!(try_once(&dir, &req(Kind::Engine, Some(1), Some(12_000))).is_err());
    drop(held);
}

// covers 23.1-c
#[cfg(unix)]
#[test]
fn refuses_shared_and_linked_directories() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new();
    std::fs::create_dir(&tmp.0).unwrap();
    std::fs::set_permissions(&tmp.0, std::fs::Permissions::from_mode(0o777)).unwrap();
    let err = LockDir::at(&tmp.0).unwrap_err();
    assert!(err.to_string().contains("can be written by other users"), "{err}");
    assert!(err.to_string().contains(&tmp.0.display().to_string()), "the message names the path: {err}");

    std::fs::set_permissions(&tmp.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let dir = LockDir::at(&tmp.0).unwrap();
    std::fs::create_dir(tmp.0.join("real")).unwrap();
    std::os::unix::fs::symlink(tmp.0.join("real"), tmp.0.join("linked")).unwrap();
    let r = Request { group: "linked", ..req(Kind::Engine, None, None) };
    let err = try_once(&dir, &r).unwrap_err();
    assert!(err.to_string().contains("symbolic link"), "{err}");

    // a group directory that others can write is refused too
    std::fs::create_dir(tmp.0.join("open")).unwrap();
    std::fs::set_permissions(tmp.0.join("open"), std::fs::Permissions::from_mode(0o777)).unwrap();
    let r = Request { group: "open", ..req(Kind::Engine, None, None) };
    assert!(matches!(try_once(&dir, &r), Err(LockError::Unsafe { .. })));
}

// covers 23.1-c
#[cfg(unix)]
#[test]
fn created_directories_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new();
    let dir = LockDir::at(&tmp.0).unwrap();
    let lease = try_once(&dir, &req(Kind::Engine, None, None)).unwrap();
    for p in [tmp.0.clone(), tmp.0.join("g")] {
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o700, "{}", p.display());
    }
    for e in std::fs::read_dir(tmp.0.join("g")).unwrap() {
        let p = e.unwrap().path();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o077, 0, "{}", p.display());
    }
    drop(lease);
}

// covers 23.1-e
#[test]
fn a_kept_admission_lock_cannot_outlast_the_timeout() {
    let tmp = TempDir::new();
    let dir = LockDir::at(&tmp.0).unwrap();
    assert!(dir.holders("g").unwrap().is_empty());
    let other = std::fs::OpenOptions::new().write(true).open(tmp.0.join("g").join("admission.lock")).unwrap();
    other.lock().unwrap(); // another process (here another handle) never lets go
    let start = Instant::now();
    let r = dir.acquire(&req(Kind::Engine, None, None), Duration::from_millis(500), &mut |_| true);
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    let err = r.unwrap_err();
    assert!(err.to_string().contains("kept its admission lock"), "{err}");
    drop(other);
    assert!(try_once(&dir, &req(Kind::Engine, None, None)).is_ok());
}

// covers 23.1-c
#[cfg(unix)]
#[test]
fn a_fifo_planted_as_record_does_not_stall_the_group() {
    let tmp = TempDir::new();
    let dir = LockDir::at(&tmp.0).unwrap();
    let held = try_once(&dir, &req(Kind::Resident, Some(10), Some(12_000))).unwrap();
    let record = std::fs::read_dir(tmp.0.join("g"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    std::fs::remove_file(&record).unwrap();
    let made = Command::new("mkfifo").arg(&record).status().unwrap();
    assert!(made.success());
    let start = Instant::now();
    let r = dir.acquire(&req(Kind::Engine, Some(1), Some(12_000)), Duration::from_millis(300), &mut |_| true);
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    match r {
        Err(LockError::Timeout { held, .. }) => assert_eq!(held[0].lease_mib, None, "unreadable: the whole group"),
        other => panic!("{other:?}"),
    }
    drop(held);
}

#[test]
fn a_leaving_holder_is_not_reported() {
    let tmp = TempDir::new();
    let dir = LockDir::at(&tmp.0).unwrap();
    let held = try_once(&dir, &req(Kind::Resident, Some(10), Some(12_000))).unwrap();
    // the moment between removing the record and unlocking, frozen
    for e in std::fs::read_dir(tmp.0.join("g")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|e| e == "json") {
            std::fs::remove_file(p).unwrap();
        }
    }
    assert!(dir.holders("g").unwrap().is_empty());
    drop(held);
}
