//! What each view mode offers, and the segmentation workflow: tools work on
//! slices only, the 3D view never starts a segmentation, and the built-in
//! region tool creates segments without an engine.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use ferrum_app::{InputKind, SegmentationStep, ToolKind, ToolOutcome, ViewMode, Viewer};
use ferrum_domain::{
    Author, Dims3, LoadedSeries, RepositoryError, ReviewStatus, SeriesDescriptor, SeriesMetadata, SliceAxis, Volume,
    VolumeRepository,
};
use ferrum_engines::MockEngine;
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

/// A bright ball (400 HU, radius 6 voxels of 1 mm) in air.
fn viewer() -> Viewer {
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
    let mut v = Viewer::new(Arc::new(NoRepo));
    v.install(LoadedSeries {
        volume: Volume::from_physical(dims, Vec3::ONE, &vals).unwrap(),
        metadata: SeriesMetadata { modality: "CT".into(), ..SeriesMetadata::default() },
    });
    v
}

/// A click in the middle of the axial slice through the ball's centre.
fn click_centre(v: &mut Viewer) -> ToolOutcome {
    v.set_slice_index(SliceAxis::Axial, 10);
    v.slice_input(SliceAxis::Axial, InputKind::Press, Vec2::splat(100.0), Vec2::splat(200.0))
}

#[test]
fn tools_work_on_slices_only() {
    let empty = Viewer::new(Arc::new(NoRepo));
    assert!(empty.tool_availability(ToolKind::Distance).is_err(), "nothing to measure without a study");
    assert!(matches!(empty.segmentation_step(), SegmentationStep::Unavailable(_)));

    let mut v = viewer();
    for mode in [ViewMode::Slice2d, ViewMode::Mpr] {
        v.set_view_mode(mode);
        for t in ToolKind::ALL {
            assert_eq!(v.tool_availability(t), Ok(()), "{t:?} in {mode:?}");
        }
        assert_eq!(v.tool_availability(ToolKind::Region), Ok(()), "no engine needed");
        assert!(v.tool_availability(ToolKind::AiPoint).unwrap_err().contains("engine"));
    }
    v.select_tool(ToolKind::Distance);
    v.set_view_mode(ViewMode::Volume3d);
    for t in ToolKind::ALL.into_iter().chain(ToolKind::SEGMENT) {
        let reason = v.tool_availability(t).unwrap_err();
        assert!(reason.contains("2D or MPR"), "{t:?}: {reason}");
    }
    assert!(!v.select_tool(ToolKind::Region), "no segmentation tool in 3D");
    assert_eq!(v.tool, ToolKind::Distance, "the previous tool stays for the slice views");
    assert!(v.status.message.contains("2D or MPR"), "the reason is shown");
    assert!(v.can_segment().is_err());
    assert!(matches!(v.segmentation_step(), SegmentationStep::Unavailable(r) if r.contains("2D or MPR")));
    assert!(v.grow_region_at(UVec3::new(16, 16, 10)).is_err());
    assert!(v.segmentation().set().is_none(), "nothing was created from the 3D view");
    assert!(ViewMode::Mpr.has_slices() && ViewMode::Mpr.has_volume());
    assert!(!ViewMode::Slice2d.has_volume() && !ViewMode::Volume3d.has_slices());
}

#[test]
fn the_eraser_stays_in_the_3d_view() {
    let mut v = viewer();
    v.set_view_mode(ViewMode::Volume3d);
    v.volume.eraser_enabled = true;
    v.set_view_mode(ViewMode::Mpr);
    assert!(!v.volume.eraser_enabled, "MPR's 3D cell must not erase unnoticed");
    v.volume.eraser_enabled = true;
    assert!(!v.erase_at(Vec2::ZERO, 1.0), "only the 3D view erases");
    assert!(v.mask().is_none());

    // switching layouts cancels a drawing in progress
    v.set_view_mode(ViewMode::Slice2d);
    v.select_tool(ToolKind::Distance);
    v.slice_input(SliceAxis::Axial, InputKind::Press, Vec2::splat(50.0), Vec2::splat(200.0));
    assert!(v.annotation_preview().is_some());
    v.set_view_mode(ViewMode::Volume3d);
    assert!(v.annotation_preview().is_none());
}

