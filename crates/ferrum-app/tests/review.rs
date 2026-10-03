//! Review use cases: opening a workspace through the `ResultStore` port,
//! the queue of proposals, decisions saved and logged.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ferrum_app::Viewer;
use ferrum_domain::{
    Annotation, AnnotationReport, AnnotationSet, Dims3, LoadedSeries, ProgressSink, Provenance, RepositoryError,
    ResultStore, ReviewDecision, ReviewItem, ReviewStatus, SegmentationSet, SeriesDescriptor, SeriesMetadata,
    SliceAxis, SliceKey, Timestamp, Volume, VolumeRepository,
};
use glam::{Vec2, Vec3};

/// Two series; the workspace belongs to the second.
struct Repo;

fn volume(n: u32) -> Volume {
    let dims = Dims3::new(n, 16, 8);
    Volume::from_physical(dims, Vec3::ONE, &vec![1.0; dims.voxel_count()]).unwrap()
}

impl VolumeRepository for Repo {
    fn name(&self) -> &str {
        "fake"
    }
    fn scan(&self, _: &[PathBuf], _: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        Ok(["a", "b"]
            .iter()
            .map(|id| SeriesDescriptor {
                id: (*id).into(),
                format: "fake".into(),
                description: (*id).into(),
                modality: "CT".into(),
                dims: Dims3::new(16, 16, 8),
                sources: vec![],
            })
            .collect())
    }
    fn load(&self, s: &SeriesDescriptor, _: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        let n = if s.id == "b" { 20 } else { 16 };
        Ok(LoadedSeries { volume: volume(n), metadata: SeriesMetadata { modality: "CT".into(), ..Default::default() } })
    }
}

#[derive(Default)]
struct Store {
    series: String,
    changed: bool,
    annotations: Option<AnnotationSet>,
    segments: Option<SegmentationSet>,
    saves: Mutex<Vec<(usize, usize)>>,
    log: Mutex<Vec<ReviewDecision>>,
}

impl ResultStore for Store {
    fn describe(&self) -> String {
        "ws/ct1".into()
    }
    fn source_paths(&self) -> Vec<PathBuf> {
        vec![PathBuf::from("/data/ct")]
    }
    fn series_id(&self) -> String {
        self.series.clone()
    }
    fn verify(&self) -> Result<(), String> {
        if self.changed {
            Err("a.dcm: source changed".into())
        } else {
            Ok(())
        }
    }
    fn load(&self, _: &Volume) -> Result<(Option<AnnotationSet>, Option<SegmentationSet>), String> {
        Ok((self.annotations.clone(), self.segments.clone()))
    }
    fn save(&self, a: &AnnotationReport, s: &SegmentationSet, _: &Volume) -> Result<(), String> {
        self.saves.lock().unwrap().push((a.annotations.len(), s.segments().len()));
        Ok(())
    }
    fn log(&self, d: &ReviewDecision) -> Result<(), String> {
        self.log.lock().unwrap().push(d.clone());
        Ok(())
    }
}

fn store() -> Arc<Store> {
    let mut annotations = AnnotationSet::default();
    let key = SliceKey::new(SliceAxis::Axial, 3);
    annotations.add(key, Annotation::Text { pos: Vec2::ONE, text: "drawn".into() });
    let agent = Provenance::agent(Some("run-1".into()), Timestamp(1));
    annotations.add_with(key, Annotation::Distance { a: Vec2::ZERO, b: Vec2::X }, agent.clone());
    let mut segments = SegmentationSet::new(Dims3::new(20, 16, 8));
    let label = segments.add_segment("Liver").unwrap();
    segments.set_provenance(label, Provenance::engine("TotalSegmentator", "2.18", false, Timestamp(2))).unwrap();
    Arc::new(Store {
        series: "b".into(),
        annotations: Some(annotations),
        segments: Some(segments),
        ..Default::default()
    })
}

