//! A C-STORE SCP that receives the sub-operations of a C-MOVE (Stage 32, PR 2).
//!
//! [`StoreScp`] binds a TCP listener, accepts the storage association the
//! move source opens, and collects each received instance **in memory** as a
//! Part 10 object — the same shape the DICOMweb reader retrieves, so the
//! shared assembly pipeline ([`crate::dicom::assemble::assemble_with`]) builds
//! a voxel-identical volume. When a receive directory is configured it also
//! writes each instance there (a persistent storage SCP, e.g. into a
//! workspace receive area); by default nothing is written to disk
//! (ADR 0021 §2 privacy).
//!
//! C-MOVE and this C-STORE SCP are **host-only** (ADR 0021 §5): they are
//! called from a person's explicit action in a host, never registered as
//! agent commands.

use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dicom_dictionary_std::{tags, uids};
use dicom_object::mem::InMemDicomObject;
use dicom_object::{FileDicomObject, FileMetaTableBuilder};
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use dicom_ul::association::ServerAssociationOptions;
use dicom_ul::pdu::{PDataValue, PDataValueType, Pdu};
use dicom_ul::ServerAssociation;
use ferrum_domain::RepositoryError;

use super::command::{
    self, cmd_u16, decode_command, echo_response, store_response, DimseError, C_ECHO_RQ, C_STORE_RQ,
};

/// Default maximum PDU length the SCP admits.
const MAX_PDU: u32 = 16384;
/// Socket read/write deadline of the storage association.
const SCP_TIMEOUT: Duration = Duration::from_secs(30);

/// A bound C-STORE SCP listener. [`spawn`](Self::spawn) runs the accept loop
/// on a background thread; [`ScpHandle::finish`] stops it and returns the
/// received instances.
#[derive(Debug)]
pub struct StoreScp {
    listener: TcpListener,
    local_addr: SocketAddr,
    ae_title: String,
    receive_dir: Option<PathBuf>,
}

