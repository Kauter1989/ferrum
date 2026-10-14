//! Metrics use cases of the viewer: the modality of an image that carries
//! none, kept in the workspace, and the quantitative profile of a segment
//! measured in the background (Stage 18, `docs/radiomics.md`).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ferrum_app::{MetricsOutcome, Viewer};
use ferrum_domain::{
    AnnotationReport, AnnotationSet, Dims3, LoadedSeries, ProgressSink, RepositoryError, ResultStore, ReviewDecision,
    SegmentationSet, SeriesDescriptor, SeriesMetadata, Volume, VolumeRepository, VoxelBox,
};
use ferrum_radiomics::{HuRange, Params, ValueUnit, CLINICAL, QUICK};
use glam::{UVec3, Vec3};

/// One series of 12 x 12 x 12 voxels of 1 x 1 x 2 mm whose value is
/// `100 * k - 600`; `modality` is what the source says (empty for NIfTI).
struct Repo {
    modality: &'static str,
}

fn volume() -> Volume {
    let dims = Dims3::new(12, 12, 12);
    let values: Vec<f32> = (0..dims.voxel_count()).map(|n| 100.0 * (n / 144) as f32 - 600.0).collect();
    Volume::from_physical(dims, Vec3::new(1.0, 1.0, 2.0), &values).unwrap()
}

impl VolumeRepository for Repo {
    fn name(&self) -> &str {
        "fake"
    }
    fn scan(&self, _: &[PathBuf], _: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        Ok(vec![SeriesDescriptor {
            id: "s".into(),
            format: "fake".into(),
            description: "s".into(),
            modality: self.modality.into(),
            dims: Dims3::new(12, 12, 12),
            sources: vec![],
            volume: None,
        }])
    }
    fn load(&self, _: &SeriesDescriptor, _: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        Ok(LoadedSeries {
            volume: volume(),
            metadata: SeriesMetadata { modality: self.modality.into(), ..Default::default() },
        })
    }
}

/// A workspace that keeps a modality (or refuses to).
#[derive(Default)]
struct Store {
    modality: Mutex<Option<String>>,
    refuse: bool,
}

