//! GPU-group leases shared between processes: protocol `ferrum-gpu-lock/1`
//! (`docs/gpu-sharing.md` §4.1, ADR 0015).
//!
//! Engines that share a card form a *group*. Every user of the group (one
//! engine call of FERRUM, a resident model server) is a *holder*: it keeps
//! an OS lock on its own file `holder-<id>.lock` for as long as it holds the
//! card, and describes itself in `holder-<id>.json` (name, process id, kind,
//! leased MiB). When a process dies, the kernel drops its lock and its
//! record becomes stale, so a killed holder never blocks the group.
//!
//! A new holder is admitted under the group's `admission.lock`
//! ([`decide`]): with a capacity, while the leases fit; without one, only
//! when the group is empty. Engine calls never run beside each other.
//!
//! ```no_run
//! use std::time::Duration;
//! use ferrum_gpu_lock::{Kind, LockDir, Request};
//!
//! let dir = LockDir::default_for_user()?;
//! let req = Request { group: "card", holder: "llama-server", kind: Kind::Resident,
//!                     lease_mib: Some(6144), capacity_mib: Some(11264) };
//! let lease = dir.acquire(&req, Duration::from_secs(60), &mut |_wait| true)?;
//! // … run the model server; the lease is released when `lease` is dropped
//! drop(lease);
//! # Ok::<(), ferrum_gpu_lock::LockError>(())
//! ```

mod dir;
mod record;

use std::fs::{File, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use dir::{default_path, default_path_from, group_name, unsafe_reason, Meta};
pub use record::{clean_name, describe_holders, HolderInfo, Kind, MAX_NAME_CHARS, MAX_RECORD_BYTES};

/// Protocol identifier written into every record.
pub const PROTOCOL: &str = "ferrum-gpu-lock/1";

/// Pause between two admission attempts.
const RETRY: Duration = Duration::from_millis(200);

/// A request for a lease.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// Group name (as in `network.gpu_groups`).
    pub group: &'a str,
    /// Name of the holder in records and messages.
    pub holder: &'a str,
    /// What the card is used for.
    pub kind: Kind,
    /// GPU memory to lease, in MiB; `None` takes the whole group.
    pub lease_mib: Option<u64>,
    /// Capacity of the group in MiB, as the requester's configuration says;
    /// `None` admits one holder at a time.
    pub capacity_mib: Option<u64>,
}

/// The outcome of one admission attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Admitted, with a warning to pass on (a lease above the capacity).
    Admit {
        /// Warning for the caller, if any.
        warning: Option<String>,
    },
    /// Not now: wait for a holder to leave.
    Wait,
}

/// Decides whether `req` may join the live holders `held` (ADR 0015 §2).
pub fn decide(held: &[HolderInfo], req: &Request) -> Decision {
    if held.is_empty() {
        let warning = match (req.lease_mib, req.capacity_mib) {
            (Some(l), Some(c)) if l > c => Some(format!(
                "{} needs {l} MiB, more than the {c} MiB of GPU group {}; it runs alone",
                req.holder, req.group
            )),
            _ => None,
        };
        return Decision::Admit { warning };
    }
    let Some(capacity) = req.capacity_mib else { return Decision::Wait };
    let engine_running = held.iter().any(|h| h.kind == Kind::Engine);
    let whole_group_held = held.iter().any(|h| h.lease_mib.is_none());
    let lease = req.lease_mib.filter(|l| *l <= capacity);
    match lease {
        _ if req.kind == Kind::Engine && engine_running => Decision::Wait,
        _ if whole_group_held => Decision::Wait,
        None => Decision::Wait,
        Some(l) => {
            let used = held.iter().filter_map(|h| h.lease_mib).fold(0u64, u64::saturating_add);
            if used.saturating_add(l) <= capacity {
                Decision::Admit { warning: None }
            } else {
                Decision::Wait
            }
        }
    }
}

/// What a waiting request sees between two attempts.
#[derive(Debug)]
pub struct Wait<'a> {
    /// Group waited for.
    pub group: &'a str,
    /// Live holders at the last attempt.
    pub held: &'a [HolderInfo],
    /// Time waited so far.
    pub waited: Duration,
}

