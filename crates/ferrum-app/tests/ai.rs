//! AI segmentation use cases against the in-process mock engine (and a
//! failing remote one), without GPU or UI.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ferrum_app::{AiStatus, InputKind, ToolKind, ToolOutcome, Viewer};
use ferrum_domain::{
    Dims3, EngineError, EngineInfo, InteractiveSession, LoadedSeries, Prompt, PromptKind, RepositoryError,
    SegmentationEngine, SeriesDescriptor, SeriesMetadata, SliceAxis, Volume, VolumeRepository,
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
    v.select_tool(ToolKind::AiPoint);
    let out = v.slice_input(SliceAxis::Axial, InputKind::Press, Vec2::splat(100.0), Vec2::splat(200.0));
    assert!(matches!(out, ToolOutcome::Prompt(_)), "the tool still draws");
    assert!(v.segmentation().set().is_none(), "but nothing is sent or created");
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
    assert!(matches!(v.ai().status(), AiStatus::Failed(m) if m.contains("no interactive")));
}
