//! STOW-RS send-off tests against a mock PACS (Stage 32, PR 3).
//!
//! The mock is a `tiny_http` server on loopback that accepts the STOW POST,
//! keeps the multipart body and answers a canned Store Instances Response
//! (PS3.18 Annex I). The objects sent are the de-identified SEG/SR of a
//! Stage 31 export built at run time from a synthetic phantom with patient
//! identifiers: no patient or volume data is committed. Integration against
//! a real Orthanc (STOW validated by highdicom) is PR 4.
//!
//! // covers 32.2-a, 32.2-b

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dicom_dictionary_std::{tags, uids};
use ferrum_domain::{
    Annotation, AnnotationReport, AnnotationSet, Dims3, LabelMap, NoProgress, Provenance, Segment, SegmentationSet,
    SliceAxis, SliceKey, Timestamp, Volume,
};
use ferrum_io::dicom::deid::ProfileOptions;
use ferrum_io::dicom::export::{DicomExportOptions, DicomExporter, DicomSource, SourceInstance};
use ferrum_io::{DicomWebStow, StowStatus};
use glam::{DVec3, Vec2, Vec3};

const PATIENT_NAME: &str = "DOE^JANE";
const PATIENT_ID: &str = "MRN-4711";
const BIRTH_DATE: &str = "19700101";
const ACCESSION: &str = "ACC-0815";
const STUDY_UID: &str = "1.2.826.0.1.3680043.2.1";
const SERIES_UID: &str = "1.2.826.0.1.3680043.2.1125.1";
/// Strings that must never reach the PACS in a de-identified export.
const SECRETS: [&str; 4] = [PATIENT_NAME, PATIENT_ID, BIRTH_DATE, ACCESSION];

/// What the mock answers with.
#[derive(Clone)]
struct Plan {
    status: u16,
    body: String,
    delay: Duration,
    auth: Option<String>,
}

impl Default for Plan {
    fn default() -> Self {
        Self { status: 200, body: "{}".into(), delay: Duration::ZERO, auth: None }
    }
}

struct MockPacs {
    root: String,
    /// `(Content-Type, body)` of each request.
    bodies: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    auths: Arc<Mutex<Vec<Option<String>>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockPacs {
    fn start(plan: Plan) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            _ => panic!("expected an IP listen address"),
        };
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let auths = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (bodies_t, auths_t, stop_t) = (bodies.clone(), auths.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while !stop_t.load(Ordering::Relaxed) {
                let Ok(Some(mut req)) = server.recv_timeout(Duration::from_millis(50)) else { continue };
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("Authorization"))
                    .map(|h| h.value.as_str().to_string());
                auths_t.lock().unwrap().push(auth.clone());
                let content_type = req
                    .headers()
                    .iter()
                    .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("Content-Type"))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                let mut body = Vec::new();
                let _ = req.as_reader().read_to_end(&mut body);
                bodies_t.lock().unwrap().push((content_type, body));
                if let Some(required) = &plan.auth {
                    if auth.as_deref() != Some(required.as_str()) {
                        let _ = req.respond(tiny_http::Response::from_string("denied").with_status_code(401));
                        continue;
                    }
                }
                if !plan.delay.is_zero() {
                    std::thread::sleep(plan.delay);
                }
                let response = tiny_http::Response::from_string(plan.body.clone())
                    .with_status_code(plan.status)
                    .with_header("Content-Type: application/dicom+json".parse::<tiny_http::Header>().unwrap());
                let _ = req.respond(response);
            }
        });
        Self { root: format!("dicomweb://127.0.0.1:{port}/dicom-web"), bodies, auths, stop, handle: Some(handle) }
    }

    fn bodies(&self) -> Vec<(String, Vec<u8>)> {
        self.bodies.lock().unwrap().clone()
    }

    fn auths(&self) -> Vec<Option<String>> {
        self.auths.lock().unwrap().clone()
    }
}

