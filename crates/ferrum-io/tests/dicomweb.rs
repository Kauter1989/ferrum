//! DICOMweb client tests against a mock PACS (Stage 32, PR 1).
//!
//! The mock is a `tiny_http` server on loopback serving canned QIDO-RS JSON
//! and WADO-RS multipart responses built from the run-time synthetic phantom
//! (`common`): no patient or volume data is committed. Integration against a
//! real Orthanc (the pinned phantom of 32.1-b, STOW, highdicom) is PR 4.
//!
//! // covers 32.1-a, 32.1-b (mock), 32.1-c

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::SyntheticSlice;
use ferrum_domain::{CancelFlag, Dims3, NoProgress, ProgressSink, RepositoryError, VolumeRepository};
use ferrum_io::{CompositeRepository, DicomRepository, DicomWebRepository};

const STUDY_UID: &str = "1.2.826.0.1.3680043.2.1";
const SERIES_UID: &str = "1.2.826.0.1.3680043.2.1125.1";

/// One request as the mock PACS saw it.
#[derive(Debug, Clone)]
struct Seen {
    url: String,
    auth: Option<String>,
}

/// What the mock answers with.
#[derive(Clone)]
struct Plan {
    qido_status: u16,
    qido_type: String,
    qido_body: String,
    wado_parts: Vec<Vec<u8>>,
    /// Part content type override (a non-DICOM part must be refused).
    wado_part_type: String,
    delay: Duration,
    /// Required Authorization header; a mismatch answers 401.
    auth: Option<String>,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            qido_status: 200,
            qido_type: "application/dicom+json".into(),
            qido_body: qido_json(4, 5, 8),
            wado_parts: Vec::new(),
            wado_part_type: "application/dicom".into(),
            delay: Duration::ZERO,
            auth: None,
        }
    }
}

/// A QIDO series result in the DICOM JSON model.
fn qido_json(instances: u32, rows: u16, cols: u16) -> String {
    serde_json::json!([{
        "00080060": { "vr": "CS", "Value": ["CT"] },
        "0008103E": { "vr": "LO", "Value": ["SYNTH"] },
        "0020000D": { "vr": "UI", "Value": [STUDY_UID] },
        "0020000E": { "vr": "UI", "Value": [SERIES_UID] },
        "00201209": { "vr": "IS", "Value": [instances.to_string()] },
        "00280010": { "vr": "US", "Value": [rows] },
        "00280011": { "vr": "US", "Value": [cols] },
    }])
    .to_string()
}

