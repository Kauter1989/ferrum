//! Use-case tests of the application layer with an in-memory repository
//! and a recording GPU sink (no files, no GPU, no UI).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use ferrum_app::{FilterKind, GpuSink, GpuSyncState, InputKind, ToolKind, ToolOutcome, ViewMode, Viewer};
use ferrum_domain::{
    Annotation, Dims3, LabelMap, LoadedSeries, ProgressSink, RenderMode, RepositoryError, Rgba8, SeriesDescriptor,
    SeriesMetadata, SliceAxis, SliceKey, StudyInfo, TransferFunction, Volume, VolumeRepository, VoxelBox, VoxelMask,
    WindowLevel, WindowPreset,
};
use ferrum_processing::AmbientOcclusion;
use glam::{UVec3, Vec2, Vec3};

/// Sphere phantom: value 1000 inside radius 0.35 of the box, 0 outside.
fn phantom(dims: Dims3) -> Volume {
    let mut vals = Vec::with_capacity(dims.voxel_count());
    let c = (dims.as_vec3() - Vec3::ONE) * 0.5;
    for k in 0..dims.z {
        for j in 0..dims.y {
            for i in 0..dims.x {
                let r = ((Vec3::new(i as f32, j as f32, k as f32) - c) / dims.as_vec3()).length();
                vals.push(if r < 0.35 { 1000.0 } else { 0.0 });
            }
        }
    }
    Volume::from_physical(dims, Vec3::ONE, &vals).unwrap()
}

struct FakeRepo {
    series: Vec<(SeriesDescriptor, Volume)>,
    fail: bool,
}

impl FakeRepo {
    fn new(n: usize) -> Arc<Self> {
        let series = (0..n)
            .map(|i| {
                let dims = Dims3::new(24 + i as u32 * 8, 24, 20);
                let d = SeriesDescriptor {
                    id: format!("s{i}"),
                    format: "fake".into(),
                    description: format!("series {i}"),
                    modality: "CT".into(),
                    dims,
                    sources: vec![],
                };
                (d, phantom(dims))
            })
            .collect();
        Arc::new(Self { series, fail: false })
    }
}

impl VolumeRepository for FakeRepo {
    fn name(&self) -> &str {
        "fake"
    }
    fn scan(&self, _paths: &[PathBuf], p: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        p.report(1.0, "scanned");
        if self.fail {
            return Err(RepositoryError::NothingFound);
        }
        Ok(self.series.iter().map(|(d, _)| d.clone()).collect())
    }
    fn load(&self, s: &SeriesDescriptor, _p: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        let (_, v) = self.series.iter().find(|(d, _)| d.id == s.id).ok_or(RepositoryError::NothingFound)?;
        Ok(LoadedSeries {
            volume: v.clone(),
            metadata: SeriesMetadata {
                modality: "CT".into(),
                description: s.description.clone(),
                default_window: Some(WindowLevel::new(500.0, 1000.0)),
                attributes: vec![],
                study: StudyInfo { study_date: "20240428".into(), modality: "CT".into(), ..Default::default() },
                source: PathBuf::from("/data/phantom"),
            },
        })
    }
}

#[derive(Default, Debug)]
struct RecordingSink {
    log: Vec<String>,
}

impl GpuSink for RecordingSink {
    fn upload_volume(&mut self, v: &Volume) {
        self.log.push(format!("volume {:?}", v.dims()));
    }
    fn upload_transfer_function(&mut self, lut: &[Rgba8]) {
        assert_eq!(lut.len(), 256);
        self.log.push("tf".into());
    }
    fn upload_occupancy(&mut self, grid: Dims3, occ: &[u8]) {
        assert_eq!(grid.voxel_count(), occ.len());
        self.log.push("occupancy".into());
    }
    fn upload_mask(&mut self, _m: &VoxelMask, dirty: Option<(UVec3, UVec3)>) {
        self.log.push(if dirty.is_some() { "mask partial".into() } else { "mask full".into() });
    }
    fn clear_mask(&mut self) {
        self.log.push("mask clear".into());
    }
    fn upload_ambient_occlusion(&mut self, ao: Option<&AmbientOcclusion>) {
        self.log.push(if ao.is_some() { "ao".into() } else { "ao none".into() });
    }
    fn upload_labels(&mut self, labels: Option<&LabelMap>, dirty: Option<VoxelBox>) {
        self.log.push(match (labels, dirty) {
            (None, _) => "labels none".into(),
            (Some(_), None) => "labels full".into(),
            (Some(_), Some(b)) => format!("labels {:?}..{:?}", b.min.to_array(), b.max.to_array()),
        });
    }
    fn upload_segment_colors(&mut self, lut: &[Rgba8; 256]) {
        self.log.push(format!("segment colours {:?}", lut[1]));
    }
}

