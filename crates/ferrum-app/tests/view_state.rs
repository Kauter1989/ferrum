//! The view-state apply use case (Stage 28, #70): a state becomes use cases
//! and the events it produced; the person takes over (nothing is locked).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use ferrum_app::Viewer;
use ferrum_domain::{
    Dims3, LoadedSeries, OrbitCamera, Overlays, ProgressSink, RepositoryError, SegmentStyle, SeriesDescriptor,
    SeriesMetadata, SliceAxis, StudyInfo, ViewComponent, ViewEvent, ViewState, ViewTool, Volume, VolumeRepository,
    WindowLevel,
};
use glam::Vec3;

fn phantom(dims: Dims3) -> Volume {
    let vals: Vec<f32> = (0..dims.voxel_count()).map(|i| if i % 2 == 0 { 0.0 } else { 1000.0 }).collect();
    Volume::from_physical(dims, Vec3::ONE, &vals).unwrap()
}

struct Repo;

impl VolumeRepository for Repo {
    fn name(&self) -> &str {
        "fake"
    }
    fn scan(&self, _: &[PathBuf], _: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        Ok(vec![SeriesDescriptor {
            id: "s0".into(),
            format: "fake".into(),
            description: "phantom".into(),
            modality: "CT".into(),
            dims: Dims3::new(16, 16, 12),
            sources: vec![],
            volume: None,
        }])
    }
    fn load(&self, _: &SeriesDescriptor, _: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        Ok(LoadedSeries {
            volume: phantom(Dims3::new(16, 16, 12)),
            metadata: SeriesMetadata {
                modality: "CT".into(),
                default_window: Some(WindowLevel::new(500.0, 1000.0)),
                study: StudyInfo { modality: "CT".into(), ..Default::default() },
                source: PathBuf::from("/data/phantom"),
                ..Default::default()
            },
        })
    }
}

fn loaded_viewer() -> Viewer {
    let mut v = Viewer::new(Arc::new(Repo));
    v.open_paths(vec![PathBuf::from("/data")]);
    v.wait_idle();
    assert!(v.dataset().is_some());
    v
}

// covers 28.2-a
#[test]
fn applying_a_state_produces_the_events_and_reads_back() {
    let mut v = loaded_viewer();
    let state = ViewState {
        component: ViewComponent::Volume,
        plane: SliceAxis::Coronal,
        slice_index: 5,
        window: WindowLevel::new(-600.0, 1500.0),
        overlays: Overlays { segments_shown: true, fill_opacity: 0.4, style: SegmentStyle::FillAndOutline },
        selected_segment: Some(3),
        tool: ViewTool::Probe,
        camera: Some(OrbitCamera::default()),
        probe: None,
    };
    let events = v.apply_view_state(&state);
    let kinds: Vec<&str> = events.iter().map(ViewEvent::kind).collect();
    assert!(kinds.contains(&"plane_changed"), "{kinds:?}");
    assert!(kinds.contains(&"slice_changed"), "{kinds:?}");
    assert!(kinds.contains(&"window_changed"), "{kinds:?}");
    assert!(kinds.contains(&"segments_changed"), "{kinds:?}");
    assert!(kinds.contains(&"camera_changed"), "{kinds:?}");

    let read = v.view_state();
    assert_eq!(read.component, ViewComponent::Volume);
    assert_eq!(read.plane, SliceAxis::Coronal);
    assert_eq!(read.slice_index, 5);
    assert_eq!(read.window, WindowLevel::new(-600.0, 1500.0));
    assert_eq!(read.tool, ViewTool::Probe);
    assert_eq!(read.selected_segment, Some(3));
    assert!(read.overlays.segments_shown);
    assert!(read.camera.is_some());
}

// covers 28.2-a
#[test]
fn applying_the_current_state_produces_no_events() {
    let mut v = loaded_viewer();
    let state = v.view_state();
    assert!(v.apply_view_state(&state).is_empty());
}

// covers 28.2-a
#[test]
fn the_slice_index_is_clamped_to_the_volume() {
    let mut v = loaded_viewer();
    let mut state = v.view_state();
    state.slice_index = 9999;
    let events = v.apply_view_state(&state);
    assert_eq!(v.view_state().slice_index, 11); // 12 slices, 0-based
    assert!(events.iter().any(|e| matches!(e, ViewEvent::SliceChanged { slice_index: 11, .. })));
}

// covers 28.1-a
#[test]
fn component_and_tool_round_trip_through_the_viewer() {
    let mut v = loaded_viewer();
    let mut state = v.view_state();
    state.component = ViewComponent::Mpr;
    state.tool = ViewTool::Region;
    v.apply_view_state(&state);
    let read = v.view_state();
    assert_eq!(read.component, ViewComponent::Mpr);
    assert_eq!(read.tool, ViewTool::Region);
    assert!(read.camera.is_some(), "MPR carries a camera");
}