/// Errors of the lock directory and of admission.
#[derive(Debug)]
pub enum LockError {
    /// The environment names no home, so there is no default directory.
    NoDirectory,
    /// A path of the lock directory may be controlled by another user.
    Unsafe {
        /// The refused path.
        path: PathBuf,
        /// Why it is refused.
        reason: String,
    },
    /// An I/O error on a path of the lock directory.
    Io {
        /// The path.
        path: PathBuf,
        /// The error.
        error: io::Error,
    },
    /// The group stayed full for the whole timeout.
    Timeout {
        /// The group.
        group: String,
        /// Its holders at the last attempt.
        held: Vec<HolderInfo>,
    },
    /// The caller stopped waiting.
    Cancelled,
}

impl LockError {
    fn io(path: &Path, error: io::Error) -> Self {
        Self::Io { path: path.to_path_buf(), error }
    }
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDirectory => write!(f, "no GPU lock directory: neither XDG_RUNTIME_DIR nor HOME is set"),
            Self::Unsafe { path, reason } => write!(f, "GPU lock path {} {reason}", path.display()),
            Self::Io { path, error } => write!(f, "GPU lock path {}: {error}", path.display()),
            Self::Timeout { group, held } if held.is_empty() => {
                write!(f, "GPU group {group}: another process kept its admission lock")
            }
            Self::Timeout { group, held } => {
                write!(f, "GPU group {group} stayed full: {}", describe_holders(held))
            }
            Self::Cancelled => write!(f, "stopped waiting for the GPU group"),
        }
    }
}

impl std::error::Error for LockError {}

/// A checked, per-user lock directory.
#[derive(Debug, Clone)]
pub struct LockDir {
    root: PathBuf,
}

impl LockDir {
    /// The user's default directory ([`default_path`]); its `ferrum` parent
    /// is checked as well.
    pub fn default_for_user() -> Result<Self, LockError> {
        let root = default_path().ok_or(LockError::NoDirectory)?;
        if let Some(parent) = root.parent() {
            if let Some(base) = parent.parent() {
                std::fs::create_dir_all(base).map_err(|e| LockError::io(base, e))?;
            }
            dir::ensure_dir(parent)?;
        }
        Self::at(root)
    }

    /// A directory chosen by the operator (`network.gpu_lock_dir`); created
    /// with mode 0700 when missing, its parent must exist.
    pub fn at(root: impl Into<PathBuf>) -> Result<Self, LockError> {
        let root = root.into();
        dir::ensure_dir(&root)?;
        Ok(Self { root })
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.root
    }

    fn group_dir(&self, group: &str) -> Result<PathBuf, LockError> {
        let path = self.root.join(group_name(group));
        dir::ensure_dir(&path)?;
        Ok(path)
    }

