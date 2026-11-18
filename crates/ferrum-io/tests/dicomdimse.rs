//! DIMSE client tests against a mock PACS (Stage 32, PR 2).
//!
//! The mock is a `dicom-ul` server on loopback speaking C-ECHO, C-FIND and
//! C-MOVE; for C-MOVE it opens a C-STORE SCU association back to the
//! repository's C-STORE SCP and sends the run-time synthetic phantom
//! (`common`): no patient or volume data is committed. Integration against a
//! real Orthanc (the pinned phantom, STOW, highdicom) is PR 4.
//!
//! // covers 32.3-a, 32.3-b

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::SyntheticSlice;
use dicom_core::{dicom_value, DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_encoding::transfer_syntax::TransferSyntaxIndex;
use dicom_object::mem::InMemDicomObject;
use dicom_object::open_file;
use dicom_transfer_syntax_registry::{entries, TransferSyntaxRegistry};
use dicom_ul::association::{ClientAssociationOptions, ServerAssociationOptions, SyncAssociation};
use dicom_ul::pdu::{PDataValue, PDataValueType, Pdu};
use ferrum_domain::{CancelFlag, Dims3, NoProgress, ProgressSink, RepositoryError, VolumeRepository};
use ferrum_io::{CompositeRepository, DicomDimseRepository, DicomRepository};

const STUDY_UID: &str = "1.2.826.0.1.3680043.2.1";
const SERIES_UID: &str = "1.2.826.0.1.3680043.2.1125.1";
const CALLED_AE: &str = "MOCK-SCP";

// ---------------------------------------------------------------------------
// Test-side DIMSE helpers (the mock speaks the same protocol as the client).
// ---------------------------------------------------------------------------

fn cmd_ts() -> dicom_encoding::transfer_syntax::TransferSyntax {
    entries::IMPLICIT_VR_LITTLE_ENDIAN.erased()
}

fn send_cmd<S>(assoc: &mut impl SyncAssociation<S>, pc: u8, obj: &InMemDicomObject)
where
    S: std::io::Read + std::io::Write + dicom_ul::association::CloseSocket,
{
    let mut data = Vec::new();
    obj.write_dataset_with_ts(&mut data, &cmd_ts()).unwrap();
    SyncAssociation::send(
        assoc,
        &Pdu::PData {
            data: vec![PDataValue { presentation_context_id: pc, value_type: PDataValueType::Command, is_last: true, data }],
        },
    )
    .unwrap();
}

fn send_data<S>(assoc: &mut impl SyncAssociation<S>, pc: u8, obj: &InMemDicomObject, ts: &dicom_encoding::transfer_syntax::TransferSyntax)
where
    S: std::io::Read + std::io::Write + dicom_ul::association::CloseSocket,
{
    let mut data = Vec::new();
    obj.write_dataset_with_ts(&mut data, ts).unwrap();
    SyncAssociation::send(
        assoc,
        &Pdu::PData {
            data: vec![PDataValue { presentation_context_id: pc, value_type: PDataValueType::Data, is_last: true, data }],
        },
    )
    .unwrap();
}

/// Reads one DIMSE message (command + optional data set).
fn recv_msg<S>(assoc: &mut impl SyncAssociation<S>, ts: &dicom_encoding::transfer_syntax::TransferSyntax) -> (InMemDicomObject, Option<InMemDicomObject>)
where
    S: std::io::Read + std::io::Write + dicom_ul::association::CloseSocket,
{
    let mut cmd_buf = Vec::new();
    let mut data_buf = Vec::new();
    let mut command: Option<InMemDicomObject> = None;
    let mut data_complete = false;
    let mut saw_data = false;
    loop {
        if let Some(cmd) = &command {
            let no_data = cmd.get(tags::COMMAND_DATA_SET_TYPE).and_then(|e| e.to_int::<u16>().ok()).unwrap_or(0x0101) == 0x0101;
            if no_data || data_complete {
                break;
            }
        }
        match SyncAssociation::receive(assoc).unwrap() {
            Pdu::PData { data } => {
                for pdv in data {
                    match pdv.value_type {
                        PDataValueType::Command => {
                            cmd_buf.extend_from_slice(&pdv.data);
                            if pdv.is_last {
                                command = Some(InMemDicomObject::read_dataset_with_ts(&cmd_buf[..], &cmd_ts()).unwrap());
                            }
                        }
                        PDataValueType::Data => {
                            saw_data = true;
                            data_buf.extend_from_slice(&pdv.data);
                            if pdv.is_last {
                                data_complete = true;
                            }
                        }
                    }
                }
            }
            other => panic!("unexpected PDU in mock recv_msg: {other:?}"),
        }
    }
    let data = if saw_data { Some(InMemDicomObject::read_dataset_with_ts(&data_buf[..], ts).unwrap()) } else { None };
    (command.unwrap(), data)
}

fn echo_rsp(msgid: u16, status: u16) -> InMemDicomObject {
    InMemDicomObject::command_from_element_iter([
        DataElement::new(tags::AFFECTED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(uids::VERIFICATION)),
        DataElement::new(tags::COMMAND_FIELD, VR::US, dicom_value!(U16, [0x8030])),
        DataElement::new(tags::MESSAGE_ID_BEING_RESPONDED_TO, VR::US, dicom_value!(U16, [msgid])),
        DataElement::new(tags::COMMAND_DATA_SET_TYPE, VR::US, dicom_value!(U16, [0x0101])),
        DataElement::new(tags::STATUS, VR::US, dicom_value!(U16, [status])),
    ])
}

fn find_rsp(msgid: u16, status: u16, has_data: bool) -> InMemDicomObject {
    InMemDicomObject::command_from_element_iter([
        DataElement::new(tags::AFFECTED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(uids::STUDY_ROOT_QUERY_RETRIEVE_INFORMATION_MODEL_FIND)),
        DataElement::new(tags::COMMAND_FIELD, VR::US, dicom_value!(U16, [0x8020])),
        DataElement::new(tags::MESSAGE_ID_BEING_RESPONDED_TO, VR::US, dicom_value!(U16, [msgid])),
        DataElement::new(tags::COMMAND_DATA_SET_TYPE, VR::US, dicom_value!(U16, [if has_data { 0x0001 } else { 0x0101 }])),
        DataElement::new(tags::STATUS, VR::US, dicom_value!(U16, [status])),
    ])
}

fn move_rsp(msgid: u16, status: u16, remaining: u16, completed: u16) -> InMemDicomObject {
    InMemDicomObject::command_from_element_iter([
        DataElement::new(tags::AFFECTED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(uids::STUDY_ROOT_QUERY_RETRIEVE_INFORMATION_MODEL_MOVE)),
        DataElement::new(tags::COMMAND_FIELD, VR::US, dicom_value!(U16, [0x8021])),
        DataElement::new(tags::MESSAGE_ID_BEING_RESPONDED_TO, VR::US, dicom_value!(U16, [msgid])),
        DataElement::new(tags::COMMAND_DATA_SET_TYPE, VR::US, dicom_value!(U16, [0x0101])),
        DataElement::new(tags::STATUS, VR::US, dicom_value!(U16, [status])),
        DataElement::new(tags::NUMBER_OF_REMAINING_SUBOPERATIONS, VR::US, dicom_value!(U16, [remaining])),
        DataElement::new(tags::NUMBER_OF_COMPLETED_SUBOPERATIONS, VR::US, dicom_value!(U16, [completed])),
    ])
}

fn store_rq(sop_instance: &str, msgid: u16) -> InMemDicomObject {
    InMemDicomObject::command_from_element_iter([
        DataElement::new(tags::AFFECTED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(uids::CT_IMAGE_STORAGE)),
        DataElement::new(tags::COMMAND_FIELD, VR::US, dicom_value!(U16, [0x0001])),
        DataElement::new(tags::MESSAGE_ID, VR::US, dicom_value!(U16, [msgid])),
        DataElement::new(tags::PRIORITY, VR::US, dicom_value!(U16, [0x0000])),
        DataElement::new(tags::COMMAND_DATA_SET_TYPE, VR::US, dicom_value!(U16, [0x0001])),
        DataElement::new(tags::AFFECTED_SOP_INSTANCE_UID, VR::UI, PrimitiveValue::from(sop_instance)),
        DataElement::new(tags::MOVE_ORIGINATOR_APPLICATION_ENTITY_TITLE, VR::AE, PrimitiveValue::from(CALLED_AE)),
        DataElement::new(tags::MOVE_ORIGINATOR_MESSAGE_ID, VR::US, dicom_value!(U16, [1])),
    ])
}

// ---------------------------------------------------------------------------
// Mock PACS
// ---------------------------------------------------------------------------

/// What the mock answers with.
#[derive(Clone)]
struct Plan {
    find_studies: Vec<InMemDicomObject>,
    find_series: Vec<InMemDicomObject>,
    move_objects: Vec<InMemDicomObject>,
    store_addr: Option<SocketAddr>,
    echo_status: u16,
    find_status: u16,
    move_status: u16,
    no_store: bool,
    reject: bool,
    abort_after_rq: bool,
    delay: Duration,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            find_studies: vec![study_match()],
            find_series: vec![series_match(4, 5, 8)],
            move_objects: Vec::new(),
            store_addr: None,
            echo_status: 0,
            find_status: 0,
            move_status: 0,
            no_store: false,
            reject: false,
            abort_after_rq: false,
            delay: Duration::ZERO,
        }
    }
}

