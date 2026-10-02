//! AI segmentation use cases against the in-process mock engine (and a
//! failing remote one), without GPU or UI.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ferrum_app::{AiStatus, InputKind, ToolKind, ToolOutcome, Viewer};
use ferrum_domain::{
    Author, Dims3, EngineError, EngineInfo, InteractiveSession, LoadedSeries, Prompt, PromptKind, RepositoryError,
    ReviewStatus, SegmentationEngine, SeriesDescriptor, SeriesMetadata, SliceAxis, Volume, VolumeRepository,
};
use ferrum_engines::{HttpConfig, HttpEngine, MockEngine};
use glam::{UVec3, Vec2, Vec3};

struct NoRepo;

impl VolumeRepository for NoRepo {
    fn name(&self) -> &str {
        "none"
    }
    fn scan(
        &self,
        _: &[PathBuf],
        _: &dyn ferrum_domain::ProgressSink,
    ) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        Err(RepositoryError::NothingFound)
    }
    fn load(&self, _: &SeriesDescriptor, _: &dyn ferrum_domain::ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        Err(RepositoryError::NothingFound)
    }
}

/// A bright ball (400 HU) in air.
fn ball() -> Volume {
    let dims = Dims3::new(32, 32, 20);
    let c = Vec3::new(16.0, 16.0, 10.0);
    let vals: Vec<f32> = (0..dims.voxel_count())
        .map(|n| {
            let p = Vec3::new((n % 32) as f32, ((n / 32) % 32) as f32, (n / 1024) as f32);
            if (p - c).length() < 6.0 {
                400.0
            } else {
                -1000.0
            }
        })
        .collect();
    Volume::from_physical(dims, Vec3::ONE, &vals).unwrap()
}

fn viewer() -> Viewer {
    let mut v = Viewer::new(Arc::new(NoRepo));
    v.install(LoadedSeries {
        volume: ball(),
        metadata: SeriesMetadata { modality: "CT".into(), ..SeriesMetadata::default() },
    });
    v
}

/// Mock engine that counts sessions and can drop features.
struct Counting {
    sessions: AtomicUsize,
    interactive: bool,
}

impl SegmentationEngine for Counting {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        let mut i = MockEngine::describe();
        i.capabilities.interactive = self.interactive;
        i.capabilities.automatic = self.interactive;
        i.research_only = true;
        Ok(i)
    }
    fn open_session(&self, v: &Volume, m: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        self.sessions.fetch_add(1, Ordering::SeqCst);
        MockEngine::default().open_session(v, m)
    }
}

fn voxels(v: &Viewer, label: u8) -> u64 {
    v.segmentation().set().map_or(0, |s| s.voxel_count(label))
}

const CENTRE: UVec3 = UVec3::new(16, 16, 10);

#[test]
fn tools_are_inactive_without_an_engine() {
    let mut v = viewer();
    assert_eq!(*v.ai().status(), AiStatus::Disconnected);
    assert!(PromptKind::ALL.iter().all(|k| !v.ai().supports(*k)));
    assert!(!v.ai_prompt(Prompt::Point { positive: true, voxel: CENTRE }));
    let reason = v.tool_availability(ToolKind::AiPoint).unwrap_err();
    assert!(reason.contains("connect"), "{reason}");
    assert!(!v.select_tool(ToolKind::AiPoint), "AI tools cannot be chosen without an engine");
    assert_eq!((v.tool, v.status.message.as_str()), (ToolKind::Pan, reason.as_str()), "the reason is shown");
    let out = v.slice_input(SliceAxis::Axial, InputKind::Press, Vec2::splat(100.0), Vec2::splat(200.0));
    assert!(!matches!(out, ToolOutcome::Prompt(_)));
    assert!(v.segmentation().set().is_none(), "nothing is sent or created");
    assert!(!v.ai_undo());
    v.ai_accept();
    v.ai_discard();
}