#[test]
fn the_region_tool_segments_without_an_engine() {
    let mut v = viewer();
    assert_eq!(v.segmentation_step(), SegmentationStep::ChooseTool);
    assert!(v.select_tool(ToolKind::Region));
    assert_eq!(v.segmentation_step(), SegmentationStep::Draw(ToolKind::Region));
    let tolerance = v.segmentation().region.tolerance;
    assert_eq!(tolerance, 40.0, "a tenth of the 1 400 wide window, capped");

    let out = click_centre(&mut v);
    assert!(matches!(out, ToolOutcome::Seed(s) if s == UVec3::new(16, 16, 10)), "{out:?}");
    let rows = v.segment_summaries();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].segment.name, "Region 1");
    // the ball: (4/3)·π·6³ ≈ 905 voxels of 1 mm³, 895 on this grid; the default opening
    // (5 mm) is wider than the 12 mm ball is thick and rounds it off a little
    assert_eq!(rows[0].voxels, 799);
    let p = &rows[0].segment.provenance;
    assert_eq!((p.author.clone(), p.status), (Author::Human, ReviewStatus::Confirmed), "drawn by the user");
    assert!(v.status.message.starts_with("Region 1: 0.8 ml"), "{}", v.status.message);
    assert!(v.segmentation().show, "the result is shown");

    // a second click in the same structure explains instead of duplicating it
    click_centre(&mut v);
    assert_eq!(v.segment_summaries().len(), 1);
    assert!(v.status.message.contains("already belongs to Region 1"), "{}", v.status.message);

    // the air around leaks beyond a small limit
    v.region_settings_mut().max_ml = 5.0;
    let err = v.grow_region_at(UVec3::new(1, 1, 1)).unwrap_err();
    assert!(err.contains("beyond 5 ml") && err.contains("tolerance"), "{err}");
    assert_eq!(v.segment_summaries().len(), 1);
    v.region_settings_mut().max_ml = 1000.0;
    let air = v.grow_region_at(UVec3::new(1, 1, 1)).unwrap();
    assert_eq!(v.segment_summaries().iter().find(|r| r.segment.label == air).unwrap().segment.name, "Region 2");

    // undo takes the voxels back; a new study starts the numbering again
    assert!(v.undo_segmentation());
    assert_eq!(v.segmentation().set().unwrap().voxel_count(air), 0);
    let mut fresh = viewer();
    fresh.grow_region_at(UVec3::new(16, 16, 10)).unwrap();
    assert_eq!(fresh.segment_summaries()[0].segment.name, "Region 1");
}

#[test]
fn engines_never_start_from_the_3d_view() {
    let mut v = viewer();
    v.connect_engine(Arc::new(MockEngine::default()), "mock");
    v.wait_ai_idle();
    assert_eq!(v.tool_availability(ToolKind::AiPoint), Ok(()));
    assert!(v.select_tool(ToolKind::AiPoint));

    v.set_view_mode(ViewMode::Volume3d);
    assert!(!v.ai_run_automatic(None), "no automatic run from 3D");
    assert!(!v.ai_prompt(ferrum_domain::Prompt::Point { positive: true, voxel: UVec3::new(16, 16, 10) }));
    assert!(v.segmentation().set().is_none() && v.ai().job().is_none());

    v.set_view_mode(ViewMode::Mpr);
    click_centre(&mut v);
    assert_eq!(v.segmentation_step(), SegmentationStep::Working);
    v.wait_ai_idle();
    assert_eq!(v.segmentation_step(), SegmentationStep::ReviewObject);
    v.ai_accept();
    v.wait_ai_idle();
    assert_eq!(v.segmentation_step(), SegmentationStep::Draw(ToolKind::AiPoint));
    assert!(v.ai_run_automatic(None), "automatic runs from the slice views");
    v.wait_ai_idle();
    assert!(v.segment_summaries().len() >= 2);
}

// covers 16.6-e
// covers 16.6-f
#[test]
fn region_settings_keep_the_edge_and_the_failures_are_told_apart() {
    let seed = UVec3::new(16, 16, 10);
    assert!((v_default_limit() - 8000.0).abs() < f32::EPSILON, "a lung is about 6 000 ml");
    for (smoothing_mm, opening_mm, fill_holes, expected) in
        [(0.0, 0.0, false, 895), (1.0, 0.0, false, 895), (1.0, 0.0, true, 895), (1.0, 1.5, true, 871)]
    {
        let mut v = viewer();
        v.select_tool(ToolKind::Region);
        let r = v.region_settings_mut();
        (r.smoothing_mm, r.opening_mm, r.fill_holes) = (smoothing_mm, opening_mm, fill_holes);
        let label = v.grow_region_at(seed).unwrap();
        let n = v.segmentation().set().unwrap().voxel_count(label);
        assert_eq!(n, expected, "smoothing {smoothing_mm} mm, opening {opening_mm} mm");
    }
    let mut v = viewer();
    v.select_tool(ToolKind::Region);
    v.region_settings_mut().opening_mm = 2.0;
    v.region_settings_mut().max_ml = 0.5;
    let err = v.grow_region_at(seed).unwrap_err();
    assert!(err.contains("beyond 0 ml") || err.contains("beyond 1 ml"), "{err}");
    assert!(err.contains("opening"), "{err}");
    assert!(v.grow_region_at(UVec3::splat(99)).unwrap_err().contains("outside"));
}

fn v_default_limit() -> f32 {
    ferrum_app::RegionSettings::default().max_ml
}

// covers 16.6-l
#[test]
fn the_default_tolerance_follows_the_window_but_is_capped() {
    use ferrum_domain::WindowLevel;
    let tol = |w: f32| ferrum_app::RegionSettings::for_window(WindowLevel::new(40.0, w)).tolerance;
    assert_eq!(tol(200.0), 20.0, "a tenth of a narrow window");
    assert_eq!(tol(400.0), 40.0);
    assert_eq!(tol(1500.0), 40.0, "the automatic window of a whole CT would leak");
}