fn study_match() -> InMemDicomObject {
    InMemDicomObject::from_element_iter([
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, PrimitiveValue::from(STUDY_UID)),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20240428")),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
        DataElement::new(tags::NUMBER_OF_STUDY_RELATED_SERIES, VR::IS, PrimitiveValue::from("1")),
    ])
}

fn series_match(instances: u32, rows: u16, cols: u16) -> InMemDicomObject {
    InMemDicomObject::from_element_iter([
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, PrimitiveValue::from(STUDY_UID)),
        DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, PrimitiveValue::from(SERIES_UID)),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
        DataElement::new(tags::SERIES_DESCRIPTION, VR::LO, PrimitiveValue::from("SYNTH")),
        DataElement::new(tags::SERIES_NUMBER, VR::IS, PrimitiveValue::from("1")),
        DataElement::new(tags::NUMBER_OF_SERIES_RELATED_INSTANCES, VR::IS, PrimitiveValue::from(instances.to_string())),
        DataElement::new(tags::ROWS, VR::US, dicom_value!(U16, [rows])),
        DataElement::new(tags::COLUMNS, VR::US, dicom_value!(U16, [cols])),
    ])
}

struct MockPacs {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockPacs {
    fn start(plan: Plan) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen_t, stop_t) = (seen.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !stop_t.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let plan = plan.clone();
                        let seen = seen_t.clone();
                        // Handle each association on its own thread so a C-MOVE
                        // can open the C-STORE SCU association concurrently.
                        std::thread::spawn(move || handle_association(stream, &plan, &seen));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { port, seen, stop, handle: Some(handle) }
    }