#[test]
fn prompts_include_exclude_undo_accept_and_discard() {
    let mut v = viewer();
    let engine = Arc::new(Counting { sessions: AtomicUsize::new(0), interactive: true });
    v.connect_engine(engine.clone(), "mock");
    v.wait_ai_idle();
    assert_eq!(*v.ai().status(), AiStatus::Connected);
    assert_eq!(v.ai().engine_label(), "mock");
    assert!(v.ai().research_only());
    assert!(PromptKind::ALL.iter().all(|k| v.ai().supports(*k)));

    // include: the ball is segmented into a new target segment
    assert!(v.ai_prompt(Prompt::Point { positive: true, voxel: CENTRE }));
    assert_eq!(*v.ai().status(), AiStatus::Uploading);
    v.wait_ai_idle();
    assert_eq!(*v.ai().status(), AiStatus::Ready);
    let first = v.ai().target().unwrap();
    let ball_voxels = voxels(&v, first);
    assert!(ball_voxels > 500, "{ball_voxels}");
    assert_eq!(v.segment_summaries()[0].segment.name, "AI segment 1");

    // exclude removes it; undo brings it back
    v.set_ai_positive(false);
    assert!(v.ai_prompt(Prompt::Point { positive: false, voxel: CENTRE }));
    v.wait_ai_idle();
    assert_eq!(voxels(&v, first), 0);
    assert!(v.ai().can_undo());
    assert!(v.ai_undo());
    v.wait_ai_idle();
    assert_eq!(voxels(&v, first), ball_voxels);
    v.set_ai_positive(true);

    // accept keeps the segment and starts a new object
    v.ai_accept();
    assert_eq!(v.ai().target(), None);
    assert!(v.ai_prompt(Prompt::Point { positive: true, voxel: CENTRE }));
    v.wait_ai_idle();
    let second = v.ai().target().unwrap();
    assert_ne!(first, second);
    assert_eq!(voxels(&v, first), ball_voxels, "the accepted segment is unchanged");
    assert_eq!(voxels(&v, second), 0, "the ball is taken; segments are not overwritten");

    // discard removes the current object's segment
    v.ai_discard();
    assert!(v.segmentation().set().unwrap().segment(second).is_none());
    assert_eq!(engine.sessions.load(Ordering::SeqCst), 1, "one upload per volume");
}

#[test]
fn ai_results_are_proposals_until_accepted() {
    let mut v = viewer();
    v.connect_engine(Arc::new(Counting { sessions: AtomicUsize::new(0), interactive: true }), "mock");
    v.wait_ai_idle();
    assert!(v.ai_prompt(Prompt::Point { positive: true, voxel: CENTRE }));
    v.wait_ai_idle();
    let target = v.ai().target().unwrap();
    let provenance = |v: &Viewer| v.segmentation().set().unwrap().segment(target).unwrap().provenance.clone();
    let p = provenance(&v);
    assert_eq!((p.author.kind(), p.status), ("engine", ReviewStatus::Proposed), "AI results are proposals");
    assert!(matches!(p.author, Author::Engine { research_only: true, .. }));
    assert!(p.created.is_some());
    v.ai_accept();
    let p = provenance(&v);
    assert_eq!((p.author.kind(), p.status), ("engine", ReviewStatus::Confirmed), "the author stays the engine");
    assert!(p.reviewed.is_some());
}