impl RecordingSink {
    fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }
}

fn loaded_viewer() -> Viewer {
    let mut v = Viewer::new(FakeRepo::new(1));
    v.open_paths(vec![PathBuf::from("/data")]);
    v.wait_idle();
    assert!(v.dataset().is_some(), "{:?}", v.status);
    v
}

#[test]
fn single_series_is_loaded_automatically() {
    let v = loaded_viewer();
    let d = v.dataset().unwrap();
    assert_eq!(d.volume.dims(), Dims3::new(24, 24, 20));
    assert_eq!(v.slices.indices, [12, 12, 10]);
    assert_eq!(v.slices.window, WindowLevel::new(500.0, 1000.0));
    assert!(v.status.message.contains("Loaded"));
    assert!(!v.is_loading());
}

#[test]
fn multiple_series_require_a_choice() {
    let mut v = Viewer::new(FakeRepo::new(3));
    v.open_paths(vec![PathBuf::from("/data")]);
    v.wait_idle();
    let choice = v.series_choice.clone().expect("choice offered");
    assert_eq!(choice.len(), 3);
    assert!(v.dataset().is_none());
    v.load_series(choice[2].clone());
    v.wait_idle();
    assert_eq!(v.dataset().unwrap().volume.dims().x, 40);
    assert!(v.series_choice.is_none());
}

#[test]
fn repository_errors_are_reported() {
    let repo = Arc::new(FakeRepo { series: vec![], fail: true });
    let mut v = Viewer::new(repo);
    v.open_paths(vec![PathBuf::from("/nothing")]);
    v.wait_idle();
    assert!(v.dataset().is_none());
    assert_eq!(v.status.errors.len(), 1);
    assert!(v.status.message.contains("failed"));
}

#[test]
fn gpu_sync_uploads_only_what_changed() {
    let mut v = loaded_viewer();
    let mut sync = GpuSyncState::default();
    let mut sink = RecordingSink::default();
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["volume Dims3 { x: 24, y: 24, z: 20 }", "tf", "occupancy", "ao none", "labels none"]);
    v.sync_gpu(&mut sync, &mut sink);
    assert!(sink.take().is_empty());

    // TF edits upload the LUT; occupancy only depends on it in TF mode.
    v.edit_transfer_function(|tf| tf.move_point(2, 0.2, 0.9).unwrap());
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["tf"]);
    v.volume.settings.mode = RenderMode::TransferFunction;
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["occupancy"]);
    v.set_transfer_function(TransferFunction::bone(0.5));
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["tf", "occupancy"]);

    // Non-structural settings do not cause uploads.
    v.volume.settings.opacity = 0.1;
    v.volume.settings.brightness = 0.9;
    v.sync_gpu(&mut sync, &mut sink);
    assert!(sink.take().is_empty());
}

#[test]
fn eraser_uploads_mask_incrementally_and_supports_undo() {
    let mut v = loaded_viewer();
    v.volume.settings.mode = RenderMode::Isosurface;
    v.volume.settings.iso_threshold = 0.5;
    v.volume.brush.radius_mm = 3.0;
    v.volume.brush.depth_mm = 4.0;
    let mut sync = GpuSyncState::default();
    let mut sink = RecordingSink::default();
    v.sync_gpu(&mut sync, &mut sink);
    sink.take();

    assert!(v.pick(Vec2::ZERO, 1.0).is_some());
    assert!(!v.erase_at(Vec2::new(0.99, 0.99), 1.0), "corner ray misses the sphere");
    assert!(v.erase_at(Vec2::ZERO, 1.0));
    let erased = v.mask().unwrap().erased_count();
    assert!(erased > 0);
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["mask full"]);

    assert!(v.erase_at(Vec2::new(0.3, 0.0), 1.0));
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["mask partial"]);
    assert_eq!(v.erase_history_len(), 2);

    assert!(v.undo_erase());
    assert!(v.undo_erase());
    assert!(!v.undo_erase());
    assert_eq!(v.mask().unwrap().erased_count(), 0);
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["mask partial"]);

    v.reset_mask();
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["mask clear"]);
}