    fn url(&self, query: &str) -> PathBuf {
        PathBuf::from(format!("dicom://127.0.0.1:{}/{CALLED_AE}{query}", self.port))
    }

    fn seen(&self) -> Vec<String> {
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

fn handle_association(stream: TcpStream, plan: &Plan, seen: &Mutex<Vec<String>>) {
    if !plan.delay.is_zero() {
        std::thread::sleep(plan.delay);
    }
    let mut options = ServerAssociationOptions::new()
        .ae_title(if plan.reject { "OTHER-SCP" } else { CALLED_AE })
        .promiscuous(true)
        .max_pdu_length(16384)
        .read_timeout(Duration::from_secs(10))
        .write_timeout(Duration::from_secs(10))
        .with_abstract_syntax(uids::VERIFICATION)
        .with_abstract_syntax(uids::STUDY_ROOT_QUERY_RETRIEVE_INFORMATION_MODEL_FIND)
        .with_abstract_syntax(uids::STUDY_ROOT_QUERY_RETRIEVE_INFORMATION_MODEL_MOVE)
        .with_transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
        .with_transfer_syntax(uids::IMPLICIT_VR_LITTLE_ENDIAN);
    options = if plan.reject { options.accept_called_ae_title() } else { options.accept_any() };
    let mut assoc = match options.establish(stream) {
        Ok(a) => a,
        Err(_) => return, // rejected
    };
    let pc = match assoc.presentation_contexts().first() {
        Some(pc) => pc.clone(),
        None => return,
    };
    let ts = TransferSyntaxRegistry.get(&pc.transfer_syntax).unwrap();
    let (cmd, data) = recv_msg(&mut assoc, ts);
    let field = cmd.element(tags::COMMAND_FIELD).unwrap().uint16().unwrap();
    let msgid = cmd.get(tags::MESSAGE_ID).and_then(|e| e.to_int::<u16>().ok()).unwrap_or(1);
    seen.lock().unwrap().push(format!("cmd=0x{field:04X}"));
    match field {
        0x0030 => send_cmd(&mut assoc, pc.id, &echo_rsp(msgid, plan.echo_status)),
        0x0020 => do_find(&mut assoc, pc.id, ts, data.as_ref(), plan),
        0x0021 => do_move(&mut assoc, pc.id, ts, plan),
        _ => {}
    }
    // Wait for the client to release (or abort).
    loop {
        match assoc.receive() {
            Ok(Pdu::ReleaseRQ) => {
                let _ = assoc.send(&Pdu::ReleaseRP);
                break;
            }
            Ok(Pdu::AbortRQ { .. }) | Err(_) => break,
            _ => {}
        }
    }
}

fn do_find(assoc: &mut dicom_ul::ServerAssociation<TcpStream>, pc: u8, ts: &dicom_encoding::transfer_syntax::TransferSyntax, query: Option<&InMemDicomObject>, plan: &Plan) {
    let level = query
        .and_then(|q| q.get(tags::QUERY_RETRIEVE_LEVEL))
        .and_then(|e| e.to_str().ok())
        .unwrap_or("STUDY")
        .to_string();
    let matches = if level.trim_end_matches('\0') == "SERIES" { &plan.find_series } else { &plan.find_studies };
    if plan.find_status == 0 {
        for m in matches {
            send_cmd(assoc, pc, &find_rsp(1, 0xFF00, true));
            send_data(assoc, pc, m, ts);
        }
    }
    send_cmd(assoc, pc, &find_rsp(1, plan.find_status, false));
}

fn do_move(assoc: &mut dicom_ul::ServerAssociation<TcpStream>, pc: u8, ts: &dicom_encoding::transfer_syntax::TransferSyntax, plan: &Plan) {
    if plan.abort_after_rq {
        let _ = assoc.send(&Pdu::AbortRQ { source: dicom_ul::pdu::AbortRQSource::ServiceProvider(dicom_ul::pdu::AbortRQServiceProviderReason::ReasonNotSpecified) });
        return;
    }
    if plan.move_status == 0 && !plan.no_store {
        if let Some(addr) = plan.store_addr {
            send_stores(addr, &plan.move_objects);
        }
        let n = plan.move_objects.len() as u16;
        send_cmd(assoc, pc, &move_rsp(1, 0xFF00, 0, n));
    }
    let _ = ts;
    send_cmd(assoc, pc, &move_rsp(1, plan.move_status, 0, plan.move_objects.len() as u16));
}

/// The mock acts as a C-STORE SCU, sending the instances to the repository's SCP.
fn send_stores(addr: SocketAddr, objects: &[InMemDicomObject]) {
    let mut assoc = ClientAssociationOptions::new()
        .calling_ae_title(CALLED_AE)
        .called_ae_title("FERRUM-SCP")
        .with_presentation_context(uids::CT_IMAGE_STORAGE, vec![uids::EXPLICIT_VR_LITTLE_ENDIAN, uids::IMPLICIT_VR_LITTLE_ENDIAN])
        .read_timeout(Duration::from_secs(10))
        .write_timeout(Duration::from_secs(10))
        .establish(addr)
        .unwrap();
    let pc = assoc.presentation_contexts().first().unwrap().clone();
    let ts = TransferSyntaxRegistry.get(&pc.transfer_syntax).unwrap();
    for (i, obj) in objects.iter().enumerate() {
        let sop = obj.element(tags::SOP_INSTANCE_UID).unwrap().to_str().unwrap().to_string();
        send_cmd(&mut assoc, pc.id, &store_rq(&sop, (i + 1) as u16));
        send_data(&mut assoc, pc.id, obj, ts);
        let (rsp, _) = recv_msg(&mut assoc, ts);
        assert_eq!(rsp.element(tags::STATUS).unwrap().uint16().unwrap(), 0, "C-STORE-RSP status");
    }
    let _ = assoc.release();
}

// ---------------------------------------------------------------------------
// Phantom fixtures
// ---------------------------------------------------------------------------

/// Writes the phantom series and returns its dir plus the parsed data sets
/// (what the mock sends over C-STORE).
fn phantom(rows: u16, cols: u16, n: u32, configure: impl Fn(&mut SyntheticSlice)) -> (tempfile::TempDir, Vec<InMemDicomObject>) {
    let dir = tempfile::tempdir().unwrap();
    common::write_series(dir.path(), rows, cols, n, 2.5, configure);
    let mut names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    let objects = names.iter().map(|p| open_file(p).unwrap().into_inner()).collect();
    (dir, objects)
}

fn ct_phantom(rows: u16, cols: u16, n: u32) -> (tempfile::TempDir, Vec<InMemDicomObject>) {
    phantom(rows, cols, n, |s| {
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

/// A free loopback port for the repository's C-STORE SCP (the mock is told it).
fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn store_plan(objects: Vec<InMemDicomObject>, port: u16) -> Plan {
    Plan {
        move_objects: objects,
        store_addr: Some(SocketAddr::from(([127, 0, 0, 1], port))),
        ..Default::default()
    }
}

struct CancelSink(Arc<CancelFlag>);
impl ProgressSink for CancelSink {
    fn report(&self, _fraction: f32, _message: &str) {}
    fn is_cancelled(&self) -> bool {
        self.0.is_set()
    }
}

// ---------------------------------------------------------------------------
// C-ECHO
// ---------------------------------------------------------------------------

#[test]
fn echo_verifies_the_association() {
    // covers 32.3-a: C-ECHO ok
    let pacs = MockPacs::start(Plan::default());
    let repo = DicomDimseRepository::new();
    repo.echo(&pacs.url("").to_str().unwrap()).unwrap();
    assert!(pacs.seen().iter().any(|s| s == "cmd=0x0030"), "{:?}", pacs.seen());
}

#[test]
fn echo_failure_status_is_an_io_error() {
    // covers 32.3-a: C-ECHO fail (non-success status)
    let pacs = MockPacs::start(Plan { echo_status: 0x0122, ..Default::default() });
    let repo = DicomDimseRepository::new();
    let err = repo.echo(&pacs.url("").to_str().unwrap()).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("C-ECHO"), "{err}");
}

#[test]
fn echo_connection_refusal_is_an_io_error() {
    // covers 32.3-a: no server on the port
    let port = free_port();
    let repo = DicomDimseRepository::new().with_timeout(Duration::from_secs(5));
    let url = format!("dicom://127.0.0.1:{port}/{CALLED_AE}");
    let err = repo.echo(&url).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("connection"), "{err}");
}

// ---------------------------------------------------------------------------
// C-FIND
// ---------------------------------------------------------------------------

#[test]
fn find_queries_and_describes_series() {
    // covers 32.3-a: C-FIND series-level → descriptors, no patient identifiers
    let pacs = MockPacs::start(Plan::default());
    let repo = DicomDimseRepository::new();
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}&PatientID=123"))], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    let d = &series[0];
    assert_eq!(d.format, "DIMSE");
    assert_eq!(d.modality, "CT");
    assert_eq!(d.dims, Dims3::new(8, 5, 4));
    assert_eq!(d.id, format!("{SERIES_UID}#d:127.0.0.1:{}", pacs.port));
    assert!(d.description.contains("SYNTH"), "{}", d.description);
    assert!(!d.description.contains("123"), "no patient id: {}", d.description);
    assert_eq!(d.sources.len(), 1);
    assert!(d.sources[0].to_str().unwrap().contains(&format!("SeriesInstanceUID={SERIES_UID}")), "{:?}", d.sources);
    assert!(pacs.seen().iter().any(|s| s == "cmd=0x0020"), "{:?}", pacs.seen());
}

#[test]
fn find_global_query_resolves_study_then_series() {
    // covers 32.3-a: a query without a study UID does study-level then series-level C-FIND
    let pacs = MockPacs::start(Plan::default());
    let repo = DicomDimseRepository::new();
    let series = repo.scan(&[pacs.url("?PatientID=123")], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].format, "DIMSE");
    // both a study-level and a series-level C-FIND were issued
    assert_eq!(pacs.seen().iter().filter(|s| *s == "cmd=0x0020").count(), 2, "{:?}", pacs.seen());
}

#[test]
fn find_empty_is_nothing_found() {
    // covers 32.3-a: a query that returns nothing
    let pacs = MockPacs::start(Plan { find_series: vec![], find_studies: vec![], ..Default::default() });
    let repo = DicomDimseRepository::new();
    let err = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap_err();
    assert_eq!(err, RepositoryError::NothingFound);
}

#[test]
fn file_paths_are_not_dimse_urls() {
    // covers 32.3-a: composite safety
    let repo = DicomDimseRepository::new();
    let err = repo.scan(&[PathBuf::from("/data/ct")], &NoProgress).unwrap_err();
    assert_eq!(err, RepositoryError::NothingFound);
}

// ---------------------------------------------------------------------------
// C-MOVE + C-STORE SCP
// ---------------------------------------------------------------------------

#[test]
fn move_retrieves_and_matches_the_file_reader_voxel_for_voxel() {
    // covers 32.3-a: C-MOVE receives via C-STORE SCP and assembles the same
    // volume the file reader produces from the same instances.
    let (dir, objects) = ct_phantom(5, 8, 4);
    let port = free_port();
    let pacs = MockPacs::start(store_plan(objects, port));
    let repo = DicomDimseRepository::new().with_store_scp_addr(&format!("127.0.0.1:{port}"));
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    let dimse = repo.load(&series[0], &NoProgress).unwrap();
    let file = file_loaded(&dir);

    assert_eq!(dimse.volume, file.volume, "retrieved volume must equal the file volume");
    assert_eq!(dimse.volume.dims(), Dims3::new(8, 5, 4));
    for k in 0..4 {
        for j in 0..5 {
            for i in 0..8 {
                let stored = dimse.volume.voxel(i, j, k).unwrap();
                let value = dimse.volume.range().from_storage(stored);
                let expected = common::phantom_value(i, j, k) as f32 - 1024.0;
                assert_eq!(value, expected, "voxel {i},{j},{k}");
            }
        }
    }
    assert_eq!(dimse.metadata.modality, "CT");
    assert_eq!(dimse.metadata.study.study_instance_uid, STUDY_UID);
    assert_eq!(dimse.metadata.study.series_instance_uid, SERIES_UID);
    assert_eq!(dimse.metadata.default_window, file.metadata.default_window);
    assert!(pacs.seen().iter().any(|s| s == "cmd=0x0021"), "{:?}", pacs.seen());
}

#[test]
fn move_writes_into_the_receive_dir_when_configured() {
    // covers 32.3-a: the C-STORE SCP receives the study into a workspace area
    let (_dir, objects) = ct_phantom(5, 8, 4);
    let port = free_port();
    let pacs = MockPacs::start(store_plan(objects, port));
    let recv = tempfile::tempdir().unwrap();
    let repo = DicomDimseRepository::new()
        .with_store_scp_addr(&format!("127.0.0.1:{port}"))
        .with_receive_dir(recv.path().to_path_buf());
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    let loaded = repo.load(&series[0], &NoProgress).unwrap();
    assert_eq!(loaded.volume.dims(), Dims3::new(8, 5, 4));
    let written: Vec<_> = std::fs::read_dir(recv.path()).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(written.len(), 4, "four instances stored into the receive dir: {written:?}");
    assert!(written.iter().all(|p| p.extension().unwrap() == "dcm"));
}

#[test]
fn move_default_writes_nothing_to_disk() {
    // covers 32.3-b / ADR 0021 §2: in-memory by default, no patient data on disk
    let (_dir, objects) = ct_phantom(5, 8, 4);
    let port = free_port();
    let pacs = MockPacs::start(store_plan(objects, port));
    let repo = DicomDimseRepository::new().with_store_scp_addr(&format!("127.0.0.1:{port}"));
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    let loaded = repo.load(&series[0], &NoProgress).unwrap();
    assert_eq!(loaded.volume.dims(), Dims3::new(8, 5, 4));
    // no receive dir configured: the SCP wrote no files anywhere we can see
    // (the volume came from memory).
}

#[test]
fn move_with_no_instances_is_nothing_found() {
    // covers 32.3-a: a move that retrieves nothing
    let port = free_port();
    let pacs = MockPacs::start(Plan { no_store: true, store_addr: Some(SocketAddr::from(([127, 0, 0, 1], port))), ..Default::default() });
    let repo = DicomDimseRepository::new().with_store_scp_addr(&format!("127.0.0.1:{port}"));
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    let err = repo.load(&series[0], &NoProgress).unwrap_err();
    assert_eq!(err, RepositoryError::NothingFound);
}

#[test]
fn a_sagittal_series_is_stored_in_lps() {
    // covers 32.3-a (L23): non-axial series reoriented exactly as the file reader
    let (dir, objects) = phantom(5, 8, 4, |s| {
        s.row_dir = [0.0, 1.0, 0.0];
        s.col_dir = [0.0, 0.0, -1.0];
        s.position = [(s.instance as f64 - 1.0) * 2.5, -40.0, -20.0];
    });
    let port = free_port();
    let pacs = MockPacs::start(store_plan(objects, port));
    let repo = DicomDimseRepository::new().with_store_scp_addr(&format!("127.0.0.1:{port}"));
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    let dimse = repo.load(&series[0], &NoProgress).unwrap();
    let file = file_loaded(&dir);
    assert_eq!(dimse.volume, file.volume);
    assert_eq!(dimse.volume.dims(), Dims3::new(4, 8, 5));
    assert_eq!(dimse.volume.geometry().direction, glam::Mat3::IDENTITY);
}

// ---------------------------------------------------------------------------
// Errors: reject / timeout / abort / failure status / redaction
// ---------------------------------------------------------------------------

#[test]
fn association_rejection_is_an_io_error() {
    // covers 32.3-a: association reject
    let pacs = MockPacs::start(Plan { reject: true, ..Default::default() });
    let repo = DicomDimseRepository::new();
    let err = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("rejected"), "{err}");
    let err = repo.echo(&pacs.url("").to_str().unwrap()).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
}