/// A WADO-RS `multipart/related` body of Part 10 instances.
fn multipart(parts: &[Vec<u8>], part_type: &str) -> (Vec<u8>, String) {
    let boundary = "ferrum-mock-boundary";
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Type: {part_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(part);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (body, format!("multipart/related; type=\"application/dicom\"; boundary={boundary}"))
}

struct MockPacs {
    root: String,
    seen: Arc<Mutex<Vec<Seen>>>,
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
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen_t, stop_t) = (seen.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while !stop_t.load(Ordering::Relaxed) {
                let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(50)) else { continue };
                let url = req.url().to_string();
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("Authorization"))
                    .map(|h| h.value.as_str().to_string());
                seen_t.lock().unwrap().push(Seen { url: url.clone(), auth: auth.clone() });
                if let Some(required) = &plan.auth {
                    if auth.as_deref() != Some(required.as_str()) {
                        let _ = req.respond(tiny_http::Response::from_string("denied").with_status_code(401));
                        continue;
                    }
                }
                if !plan.delay.is_zero() {
                    std::thread::sleep(plan.delay);
                }
                let path = url.split('?').next().unwrap_or(&url);
                let response = if path.ends_with("/series") {
                    tiny_http::Response::from_string(plan.qido_body.clone())
                        .with_status_code(plan.qido_status)
                        .with_header(format!("Content-Type: {}", plan.qido_type).parse::<tiny_http::Header>().unwrap())
                } else {
                    let (body, ct) = multipart(&plan.wado_parts, &plan.wado_part_type);
                    tiny_http::Response::from_data(body)
                        .with_status_code(200)
                        .with_header(format!("Content-Type: {ct}").parse::<tiny_http::Header>().unwrap())
                };
                let _ = req.respond(response);
            }
        });
        Self { root: format!("http://127.0.0.1:{port}/dicom-web"), seen, stop, handle: Some(handle) }
    }

    /// The same root as a `dicomweb://` path for the repository.
    fn url(&self, query: &str) -> PathBuf {
        let rest = self.root.strip_prefix("http://").unwrap();
        PathBuf::from(format!("dicomweb://{rest}{query}"))
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
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

/// Writes the phantom series and returns its Part 10 bytes in reverse file
/// order (the reader must order by position, not by arrival).
fn phantom_parts(
    rows: u16,
    cols: u16,
    n: u32,
    configure: impl Fn(&mut SyntheticSlice),
) -> (tempfile::TempDir, Vec<Vec<u8>>) {
    let dir = tempfile::tempdir().unwrap();
    common::write_series(dir.path(), rows, cols, n, 2.5, configure);
    let mut names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    names.reverse();
    let parts = names.iter().map(|p| std::fs::read(p).unwrap()).collect();
    (dir, parts)
}

/// A CT phantom with a Modality LUT (intercept −1024) and a stored window.
fn ct_phantom(rows: u16, cols: u16, n: u32) -> (tempfile::TempDir, Vec<Vec<u8>>) {
    phantom_parts(rows, cols, n, |s| {
        s.intercept = -1024.0;
        s.window = Some((40.0, 400.0));
    })
}

fn file_loaded(dir: &tempfile::TempDir) -> ferrum_domain::LoadedSeries {
    let repo = DicomRepository;
    let series = repo.scan(&[dir.path().to_path_buf()], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    repo.load(&series[0], &NoProgress).unwrap()
}

struct CancelSink(Arc<CancelFlag>);

impl ProgressSink for CancelSink {
    fn report(&self, _fraction: f32, _message: &str) {}
    fn is_cancelled(&self) -> bool {
        self.0.is_set()
    }
}

#[test]
fn scan_queries_and_describes_series() {
    // covers 32.1-a: QIDO filters and includefield reach the server; the
    // descriptor carries what load needs, without patient identifiers.
    let (_dir, parts) = ct_phantom(5, 8, 4);
    let pacs = MockPacs::start(Plan { wado_parts: parts, ..Default::default() });
    let repo = DicomWebRepository::new();
    let series = repo.scan(&[pacs.url("?PatientID=123&limit=50")], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    let d = &series[0];
    assert_eq!(d.format, "DICOMweb");
    assert_eq!(d.modality, "CT");
    assert_eq!(d.dims, Dims3::new(8, 5, 4));
    assert_eq!(d.id, format!("{SERIES_UID}#w:127.0.0.1:{}", port_of(&pacs.root)));
    assert!(d.description.contains("SYNTH"), "{}", d.description);
    assert!(!d.description.contains("123"), "no patient id in the description: {}", d.description);
    assert_eq!(d.sources.len(), 1);
    assert_eq!(
        d.sources[0].to_str().unwrap(),
        format!("dicomweb://127.0.0.1:{}/dicom-web/studies/{STUDY_UID}/series/{SERIES_UID}", port_of(&pacs.root))
    );
    let seen = pacs.seen();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].url.starts_with("/dicom-web/series?"), "{}", seen[0].url);
    assert!(seen[0].url.contains("00100020=123"), "PatientID filter: {}", seen[0].url);
    assert!(seen[0].url.contains("limit=50"), "{}", seen[0].url);
    assert!(seen[0].url.contains("includefield=00080060"), "Modality includefield: {}", seen[0].url);
    assert!(seen[0].url.contains("includefield=00201209"), "instance count includefield: {}", seen[0].url);
}

#[test]
fn a_study_filter_uses_the_scoped_endpoint() {
    // covers 32.1-a
    let (_dir, parts) = ct_phantom(5, 8, 4);
    let pacs = MockPacs::start(Plan { wado_parts: parts, ..Default::default() });
    let repo = DicomWebRepository::new();
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}&Modality=CT"))], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    let seen = pacs.seen();
    assert!(seen[0].url.starts_with(&format!("/dicom-web/studies/{STUDY_UID}/series?")), "{}", seen[0].url);
    assert!(seen[0].url.contains("00080060=CT"), "Modality filter: {}", seen[0].url);
    assert!(!seen[0].url.contains("0020000d="), "the study uid rides in the path, not twice: {}", seen[0].url);
}

