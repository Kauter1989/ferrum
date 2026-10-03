//! Port to segmentation engines (ADR 0007).
//!
//! An engine is usually an out-of-process program reached through the
//! FERRUM Engine Protocol (`ferrum-engine/1`, see `docs/engine-protocol.md`);
//! implementations live in the `ferrum-engines` crate. This module defines
//! only the vocabulary and the traits, so the application can drive any
//! engine — remote, mock or future in-process — the same way.

use glam::UVec3;
use thiserror::Error;

use crate::segmentation::VoxelBox;
use crate::volume::Volume;

/// Protocol identifier every engine must report.
pub const ENGINE_PROTOCOL: &str = "ferrum-engine/1";

/// Kind of interactive prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptKind {
    /// A single voxel.
    Point,
    /// A box (planar or 3D).
    Box,
    /// Free-hand strokes, given as a mask over a box.
    Scribble,
    /// A closed outline, given as a filled mask over a box.
    Lasso,
}

impl PromptKind {
    /// All kinds.
    pub const ALL: [PromptKind; 4] = [PromptKind::Point, PromptKind::Box, PromptKind::Scribble, PromptKind::Lasso];

    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            PromptKind::Point => "point",
            PromptKind::Box => "box",
            PromptKind::Scribble => "scribble",
            PromptKind::Lasso => "lasso",
        }
    }

    /// Parses a wire name.
    pub fn parse(s: &str) -> Option<PromptKind> {
        PromptKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// What an engine can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineCapabilities {
    /// Supports interactive sessions.
    pub interactive: bool,
    /// Supports automatic segmentation jobs.
    pub automatic: bool,
    /// Supported prompt kinds.
    pub prompts: Vec<PromptKind>,
    /// Box prompts must be one voxel thick along some axis.
    pub planar_boxes_only: bool,
    /// Supports server-side undo.
    pub undo: bool,
    /// Replaying the same prompts on a new session gives the same mask
    /// (the protocol's `deterministic`, `true` when an engine omits it).
    pub deterministic: bool,
}

impl Default for EngineCapabilities {
    /// No capabilities; deterministic.
    fn default() -> Self {
        Self {
            interactive: false,
            automatic: false,
            prompts: Vec::new(),
            planar_boxes_only: false,
            undo: false,
            deterministic: true,
        }
    }
}

/// A label an automatic engine can produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineLabel {
    /// Label value in the engine's label maps.
    pub value: u16,
    /// Structure name.
    pub name: String,
    /// Suggested colour.
    pub color: Option<[u8; 3]>,
}

/// Description of an engine (`GET /v1/info`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineInfo {
    /// Protocol identifier, must equal [`ENGINE_PROTOCOL`].
    pub protocol: String,
    /// Engine name.
    pub name: String,
    /// Engine version.
    pub version: String,
    /// Vendor or author.
    pub vendor: String,
    /// Compute device description.
    pub device: String,
    /// Capabilities.
    pub capabilities: EngineCapabilities,
    /// Supported modalities (empty = any).
    pub modalities: Vec<String>,
    /// Labels of automatic engines.
    pub labels: Vec<EngineLabel>,
    /// Results are for research use only (e.g. non-commercial weights).
    pub research_only: bool,
    /// Licence notice shown to the user.
    pub license: String,
    /// Largest accepted volume in voxels (0 = unknown).
    pub max_voxels: u64,
    /// Session lifetime without activity in seconds (0 = unknown).
    pub session_ttl_s: u64,
}

impl EngineInfo {
    /// `true` if the engine supports `kind` in interactive sessions.
    pub fn supports(&self, kind: PromptKind) -> bool {
        self.capabilities.interactive && self.capabilities.prompts.contains(&kind)
    }
}

/// One interactive prompt in voxel indices of the uploaded grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// A voxel inside (`positive`) or outside the object.
    Point {
        /// Foreground (`true`) or background.
        positive: bool,
        /// The voxel.
        voxel: UVec3,
    },
    /// A box around the object (or around background).
    Box {
        /// Foreground (`true`) or background.
        positive: bool,
        /// Half-open voxel box.
        bx: VoxelBox,
    },
    /// Strokes: `mask` (0/1, `i` fastest) over `bx`.
    Scribble {
        /// Foreground (`true`) or background.
        positive: bool,
        /// Half-open voxel box.
        bx: VoxelBox,
        /// One byte per voxel of the box.
        mask: Vec<u8>,
    },
    /// Filled outline: `mask` (0/1, `i` fastest) over `bx`.
    Lasso {
        /// Foreground (`true`) or background.
        positive: bool,
        /// Half-open voxel box.
        bx: VoxelBox,
        /// One byte per voxel of the box.
        mask: Vec<u8>,
    },
}

impl Prompt {
    /// Kind of the prompt.
    pub fn kind(&self) -> PromptKind {
        match self {
            Prompt::Point { .. } => PromptKind::Point,
            Prompt::Box { .. } => PromptKind::Box,
            Prompt::Scribble { .. } => PromptKind::Scribble,
            Prompt::Lasso { .. } => PromptKind::Lasso,
        }
    }

