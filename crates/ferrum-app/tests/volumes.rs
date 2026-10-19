//! Volumes of a multi-volume series in the viewer (Stage 20.2,
//! `docs/multi-volume.md`): listing by name, switching without reloading
//! the study, the volume cache, metrics of another volume and volumes that
//! cannot be shown.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ferrum_app::{ViewMode, Viewer};
use ferrum_domain::{
    Annotation, AnnotationReport, AnnotationSet, Dims3, LoadedSeries, ProgressSink, RepositoryError, ResultStore,
    ReviewDecision, SegmentationSet, SeriesDescriptor, SeriesMetadata, SliceAxis, SliceKey, Volume, VolumeInfo,
    VolumeRepository, VoxelBox, WindowLevel, WindowPreset,
};
use ferrum_radiomics::QUICK;
use glam::{UVec3, Vec2, Vec3};

const DIMS: Dims3 = Dims3 { x: 16, y: 16, z: 12 };

/// A DWI series: `b = 0` (value 1000), `b = 1000` (value 400) and a third
/// volume that misses two slices.
struct Dwi {
    loads: AtomicUsize,
    /// What the files say the modality is (empty: NIfTI, says nothing).
    modality: &'static str,
    /// The window the files store, the same for every volume.
    stored_window: Option<WindowLevel>,
}

fn descriptor(id: &str, dims: Dims3) -> SeriesDescriptor {
    SeriesDescriptor {
        id: id.into(),
        format: "fake".into(),
        description: "DWI".into(),
        modality: "CT".into(),
        dims,
        sources: vec![],
        volume: None,
    }
}

fn volumes() -> Vec<SeriesDescriptor> {
    let info = |index: u32, label: &str, b: f64, complete: bool| VolumeInfo {
        group: "dwi".into(),
        index,
        count: 3,
        label: label.into(),
        b_value: Some(b),
        complete,
        ..VolumeInfo::default()
    };
    vec![
        SeriesDescriptor { volume: Some(info(0, "b = 0", 0.0, true)), ..descriptor("dwi#v=b0", DIMS) },
        SeriesDescriptor { volume: Some(info(1, "b = 1000", 1000.0, true)), ..descriptor("dwi#v=b1000", DIMS) },
        SeriesDescriptor {
            volume: Some(info(2, "b = 2000", 2000.0, false)),
            ..descriptor("dwi#v=b2000", Dims3::new(16, 16, 10))
        },
    ]
}

/// A cube of `value` in a background of 0.
fn cube(dims: Dims3, value: f32) -> Volume {
    let mut values = vec![0.0f32; dims.voxel_count()];
    for k in 3..8 {
        for j in 4..12 {
            for i in 4..12 {
                values[dims.index(i, j, k)] = value;
            }
        }
    }
    Volume::from_physical(dims, Vec3::ONE, &values).unwrap()
}

impl VolumeRepository for Dwi {
    fn name(&self) -> &str {
        "fake"
    }
    fn scan(&self, _: &[PathBuf], _: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        Ok(vec![descriptor("dwi", DIMS)])
    }
    fn volumes(&self, s: &SeriesDescriptor, _: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        assert_eq!(s.id, "dwi");
        Ok(volumes())
    }
    fn load(&self, s: &SeriesDescriptor, _: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let value = match s.id.as_str() {
            "dwi#v=b0" => 1000.0,
            "dwi#v=b1000" => 400.0,
            "dwi#v=b2000" => 100.0,
            _ => return Err(RepositoryError::NothingFound),
        };
        let metadata = SeriesMetadata {
            modality: self.modality.into(),
            description: "DWI".into(),
            default_window: self.stored_window,
            ..Default::default()
        };
        Ok(LoadedSeries { volume: cube(s.dims, value), metadata })
    }
}

fn open() -> (Viewer, Arc<Dwi>) {
    let repo = Arc::new(Dwi { loads: AtomicUsize::new(0), modality: "CT", stored_window: None });
    let mut v = Viewer::new(repo.clone());
    v.open_paths(vec![PathBuf::from("/data/dwi")]);
    v.wait_idle();
    assert!(v.dataset().is_some(), "{:?}", v.status);
    (v, repo)
}

fn value_at(v: &Viewer, i: u32, j: u32, k: u32) -> f32 {
    v.dataset().unwrap().volume.physical(i, j, k).unwrap()
}

#[test]
fn a_multi_volume_series_opens_volume_1_and_lists_every_volume() {
    // covers 20.2-a, 20.2-d
    let (v, repo) = open();
    assert!(v.series_choice.is_none(), "one series: no picker, whatever its volumes");
    assert_eq!(repo.loads.load(Ordering::SeqCst), 1, "only volume 1 is read");
    let labels: Vec<_> = v.volumes().iter().map(|e| e.label.clone()).collect();
    assert_eq!(labels, ["b = 0", "b = 1000", "b = 2000"]);
    assert_eq!(v.current_volume(), Some(0));
    assert_eq!(v.current_volume_label().as_deref(), Some("b = 0"));
    assert_eq!(value_at(&v, 8, 8, 5), 1000.0);
    assert!(v.status.message.contains("b = 0"), "{}", v.status.message);
}