#[test]
fn ambient_occlusion_is_computed_in_background() {
    let mut v = loaded_viewer();
    v.volume.settings.mode = RenderMode::Isosurface;
    v.volume.settings.ambient_occlusion = true;
    v.poll();
    assert!(v.is_computing());
    v.wait_idle();
    assert!(v.frame_params(Vec2::splat(64.0)).unwrap().ao_enabled);
    let mut sync = GpuSyncState::default();
    let mut sink = RecordingSink::default();
    v.sync_gpu(&mut sync, &mut sink);
    assert!(sink.take().contains(&"ao".to_string()));
    // Disabling AO removes it from the GPU.
    v.volume.settings.ambient_occlusion = false;
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["ao none"]);
}

#[test]
fn slice_navigation_and_windowing() {
    let mut v = loaded_viewer();
    v.step_slice(SliceAxis::Axial, 100);
    assert_eq!(v.slices.index(SliceAxis::Axial), 19);
    v.step_slice(SliceAxis::Axial, -100);
    assert_eq!(v.slices.index(SliceAxis::Axial), 0);
    v.apply_window_preset(WindowPreset::Bone);
    assert_eq!(v.slices.window, WindowLevel::new(400.0, 1800.0));
    v.auto_window();
    assert!(v.slices.window.width > 0.0);
}

#[test]
fn mpr_navigation_moves_other_slices() {
    let mut v = loaded_viewer();
    v.view_mode = ViewMode::Mpr;
    let viewport = Vec2::new(240.0, 240.0);
    // Axial image is 24×24 mm fitted into 240 pt → 10 pt per voxel.
    v.navigate_to(SliceAxis::Axial, Vec2::new(35.0, 185.0), viewport);
    assert_eq!(v.slices.index(SliceAxis::Sagittal), 3);
    assert_eq!(v.slices.index(SliceAxis::Coronal), 18);
    assert_eq!(v.slices.index(SliceAxis::Axial), 10);
}

#[test]
fn measuring_on_screen_produces_millimetres() {
    let mut v = loaded_viewer();
    v.select_tool(ToolKind::Distance);
    let viewport = Vec2::new(240.0, 240.0);
    v.slice_input(SliceAxis::Axial, InputKind::Press, Vec2::new(20.0, 20.0), viewport);
    v.slice_input(SliceAxis::Axial, InputKind::Drag { delta: Vec2::ZERO }, Vec2::new(120.0, 20.0), viewport);
    assert!(v.annotation_preview().is_some());
    let out = v.slice_input(SliceAxis::Axial, InputKind::Release, Vec2::new(120.0, 20.0), viewport);
    let ToolOutcome::Committed(id) = out else { panic!("{out:?}") };
    let a = v.annotations().get(id).unwrap();
    assert!((a.value().unwrap() - 10.0).abs() < 1e-3, "100 pt at 10 pt/mm");
    let key = SliceKey::new(SliceAxis::Axial, 10);
    assert_eq!(v.annotations().on_slice(key).count(), 1);
    v.clear_annotations();
    assert!(v.annotations().is_empty());
}

#[test]
fn text_annotation_flow() {
    let mut v = loaded_viewer();
    v.select_tool(ToolKind::Text);
    let vp = Vec2::new(240.0, 240.0);
    v.slice_input(SliceAxis::Axial, InputKind::Press, Vec2::new(50.0, 50.0), vp);
    assert!(v.pending_text.is_some());
    assert!(v.commit_text("note").is_some());
    assert!(v.pending_text.is_none());
    assert_eq!(v.annotations().len(), 1);
}

#[test]
fn probe_reports_physical_values() {
    let mut v = loaded_viewer();
    v.select_tool(ToolKind::Probe);
    let vp = Vec2::new(240.0, 240.0);
    v.slice_input(SliceAxis::Axial, InputKind::Hover, Vec2::new(120.0, 120.0), vp);
    let p = v.probe.unwrap();
    assert_eq!(p.voxel, UVec3::new(12, 12, 10));
    assert!((p.value - 1000.0).abs() < 0.1);
}

