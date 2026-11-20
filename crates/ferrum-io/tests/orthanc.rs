//! Integration tests against a real Orthanc PACS (Stage 32, PR 4, #76).
//!
//! Orthanc runs in Docker with its DICOMweb plugin. The plugin is **AGPL**:
//! Orthanc is a separate service reached over HTTP only, never linked into
//! FERRUM (ADR 0021 §6, constitution licences). In CI the `orthanc` job starts
//! it as a service container; locally a developer can run
//! `docker run -p 8042:8042 orthancteam/orthanc` and these tests detect it.
//!
//! Graceful skip (32.5-b): every test returns early when no Orthanc is
//! reachable — `FERRUM_ORTHANC_URL` is unset and nothing answers on the
//! default `127.0.0.1:8042`. The phantom DICOM is generated at run time
//! (`common`); no patient or volume data is committed.
//!
//! // covers 32.5-a, 32.5-b, 32.1-b, 32.2-a

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;
use std::time::Duration;

use dicom_dictionary_std::{tags, uids};
use ferrum_domain::{
    Annotation, AnnotationReport, AnnotationSet, Dims3, LabelMap, NoProgress, Provenance, RepositoryError, Segment,
    SegmentationSet, SliceAxis, SliceKey, Timestamp, Volume, VolumeRepository,
};
use ferrum_io::dicom::deid::ProfileOptions;
use ferrum_io::dicom::export::{DicomExportOptions, DicomExporter, DicomSource, SourceInstance};
use ferrum_io::{CompositeRepository, DicomRepository, DicomWebRepository, DicomWebStow, StowStatus};
use glam::{DVec3, Vec2, Vec3};

const STUDY_UID: &str = "1.2.826.0.1.3680043.2.1";
const SERIES_UID: &str = "1.2.826.0.1.3680043.2.1125.1";

/// A reachable Orthanc, or `None` when the tests must skip (32.5-b).
struct Orthanc {
    /// DICOMweb service root, e.g. `http://127.0.0.1:8042/dicom-web`.
    root: String,
    user: String,
    pass: String,
}

impl Orthanc {
    /// Detects the test PACS: an explicit `FERRUM_ORTHANC_URL` wins (CI sets
    /// it); otherwise a local Orthanc on the default port is probed. When
    /// neither is present the tests skip rather than fail.
    fn detect() -> Option<Self> {
        let user = std::env::var("FERRUM_ORTHANC_USER").unwrap_or_else(|_| "orthanc".into());
        let pass = std::env::var("FERRUM_ORTHANC_PASS").unwrap_or_else(|_| "orthanc".into());
        if let Ok(url) = std::env::var("FERRUM_ORTHANC_URL") {
            let url = url.trim().trim_end_matches('/');
            if !url.is_empty() {
                return Some(Self { root: url.to_string(), user, pass });
            }
        }
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 8042));
        match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
            Ok(_) => Some(Self { root: "http://127.0.0.1:8042/dicom-web".into(), user, pass }),
            Err(_) => {
                eprintln!("orthanc: skipping (set FERRUM_ORTHANC_URL or start Orthanc on :8042)");
                None
            }
        }
    }

    /// The service root as a `dicomweb(s)://` path for the repository/STOW.
    fn web_url(&self, query: &str) -> PathBuf {
        let (scheme, rest) = if let Some(rest) = self.root.strip_prefix("https://") {
            ("dicomwebs://", rest)
        } else {
            ("dicomweb://", self.root.strip_prefix("http://").unwrap_or(&self.root))
        };
        PathBuf::from(format!("{scheme}{rest}{query}"))
    }

    /// The STOW target (the service root, no query).
    fn target(&self) -> String {
        self.web_url("").to_str().unwrap().to_string()
    }

    fn repo(&self) -> DicomWebRepository {
        DicomWebRepository::new().with_basic_auth(&self.user, &self.pass).with_timeout(Duration::from_secs(60))
    }

    fn stow(&self) -> DicomWebStow {
        DicomWebStow::new().with_basic_auth(&self.user, &self.pass).with_timeout(Duration::from_secs(60))
    }
}

/// The phantom series' Part 10 files, in directory order.
fn phantom_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    paths.sort();
    paths
}