    /// The live holders of `group`; stale records are removed. Gives
    /// `Timeout` (with no holders) when another process keeps the
    /// admission lock for longer than two seconds.
    pub fn holders(&self, group: &str) -> Result<Vec<HolderInfo>, LockError> {
        let gdir = self.group_dir(group)?;
        let start = Instant::now();
        loop {
            if let Some(_admission) = try_admission(&gdir)? {
                return scan(&gdir);
            }
            if start.elapsed() >= Duration::from_secs(2) {
                return Err(LockError::Timeout { group: group.to_owned(), held: Vec::new() });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits until `req` is admitted, `timeout` has passed, or `on_wait`
    /// returns `false`. A zero timeout tries once.
    pub fn acquire(
        &self,
        req: &Request,
        timeout: Duration,
        on_wait: &mut dyn FnMut(&Wait) -> bool,
    ) -> Result<Lease, LockError> {
        let gdir = self.group_dir(req.group)?;
        let start = Instant::now();
        loop {
            // the admission lock is only tried, so a process that keeps it
            // cannot make the caller wait past its timeout
            let held = match try_admission(&gdir)? {
                Some(_admission) => {
                    let held = scan(&gdir)?;
                    if let Decision::Admit { warning } = decide(&held, req) {
                        return Lease::create(&gdir, req, warning);
                    }
                    held
                }
                None => Vec::new(),
            };
            let waited = start.elapsed();
            if waited >= timeout {
                return Err(LockError::Timeout { group: req.group.to_owned(), held });
            }
            if !on_wait(&Wait { group: req.group, held: &held, waited }) {
                return Err(LockError::Cancelled);
            }
            std::thread::sleep(RETRY.min(timeout - waited));
        }
    }
}

/// A held OS lock that is released explicitly when dropped. Closing alone is
/// not enough: `flock` belongs to the open file, and a child process that
/// another thread forks at that moment keeps a copy of the descriptor until
/// it `exec`s, which would keep the lock.
#[derive(Debug)]
struct Held(File);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Tries the group's admission lock (held for one decision); `None` while
/// another process holds it.
fn try_admission(gdir: &Path) -> Result<Option<Held>, LockError> {
    let path = gdir.join("admission.lock");
    let file = dir::open_file(&path, true).map_err(|e| LockError::io(&path, e))?;
    dir::check_open(&file, &path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Held(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(LockError::io(&path, e)),
    }
}

/// Live holders of a group directory; removes stale ones. Call it only
/// under the admission lock.
fn scan(gdir: &Path) -> Result<Vec<HolderInfo>, LockError> {
    let entries = std::fs::read_dir(gdir).map_err(|e| LockError::io(gdir, e))?;
    let mut held = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| LockError::io(gdir, e))?;
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_prefix("holder-")).and_then(|n| n.strip_suffix(".lock"))
        else {
            continue;
        };
        let lock_path = entry.path();
        let record_path = gdir.join(format!("holder-{stem}.json"));
        if let Some(h) = live_holder(&lock_path, &record_path)? {
            held.push(h);
        }
    }
    held.sort_by_key(|h| (h.since_unix_s, h.pid));
    Ok(held)
}

/// The holder of `lock_path` if it is alive; a stale pair is removed.
fn live_holder(lock_path: &Path, record_path: &Path) -> Result<Option<HolderInfo>, LockError> {
    let file = match dir::open_file(lock_path, false) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(LockError::io(lock_path, e)),
    };
    dir::check_open(&file, lock_path)?;
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock(); // see Held
            drop(file);
            remove(record_path)?;
            remove(lock_path)?;
            Ok(None)
        }
        // a locked file whose record is gone belongs to a holder that is leaving
        // (records are created under the admission lock, so it is never one arriving)
        Err(TryLockError::WouldBlock) if !record_path.exists() => Ok(None),
        Err(TryLockError::WouldBlock) => Ok(Some(read_record(record_path))),
        Err(TryLockError::Error(e)) => Err(LockError::io(lock_path, e)),
    }
}

fn read_record(path: &Path) -> HolderInfo {
    let read = || -> Option<HolderInfo> {
        let file = dir::open_read(path).ok()?;
        dir::check_open(&file, path).ok()?;
        let mut text = String::new();
        file.take(MAX_RECORD_BYTES + 1).read_to_string(&mut text).ok()?;
        if text.len() as u64 > MAX_RECORD_BYTES {
            return None;
        }
        HolderInfo::from_json(&text)
    };
    read().unwrap_or_else(HolderInfo::unreadable)
}

fn remove(path: &Path) -> Result<(), LockError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(LockError::io(path, e)),
    }
}

/// A held lease; released when dropped (also when the process dies).
#[derive(Debug)]
pub struct Lease {
    lock: Option<File>,
    lock_path: PathBuf,
    record_path: PathBuf,
    info: HolderInfo,
    warning: Option<String>,
}

impl Lease {
    /// Lets child processes started from now on inherit the lease's lock
    /// file (it is opened close-on-exec). The OS lock then lasts while this
    /// process or any such child runs, so a wrapper (`ferrum-cli gpu hold`)
    /// that is killed does not release a lease its command still uses.
    /// Only for a process that starts the holder's command and nothing else:
    /// every child started meanwhile keeps the lease too (lesson L27).
    pub fn share_with_children(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(lock) = &self.lock {
            rustix::io::fcntl_setfd(lock, rustix::io::FdFlags::empty())?;
        }
        Ok(())
    }