#[test]
fn slice_params_fit_image_into_view() {
    let v = loaded_viewer();
    let p = v.slice_params(SliceAxis::Axial, (Vec2::new(100.0, 50.0), Vec2::new(500.0, 250.0)), 2.0).unwrap();
    // 400×200 px = 200×100 pt; 24×24 mm image fits to 100×100 pt, centred
    assert_eq!(p.rect.0, Vec2::new(100.0 + 100.0, 50.0));
    assert_eq!(p.rect.1, Vec2::new(100.0 + 300.0, 250.0));
    assert_eq!(p.axis, SliceAxis::Axial.id());
}

#[test]
fn filters_replace_the_dataset() {
    let mut v = loaded_viewer();
    let rev = v.dataset().unwrap().revision;
    let mut sync = GpuSyncState::default();
    let mut sink = RecordingSink::default();
    v.sync_gpu(&mut sync, &mut sink);
    sink.take();
    v.apply_filter(FilterKind::Gaussian(1.0));
    v.wait_idle();
    assert!(v.dataset().unwrap().revision > rev);
    v.sync_gpu(&mut sync, &mut sink);
    let log = sink.take();
    assert!(log[0].starts_with("volume"), "{log:?}");
}

#[test]
fn reloading_resets_view_state() {
    let mut v = loaded_viewer();
    v.volume.camera.rotate(1.0, 1.0);
    v.slices.views[2].zoom = 4.0;
    v.select_tool(ToolKind::Distance);
    v.slice_input(SliceAxis::Axial, InputKind::Press, Vec2::ZERO, Vec2::splat(100.0));
    v.open_paths(vec![PathBuf::from("/again")]);
    v.wait_idle();
    assert_eq!(v.slices.views[2].zoom, 1.0);
    assert!(v.annotation_preview().is_none());
    assert_eq!(v.volume.camera, ferrum_domain::OrbitCamera::default());
}

#[test]
fn annotations_are_named_listed_navigated_and_reported() {
    let mut v = loaded_viewer();
    v.view_mode = ViewMode::Slice2d;
    let a = v.add_annotation(SliceKey::new(SliceAxis::Coronal, 5), Annotation::Distance { a: Vec2::ZERO, b: Vec2::X });
    let b = v.add_annotation(SliceKey::new(SliceAxis::Axial, 3), Annotation::Rect { a: Vec2::ZERO, b: Vec2::ONE });
    assert!(v.rename_annotation(a, "Bronchus"));
    assert_eq!(v.annotations().name(a), Some("Bronchus"));

    assert!(v.go_to_annotation(a));
    assert_eq!(v.slices.axis, SliceAxis::Coronal);
    assert_eq!(v.slices.index(SliceAxis::Coronal), 5);
    assert!(v.go_to_annotation(b));
    assert_eq!((v.slices.axis, v.slices.index(SliceAxis::Axial)), (SliceAxis::Axial, 3));
    assert!(!v.go_to_annotation(999));

    let r = v.annotation_report().expect("dataset loaded");
    assert_eq!(r.source, PathBuf::from("/data/phantom"));
    assert_eq!(r.study.study_date, "20240428");
    let names: Vec<_> = r.annotations.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Bronchus", "Rectangle 2"]);

    v.remove_annotation(b);
    assert_eq!(v.annotations().len(), 1);
}

