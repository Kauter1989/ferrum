//! Engine sessions between calls and GPU sharing
//! (`docs/agent-segmentation.md` §2 and §5.3).
//!
//! - [`GpuLock`]: engines the operator put into one GPU group are used one
//!   call at a time, across processes, through an advisory file lock.
//! - [`EngineSessions`]: a long-running server (MCP) keeps the engine
//!   session of an interactive object open, so a refinement sends only the
//!   new prompts. The session is only a cache of the prompt history stored
//!   in the workspace; the command line replays that history instead, and
//!   both give the same result. Sessions of engines in a GPU group are not
//!   kept, so an idle session never holds memory another engine of the
//!   group needs.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ferrum_domain::{InteractiveSession, Prompt, Timestamp, VoxelBox};

use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode};

/// Held while an engine of a GPU group runs; released on drop.
#[derive(Debug)]
pub struct GpuLock {
    _file: Option<File>,
}

impl GpuLock {
    /// Path of the lock file of `group`.
    pub fn path(group: &str) -> PathBuf {
        let safe: String = group.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        std::env::temp_dir().join(format!("ferrum-gpu-{safe}.lock"))
    }

    /// Waits up to `timeout` for the GPU group of `url`; engines outside a
    /// group need no lock.
    pub fn acquire(config: &AgentConfig, url: &str, timeout: Duration) -> Result<Self, AgentError> {
        let Some(group) = config.gpu_group(url) else { return Ok(Self { _file: None }) };
        let path = Self::path(group);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| AgentError::internal(format!("GPU lock {}: {e}", path.display())))?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: Some(file) }),
                Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(AgentError::new(
                        ErrorCode::EngineUnavailable,
                        format!("another engine of GPU group {group} is still running"),
                    )
                    .hint("wait for it to finish and try again; engines of one GPU group run one at a time"))
                }
                Err(std::fs::TryLockError::Error(e)) => {
                    return Err(AgentError::internal(format!("GPU lock {}: {e}", path.display())))
                }
            }
        }
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

    #[test]
    fn gpu_groups_serialise_engine_calls() {
        let group = format!("test-{}", std::process::id());
        let url = "http://127.0.0.1:8765".to_owned();
        let config = AgentConfig { gpu_groups: vec![(group.clone(), vec![url.clone()])], ..AgentConfig::default() };
        let held = GpuLock::acquire(&config, &url, Duration::ZERO).unwrap();
        let busy = GpuLock::acquire(&config, &url, Duration::from_millis(250)).unwrap_err();
        assert_eq!(busy.code, ErrorCode::EngineUnavailable);
        drop(held);
        GpuLock::acquire(&config, &url, Duration::ZERO).unwrap();
        GpuLock::acquire(&config, "http://127.0.0.1:9", Duration::ZERO).unwrap();
        assert!(GpuLock::path("a b/c").ends_with("ferrum-gpu-a_b_c.lock"));
        let _ = std::fs::remove_file(GpuLock::path(&group));
    }
}