impl StoreScp {
    /// Binds the SCP to `addr` (use port `0` for an ephemeral port, then read
    /// [`local_addr`](Self::local_addr)). `ae_title` is this SCP's application
    /// entity title — the C-MOVE move destination. When `receive_dir` is set,
    /// received instances are also written there as Part 10 files.
    pub fn bind(
        addr: impl ToSocketAddrs,
        ae_title: &str,
        receive_dir: Option<PathBuf>,
    ) -> Result<Self, RepositoryError> {
        let listener = TcpListener::bind(addr).map_err(|e| RepositoryError::Io(format!("C-STORE SCP bind: {e}")))?;
        let local_addr = listener.local_addr().map_err(|e| RepositoryError::Io(format!("C-STORE SCP addr: {e}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| RepositoryError::Io(format!("C-STORE SCP nonblocking: {e}")))?;
        if let Some(dir) = &receive_dir {
            std::fs::create_dir_all(dir).map_err(|e| RepositoryError::Io(format!("C-STORE SCP receive dir: {e}")))?;
        }
        Ok(Self { listener, local_addr, ae_title: ae_title.to_string(), receive_dir })
    }

    /// The address the SCP actually bound (the move source must be able to
    /// reach it; a host configures the PACS's move destination AE with it).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Runs the accept loop on a background thread.
    pub fn spawn(self) -> ScpHandle {
        let received = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (listener, ae_title, receive_dir) = (self.listener, self.ae_title, self.receive_dir);
        let (received_t, stop_t) = (received.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            accept_loop(listener, &ae_title, receive_dir.as_deref(), &received_t, &stop_t);
        });
        ScpHandle { local_addr: self.local_addr, received, stop, handle: Some(handle) }
    }
}

/// A running C-STORE SCP. [`finish`](Self::finish) stops the accept loop,
/// waits for the storage association to settle, and returns the instances.
#[derive(Debug)]
pub struct ScpHandle {
    local_addr: SocketAddr,
    received: Arc<Mutex<Vec<FileDicomObject<InMemDicomObject>>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ScpHandle {
    /// The address the SCP bound.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Signals the accept loop to stop, joins it, and returns the received
    /// instances (in arrival order).
    pub fn finish(mut self) -> Vec<FileDicomObject<InMemDicomObject>> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        std::mem::take(&mut *self.received.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Accepts storage associations until stopped. Each association is handled
/// to completion (the move source releases it once its sub-operations end).
fn accept_loop(
    listener: TcpListener,
    ae_title: &str,
    receive_dir: Option<&std::path::Path>,
    received: &Mutex<Vec<FileDicomObject<InMemDicomObject>>>,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let _ = stream.set_nonblocking(false);
                handle_association(stream, ae_title, receive_dir, received);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Negotiates and serves one storage association, collecting instances.
fn handle_association(
    stream: TcpStream,
    ae_title: &str,
    receive_dir: Option<&std::path::Path>,
    received: &Mutex<Vec<FileDicomObject<InMemDicomObject>>>,
) {
    let mut options = ServerAssociationOptions::new()
        .accept_any()
        .ae_title(ae_title)
        .promiscuous(true)
        .max_pdu_length(MAX_PDU)
        .read_timeout(SCP_TIMEOUT)
        .write_timeout(SCP_TIMEOUT)
        .with_abstract_syntax(uids::VERIFICATION);
    for ts in TransferSyntaxRegistry.iter() {
        if !ts.is_unsupported() {
            options = options.with_transfer_syntax(ts.uid());
        }
    }
    let mut association = match options.establish(stream) {
        Ok(a) => a,
        Err(e) => {
            log::warn!("C-STORE SCP association failed: {e}");
            return;
        }
    };
    let mut state = StoreState::default();
    loop {
        match association.receive() {
            Ok(Pdu::PData { data }) => {
                if let Err(e) = handle_pdata(&mut association, data, &mut state, received, receive_dir) {
                    log::warn!("C-STORE SCP message failed: {e:?}");
                    break;
                }
            }
            Ok(Pdu::ReleaseRQ) => {
                let _ = association.send(&Pdu::ReleaseRP);
                break;
            }
            Ok(Pdu::AbortRQ { .. }) => break,
            Ok(_) => {}
            Err(e) => {
                log::debug!("C-STORE SCP receive ended: {e}");
                break;
            }
        }
    }
}

/// Per-association accumulation state (command and instance fragments).
#[derive(Default)]
struct StoreState {
    cmd_buffer: Vec<u8>,
    instance_buffer: Vec<u8>,
    msgid: u16,
    sop_class: String,
    sop_instance: String,
}

/// Processes the P-Data values of one PDU: command fragments build a command,
/// data fragments build an instance which is stored and acknowledged.
fn handle_pdata(
    association: &mut ServerAssociation<TcpStream>,
    data: Vec<PDataValue>,
    state: &mut StoreState,
    received: &Mutex<Vec<FileDicomObject<InMemDicomObject>>>,
    receive_dir: Option<&std::path::Path>,
) -> Result<(), DimseError> {
    for pdv in data {
        match pdv.value_type {
            PDataValueType::Command => {
                state.cmd_buffer.extend_from_slice(&pdv.data);
                if pdv.is_last {
                    let cmd = decode_command(&state.cmd_buffer)?;
                    state.cmd_buffer.clear();
                    on_command(association, &cmd, pdv.presentation_context_id, state)?;
                }
            }
            PDataValueType::Data => {
                state.instance_buffer.extend_from_slice(&pdv.data);
                if pdv.is_last {
                    on_instance(association, pdv.presentation_context_id, state, received, receive_dir)?;
                    state.instance_buffer.clear();
                }
            }
        }
    }
    Ok(())
}

/// A complete command: answer a C-ECHO-RQ, or record a C-STORE-RQ's identity
/// (the instance data set follows and is acknowledged then).
fn on_command(
    association: &mut ServerAssociation<TcpStream>,
    cmd: &InMemDicomObject,
    pc_id: u8,
    state: &mut StoreState,
) -> Result<(), DimseError> {
    let field = cmd_u16(cmd, tags::COMMAND_FIELD, "CommandField")?;
    if field == C_ECHO_RQ {
        let msgid = cmd_u16(cmd, tags::MESSAGE_ID, "MessageID")?;
        command::send_command(association, pc_id, &echo_response(msgid))?;
        return Ok(());
    }
    if field == C_STORE_RQ {
        state.msgid = cmd_u16(cmd, tags::MESSAGE_ID, "MessageID")?;
        state.sop_class = cmd_str(cmd, tags::AFFECTED_SOP_CLASS_UID, "AffectedSOPClassUID")?;
        state.sop_instance = cmd_str(cmd, tags::AFFECTED_SOP_INSTANCE_UID, "AffectedSOPInstanceUID")?;
    }
    Ok(())
}

/// A complete instance data set: parse it in the negotiated syntax, keep it
/// in memory (and optionally on disk), and send the C-STORE-RSP.
fn on_instance(
    association: &mut ServerAssociation<TcpStream>,
    pc_id: u8,
    state: &StoreState,
    received: &Mutex<Vec<FileDicomObject<InMemDicomObject>>>,
    receive_dir: Option<&std::path::Path>,
) -> Result<(), DimseError> {
    let ts_uid = association
        .presentation_contexts()
        .iter()
        .find(|pc| pc.id == pc_id)
        .map(|pc| pc.transfer_syntax.clone())
        .ok_or(DimseError::Missing("presentation context"))?;
    let ts = command::lookup_ts(&ts_uid).ok_or(DimseError::Missing("transfer syntax"))?;
    let obj = InMemDicomObject::read_dataset_with_ts(state.instance_buffer.as_slice(), ts)?;
    let file_obj = build_file_object(obj, &ts_uid)?;
    if let Some(dir) = receive_dir {
        let name = format!("{}.dcm", state.sop_instance.trim_end_matches('\0'));
        if let Err(e) = file_obj.write_to_file(dir.join(name)) {
            log::warn!("C-STORE SCP could not write received instance: {e}");
        }
    }
    received.lock().unwrap_or_else(|e| e.into_inner()).push(file_obj);
    let rsp = store_response(state.msgid, &state.sop_class, &state.sop_instance);
    command::send_command(association, pc_id, &rsp)?;
    Ok(())
}

/// Wraps a received data set with the Part 10 file meta information.
fn build_file_object(
    obj: InMemDicomObject,
    ts_uid: &str,
) -> Result<FileDicomObject<InMemDicomObject>, DimseError> {
    let sop_class = cmd_str(&obj, tags::SOP_CLASS_UID, "SOPClassUID")?;
    let sop_instance = cmd_str(&obj, tags::SOP_INSTANCE_UID, "SOPInstanceUID")?;
    let meta = FileMetaTableBuilder::new()
        .media_storage_sop_class_uid(sop_class)
        .media_storage_sop_instance_uid(sop_instance)
        .transfer_syntax(ts_uid)
        .build()
        .map_err(|_| DimseError::Missing("file meta information"))?;
    Ok(obj.with_exact_meta(meta))
}

/// Reads a string command attribute, or a [`DimseError::Missing`].
fn cmd_str(obj: &InMemDicomObject, tag: dicom_core::Tag, name: &'static str) -> Result<String, DimseError> {
    Ok(obj.get(tag).ok_or(DimseError::Missing(name))?.to_str().map_err(|_| DimseError::Missing(name))?.to_string())
}