#[test]
fn segments_are_created_edited_and_synced() {
    let mut v = Viewer::new(FakeRepo::new(1));
    assert_eq!(v.add_segment("x"), Err(ferrum_domain::SegmentationError::NoVolume));
    assert!(v.import_label_map(LabelMap::new(Dims3::new(1, 1, 1))).is_err());
    let mut v = loaded_viewer();
    let mut sync = GpuSyncState::default();
    let mut sink = RecordingSink::default();
    v.sync_gpu(&mut sync, &mut sink);
    sink.take();
    assert!(v.segment_summaries().is_empty());
    assert!(!v.segmentation().overlay_active());
    assert!(!v.frame_params(Vec2::splat(64.0)).unwrap().segments);

    let lesion = v.add_segment("Lesion").unwrap();
    assert!(v.segmentation().overlay_active());
    assert!(v.frame_params(Vec2::splat(64.0)).unwrap().segments);
    assert!(v.slice_params(SliceAxis::Axial, (Vec2::ZERO, Vec2::splat(100.0)), 1.0).unwrap().segments);
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["labels full".to_string(), "segment colours [230, 85, 75, 128]".into()]);

    // a 2×2×1 mask in a planar box; only that box is re-uploaded
    let bx = VoxelBox::new(UVec3::new(10, 10, 5), UVec3::new(12, 12, 6));
    assert_eq!(v.apply_segment_mask(lesion, bx, &[1; 4], false), Ok(4));
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(
        sink.take(),
        vec!["labels [10, 10, 5]..[12, 12, 6]".to_string(), "segment colours [230, 85, 75, 128]".into()]
    );
    let row = &v.segment_summaries()[0];
    assert_eq!((row.voxels, row.volume_ml), (4, 0.004));
    assert_eq!(row.segment.name, "Lesion");

    assert!(v.rename_segment(lesion, "Nodule"));
    assert!(!v.rename_segment(9, "x"));
    v.set_segment_color(lesion, [1, 2, 3]).unwrap();
    v.set_segment_opacity(lesion, 1.0).unwrap();
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["segment colours [1, 2, 3, 255]".to_string()]);
    v.set_segment_visible(lesion, false).unwrap();
    assert!(!v.segmentation().overlay_active(), "no visible segment, nothing to draw");
    v.set_segment_visible(lesion, true).unwrap();
    v.set_segments_shown(false);
    assert!(!v.segmentation().overlay_active());
    v.set_segments_shown(true);

    assert!(v.can_undo_segmentation());
    assert!(v.undo_segmentation());
    assert!(!v.undo_segmentation());
    assert_eq!(v.segment_summaries()[0].voxels, 0);
    assert!(v.remove_segment(lesion).is_ok());
    assert!(v.remove_segment(lesion).is_err());
}

#[test]
fn label_maps_are_imported_and_cleared() {
    let mut v = loaded_viewer();
    let dims = v.dataset().unwrap().volume.dims();
    let mut sync = GpuSyncState::default();
    let mut sink = RecordingSink::default();
    v.sync_gpu(&mut sync, &mut sink);
    sink.take();
    let mut data = vec![0u8; dims.voxel_count()];
    data[0] = 4;
    data[1] = 7;
    assert_eq!(
        v.import_label_map(LabelMap::new(Dims3::new(2, 2, 2))).unwrap_err().to_string(),
        format!("label map is Dims3 {{ x: 2, y: 2, z: 2 }}, the volume is {dims:?}")
    );
    assert_eq!(v.import_label_map(LabelMap::from_data(dims, data).unwrap()), Ok(2));
    let names: Vec<_> = v.segment_summaries().into_iter().map(|r| r.segment.name).collect();
    assert_eq!(names, vec!["Segment 4", "Segment 7"]);
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take()[0], "labels full");

    v.clear_segmentation();
    v.sync_gpu(&mut sync, &mut sink);
    assert_eq!(sink.take(), vec!["labels none".to_string()]);
    assert!(v.segmentation().set().is_none());
}

#[test]
fn picking_uses_the_segment_overlay() {
    let mut v = loaded_viewer();
    v.view_mode = ViewMode::Volume3d;
    v.volume.settings.mode = RenderMode::Isosurface;
    v.volume.settings.iso_threshold = 0.5;
    let before = v.pick(Vec2::ZERO, 1.0).expect("sphere hit");
    let dims = v.dataset().unwrap().volume.dims();
    let label = v.add_segment("Shell").unwrap();
    v.set_segment_opacity(label, 1.0).unwrap();
    // a slab in front of the sphere along the default view direction (+z)
    let bx = VoxelBox::new(UVec3::ZERO, UVec3::new(dims.x, dims.y, 3));
    v.apply_segment_mask(label, bx, &vec![1; bx.voxel_count()], false).unwrap();
    // the opaque slab stops the ray before the sphere: no surface hit to pick
    assert!(v.pick(Vec2::ZERO, 1.0).is_none());
    v.set_segments_shown(false);
    assert_eq!(v.pick(Vec2::ZERO, 1.0), Some(before));
}