#[test]
fn a_workspace_opens_its_series_and_proposals() {
    let mut v = Viewer::new(Arc::new(Repo));
    let s = store();
    v.open_workspace(s.clone()).unwrap();
    v.wait_idle();
    assert_eq!(v.dataset().unwrap().volume.dims(), Dims3::new(20, 16, 8), "the workspace's series, not the first one");
    assert_eq!(v.workspace().as_deref(), Some("ws/ct1"));
    assert!(v.status.message.contains("2 waiting for review"), "{}", v.status.message);
    assert!(!v.workspace_dirty());
    let queue = v.review_queue();
    assert_eq!(queue.len(), 2);
    assert_eq!((queue[0].kind.as_str(), queue[1].kind.as_str()), ("Distance", "Segment"));
    assert_eq!(queue[1].provenance.author.describe(), "engine TotalSegmentator 2.18");
    assert!(v.segmentation().show, "loaded segments are shown");

    v.decide(queue[0].item, ReviewStatus::Confirmed, Some(" dr.k ")).unwrap();
    v.decide(queue[1].item, ReviewStatus::Rejected, None).unwrap();
    assert!(v.review_queue().is_empty());
    let log = s.log.lock().unwrap().clone();
    assert_eq!(log.len(), 2);
    assert_eq!((log[0].by.as_deref(), log[0].status), (Some("dr.k"), ReviewStatus::Confirmed));
    assert_eq!((log[1].item, log[1].name.as_str(), log[1].by.clone()), (ReviewItem::Segment(1), "Liver", None));
    assert_eq!(s.saves.lock().unwrap().as_slice(), &[(2, 1), (2, 1)], "every decision is saved at once");
    assert!(!v.workspace_dirty());

    // other edits mark the workspace dirty until saved
    v.add_annotation(SliceKey::new(SliceAxis::Axial, 1), Annotation::Text { pos: Vec2::ZERO, text: "x".into() });
    assert!(v.workspace_dirty());
    v.save_workspace().unwrap();
    assert!(!v.workspace_dirty());
    assert_eq!(s.saves.lock().unwrap().last(), Some(&(3, 1)));
    assert!(v.decide(ReviewItem::Annotation(99), ReviewStatus::Confirmed, None).is_err());

    // loading another study closes the workspace
    v.open_paths(vec![PathBuf::from("/other")]);
    v.wait_idle();
    v.load_series(v.series_choice.clone().unwrap()[0].clone());
    v.wait_idle();
    assert_eq!(v.workspace(), None);
    assert!(v.save_workspace().is_err());
}

#[test]
fn changed_or_missing_sources_stop_the_opening() {
    let mut v = Viewer::new(Arc::new(Repo));
    let changed = Arc::new(Store { series: "b".into(), changed: true, ..Default::default() });
    assert!(v.open_workspace(changed).unwrap_err().contains("source changed"));
    let missing = Arc::new(Store { series: "zz".into(), ..Default::default() });
    v.open_workspace(missing).unwrap();
    v.wait_idle();
    assert!(v.dataset().is_none(), "no other series is loaded instead");
    assert!(v.status.errors.iter().any(|e| e.contains("no longer in its source")));
    // without a workspace, decisions apply in memory only
    let mut v = Viewer::new(Arc::new(Repo));
    v.open_paths(vec![PathBuf::from("/d")]);
    v.wait_idle();
    v.load_series(v.series_choice.clone().unwrap()[0].clone());
    v.wait_idle();
    let id = v.add_annotation_with(
        SliceKey::new(SliceAxis::Axial, 0),
        Annotation::Text { pos: Vec2::ZERO, text: "p".into() },
        Provenance::agent(None, Timestamp(0)),
    );
    assert_eq!(v.review_queue().len(), 1);
    v.decide(ReviewItem::Annotation(id), ReviewStatus::Confirmed, None).unwrap();
    assert!(v.review_queue().is_empty() && !v.workspace_dirty());
}