impl Drop for MockPacs {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// A de-identified SEG/SR export of a synthetic CT with patient identifiers.
/// Returns the two file paths and their `(sop class, sop instance)` UIDs.
fn deidentified_export(dir: &Path) -> (Vec<PathBuf>, Vec<(String, String)>) {
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
        patient_name: PATIENT_NAME.into(),
        patient_id: PATIENT_ID.into(),
        patient_birth_date: BIRTH_DATE.into(),
        accession_number: ACCESSION.into(),
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
        series_id: "stow-test".into(),
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

/// First index of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Splits a STOW multipart body into its Part 10 instances and opens them.
/// Parses raw bytes (a lossy string conversion would corrupt the binary
/// pixel data); the boundary comes from the request's Content-Type.
fn received_objects(
    content_type: &str,
    body: &[u8],
) -> Vec<dicom_object::FileDicomObject<dicom_object::InMemDicomObject>> {
    let boundary = content_type
        .split("boundary=")
        .nth(1)
        .and_then(|b| b.split([';', '\r', '\n', ' ']).next())
        .unwrap_or("")
        .trim();
    let delim = format!("--{boundary}");
    let delim = delim.as_bytes();
    let mut parts: Vec<&[u8]> = Vec::new();
    let mut start = 0;
    while let Some(pos) = find_subslice(&body[start..], delim) {
        parts.push(&body[start..start + pos]);
        start += pos + delim.len();
    }
    parts.push(&body[start..]);
    let mut objects = Vec::new();
    for part in parts {
        let Some(sep) = find_subslice(part, b"\r\n\r\n") else { continue };
        let payload = &part[sep + 4..];
        let payload = payload.strip_suffix(b"\r\n").unwrap_or(payload);
        if payload.is_empty() || payload == b"--" {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("part.dcm");
        std::fs::write(&path, payload).unwrap();
        if let Ok(obj) = dicom_object::open_file(&path) {
            objects.push(obj);
        }
    }
    objects
}

/// A Store Instances Response referencing `ids` with the given statuses.
fn response_json(success: &[(String, String)], warning: &[(String, String)], failure: &[(String, String)]) -> String {
    let seq = |ids: &[(String, String)], reason: Option<(&str, u16)>| {
        let items: Vec<_> = ids
            .iter()
            .map(|(class, instance)| {
                let mut item = serde_json::json!({
                    "00081150": { "vr": "UI", "Value": [class] },
                    "00081155": { "vr": "UI", "Value": [instance] },
                });
                if let Some((tag, code)) = reason {
                    item[tag] = serde_json::json!({ "vr": "US", "Value": [code] });
                }
                item
            })
            .collect();
        serde_json::json!({ "vr": "SQ", "Value": items })
    };
    let mut obj = serde_json::Map::new();
    if !success.is_empty() {
        obj.insert("00081190".into(), seq(success, None));
    }
    if !warning.is_empty() {
        obj.insert("00081199".into(), seq(warning, Some(("00081196", 0xB000))));
    }
    if !failure.is_empty() {
        obj.insert("00081198".into(), seq(failure, Some(("00081197", 0xA700))));
    }
    serde_json::Value::Object(obj).to_string()
}

#[test]
fn stow_sends_a_deidentified_export_and_checks_the_response() {
    // covers 32.2-a: a de-identified SEG/SR export is stored; the response is
    // checked per instance, and no identifier reaches the PACS.
    let dir = tempfile::tempdir().unwrap();
    let (paths, ids) = deidentified_export(dir.path());
    assert_eq!(paths.len(), 2, "a SEG and an SR");
    let plan = Plan { body: response_json(&ids, &[], &[]), ..Default::default() };
    let pacs = MockPacs::start(plan);
    let report = DicomWebStow::new().store_files(&pacs.root, &paths, &NoProgress).unwrap();
    assert_eq!(report.count(StowStatus::Success), 2);
    assert_eq!(report.count(StowStatus::Failure), 0);
    assert!(report.all_stored());
    assert_eq!(report.instances[0].sop_instance_uid, ids[0].1);

    let bodies = pacs.bodies();
    assert_eq!(bodies.len(), 1);
    let objects = received_objects(&bodies[0].0, &bodies[0].1);
    assert_eq!(objects.len(), 2, "both objects arrived");
    for obj in &objects {
        let removed = obj.element(tags::PATIENT_IDENTITY_REMOVED).unwrap().to_str().unwrap();
        assert_eq!(removed.trim(), "YES", "the object is marked de-identified");
        let bytes = obj.element(tags::PATIENT_NAME).map(|e| e.to_str().unwrap().to_string()).unwrap_or_default();
        assert!(!bytes.contains(PATIENT_NAME));
    }
    // No identifier appears anywhere in the raw multipart body.
    let raw = String::from_utf8_lossy(&bodies[0].1);
    for secret in SECRETS {
        assert!(!raw.contains(secret), "identifier {secret:?} reached the PACS");
    }
}

#[test]
fn stow_response_reports_warning_and_failure_per_instance() {
    // covers 32.2-a: success/warning/failure are distinguished per instance
    let dir = tempfile::tempdir().unwrap();
    let (paths, ids) = deidentified_export(dir.path());
    let plan = Plan {
        body: response_json(&ids[..1], &ids[1..], &[(uids::SEGMENTATION_STORAGE.into(), "1.2.3.999".into())]),
        ..Default::default()
    };
    let pacs = MockPacs::start(plan);
    let report = DicomWebStow::new().store_files(&pacs.root, &paths, &NoProgress).unwrap();
    assert_eq!(report.count(StowStatus::Success), 1);
    assert_eq!(report.count(StowStatus::Warning), 1);
    assert_eq!(report.count(StowStatus::Failure), 1);
    assert!(!report.all_stored());
    assert_eq!(report.instances[1].reason, Some(0xB000));
    assert_eq!(report.instances[2].reason, Some(0xA700));
}

#[test]
fn stow_failure_status_is_an_error() {
    // covers 32.2-a: a 409 (one or more instances rejected) is reported
    let dir = tempfile::tempdir().unwrap();
    let (paths, _) = deidentified_export(dir.path());
    let plan = Plan { status: 409, body: response_json(&[], &[], &[]), ..Default::default() };
    let pacs = MockPacs::start(plan);
    let err = DicomWebStow::new().store_files(&pacs.root, &paths, &NoProgress).unwrap_err();
    assert!(matches!(err, ferrum_domain::RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("409"), "{err}");
}

#[test]
fn stow_sends_auth_and_reports_a_refusal() {
    // covers 32.2-a: credentials ride in the header, never the URL
    let dir = tempfile::tempdir().unwrap();
    let (paths, ids) = deidentified_export(dir.path());
    let plan =
        Plan { body: response_json(&ids, &[], &[]), auth: Some("Basic dXNlcjpwYXNz".into()), ..Default::default() };
    let pacs = MockPacs::start(plan);
    let report =
        DicomWebStow::new().with_basic_auth("user", "pass").store_files(&pacs.root, &paths, &NoProgress).unwrap();
    assert!(report.all_stored());
    assert_eq!(pacs.auths()[0].as_deref(), Some("Basic dXNlcjpwYXNz"));
    // wrong credentials → 401 → Io, no panic
    let err =
        DicomWebStow::new().with_basic_auth("user", "wrong").store_files(&pacs.root, &paths, &NoProgress).unwrap_err();
    assert!(matches!(err, ferrum_domain::RepositoryError::Io(_)), "{err}");
    assert!(!err.to_string().contains("wrong"), "{err}");
}

#[test]
fn stow_empty_input_and_timeout_are_reported() {
    // covers 32.2-a: nothing to send is NothingFound; a slow PACS times out
    let pacs = MockPacs::start(Plan::default());
    let err = DicomWebStow::new().store(&pacs.root, &[], &NoProgress).unwrap_err();
    assert_eq!(err, ferrum_domain::RepositoryError::NothingFound);

    let dir = tempfile::tempdir().unwrap();
    let (paths, _) = deidentified_export(dir.path());
    let slow = MockPacs::start(Plan { delay: Duration::from_millis(400), ..Default::default() });
    let err = DicomWebStow::new()
        .with_timeout(Duration::from_millis(80))
        .store_files(&slow.root, &paths, &NoProgress)
        .unwrap_err();
    assert!(matches!(err, ferrum_domain::RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("timed out"), "{err}");
}

#[test]
fn stow_target_must_be_a_dicomweb_url() {
    // covers 32.2-b: the host-only client takes a DICOMweb service root
    let dir = tempfile::tempdir().unwrap();
    let (paths, _) = deidentified_export(dir.path());
    let err = DicomWebStow::new().store_files("dicom://127.0.0.1:104/PACS", &paths, &NoProgress).unwrap_err();
    assert!(matches!(err, ferrum_domain::RepositoryError::Unsupported(_)), "{err}");
}