    fn create(gdir: &Path, req: &Request, warning: Option<String>) -> Result<Self, LockError> {
        let info = HolderInfo::now(req.holder, req.kind, req.lease_mib);
        let id = unique_id();
        let lock_path = gdir.join(format!("holder-{id}.lock"));
        let record_path = gdir.join(format!("holder-{id}.json"));
        let lock = dir::create_new(&lock_path).map_err(|e| LockError::io(&lock_path, e))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(LockError::io(&lock_path, io::Error::other("a new lock file is locked by another process")))
            }
            Err(TryLockError::Error(e)) => return Err(LockError::io(&lock_path, e)),
        }
        // from here on, dropping `lease` removes both files
        let lease = Self { lock: Some(lock), lock_path, record_path, info, warning };
        let mut record = dir::create_new(&lease.record_path).map_err(|e| LockError::io(&lease.record_path, e))?;
        record.write_all(lease.info.to_json().as_bytes()).map_err(|e| LockError::io(&lease.record_path, e))?;
        Ok(lease)
    }

    /// The record this lease wrote.
    pub fn info(&self) -> &HolderInfo {
        &self.info
    }

    /// A warning from admission (a lease above the capacity), if any.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // the record goes first: a scanner that finds the lock file unlocked
        // removes what is left, so the order cannot leave a live-looking pair
        let _ = std::fs::remove_file(&self.record_path);
        if let Some(lock) = self.lock.take() {
            let _ = lock.unlock(); // see Held
        }
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// An id unique on this machine: the process id, the time and a counter.
fn unique_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{:x}-{n}", std::process::id(), nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(kind: Kind, lease_mib: Option<u64>) -> HolderInfo {
        HolderInfo { holder: "h".into(), pid: 1, kind, lease_mib, since_unix_s: 0 }
    }

    fn req(kind: Kind, lease_mib: Option<u64>, capacity_mib: Option<u64>) -> Request<'static> {
        Request { group: "card", holder: "ferrum", kind, lease_mib, capacity_mib }
    }

    const ADMIT: Decision = Decision::Admit { warning: None };

    // covers 23.1-f
    #[test]
    fn no_capacity_is_a_mutex() {
        assert_eq!(decide(&[], &req(Kind::Engine, Some(10), None)), ADMIT);
        assert_eq!(decide(&[holder(Kind::Resident, Some(1))], &req(Kind::Engine, Some(1), None)), Decision::Wait);
        assert_eq!(decide(&[holder(Kind::Engine, None)], &req(Kind::Resident, None, None)), Decision::Wait);
    }

    // covers 23.2-a
    #[test]
    fn engine_calls_stay_one_at_a_time_under_capacity() {
        let one_engine = [holder(Kind::Engine, Some(1000))];
        assert_eq!(decide(&one_engine, &req(Kind::Engine, Some(1000), Some(12_000))), Decision::Wait);
        assert_eq!(decide(&one_engine, &req(Kind::Resident, Some(1000), Some(12_000))), ADMIT);
    }

    // covers 23.2-c
    #[test]
    fn lease_6000_blocks_7000_admits_4000() {
        let model = [holder(Kind::Resident, Some(6000))];
        assert_eq!(decide(&model, &req(Kind::Engine, Some(7000), Some(12_000))), Decision::Wait);
        assert_eq!(decide(&model, &req(Kind::Engine, Some(4000), Some(12_000))), ADMIT);
        assert_eq!(decide(&model, &req(Kind::Engine, Some(6000), Some(12_000))), ADMIT, "exactly full fits");
        assert_eq!(decide(&model, &req(Kind::Engine, None, Some(12_000))), Decision::Wait, "no peak: whole group");
        let two = [holder(Kind::Resident, Some(6000)), holder(Kind::Resident, Some(u64::MAX))];
        assert_eq!(decide(&two, &req(Kind::Engine, Some(1), Some(12_000))), Decision::Wait, "no overflow");
    }

    // covers 23.2-d
    #[test]
    fn peak_over_capacity_runs_alone_with_warning() {
        let alone = decide(&[], &req(Kind::Engine, Some(13_000), Some(12_000)));
        let Decision::Admit { warning: Some(w) } = alone else { panic!("{alone:?}") };
        assert!(w.contains("13000 MiB") && w.contains("12000 MiB") && w.contains("runs alone"), "{w}");
        let model = [holder(Kind::Resident, Some(10))];
        assert_eq!(decide(&model, &req(Kind::Engine, Some(13_000), Some(12_000))), Decision::Wait);
        assert_eq!(decide(&[holder(Kind::Resident, None)], &req(Kind::Engine, Some(1), Some(12_000))), Decision::Wait);
    }
}