#[test]
fn a_single_volume_series_has_no_volume_list() {
    // covers 20.2-a
    struct One;
    impl VolumeRepository for One {
        fn name(&self) -> &str {
            "fake"
        }
        fn scan(&self, _: &[PathBuf], _: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
            Ok(vec![descriptor("t1", DIMS)])
        }
        fn load(&self, s: &SeriesDescriptor, _: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
            Ok(LoadedSeries { volume: cube(s.dims, 10.0), metadata: SeriesMetadata::default() })
        }
    }
    let mut v = Viewer::new(Arc::new(One));
    v.open_paths(vec![PathBuf::from("/data/t1")]);
    v.wait_idle();
    assert!(v.volumes().is_empty());
    assert_eq!(v.current_volume(), None);
    assert!(v.switch_volume(1).is_err());
}

#[test]
fn switching_keeps_the_view() {
    // covers 20.2-b
    let (mut v, repo) = open();
    v.set_view_mode(ViewMode::Mpr);
    v.set_slice_index(SliceAxis::Axial, 4);
    v.slices.views[0].zoom = 2.5;
    let key = SliceKey::new(SliceAxis::Axial, 4);
    v.add_annotation(key, Annotation::Text { pos: Vec2::ONE, text: "lesion".into() });
    let label = v.add_segment("Cube").unwrap();
    let bx = VoxelBox::new(UVec3::new(4, 4, 3), UVec3::new(12, 12, 8));
    v.apply_segment_mask(label, bx, &vec![1; bx.voxel_count()], true).unwrap();
    let auto_b0 = v.slices.window;
    let revision = v.dataset().unwrap().revision;

    v.switch_volume(1).unwrap();
    assert!(v.is_switching_volume(), "the volume loads in the background");
    v.wait_idle();
    assert!(!v.is_switching_volume());
    assert_eq!(value_at(&v, 8, 8, 5), 400.0);
    assert_eq!(v.current_volume_label().as_deref(), Some("b = 1000"));
    assert_ne!(v.dataset().unwrap().revision, revision, "the GPU uploads the new voxels");
    assert_eq!(v.view_mode(), ViewMode::Mpr);
    assert_eq!(v.slices.index(SliceAxis::Axial), 4);
    assert_eq!(v.slices.views[0].zoom, 2.5);
    assert_eq!(v.annotations().len(), 1);
    assert_eq!(v.segment_summaries().len(), 1);
    assert_ne!(v.slices.window, auto_b0, "the automatic window follows the new volume");

    // a window the person chose stays; a volume loaded before is shown at once
    v.apply_window_preset(WindowPreset::ALL[0]);
    let chosen = v.slices.window;
    v.switch_volume(0).unwrap();
    assert!(!v.is_switching_volume(), "from the cache, without a load");
    assert_eq!(repo.loads.load(Ordering::SeqCst), 2);
    assert_eq!(value_at(&v, 8, 8, 5), 1000.0);
    assert_eq!(v.slices.window, chosen);
    assert_eq!(v.cached_volumes(), 2);

    // ‹ › step over the volume that cannot be shown
    v.step_volume(1).unwrap();
    assert_eq!(v.current_volume(), Some(1));
    v.step_volume(1).unwrap();
    assert_eq!(v.current_volume(), Some(0), "b = 2000 is skipped");
    v.step_volume(-1).unwrap();
    assert_eq!(v.current_volume(), Some(1));
}

#[test]
fn the_cache_keeps_volumes_within_its_budget() {
    // covers 20.2-b
    let (mut v, repo) = open();
    let one_volume = DIMS.voxel_count() * 2;
    v.set_volume_cache_budget(one_volume);
    v.switch_volume(1).unwrap();
    v.wait_idle();
    v.switch_volume(0).unwrap();
    v.wait_idle();
    assert_eq!(repo.loads.load(Ordering::SeqCst), 3, "only the volume shown fits the budget");
    assert_eq!(v.cached_volumes(), 1);
}

#[test]
fn metrics_of_another_volume_are_out_of_date_and_named() {
    // covers 20.2-c
    let (mut v, _) = open();
    let label = v.add_segment("Cube").unwrap();
    let bx = VoxelBox::new(UVec3::new(4, 4, 3), UVec3::new(12, 12, 8));
    v.apply_segment_mask(label, bx, &vec![1; bx.voxel_count()], true).unwrap();
    v.request_segment_metrics(label, &QUICK).unwrap();
    v.wait_idle();
    let view = v.segment_metrics_view(label).unwrap();
    assert!(!view.stale);
    assert_eq!(view.volume, Some("b = 0"));
    v.switch_volume(1).unwrap();
    v.wait_idle();
    let view = v.segment_metrics_view(label).unwrap();
    assert!(view.stale, "measured on b = 0, b = 1000 is shown");
    assert_eq!(view.volume, Some("b = 0"));
    v.request_segment_metrics(label, &QUICK).unwrap();
    v.wait_idle();
    let view = v.segment_metrics_view(label).unwrap();
    assert_eq!((view.stale, view.volume), (false, Some("b = 1000")));
}