#[test]
fn sessions_follow_the_dataset_and_disconnect_cleans_up() {
    let mut v = viewer();
    let engine = Arc::new(Counting { sessions: AtomicUsize::new(0), interactive: true });
    v.connect_engine(engine.clone(), "mock");
    v.wait_ai_idle();
    assert!(v.ai_prompt(Prompt::Point { positive: true, voxel: CENTRE }));
    v.wait_ai_idle();
    assert_eq!(engine.sessions.load(Ordering::SeqCst), 1);

    // a new dataset needs a new session
    v.install(LoadedSeries { volume: ball(), metadata: SeriesMetadata::default() });
    assert!(v.ai_prompt(Prompt::Point { positive: true, voxel: CENTRE }));
    v.wait_ai_idle();
    assert_eq!(engine.sessions.load(Ordering::SeqCst), 2);

    // invalid prompts are rejected locally
    assert!(!v.ai_prompt(Prompt::Point { positive: true, voxel: UVec3::new(99, 0, 0) }));

    v.disconnect_engine();
    assert_eq!(*v.ai().status(), AiStatus::Disconnected);
    assert!(!v.ai().supports(PromptKind::Point));
}

#[test]
fn slice_tools_send_prompts() {
    let mut v = viewer();
    v.connect_engine(Arc::new(MockEngine::default()), "mock");
    v.wait_ai_idle();
    v.select_tool(ToolKind::AiPoint);
    v.set_slice_index(SliceAxis::Axial, 10);
    // the default view fits the 32×32 mm image into the 200×200 viewport
    let out = v.slice_input(SliceAxis::Axial, InputKind::Press, Vec2::splat(100.0), Vec2::splat(200.0));
    assert!(matches!(out, ToolOutcome::Prompt(Prompt::Point { voxel, .. }) if voxel == CENTRE));
    v.wait_ai_idle();
    assert!(voxels(&v, v.ai().target().unwrap()) > 100);
    v.select_tool(ToolKind::AiPoint);
    v.disconnect_engine();
    assert_eq!(v.tool, ToolKind::Pan, "AI tools are deselected on disconnect");
}

#[test]
fn engine_failures_are_reported() {
    let mut v = viewer();
    // nothing listens on port 9 of localhost
    v.connect_engine(Arc::new(HttpEngine::new(HttpConfig::new("http://127.0.0.1:9"))), "http://127.0.0.1:9");
    v.wait_ai_idle();
    assert!(matches!(v.ai().status(), AiStatus::Failed(m) if m.contains("unreachable")), "{:?}", v.ai().status());
    assert!(!v.status.errors.is_empty());
    assert!(!v.ai_prompt(Prompt::Point { positive: true, voxel: CENTRE }));

    let mut v = viewer();
    v.connect_engine(Arc::new(Counting { sessions: AtomicUsize::new(0), interactive: false }), "auto");
    v.wait_ai_idle();
    assert!(matches!(v.ai().status(), AiStatus::Failed(m) if m.contains("neither interactive nor automatic")));
}

#[test]
fn automatic_segmentation_creates_named_segments() {
    let mut v = viewer();
    v.connect_engine(Arc::new(MockEngine::default()), "mock");
    v.wait_ai_idle();
    assert!(v.ai().supports_automatic());
    assert!(v.ai_run_automatic(None));
    assert!(!v.ai_run_automatic(None), "one job at a time");
    v.wait_ai_idle();
    assert!(v.ai().job().is_none());
    let rows = v.segment_summaries();
    assert_eq!(rows.len(), 1, "only structures that were found become segments");
    assert_eq!(rows[0].segment.name, "bright");
    assert!(rows[0].segment.provenance.is_pending(), "automatic results wait for review");
    assert_eq!(v.segmentation().set().unwrap().pending(), 1);
    let label = rows[0].segment.label;
    v.review_segment(label, ReviewStatus::Rejected, Some("dr.k")).unwrap();
    let p = &v.segment_summaries()[0].segment.provenance;
    assert_eq!((p.status, p.reviewed_by.as_deref()), (ReviewStatus::Rejected, Some("dr.k")));
    assert!(p.reviewed.is_some());
    assert!(v.review_segment(200, ReviewStatus::Confirmed, None).is_err());
    assert!(rows[0].voxels > 500);
    assert!(v.status.message.contains("1 structure"), "{}", v.status.message);
    assert_eq!(*v.ai().status(), AiStatus::Ready);

    // a run for a label that is absent adds nothing; a rerun keeps existing voxels
    assert!(v.ai_run_automatic(Some(vec!["intermediate".into()])));
    v.wait_ai_idle();
    assert_eq!(v.segment_summaries().len(), 1);
    assert!(v.ai_run_automatic(None));
    v.wait_ai_idle();
    let rows = v.segment_summaries();
    assert_eq!(rows.len(), 2, "a second 'bright' segment");
    assert_eq!(rows[1].voxels, 0, "voxels already segmented are kept");
    assert!(v.can_undo_segmentation());

    assert!(
        !v.ai_run_automatic(Some(vec!["liver".into()])) || {
            v.wait_ai_idle();
            v.status.errors.iter().any(|e| e.contains("unknown label"))
        }
    );
}