impl ResultStore for Store {
    fn describe(&self) -> String {
        "ws".into()
    }
    fn source_paths(&self) -> Vec<PathBuf> {
        vec![PathBuf::from("/data/img.nii.gz")]
    }
    fn series_id(&self) -> String {
        "s".into()
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
    fn modality(&self) -> Option<String> {
        self.modality.lock().unwrap().clone()
    }
    fn set_modality(&self, m: &str) -> Result<(), String> {
        if self.refuse {
            return Err("the workspace is read-only".into());
        }
        *self.modality.lock().unwrap() = Some(m.to_owned());
        Ok(())
    }
}

fn open(modality: &'static str) -> Viewer {
    let mut v = Viewer::new(Arc::new(Repo { modality }));
    v.open_paths(vec![PathBuf::from("/data/img.nii.gz")]);
    v.wait_idle();
    assert!(v.dataset().is_some());
    v
}

/// The block 2..6 x 3..7 x 4..8 as segment 1 (slices 4..=7 hold -200, -100, 0, 100 HU).
fn with_block(v: &mut Viewer) -> u8 {
    let label = v.add_segment("Block").unwrap();
    let (lo, hi) = (UVec3::new(2, 3, 4), UVec3::new(6, 7, 8));
    let bx = VoxelBox::new(lo, hi);
    v.apply_segment_mask(label, bx, &vec![1; bx.voxel_count()], true).unwrap();
    label
}

// covers 18.3-f
#[test]
fn an_image_without_a_modality_asks_and_remembers_the_answer() {
    let mut v = open("");
    assert!(v.needs_modality(), "a NIfTI file says nothing about what it is");
    assert_eq!(v.value_unit(), ValueUnit::Unknown { modality: String::new() });

    // not now: the question is not asked again for this image
    v.skip_modality_prompt();
    assert!(!v.needs_modality());
    assert!(!v.value_unit().is_known());

    assert!(v.declare_modality("not a code!").is_err());
    assert!(!v.value_unit().is_known(), "a refused answer changes nothing");
    assert_eq!(v.declare_modality(" ct ").unwrap(), "CT");
    assert!(!v.needs_modality());
    assert_eq!(v.value_unit(), ValueUnit::Hu);
    assert_eq!(v.dataset().unwrap().metadata.modality, "CT");
    // the answer can be corrected: it was declared, not read from the source
    assert_eq!(v.declare_modality("mr").unwrap(), "MR");
    assert!(!v.value_unit().is_known());

    // another image asks again
    let mut again = open("");
    again.skip_modality_prompt();
    again.open_paths(vec![PathBuf::from("/data/img.nii.gz")]);
    again.wait_idle();
    assert!(again.needs_modality(), "a new image is a new question");
}

// covers 18.3-f
#[test]
fn an_image_with_its_own_modality_keeps_it() {
    let mut v = open("MR");
    assert!(!v.needs_modality());
    assert_eq!(v.declare_modality("mr").unwrap(), "MR", "saying what it already is changes nothing");
    let e = v.declare_modality("CT").unwrap_err();
    assert!(e.contains("carries the modality MR"), "{e}");
    assert_eq!(v.dataset().unwrap().metadata.modality, "MR");
}

// covers 18.3-f
#[test]
fn the_declaration_is_kept_in_the_workspace_and_taken_from_it() {
    let store = Arc::new(Store::default());
    let mut v = Viewer::new(Arc::new(Repo { modality: "" }));
    v.open_workspace(store.clone()).unwrap();
    v.wait_idle();
    assert!(v.needs_modality(), "the workspace holds no modality yet");
    v.declare_modality("ct").unwrap();
    assert_eq!(store.modality().as_deref(), Some("CT"), "kept in the workspace, as the command line does");

    // the next time the workspace is opened nobody is asked
    let mut next = Viewer::new(Arc::new(Repo { modality: "" }));
    next.open_workspace(store.clone()).unwrap();
    next.wait_idle();
    assert!(!next.needs_modality());
    assert_eq!(next.value_unit(), ValueUnit::Hu);

    // a workspace that cannot keep it refuses the declaration, and nothing changes
    let ro = Arc::new(Store { refuse: true, ..Default::default() });
    let mut v = Viewer::new(Arc::new(Repo { modality: "" }));
    v.open_workspace(ro).unwrap();
    v.wait_idle();
    let e = v.declare_modality("CT").unwrap_err();
    assert!(e.contains("read-only"), "{e}");
    assert!(v.needs_modality());
}

// covers 18.4-e
#[test]
fn a_segment_is_measured_in_the_background_with_known_answers() {
    let mut v = open("");
    let label = with_block(&mut v);
    v.declare_modality("CT").unwrap();

    // on the calling thread (embedding applications) ...
    let direct = v.segment_metrics(label, &QUICK).unwrap();
    // ... and in the background (the panel): the same numbers
    v.request_segment_metrics(label, &QUICK).unwrap();
    assert!(matches!(v.segment_metrics_view(label).unwrap().outcome, MetricsOutcome::Pending));
    v.wait_idle();
    let view = v.segment_metrics_view(label).unwrap();
    assert!(!view.stale);
    let MetricsOutcome::Ready(r) = view.outcome else { panic!("{:?}", view.outcome) };
    assert_eq!(**r, direct);
    // 4 x 4 x 4 voxels of 1 x 1 x 2 mm; 16 voxels each of -200, -100, 0 and 100 HU
    assert_eq!(r.number("size.voxels"), Some(64.0));
    assert_eq!(r.number("size.volume_ml"), Some(0.128));
    assert!((r.number("intensity.mean").unwrap() + 50.0).abs() < 0.02);
    assert_eq!(r.get("intensity.mean").unwrap().unit, "HU");
    assert_eq!(v.segment_metrics_view(label + 1).map(|m| m.stale), None, "no request for another segment");
}

// covers 18.4-e
#[test]
fn a_result_is_marked_out_of_date_when_the_segment_changes() {
    let mut v = open("CT");
    let label = with_block(&mut v);
    v.request_segment_metrics(label, &QUICK).unwrap();
    v.wait_idle();
    assert!(!v.segment_metrics_view(label).unwrap().stale);

    let bx = VoxelBox::new(UVec3::new(0, 0, 0), UVec3::new(2, 2, 2));
    v.apply_segment_mask(label, bx, &vec![1; bx.voxel_count()], true).unwrap();
    let view = v.segment_metrics_view(label).unwrap();
    assert!(view.stale, "the segment is not what was measured");
    assert!(matches!(view.outcome, MetricsOutcome::Ready(_)), "the old numbers stay, marked");

    v.request_segment_metrics(label, &QUICK).unwrap();
    v.wait_idle();
    let view = v.segment_metrics_view(label).unwrap();
    assert!(!view.stale);
    let MetricsOutcome::Ready(r) = view.outcome else { panic!() };
    assert_eq!(r.number("size.voxels"), Some(64.0 + 8.0));
}

// covers 18.4-f
#[test]
fn an_unknown_image_type_explains_instead_of_guessing() {
    let mut v = open("");
    let label = with_block(&mut v);
    assert!(v.segment_metrics(label, &QUICK).unwrap_err().contains("unit of the voxel values is unknown"));
    v.request_segment_metrics(label, &QUICK).unwrap();
    let MetricsOutcome::Failed(why) = v.segment_metrics_view(label).unwrap().outcome else { panic!() };
    assert!(why.contains("image type is not known"), "{why}");
    assert!(!v.is_computing(), "nothing was started");

    v.declare_modality("MR").unwrap();
    v.request_segment_metrics(label, &QUICK).unwrap();
    let MetricsOutcome::Failed(why) = v.segment_metrics_view(label).unwrap().outcome else { panic!() };
    assert!(why.contains("CT images only") && why.contains("MR"), "{why}");

    // asking for something that does not exist is an error, not an empty card
    assert!(v.request_segment_metrics(9, &QUICK).unwrap_err().contains("no segment 9"));
    let empty = v.add_segment("Empty").unwrap();
    assert!(v.request_segment_metrics(empty, &QUICK).unwrap_err().contains("no voxels"));
    assert!(Viewer::new(Arc::new(Repo { modality: "CT" })).request_segment_metrics(1, &QUICK).is_err());
}

// covers 18.9-c, 18.11-c, 18.11-d
#[test]
fn the_bin_width_and_ranges_of_the_card_are_set_validated_and_used() {
    let mut v = open("");
    let label = with_block(&mut v);
    v.declare_modality("CT").unwrap();
    assert_eq!(v.metrics_params(), &Params::default());

    let own = Params {
        bin_width: 100.0,
        hu_ranges: Some(vec![HuRange::new(-250.0, -50.0), HuRange::new(-50.0, 150.0)]),
        ..Params::default()
    };
    v.set_metrics_params(own.clone()).unwrap();
    assert_eq!(v.metrics_params(), &own);
    // the same numbers on the calling thread and in the background, and the parameters are in the result
    let direct = v.segment_metrics(label, &CLINICAL).unwrap();
    v.request_segment_metrics(label, &CLINICAL).unwrap();
    v.wait_idle();
    let MetricsOutcome::Ready(r) = v.segment_metrics_view(label).unwrap().outcome else { panic!() };
    assert_eq!(**r, direct);
    assert_eq!(r.params, own);
    assert_eq!(r.bin_width, Some(100.0));
    let shares = r.hu_ranges.as_ref().unwrap();
    assert!(!shares.is_default);
    assert_eq!(
        shares.ranges.iter().map(|s| s.voxels).collect::<Vec<_>>(),
        [32, 32].map(|n| n as u64),
        "[-250, -50): -200 and -100; [-50, 150): 0 and 100"
    );

    // nonsense is refused and the old parameters stay
    let bad =
        Params { hu_ranges: Some(vec![HuRange::new(0.0, 100.0), HuRange::new(50.0, 150.0)]), ..Params::default() };
    assert!(v.set_metrics_params(bad).unwrap_err().contains("overlap"));
    assert!(v.set_metrics_params(Params { bin_width: 0.0, ..Params::default() }).is_err());
    assert_eq!(v.metrics_params(), &own);
}

// covers 18.4-e
#[test]
fn a_second_request_is_not_mistaken_for_the_first() {
    let mut v = open("");
    let label = with_block(&mut v);
    v.declare_modality("CT").unwrap();
    v.request_segment_metrics(label, &CLINICAL).unwrap();
    // other parameters before the first answer arrives: only the second request is kept
    v.set_metrics_params(Params { bin_width: 500.0, ..Params::default() }).unwrap();
    v.request_segment_metrics(label, &CLINICAL).unwrap();
    v.wait_idle();
    let MetricsOutcome::Ready(r) = v.segment_metrics_view(label).unwrap().outcome else { panic!() };
    assert_eq!(r.bin_width, Some(500.0));
}