#[test]
fn load_matches_the_file_reader_voxel_for_voxel() {
    // covers 32.1-a, 32.1-b (mock): the retrieved study is the same volume
    // the file reader produces from the same instances — pixels, grid,
    // geometry, window — and the values are the phantom's, not noise.
    let (dir, parts) = ct_phantom(5, 8, 4);
    let pacs = MockPacs::start(Plan { wado_parts: parts, ..Default::default() });
    let repo = DicomWebRepository::new();
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    let web = repo.load(&series[0], &NoProgress).unwrap();
    let file = file_loaded(&dir);

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
    assert_eq!(web.metadata.modality, "CT");
    assert_eq!(web.metadata.study.study_instance_uid, STUDY_UID);
    assert_eq!(web.metadata.study.series_instance_uid, SERIES_UID);
    assert_eq!(web.metadata.default_window, file.metadata.default_window);
    assert_eq!(web.metadata.warnings, file.metadata.warnings);
    assert_eq!(web.metadata.source, series[0].sources[0]);
    // The WADO request addressed the series the descriptor named.
    let seen = pacs.seen();
    assert!(seen.iter().any(|s| s.url == format!("/dicom-web/studies/{STUDY_UID}/series/{SERIES_UID}")), "{seen:?}");
}

#[test]
fn a_sagittal_series_is_stored_in_lps() {
    // covers 32.1-a (L23): a non-axial series is reoriented exactly as the
    // file reader does — canonical axes, permuted dims, same voxels.
    let (dir, parts) = phantom_parts(5, 8, 4, |s| {
        s.row_dir = [0.0, 1.0, 0.0];
        s.col_dir = [0.0, 0.0, -1.0];
        s.position = [(s.instance as f64 - 1.0) * 2.5, -40.0, -20.0];
    });
    let pacs = MockPacs::start(Plan { wado_parts: parts, ..Default::default() });
    let repo = DicomWebRepository::new();
    let series = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap();
    let web = repo.load(&series[0], &NoProgress).unwrap();
    let file = file_loaded(&dir);
    assert_eq!(web.volume, file.volume);
    assert_eq!(web.volume.dims(), Dims3::new(4, 8, 5), "sagittal stack permuted into LPS");
    let d = web.volume.geometry().direction;
    assert_eq!(d, glam::Mat3::IDENTITY, "canonical LPS axes");
}

#[test]
fn an_empty_query_is_nothing_found() {
    // covers 32.1-c
    let pacs = MockPacs::start(Plan { qido_body: "[]".into(), ..Default::default() });
    let repo = DicomWebRepository::new();
    let err = repo.scan(&[pacs.url("?PatientID=nobody")], &NoProgress).unwrap_err();
    assert_eq!(err, RepositoryError::NothingFound);
}

#[test]
fn an_empty_retrieval_is_nothing_found() {
    // covers 32.1-c: the series is advertised but retrieves no instances
    let pacs = MockPacs::start(Plan::default());
    let repo = DicomWebRepository::new();
    let series = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap();
    let err = repo.load(&series[0], &NoProgress).unwrap_err();
    assert_eq!(err, RepositoryError::NothingFound);
}

#[test]
fn a_connection_refusal_is_an_io_error() {
    // covers 32.1-c: no server on the port — an error, never a panic
    let port = {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            _ => panic!("expected an IP listen address"),
        }
    };
    let repo = DicomWebRepository::new().with_timeout(Duration::from_secs(5));
    let url = PathBuf::from(format!("dicomweb://127.0.0.1:{port}/dicom-web?PatientID=PID-SECRET-987654"));
    let err = repo.scan(&[url], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("connection"), "{err}");
    assert!(!err.to_string().contains("PID-SECRET-987654"), "the query never leaks: {err}");
}

#[test]
fn http_errors_are_io_errors() {
    // covers 32.1-c
    let pacs = MockPacs::start(Plan { qido_status: 500, qido_body: "boom".into(), ..Default::default() });
    let repo = DicomWebRepository::new();
    let err = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("500"), "{err}");
}