/// Automatic-only engine whose jobs run until cancelled, or fail.
struct SlowEngine {
    fail: bool,
}

struct SlowSession {
    fail: bool,
    cancelled: bool,
}

impl InteractiveSession for SlowSession {
    fn prompt(&mut self, _: &Prompt) -> Result<ferrum_domain::PromptResult, EngineError> {
        Err(EngineError::Unsupported("prompts".into()))
    }
    fn mask(&mut self, _: ferrum_domain::VoxelBox) -> Result<Vec<u8>, EngineError> {
        Err(EngineError::Unsupported("masks".into()))
    }
    fn undo(&mut self) -> Result<ferrum_domain::PromptResult, EngineError> {
        Err(EngineError::Unsupported("undo".into()))
    }
    fn reset(&mut self) -> Result<(), EngineError> {
        Ok(())
    }
    fn start_job(&mut self, _: Option<&[String]>) -> Result<String, EngineError> {
        Ok("j1".into())
    }
    fn job_status(&mut self, _: &str) -> Result<ferrum_domain::JobStatus, EngineError> {
        use ferrum_domain::{JobState, JobStatus};
        let state = match (self.fail, self.cancelled) {
            (true, _) => JobState::Failed,
            (_, true) => JobState::Cancelled,
            _ => JobState::Running,
        };
        Ok(JobStatus { state, progress: 0.5, message: if self.fail { "out of memory".into() } else { "liver".into() } })
    }
    fn cancel_job(&mut self, _: &str) -> Result<(), EngineError> {
        self.cancelled = true;
        Ok(())
    }
}

impl SegmentationEngine for SlowEngine {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        let mut i = MockEngine::describe();
        i.capabilities.interactive = false;
        i.capabilities.prompts.clear();
        Ok(i)
    }
    fn open_session(&self, _: &Volume, _: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        Ok(Box::new(SlowSession { fail: self.fail, cancelled: false }))
    }
}

#[test]
fn automatic_jobs_report_progress_and_can_be_cancelled() {
    let mut v = viewer();
    v.connect_engine(Arc::new(SlowEngine { fail: false }), "slow");
    v.wait_ai_idle();
    assert!(v.ai().status().is_connected(), "automatic-only engines are accepted");
    assert!(!v.ai().supports(PromptKind::Point));
    assert!(v.ai_run_automatic(None));
    let start = std::time::Instant::now();
    while v.ai().job().is_none_or(|j| j.progress < 0.5) {
        assert!(start.elapsed().as_secs() < 10, "no progress reported");
        v.poll();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(v.ai().job().unwrap().message, "liver");
    v.ai_cancel_job();
    v.wait_ai_idle();
    assert!(v.ai().job().is_none());
    assert!(v.status.message.contains("cancelled"), "{}", v.status.message);

    let mut v = viewer();
    v.connect_engine(Arc::new(SlowEngine { fail: true }), "failing");
    v.wait_ai_idle();
    assert!(v.ai_run_automatic(None));
    v.wait_ai_idle();
    assert!(matches!(v.ai().status(), AiStatus::Failed(m) if m.contains("out of memory")));
    v.ai_cancel_job(); // no job: nothing happens
}