/// A CT phantom with a Modality LUT (intercept −1024), written to `dir`.
fn write_ct_phantom(dir: &std::path::Path) {
    common::write_series(dir, 5, 8, 4, 2.5, |s| {
        s.intercept = -1024.0;
        s.window = Some((40.0, 400.0));
    });
}

fn file_loaded(dir: &std::path::Path) -> ferrum_domain::LoadedSeries {
    let repo = DicomRepository;
    let series = repo.scan(&[dir.to_path_buf()], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    repo.load(&series[0], &NoProgress).unwrap()
}

#[test]
fn query_retrieve_open_matches_the_phantom_voxel_for_voxel() {
    // covers 32.5-a, 32.1-b: a pinned phantom stored in a real Orthanc is
    // queried (QIDO), retrieved (WADO) and opened through the existing
    // repository path; the retrieved volume equals the file reader's on the
    // same instances, voxel for voxel, in the canonical LPS frame.
    let Some(pacs) = Orthanc::detect() else { return };
    let dir = tempfile::tempdir().unwrap();
    write_ct_phantom(dir.path());
    let paths = phantom_files(dir.path());

    // Store the phantom in Orthanc over STOW-RS (the host-only send-off).
    let report = pacs.stow().store_files(&pacs.target(), &paths, &NoProgress).unwrap();
    assert!(report.all_stored(), "the phantom stored: {report:?}");
    assert_eq!(report.count(StowStatus::Success), 4);

    // Query → retrieve → open through the DICOMweb repository.
    let repo = pacs.repo();
    let query = pacs.web_url(&format!("?StudyInstanceUID={STUDY_UID}"));
    let series = repo.scan(std::slice::from_ref(&query), &NoProgress).unwrap();
    assert_eq!(series.len(), 1, "one phantom series");
    assert_eq!(series[0].format, "DICOMweb");
    let web = repo.load(&series[0], &NoProgress).unwrap();
    let file = file_loaded(dir.path());

    assert_eq!(web.volume, file.volume, "retrieved volume must equal the file volume");
    assert_eq!(web.volume.dims(), Dims3::new(8, 5, 4));
    // Absolute pixel check: the phantom gradient through the Modality LUT.
    for k in 0..4 {
        for j in 0..5 {
            for i in 0..8 {
                let stored = web.volume.voxel(i, j, k).unwrap();
                let value = web.volume.range().from_storage(stored);
                let expected = common::phantom_value(i, j, k) as f32 - 1024.0;
                assert_eq!(value, expected, "voxel {i},{j},{k}");
            }
        }
    }
    assert_eq!(web.metadata.study.study_instance_uid, STUDY_UID);
    assert_eq!(web.metadata.study.series_instance_uid, SERIES_UID);

    // The existing viewer path (composite repository) opens the same study.
    let composite = CompositeRepository::default();
    let cs = composite.scan(&[query], &NoProgress).unwrap();
    assert_eq!(cs.len(), 1);
    let cw = composite.load(&cs[0], &NoProgress).unwrap();
    assert_eq!(cw.volume, file.volume, "the composite opens the PACS study identically");
}

#[test]
fn stow_of_a_deidentified_seg_reports_the_success_sequence() {
    // covers 32.5-a, 32.2-a: a de-identified SEG/SR export (Stage 31) is
    // STOW'd to a real Orthanc and the response is checked per instance.
    //
    // STOW tag confirmation (PR 3 quirk): `parse_response` reads the success
    // sequence only from tag (0008,1190), which the 0.10 dictionary names
    // `RETRIEVE_URL`. A green `count(Success) == 2` against real Orthanc
    // confirms Orthanc reports its stored instances under (0008,1190) — if it
    // used another tag, the success count would be 0 and this test would fail.
    let Some(pacs) = Orthanc::detect() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (paths, ids) = deidentified_export(dir.path());
    assert_eq!(paths.len(), 2, "a SEG and an SR");

    let report = pacs.stow().store_files(&pacs.target(), &paths, &NoProgress).unwrap();
    assert!(report.all_stored(), "Orthanc accepted the export: {report:?}");
    assert_eq!(report.count(StowStatus::Failure), 0);
    assert_eq!(report.count(StowStatus::Success), 2, "both instances in the (0008,1190) success sequence");
    for (class, instance) in &ids {
        assert!(
            report.instances.iter().any(|i| &i.sop_class_uid == class
                && &i.sop_instance_uid == instance
                && i.status == StowStatus::Success),
            "referenced {instance} in the success sequence: {report:?}"
        );
    }
}

#[test]
fn an_absent_study_is_nothing_found() {
    // covers 32.5-a, 32.1-c: a query that matches nothing on a real Orthanc
    // is NothingFound, never a panic.
    let Some(pacs) = Orthanc::detect() else { return };
    let repo = pacs.repo();
    let query = pacs.web_url("?StudyInstanceUID=1.2.826.0.1.3680043.9.9.9.9.9");
    let err = repo.scan(&[query], &NoProgress).unwrap_err();
    assert_eq!(err, RepositoryError::NothingFound);
}

/// A de-identified SEG/SR export of a synthetic CT with patient identifiers
/// (the Stage 31 exporter, `DicomExportOptions::deid`). Returns the file paths
/// and their `(sop class, sop instance)` UIDs. Mirrors `tests/dicomstow.rs`.
fn deidentified_export(dir: &std::path::Path) -> (Vec<PathBuf>, Vec<(String, String)>) {
    let dims = Dims3::new(8, 8, 4);
    let values = vec![0.0f32; dims.voxel_count()];
    let volume = Volume::from_physical(dims, Vec3::new(1.0, 1.0, 2.0), &values).unwrap();
    let mut data = vec![0u8; dims.voxel_count()];
    for k in 1..3 {
        for j in 2..6 {
            for i in 2..6 {
                data[dims.index(i, j, k)] = 1;
            }
        }
    }
    let mut seg = Segment::new(1, "lesion");
    seg.provenance = Provenance::human(Timestamp(1_790_000_000));
    let set = SegmentationSet::from_labels(LabelMap::from_data(dims, data).unwrap(), vec![seg]);
    let mut annotations = AnnotationSet::default();
    annotations.add(
        SliceKey::new(SliceAxis::Axial, 1),
        Annotation::Distance { a: Vec2::new(2.0, 3.0), b: Vec2::new(6.0, 3.0) },
    );
    let report = AnnotationReport::build(PathBuf::new(), Default::default(), &volume, &annotations);
    let source = DicomSource {
        patient_name: "DOE^JANE".into(),
        patient_id: "MRN-4711".into(),
        patient_birth_date: "19700101".into(),
        accession_number: "ACC-0815".into(),
        study_uid: STUDY_UID.into(),
        series_uid: SERIES_UID.into(),
        study_date: "20240428".into(),
        instances: (0..4)
            .map(|k| SourceInstance {
                sop_class_uid: uids::CT_IMAGE_STORAGE.into(),
                sop_instance_uid: format!("{SERIES_UID}.{}", k + 1),
                position: Some(DVec3::new(0.0, 0.0, k as f64 * 2.0)),
                normal: Some(DVec3::new(0.0, 0.0, 1.0)),
            })
            .collect(),
        ..Default::default()
    };
    let opts = DicomExportOptions {
        series_id: "orthanc-test".into(),
        software_version: "test".into(),
        deid: Some(ProfileOptions { uid_salt: "site-secret".into(), ..Default::default() }),
        ..Default::default()
    };
    let exporter = DicomExporter::new(Some(&source), &opts);
    let (seg_path, sr_path) = (dir.join("segmentation.dcm"), dir.join("measurements.dcm"));
    exporter.export(&volume, &set, &report, &seg_path, &sr_path).unwrap();
    let mut paths = Vec::new();
    let mut ids = Vec::new();
    for path in [seg_path, sr_path] {
        if path.exists() {
            let obj = dicom_object::open_file(&path).unwrap();
            let class = obj.element(tags::SOP_CLASS_UID).unwrap().to_str().unwrap().to_string();
            let instance = obj.element(tags::SOP_INSTANCE_UID).unwrap().to_str().unwrap().to_string();
            ids.push((class, instance));
            paths.push(path);
        }
    }
    (paths, ids)
}
