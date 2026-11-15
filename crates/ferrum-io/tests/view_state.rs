//! The view-state workspace artifact (Stage 28, #70): persistence, the event
//! log, and the change notification a live client applies.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use ferrum_domain::{
    Dims3, NoProgress, SliceAxis, ViewComponent, ViewEvent, ViewState, ViewTool, Volume, VolumeRepository, WindowLevel,
};
use ferrum_io::workspace::changes::{WorkspaceChange, WorkspaceWatcher};
use ferrum_io::{write_nifti, CompositeRepository, Workspace};
use glam::Vec3;

fn workspace(dir: &std::path::Path) -> Workspace {
    let src = dir.join("phantom.nii");
    let dims = Dims3::new(8, 8, 4);
    let vals: Vec<f32> = (0..dims.voxel_count()).map(|i| if i % 2 == 0 { 0.0 } else { 100.0 }).collect();
    let volume = Volume::from_physical(dims, Vec3::ONE, &vals).unwrap();
    write_nifti(&volume, &src).unwrap();
    let repo = CompositeRepository::default();
    let series = repo.scan(std::slice::from_ref(&src), &NoProgress).unwrap().remove(0);
    Workspace::create(&dir.join("ws"), &src, &series, "test").unwrap()
}

// covers 28.1-a
#[test]
fn a_view_state_round_trips_through_the_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    assert!(ws.load_view_state().unwrap().is_none(), "no state saved yet");

    let state = ViewState {
        component: ViewComponent::Mpr,
        plane: SliceAxis::Sagittal,
        slice_index: 3,
        window: WindowLevel::new(-600.0, 1500.0),
        tool: ViewTool::Distance,
        ..Default::default()
    };
    ws.save_view_state(&state, 7, "abc123").unwrap();
    let (read, revision, hash) = ws.load_view_state().unwrap().unwrap();
    assert_eq!(read, state);
    assert_eq!(revision, 7);
    assert_eq!(hash, "abc123");
}

// covers 28.1-b
#[test]
fn the_event_log_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let (events, next, dropped) = ws.load_view_events().unwrap();
    assert!(events.is_empty());
    assert_eq!((next, dropped), (0, 0));

    let log = vec![
        (1, ViewEvent::SliceChanged { plane: SliceAxis::Axial, slice_index: 4 }),
        (2, ViewEvent::WindowChanged { center: -600.0, width: 1500.0 }),
        (3, ViewEvent::RoiDrawn { item: ferrum_domain::RoiItem::Segment(2) }),
    ];
    ws.save_view_events(&log, 3, 0).unwrap();
    let (read, next, dropped) = ws.load_view_events().unwrap();
    assert_eq!(read, log);
    assert_eq!((next, dropped), (3, 0));
}

// covers 28.2-a
#[test]
fn a_view_state_save_notifies_the_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let mut watcher = WorkspaceWatcher::new(&dir.path().join("ws"));
    assert!(watcher.poll().is_empty());

    ws.save_view_state(&ViewState::default(), 1, "hash").unwrap();
    let changes = watcher.poll();
    assert!(
        changes.iter().any(|c| matches!(c, WorkspaceChange::ViewStateChanged { revision: 1 })),
        "{changes:?}"
    );

    // a second save with a new revision is reported again
    ws.save_view_state(&ViewState::default(), 2, "hash").unwrap();
    let changes = watcher.poll();
    assert!(changes.iter().any(|c| matches!(c, WorkspaceChange::ViewStateChanged { revision: 2 })), "{changes:?}");
}