    /// Foreground or background prompt.
    pub fn positive(&self) -> bool {
        match self {
            Prompt::Point { positive, .. }
            | Prompt::Box { positive, .. }
            | Prompt::Scribble { positive, .. }
            | Prompt::Lasso { positive, .. } => *positive,
        }
    }

    /// Checks the prompt against a grid of `dims` voxels: inside the grid,
    /// non-empty boxes, mask sizes matching their boxes.
    pub fn validate(&self, dims: crate::geometry::Dims3) -> Result<(), EngineError> {
        let bad = |m: String| Err(EngineError::BadRequest(m));
        match self {
            Prompt::Point { voxel, .. } => {
                if voxel.cmplt(dims.as_uvec3()).all() {
                    Ok(())
                } else {
                    bad(format!("point {voxel} is outside the volume"))
                }
            }
            Prompt::Box { bx, .. } => {
                if bx.fits(dims) {
                    Ok(())
                } else {
                    bad(format!("box {}..{} is empty or outside the volume", bx.min, bx.max))
                }
            }
            Prompt::Scribble { bx, mask, .. } | Prompt::Lasso { bx, mask, .. } => {
                if !bx.fits(dims) {
                    bad(format!("box {}..{} is empty or outside the volume", bx.min, bx.max))
                } else if mask.len() != bx.voxel_count() {
                    bad(format!("mask has {} values, the box has {} voxels", mask.len(), bx.voxel_count()))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Answer to a prompt, undo or reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptResult {
    /// Revision of the target mask; increases with every change.
    pub revision: u64,
    /// Box bounding every voxel that changed, if any.
    pub changed: Option<VoxelBox>,
    /// The target mask is empty.
    pub empty: bool,
}

/// Errors of engine operations.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum EngineError {
    /// The engine cannot be reached.
    #[error("engine unreachable: {0}")]
    Unreachable(String),
    /// The engine speaks another protocol or sent an invalid answer.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// Invalid request (e.g. a prompt outside the volume).
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Missing or wrong token.
    #[error("unauthorized")]
    Unauthorized,
    /// Unknown or expired session or job.
    #[error("session not found or expired")]
    NotFound,
    /// The volume is larger than the engine accepts.
    #[error("volume too large for the engine: {0}")]
    TooLarge(String),
    /// The engine does not support the request (prompt kind, undo, …).
    #[error("not supported by the engine: {0}")]
    Unsupported(String),
    /// The engine is at capacity.
    #[error("engine busy, retry in {retry_after_s} s")]
    Busy {
        /// Suggested wait in seconds.
        retry_after_s: u64,
    },
    /// Any other engine-side failure.
    #[error("engine error: {0}")]
    Internal(String),
}

/// State of an automatic segmentation job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Waiting to start.
    Queued,
    /// In progress.
    Running,
    /// Finished; the label map is available.
    Done,
    /// Failed (see [`JobStatus::message`]).
    Failed,
    /// Cancelled by the client.
    Cancelled,
}

impl JobState {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }

    /// Parses a wire name.
    pub fn parse(s: &str) -> Option<JobState> {
        [JobState::Queued, JobState::Running, JobState::Done, JobState::Failed, JobState::Cancelled]
            .into_iter()
            .find(|j| j.as_str() == s)
    }

    /// `true` once the job will not change any more.
    pub fn is_final(self) -> bool {
        matches!(self, JobState::Done | JobState::Failed | JobState::Cancelled)
    }
}

/// Progress of an automatic segmentation job.
#[derive(Debug, Clone, PartialEq)]
pub struct JobStatus {
    /// State.
    pub state: JobState,
    /// Completion in `[0, 1]` (best effort).
    pub progress: f32,
    /// Human-readable detail (error message for failed jobs).
    pub message: String,
}

/// A segmentation engine.
pub trait SegmentationEngine: Send + Sync {
    /// Describes the engine.
    fn info(&self) -> Result<EngineInfo, EngineError>;

    /// Starts an interactive session on `volume` (uploaded once).
    /// `modality` is the DICOM modality string (may be empty).
    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError>;
}

/// A session on one uploaded volume. Interactive engines refine one target
/// mask by prompts; automatic engines run jobs that produce a label map
/// (`capabilities.automatic`). Dropping the session frees it on the engine.
pub trait InteractiveSession: Send {
    /// Adds a prompt to the current object.
    fn prompt(&mut self, prompt: &Prompt) -> Result<PromptResult, EngineError>;

    /// Target mask over `bx` (0/1, `i` fastest).
    fn mask(&mut self, bx: VoxelBox) -> Result<Vec<u8>, EngineError>;

    /// Removes the last prompt ([`EngineError::Unsupported`] if the engine
    /// cannot undo).
    fn undo(&mut self) -> Result<PromptResult, EngineError>;

    /// Clears prompts and the target mask, keeping the volume.
    fn reset(&mut self) -> Result<(), EngineError>;

    /// Starts automatic segmentation of `labels` (names from
    /// [`EngineInfo::labels`]; `None` = all) and returns the job id.
    fn start_job(&mut self, labels: Option<&[String]>) -> Result<String, EngineError> {
        let _ = labels;
        Err(EngineError::Unsupported("automatic segmentation".into()))
    }

    /// Status of job `job`.
    fn job_status(&mut self, job: &str) -> Result<JobStatus, EngineError> {
        let _ = job;
        Err(EngineError::Unsupported("automatic segmentation".into()))
    }

    /// Cancels job `job`.
    fn cancel_job(&mut self, job: &str) -> Result<(), EngineError> {
        let _ = job;
        Err(EngineError::Unsupported("automatic segmentation".into()))
    }

    /// Label map of the last finished job: one value per voxel of the
    /// uploaded grid (`i` fastest), referring to [`EngineInfo::labels`].
    fn label_map(&mut self) -> Result<Vec<u16>, EngineError> {
        Err(EngineError::Unsupported("automatic segmentation".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Dims3;

    #[test]
    fn prompt_kinds_roundtrip() {
        for k in PromptKind::ALL {
            assert_eq!(PromptKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(PromptKind::parse("polygon"), None);
    }

    #[test]
    fn prompts_are_validated_against_the_grid() {
        let d = Dims3::new(4, 4, 2);
        let bx = VoxelBox::new(UVec3::new(1, 1, 0), UVec3::new(3, 3, 1));
        let ok = [
            Prompt::Point { positive: true, voxel: UVec3::new(3, 3, 1) },
            Prompt::Box { positive: false, bx },
            Prompt::Scribble { positive: true, bx, mask: vec![1; 4] },
            Prompt::Lasso { positive: true, bx, mask: vec![0; 4] },
        ];
        for p in &ok {
            assert!(p.validate(d).is_ok(), "{p:?}");
        }
        assert_eq!(ok.iter().map(Prompt::kind).collect::<Vec<_>>(), PromptKind::ALL.to_vec());
        assert!(ok[0].positive() && !ok[1].positive());
        let outside = VoxelBox::new(UVec3::ZERO, UVec3::new(5, 1, 1));
        let bad = [
            Prompt::Point { positive: true, voxel: UVec3::new(4, 0, 0) },
            Prompt::Box { positive: true, bx: outside },
            Prompt::Scribble { positive: true, bx, mask: vec![1; 3] },
            Prompt::Lasso { positive: true, bx: outside, mask: vec![1; 5] },
        ];
        for p in &bad {
            assert!(matches!(p.validate(d), Err(EngineError::BadRequest(_))), "{p:?}");
        }
    }

    #[test]
    fn job_states_roundtrip() {
        for s in ["queued", "running", "done", "failed", "cancelled"] {
            assert_eq!(JobState::parse(s).unwrap().as_str(), s);
        }
        assert_eq!(JobState::parse("paused"), None);
        assert!(JobState::Done.is_final() && JobState::Failed.is_final() && JobState::Cancelled.is_final());
        assert!(!JobState::Running.is_final() && !JobState::Queued.is_final());
    }

    struct Bare;

    impl InteractiveSession for Bare {
        fn prompt(&mut self, _: &Prompt) -> Result<PromptResult, EngineError> {
            Err(EngineError::NotFound)
        }
        fn mask(&mut self, _: VoxelBox) -> Result<Vec<u8>, EngineError> {
            Err(EngineError::NotFound)
        }
        fn undo(&mut self) -> Result<PromptResult, EngineError> {
            Err(EngineError::NotFound)
        }
        fn reset(&mut self) -> Result<(), EngineError> {
            Ok(())
        }
    }

    #[test]
    fn sessions_without_jobs_report_unsupported() {
        let mut b = Bare;
        let unsupported = |r: Result<(), EngineError>| matches!(r, Err(EngineError::Unsupported(_)));
        assert!(unsupported(b.start_job(None).map(|_| ())));
        assert!(unsupported(b.job_status("j").map(|_| ())));
        assert!(unsupported(b.cancel_job("j")));
        assert!(unsupported(b.label_map().map(|_| ())));
        assert!(b.prompt(&Prompt::Point { positive: true, voxel: UVec3::ZERO }).is_err());
        assert!(b.mask(VoxelBox::new(UVec3::ZERO, UVec3::ONE)).is_err() && b.undo().is_err() && b.reset().is_ok());
    }

    #[test]
    fn info_reports_support() {
        let mut info = EngineInfo::default();
        info.capabilities.prompts = vec![PromptKind::Point];
        assert!(!info.supports(PromptKind::Point), "not interactive");
        info.capabilities.interactive = true;
        assert!(info.supports(PromptKind::Point));
        assert!(!info.supports(PromptKind::Lasso));
        assert_eq!(EngineError::Busy { retry_after_s: 3 }.to_string(), "engine busy, retry in 3 s");
    }
}