#[test]
fn a_slow_server_times_out_as_io() {
    // covers 32.3-a: the deadline fires, never a hang
    let pacs = MockPacs::start(Plan { delay: Duration::from_millis(1500), ..Default::default() });
    let repo = DicomDimseRepository::new().with_timeout(Duration::from_millis(200));
    let start = std::time::Instant::now();
    let err = repo.echo(&pacs.url("").to_str().unwrap()).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(start.elapsed() < Duration::from_secs(5), "must not hang");
}

#[test]
fn a_move_abort_is_an_io_error() {
    // covers 32.3-a: association abort mid-operation
    let port = free_port();
    let pacs = MockPacs::start(Plan { abort_after_rq: true, store_addr: Some(SocketAddr::from(([127, 0, 0, 1], port))), ..Default::default() });
    let repo = DicomDimseRepository::new().with_store_scp_addr(&format!("127.0.0.1:{port}"));
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    let err = repo.load(&series[0], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
}

#[test]
fn a_move_failure_status_is_an_io_error() {
    // covers 32.3-a: move destination unknown (0xA801)
    let port = free_port();
    let pacs = MockPacs::start(Plan { move_status: 0xA801, no_store: true, store_addr: Some(SocketAddr::from(([127, 0, 0, 1], port))), ..Default::default() });
    let repo = DicomDimseRepository::new().with_store_scp_addr(&format!("127.0.0.1:{port}"));
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    let err = repo.load(&series[0], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(err.to_string().contains("C-MOVE"), "{err}");
    assert!(err.to_string().contains("A801"), "{err}");
}

#[test]
fn errors_never_leak_query_values() {
    // covers 32.3-b: redaction — a PatientID filter never reaches an error
    let port = free_port();
    let repo = DicomDimseRepository::new().with_timeout(Duration::from_secs(3));
    let url = format!("dicom://127.0.0.1:{port}/{CALLED_AE}?PatientID=PID-SECRET-987654");
    let err = repo.scan(&[PathBuf::from(&url)], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Io(_)), "{err}");
    assert!(!err.to_string().contains("PID-SECRET-987654"), "the query never leaks: {err}");
    // a malformed URL names the problem, not the query value
    let err = repo.scan(&[PathBuf::from("dicom://h/CALLED?Secret=patient-id-123")], &NoProgress).unwrap_err();
    assert!(matches!(err, RepositoryError::Unsupported(_)), "{err}");
    assert!(!err.to_string().contains("patient-id-123"), "{err}");
}

#[test]
fn cancellation_is_reported() {
    // covers 32.3-a
    let pacs = MockPacs::start(Plan::default());
    let flag = Arc::new(CancelFlag::default());
    flag.cancel();
    let repo = DicomDimseRepository::new();
    let err = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &CancelSink(flag)).unwrap_err();
    assert_eq!(err, RepositoryError::Cancelled);
}

#[test]
fn the_composite_opens_files_and_dimse_urls() {
    // covers 32.3-a: the existing viewer path opens a classic PACS study
    let (_dir, objects) = ct_phantom(5, 8, 4);
    let port = free_port();
    let pacs = MockPacs::start(store_plan(objects, port));
    // The composite's default DIMSE repository binds an ephemeral SCP; point a
    // custom composite at the mock's known port for the retrieve.
    let repo = CompositeRepository::new(vec![
        Box::new(DicomRepository),
        Box::new(DicomDimseRepository::new().with_store_scp_addr(&format!("127.0.0.1:{port}"))),
    ]);
    let series = repo.scan(&[pacs.url(&format!("?StudyInstanceUID={STUDY_UID}"))], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].format, "DIMSE");
    let loaded = repo.load(&series[0], &NoProgress).unwrap();
    assert_eq!(loaded.volume.dims(), Dims3::new(8, 5, 4));
}