#[test]
fn a_volume_missing_slices_is_listed_but_disabled() {
    // covers 20.2-e
    let (mut v, repo) = open();
    let entries = v.volumes();
    assert!(entries[1].disabled.is_none());
    let why = entries[2].disabled.clone().unwrap();
    assert!(why.contains("misses slices"), "{why}");
    let err = v.switch_volume(2).unwrap_err();
    assert!(err.contains("b = 2000") && err.contains("misses slices"), "{err}");
    assert_eq!(v.current_volume(), Some(0));
    assert_eq!(repo.loads.load(Ordering::SeqCst), 1, "nothing was loaded");
    assert!(v.switch_volume(7).is_err());
}

/// A workspace that keeps its volume.
#[derive(Default)]
struct Store {
    volume: Mutex<Option<String>>,
}

impl ResultStore for Store {
    fn describe(&self) -> String {
        "ws".into()
    }
    fn source_paths(&self) -> Vec<PathBuf> {
        vec![PathBuf::from("/data/dwi")]
    }
    fn series_id(&self) -> String {
        "dwi".into()
    }
    fn verify(&self) -> Result<(), String> {
        Ok(())
    }
    fn load(&self, _: &Volume) -> Result<(Option<AnnotationSet>, Option<SegmentationSet>), String> {
        Ok((None, None))
    }
    fn save(&self, _: &AnnotationReport, _: &SegmentationSet, _: &Volume) -> Result<(), String> {
        Ok(())
    }
    fn log(&self, _: &ReviewDecision) -> Result<(), String> {
        Ok(())
    }
    fn volume(&self) -> Option<String> {
        self.volume.lock().unwrap().clone()
    }
    fn set_volume(&self, volume: &str) -> Result<(), String> {
        *self.volume.lock().unwrap() = Some(volume.to_owned());
        Ok(())
    }
}

#[test]
fn a_workspace_opens_on_its_volume_and_keeps_a_switch() {
    // covers 20.2-a, 20.2-b
    let repo = Arc::new(Dwi { loads: AtomicUsize::new(0), modality: "CT", stored_window: None });
    let store = Arc::new(Store { volume: Mutex::new(Some("dwi#v=b1000".into())) });
    let mut v = Viewer::new(repo);
    v.open_workspace(store.clone()).unwrap();
    v.wait_idle();
    assert_eq!(v.current_volume_label().as_deref(), Some("b = 1000"), "{:?}", v.status);
    assert_eq!(value_at(&v, 8, 8, 5), 400.0);
    v.switch_volume(0).unwrap();
    v.wait_idle();
    assert_eq!(store.volume().as_deref(), Some("dwi#v=b0"), "the workspace keeps the volume shown");
}

#[test]
fn the_image_type_said_once_holds_for_every_volume() {
    // covers 20.2-b (found in the field check on a 4D PET NIfTI: the question came back after each switch)
    let open_nifti = || {
        let mut v = Viewer::new(Arc::new(Dwi { loads: AtomicUsize::new(0), modality: "", stored_window: None }));
        v.open_paths(vec![PathBuf::from("/data/pet.nii.gz")]);
        v.wait_idle();
        v
    };
    let mut v = open_nifti();
    assert!(v.needs_modality());
    v.declare_modality("PT").unwrap();
    v.switch_volume(1).unwrap();
    v.wait_idle();
    assert!(!v.needs_modality(), "PET was said for the series, not for one volume");
    assert_eq!(v.dataset().unwrap().metadata.modality, "PT");
    v.switch_volume(0).unwrap();
    assert_eq!(v.dataset().unwrap().metadata.modality, "PT", "also for a volume from the cache");

    // "Not now" holds too
    let mut v = open_nifti();
    v.skip_modality_prompt();
    v.switch_volume(1).unwrap();
    v.wait_idle();
    assert!(!v.needs_modality());
}

#[test]
fn a_window_stored_for_the_series_does_not_hide_the_other_volumes() {
    // covers 20.2-b (found in the field check on a Canon Enhanced DTI: b = 1500 was black under the b = 0 window)
    let stored = WindowLevel::new(500.0, 1000.0);
    let repo = Dwi { loads: AtomicUsize::new(0), modality: "MR", stored_window: Some(stored) };
    let mut v = Viewer::new(Arc::new(repo));
    v.open_paths(vec![PathBuf::from("/data/dwi")]);
    v.wait_idle();
    assert_eq!(v.slices.window, stored, "the file's window is used on opening");
    v.switch_volume(1).unwrap();
    v.wait_idle();
    let d = v.dataset().unwrap();
    let auto = WindowLevel::auto(d.volume.range(), d.histogram.counts());
    assert_eq!(v.slices.window, auto, "the window follows the volume shown");
    assert!(v.slices.window.center < 400.0, "b = 1000 (value 400) is not left dark: {:?}", v.slices.window);
}
