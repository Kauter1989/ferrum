//! Engine sessions between calls and GPU sharing
//! (`docs/agent-segmentation.md` §2 and §5.3, `docs/gpu-sharing.md`).
//!
//! - [`gpu_lease`]: an engine call of a GPU group first takes a lease of
//!   `ferrum-gpu-lock/1`, shared with every process of the user: other
//!   FERRUM processes, the desktop app, and a model server such as Medical
//!   Harness's `llama-server`. Engine calls of a group run one at a time,
//!   and next to other holders only when the leases fit the group's
//!   capacity.
//! - [`EngineSessions`]: a long-running server (MCP) keeps the engine
//!   session of an interactive object open, so a refinement sends only the
//!   new prompts. The session is only a cache of the prompt history stored
//!   in the workspace; the command line replays that history instead, and
//!   both give the same result. Sessions of engines in a GPU group are not
//!   kept, so an idle session never holds memory another engine of the
//!   group needs.

use std::path::PathBuf;
use std::time::Duration;

use ferrum_domain::{InteractiveSession, Prompt, Timestamp, VoxelBox};
use ferrum_engines::gpu::{describe_holders, effective_peak, Lease, LockError};

use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode};

/// Takes the GPU lease of an engine call of `url`, or `None` when the
/// engine is in no GPU group. It waits up to `limits.engine_job_timeout_s`
/// and tells `progress` who holds the group meanwhile. `manifest_peak` is
/// the engine's own `limits.expected_peak_mib`.
pub fn gpu_lease(
    config: &AgentConfig,
    url: &str,
    holder: &str,
    manifest_peak: Option<u64>,
    progress: &mut dyn FnMut(f64, &str),
) -> Result<Option<Lease>, AgentError> {
    let timeout = Duration::from_secs(config.engine_job_timeout_s);
    let needs = match effective_peak(config.gpu.operator_peak(url), manifest_peak) {
        Some(m) => format!("this call needs {m} MiB"),
        None => "this call needs the whole group".to_owned(),
    };
    let mut on_wait = |w: &ferrum_engines::gpu::Wait| {
        progress(0.0, &format!("waiting for GPU group {}: {}; {needs}", w.group, describe_holders(w.held)));
        true
    };
    let dir = || match &config.gpu.lock_dir {
        Some(d) => d.display().to_string(),
        None => ferrum_engines::gpu::default_path().map_or_else(|| "none".into(), |d| d.display().to_string()),
    };
    config.gpu.acquire(url, holder, manifest_peak, timeout, &mut on_wait).map_err(|e| lease_error(e, timeout, &dir()))
}

fn lease_error(e: LockError, timeout: Duration, dir: &str) -> AgentError {
    match &e {
        LockError::Timeout { held, .. } if held.is_empty() => {
            AgentError::new(ErrorCode::EngineUnavailable, e.to_string())
                .hint(format!("another process keeps the admission lock in {dir}; it must hold it only to decide"))
        }
        LockError::Timeout { group, held } => AgentError::new(
            ErrorCode::EngineUnavailable,
            format!("GPU group {group} stayed full for {} s: {}", timeout.as_secs(), describe_holders(&held)),
        )
        .hint(format!(
            "engines of one GPU group run one at a time, and next to other holders only when their leases fit \
                 network.gpu_capacity_mib; try again when the holders are done (lock directory {dir}; every \
                 process sharing the card must use the same one)"
        )),
        other => AgentError::internal(other.to_string()).hint(format!(
            "the GPU lock directory {dir} (network.gpu_lock_dir) must belong to this user and be closed to others"
        )),
    }
}

/// What an open engine session holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionKey {
    /// Workspace root.
    pub workspace: PathBuf,
    /// Hash of the source data.
    pub source_sha256: String,
    /// Engine URL.
    pub engine: String,
    /// Engine version.
    pub version: String,
    /// Segment label of the object.
    pub label: u8,
    /// Creation time of the segment.
    pub created: Option<Timestamp>,
    /// Region uploaded to the engine.
    pub roi: VoxelBox,
}

/// An engine session kept open between calls.
pub struct CachedSession {
    /// What it holds.
    pub key: SessionKey,
    /// The session.
    pub session: Box<dyn InteractiveSession>,
    /// Prompts applied so far (full-grid voxels).
    pub prompts: Vec<Prompt>,
    /// Box (in ROI voxels) bounding every change of the object so far.
    pub extent: Option<VoxelBox>,
}

impl std::fmt::Debug for CachedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedSession").field("key", &self.key).field("prompts", &self.prompts.len()).finish()
    }
}