#[test]
fn auth_headers_are_sent_and_401_is_reported() {
    // covers 32.1-c: basic and bearer reach the server; a refusal is an
    // authentication error naming the host, not the credentials.
    // base64("orthanc:orthanc")
    let basic = "Basic b3J0aGFuYzpvcnRoYW5j";
    let pacs = MockPacs::start(Plan { auth: Some(basic.into()), ..Default::default() });
    let anonymous = DicomWebRepository::new();
    let err = anonymous.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("authentication"), "{err}");
    assert!(!err.to_string().contains("orthanc"), "credentials never leak: {err}");

    let repo = DicomWebRepository::new().with_basic_auth("orthanc", "orthanc");
    let series = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    assert!(pacs.seen().iter().any(|s| s.auth.as_deref() == Some(basic)), "{:?}", pacs.seen());
    drop(pacs);

    let pacs = MockPacs::start(Plan { auth: Some("Bearer tok-123".into()), ..Default::default() });
    let repo = DicomWebRepository::new().with_bearer_token("tok-123");
    let series = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    assert!(pacs.seen().iter().any(|s| s.auth.as_deref() == Some("Bearer tok-123")), "{:?}", pacs.seen());
}

#[test]
fn a_non_dicom_response_is_corrupt() {
    // covers 32.1-c: an HTML login page where DICOM JSON belongs
    let pacs = MockPacs::start(Plan {
        qido_type: "text/html".into(),
        qido_body: "<html>please log in</html>".into(),
        ..Default::default()
    });
    let repo = DicomWebRepository::new();
    let err = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Corrupt(_)), "{err}");
}

#[test]
fn a_non_dicom_multipart_part_is_corrupt() {
    // covers 32.1-c: WADO parts must be DICOM
    let pacs = MockPacs::start(Plan {
        wado_parts: vec![b"<html>not dicom</html>".to_vec()],
        wado_part_type: "text/html".into(),
        ..Default::default()
    });
    let repo = DicomWebRepository::new();
    let series = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap();
    let err = repo.load(&series[0], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Corrupt(_)), "{err}");
}

#[test]
fn a_slow_server_times_out_as_io() {
    // covers 32.1-c: the deadline fires and is reported, never a hang
    let pacs = MockPacs::start(Plan { delay: Duration::from_millis(1500), ..Default::default() });
    let repo = DicomWebRepository::new().with_timeout(Duration::from_millis(200));
    let err = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("timed out"), "{err}");
}

#[test]
fn cancellation_is_reported() {
    // covers 32.1-c
    let (_dir, parts) = ct_phantom(5, 8, 4);
    let pacs = MockPacs::start(Plan { wado_parts: parts, ..Default::default() });
    let flag = Arc::new(CancelFlag::default());
    flag.cancel();
    let sink = CancelSink(flag.clone());
    let repo = DicomWebRepository::new();
    let err = repo.scan(&[pacs.url("?PatientID=123")], &sink).unwrap_err();
    assert_eq!(err, RepositoryError::Cancelled);
    // A load cancelled mid-retrieval stops at the first instance.
    let flag = Arc::new(CancelFlag::default());
    let series = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap();
    flag.cancel();
    let err = repo.load(&series[0], &CancelSink(flag)).unwrap_err();
    assert_eq!(err, RepositoryError::Cancelled);
}

#[test]
fn the_composite_opens_files_and_pacs_urls() {
    // covers 32.1-a, 32.1-b (mock): the existing viewer path (composite
    // repository) opens a PACS study with no app-layer change, and file
    // opening is unaffected.
    let (dir, parts) = ct_phantom(5, 8, 4);
    let pacs = MockPacs::start(Plan { wado_parts: parts, ..Default::default() });
    let repo = CompositeRepository::default();
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].format, "DICOMweb");
    let web = repo.load(&series[0], &NoProgress).unwrap();
    assert_eq!(web.volume, file_loaded(&dir).volume);
    // The same composite still opens the files themselves.
    let files = repo.scan(&[dir.path().to_path_buf()], &NoProgress).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].format, "DICOM");
}

fn port_of(root: &str) -> u16 {
    let authority = root.strip_prefix("http://").unwrap().split('/').next().unwrap();
    authority.rsplit_once(':').unwrap().1.parse().unwrap()
}