/// Engine sessions of a long-running server.
#[derive(Debug, Default)]
pub struct EngineSessions {
    list: Vec<CachedSession>,
}

impl EngineSessions {
    /// Most sessions kept; the oldest is closed first.
    pub const CAPACITY: usize = 4;

    /// Number of open sessions.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// `true` if no session is open.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Takes the session for `key` out of the cache.
    pub fn take(&mut self, key: &SessionKey) -> Option<CachedSession> {
        let at = self.list.iter().position(|c| &c.key == key)?;
        Some(self.list.remove(at))
    }

    /// Keeps `session`, closing the oldest one beyond [`Self::CAPACITY`]
    /// and any other session of the same object.
    pub fn put(&mut self, session: CachedSession) {
        self.list.retain(|c| !(c.key.workspace == session.key.workspace && c.key.label == session.key.label));
        self.list.push(session);
        if self.list.len() > Self::CAPACITY {
            self.list.remove(0);
        }
    }

    /// Closes every session on one of `engines`.
    pub fn close_engines(&mut self, engines: &[String]) {
        self.list.retain(|c| !engines.contains(&c.key.engine));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::{EngineError, PromptResult};
    use glam::UVec3;

    struct Idle;
    impl InteractiveSession for Idle {
        fn prompt(&mut self, _: &Prompt) -> Result<PromptResult, EngineError> {
            Ok(PromptResult { revision: 0, changed: None, empty: true })
        }
        fn mask(&mut self, _: VoxelBox) -> Result<Vec<u8>, EngineError> {
            Ok(Vec::new())
        }
        fn undo(&mut self) -> Result<PromptResult, EngineError> {
            Err(EngineError::Unsupported("undo".into()))
        }
        fn reset(&mut self) -> Result<(), EngineError> {
            Ok(())
        }
    }

    fn key(label: u8, engine: &str) -> SessionKey {
        SessionKey {
            workspace: "/ws".into(),
            source_sha256: "s".into(),
            engine: engine.into(),
            version: "1".into(),
            label,
            created: None,
            roi: VoxelBox::new(UVec3::ZERO, UVec3::ONE),
        }
    }

    fn cached(label: u8, engine: &str) -> CachedSession {
        CachedSession { key: key(label, engine), session: Box::new(Idle), prompts: Vec::new(), extent: None }
    }

    #[test]
    fn sessions_are_cached_by_object() {
        let mut s = EngineSessions::default();
        assert!(s.is_empty());
        for l in 1..=5 {
            s.put(cached(l, "a"));
        }
        assert_eq!(s.len(), EngineSessions::CAPACITY);
        assert!(s.take(&key(1, "a")).is_none(), "the oldest was closed");
        assert!(s.take(&key(5, "a")).is_some());
        s.put(cached(4, "b"));
        assert!(s.take(&key(4, "a")).is_none(), "one session per object");
        s.close_engines(&["b".into()]);
        assert_eq!(s.len(), 2);
        assert!(format!("{:?}", cached(1, "a")).contains("CachedSession"));
    }

    // covers 23.1-e, 23.1-f
    #[test]
    fn gpu_groups_serialise_engine_calls_and_name_the_holder() {
        let dir = std::env::temp_dir().join(format!("ferrum-agent-gpu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let url = "http://127.0.0.1:8765".to_owned();
        let mut config = AgentConfig { engine_job_timeout_s: 1, ..AgentConfig::default() };
        config.gpu.groups = vec![("card".into(), vec![url.clone()])];
        config.gpu.lock_dir = Some(dir.clone());
        let mut heard = Vec::new();
        let held = gpu_lease(&config, &url, "first", None, &mut |_, _| {}).unwrap().unwrap();
        let busy = gpu_lease(&config, &url, "second", None, &mut |_, m| heard.push(m.to_owned())).unwrap_err();
        assert_eq!(busy.code, ErrorCode::EngineUnavailable);
        assert!(busy.message.contains("GPU group card stayed full for 1 s: first (pid"), "{}", busy.message);
        assert!(heard.len() >= 3, "progress during the wait: {heard:?}");
        assert!(heard[0].starts_with("waiting for GPU group card: first (pid "), "{}", heard[0]);
        assert!(heard[0].ends_with("holds the whole group; this call needs the whole group"), "{}", heard[0]);
        drop(held);
        assert!(gpu_lease(&config, &url, "second", None, &mut |_, _| {}).unwrap().is_some());
        assert!(gpu_lease(&config, "http://127.0.0.1:9", "x", None, &mut |_, _| {}).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
