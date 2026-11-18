//! DIMSE command construction and P-Data exchange helpers (Stage 32, PR 2).
//!
//! `dicom-ul` 0.10 provides the association and PDU layers only; the DIMSE
//! service commands (C-ECHO, C-FIND, C-MOVE, C-STORE) are built here from
//! `dicom-object` command sets, following the DICOM-rs reference tools.
//! Commands always travel in Implicit VR Little Endian (PS3.7); the data set
//! uses the presentation context's negotiated transfer syntax.

use dicom_core::{dicom_value, DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_encoding::transfer_syntax::{TransferSyntax, TransferSyntaxIndex};
use dicom_object::mem::InMemDicomObject;
use dicom_object::{ReadError, WriteError};
use dicom_transfer_syntax_registry::{entries, TransferSyntaxRegistry};
use dicom_ul::association::{CloseSocket, SyncAssociation};
use dicom_ul::pdu::{PDataValue, PDataValueType, Pdu};
use ferrum_domain::RepositoryError;

/// C-ECHO-RQ command field (PS3.7 9.1.5).
pub(crate) const C_ECHO_RQ: u16 = 0x0030;
/// C-ECHO-RSP command field.
pub(crate) const C_ECHO_RSP: u16 = 0x8030;
/// C-FIND-RQ command field.
pub(crate) const C_FIND_RQ: u16 = 0x0020;
/// C-MOVE-RQ command field.
pub(crate) const C_MOVE_RQ: u16 = 0x0021;
/// C-STORE-RQ command field.
pub(crate) const C_STORE_RQ: u16 = 0x0001;
/// C-STORE-RSP command field.
pub(crate) const C_STORE_RSP: u16 = 0x8001;
/// DIMSE status: success.
pub(crate) const STATUS_SUCCESS: u16 = 0x0000;
/// DIMSE status: pending (matches/sub-operations still to come).
pub(crate) const STATUS_PENDING: u16 = 0xFF00;
/// DIMSE status: pending with warnings.
pub(crate) const STATUS_PENDING_WARNING: u16 = 0xFF01;
/// Command Data Set Type: no data set follows the command.
pub(crate) const NO_DATA_SET: u16 = 0x0101;

/// A failure in a DIMSE exchange, mapped to [`RepositoryError`] at the
/// repository boundary (messages name the host and the kind, never query
/// values or credentials).
#[derive(Debug)]
pub(crate) enum DimseError {
    /// The association layer failed (connect, reject, abort, timeout, …).
    Association(dicom_ul::association::Error),
    /// A command or data set could not be parsed.
    Read(ReadError),
    /// A command or data set could not be encoded.
    Write(WriteError),
    /// A socket I/O error.
    Io(std::io::Error),
    /// The peer answered a failure status to an operation.
    Status {
        /// The operation name (e.g. `C-MOVE`).
        operation: &'static str,
        /// The DIMSE status code.
        code: u16,
    },
    /// An unexpected PDU arrived mid-message.
    UnexpectedPdu(&'static str),
    /// The peer aborted the association mid-message.
    Aborted,
    /// A required command attribute is missing.
    Missing(&'static str),
    /// The operation was cancelled through the progress sink.
    Cancelled,
}

impl From<dicom_ul::association::Error> for DimseError {
    fn from(e: dicom_ul::association::Error) -> Self {
        Self::Association(e)
    }
}

impl From<ReadError> for DimseError {
    fn from(e: ReadError) -> Self {
        Self::Read(e)
    }
}

impl From<WriteError> for DimseError {
    fn from(e: WriteError) -> Self {
        Self::Write(e)
    }
}

impl From<std::io::Error> for DimseError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Maps a DIMSE failure to a repository error. The message names the host and
/// the failure kind only — query values (a PatientID filter is an identifier)
/// and credentials never appear, mirroring the DICOMweb error map.
pub(crate) fn map_dimse_error(e: DimseError, host: &str) -> RepositoryError {
    use dicom_ul::association::Error as A;
    match e {
        DimseError::Association(a) => match a {
            A::Connect { .. } | A::ToAddress { .. } => {
                RepositoryError::Io(format!("{host}: DIMSE connection failed"))
            }
            A::Rejected { .. } => RepositoryError::Io(format!("{host}: DIMSE association rejected")),
            A::Aborted { .. } => RepositoryError::Io(format!("{host}: DIMSE association aborted")),
            A::Timeout { .. } => RepositoryError::Io(format!("{host}: DIMSE request timed out")),
            A::NoAcceptedPresentationContexts { .. } => {
                RepositoryError::Io(format!("{host}: DIMSE association has no accepted presentation context"))
            }
            A::ConnectionClosed { .. } => {
                RepositoryError::Io(format!("{host}: DIMSE connection closed by peer"))
            }
            A::UnexpectedPdu { .. } | A::UnknownPdu { .. } => {
                RepositoryError::Corrupt(format!("{host}: unexpected DIMSE PDU from peer"))
            }
            // The raw Display is dropped: it could carry PDU detail; the host
            // and the failure kind are all a repository error names.
            _ => RepositoryError::Io(format!("{host}: DIMSE association failed")),
        },
        DimseError::Read(e) => {
            log::debug!("DIMSE read error: {e}");
            RepositoryError::Corrupt(format!("{host}: malformed DIMSE message"))
        }
        DimseError::Write(e) => {
            log::debug!("DIMSE write error: {e}");
            RepositoryError::Io(format!("{host}: failed to encode DIMSE message"))
        }
        DimseError::Io(e) => {
            if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
                RepositoryError::Io(format!("{host}: DIMSE request timed out"))
            } else {
                log::debug!("DIMSE I/O error: {e}");
                RepositoryError::Io(format!("{host}: DIMSE I/O error"))
            }
        }
        DimseError::Status { operation, code } => {
            RepositoryError::Io(format!("{host}: {operation} failed (status 0x{code:04X})"))
        }
        DimseError::UnexpectedPdu(what) => {
            RepositoryError::Corrupt(format!("{host}: unexpected DIMSE PDU ({what})"))
        }
        DimseError::Aborted => RepositoryError::Io(format!("{host}: DIMSE association aborted")),
        DimseError::Missing(attr) => {
            RepositoryError::Corrupt(format!("{host}: DIMSE message missing {attr}"))
        }
        DimseError::Cancelled => RepositoryError::Cancelled,
    }
}

/// The Implicit VR Little Endian transfer syntax every DIMSE command uses.
pub(crate) fn command_ts() -> TransferSyntax {
    entries::IMPLICIT_VR_LITTLE_ENDIAN.erased()
}

/// Looks up a negotiated transfer syntax in the registry (a static reference).
pub(crate) fn lookup_ts(uid: &str) -> Option<&'static TransferSyntax> {
    TransferSyntaxRegistry.get(uid)
}

/// Serialises a command set into bytes (Implicit VR Little Endian).
pub(crate) fn encode_command(obj: &InMemDicomObject) -> Result<Vec<u8>, DimseError> {
    let mut buf = Vec::with_capacity(128);
    obj.write_dataset_with_ts(&mut buf, &command_ts())?;
    Ok(buf)
}

/// Parses a command set from bytes (Implicit VR Little Endian).
pub(crate) fn decode_command(bytes: &[u8]) -> Result<InMemDicomObject, DimseError> {
    Ok(InMemDicomObject::read_dataset_with_ts(bytes, &command_ts())?)
}

/// Reads a `u16` command attribute, or a [`DimseError::Missing`].
pub(crate) fn cmd_u16(obj: &InMemDicomObject, tag: dicom_core::Tag, name: &'static str) -> Result<u16, DimseError> {
    obj.get(tag).ok_or(DimseError::Missing(name))?.to_int::<u16>().map_err(|_| DimseError::Missing(name))
}

/// The Status attribute of a response command (0 when absent).
pub(crate) fn cmd_status(obj: &InMemDicomObject) -> u16 {
    obj.get(tags::STATUS).and_then(|e| e.to_int::<u16>().ok()).unwrap_or(0)
}

/// The Command Data Set Type attribute (whether a data set follows).
pub(crate) fn cmd_data_set_type(obj: &InMemDicomObject) -> u16 {
    obj.get(tags::COMMAND_DATA_SET_TYPE).and_then(|e| e.to_int::<u16>().ok()).unwrap_or(NO_DATA_SET)
}

/// Sends one command set as a last Command P-Data value.
pub(crate) fn send_command<S>(assoc: &mut impl SyncAssociation<S>, pc_id: u8, obj: &InMemDicomObject) -> Result<(), DimseError>
where
    S: std::io::Read + std::io::Write + CloseSocket,
{
    let data = encode_command(obj)?;
    SyncAssociation::send(assoc, &Pdu::PData {
        data: vec![PDataValue { presentation_context_id: pc_id, value_type: PDataValueType::Command, is_last: true, data }],
    })?;
    Ok(())
}

/// Sends one data set as a last Data P-Data value in the negotiated syntax.
pub(crate) fn send_dataset<S>(
    assoc: &mut impl SyncAssociation<S>,
    pc_id: u8,
    obj: &InMemDicomObject,
    ts: &TransferSyntax,
) -> Result<(), DimseError>
where
    S: std::io::Read + std::io::Write + CloseSocket,
{
    let mut data = Vec::with_capacity(256);
    obj.write_dataset_with_ts(&mut data, ts)?;
    SyncAssociation::send(assoc, &Pdu::PData {
        data: vec![PDataValue { presentation_context_id: pc_id, value_type: PDataValueType::Data, is_last: true, data }],
    })?;
    Ok(())
}

/// Receives one DIMSE message: the command set and, when the command says a
/// data set follows, that data set (parsed in `data_ts`). Command and data
/// fragments may share a PDU or span several; an abort is an error.
pub(crate) fn receive_message<S>(
    assoc: &mut impl SyncAssociation<S>,
    data_ts: &TransferSyntax,
) -> Result<(InMemDicomObject, Option<InMemDicomObject>), DimseError>
where
    S: std::io::Read + std::io::Write + CloseSocket,
{
    let mut cmd_buf: Vec<u8> = Vec::new();
    let mut data_buf: Vec<u8> = Vec::new();
    let mut command: Option<InMemDicomObject> = None;
    let mut data_complete = false;
    let mut saw_data = false;
    loop {
        if let Some(cmd) = &command {
            if cmd_data_set_type(cmd) == NO_DATA_SET || data_complete {
                break;
            }
        }
        match SyncAssociation::receive(assoc)? {
            Pdu::PData { data } => {
                for pdv in data {
                    match pdv.value_type {
                        PDataValueType::Command => {
                            cmd_buf.extend_from_slice(&pdv.data);
                            if pdv.is_last {
                                command = Some(decode_command(&cmd_buf)?);
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
            Pdu::AbortRQ { .. } => return Err(DimseError::Aborted),
            Pdu::ReleaseRQ => return Err(DimseError::UnexpectedPdu("release")),
            _ => return Err(DimseError::UnexpectedPdu("non-P-Data")),
        }
    }
    let cmd = command.ok_or(DimseError::Missing("command"))?;
    let data = if saw_data {
        Some(InMemDicomObject::read_dataset_with_ts(data_buf.as_slice(), data_ts)?)
    } else {
        None
    };
    Ok((cmd, data))
}

/// A C-ECHO-RQ command (Verification SOP Class).
pub(crate) fn echo_request(message_id: u16) -> InMemDicomObject {
    InMemDicomObject::command_from_element_iter([
        DataElement::new(tags::AFFECTED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(uids::VERIFICATION)),
        DataElement::new(tags::COMMAND_FIELD, VR::US, dicom_value!(U16, [C_ECHO_RQ])),
        DataElement::new(tags::MESSAGE_ID, VR::US, dicom_value!(U16, [message_id])),
        DataElement::new(tags::COMMAND_DATA_SET_TYPE, VR::US, dicom_value!(U16, [NO_DATA_SET])),
    ])
}

/// A C-FIND-RQ command for the given Query/Retrieve Information Model.
pub(crate) fn find_request(sop_class: &str, message_id: u16) -> InMemDicomObject {
    InMemDicomObject::command_from_element_iter([
        DataElement::new(tags::AFFECTED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(sop_class)),
        DataElement::new(tags::COMMAND_FIELD, VR::US, dicom_value!(U16, [C_FIND_RQ])),
        DataElement::new(tags::MESSAGE_ID, VR::US, dicom_value!(U16, [message_id])),
        DataElement::new(tags::PRIORITY, VR::US, dicom_value!(U16, [0x0000])),
        DataElement::new(tags::COMMAND_DATA_SET_TYPE, VR::US, dicom_value!(U16, [0x0001])),
    ])
}

/// A C-MOVE-RQ command naming the move destination AE title.
pub(crate) fn move_request(sop_class: &str, destination: &str, message_id: u16) -> InMemDicomObject {
    InMemDicomObject::command_from_element_iter([
        DataElement::new(tags::AFFECTED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(sop_class)),
        DataElement::new(tags::COMMAND_FIELD, VR::US, dicom_value!(U16, [C_MOVE_RQ])),
        DataElement::new(tags::MESSAGE_ID, VR::US, dicom_value!(U16, [message_id])),
        DataElement::new(tags::PRIORITY, VR::US, dicom_value!(U16, [0x0000])),
        DataElement::new(tags::COMMAND_DATA_SET_TYPE, VR::US, dicom_value!(U16, [0x0001])),
        DataElement::new(tags::MOVE_DESTINATION, VR::AE, PrimitiveValue::from(destination)),
    ])
}

/// A C-STORE-RSP command (success) for a received instance.
pub(crate) fn store_response(message_id: u16, sop_class: &str, sop_instance: &str) -> InMemDicomObject {
    InMemDicomObject::command_from_element_iter([
        DataElement::new(tags::AFFECTED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(sop_class)),
        DataElement::new(tags::COMMAND_FIELD, VR::US, dicom_value!(U16, [C_STORE_RSP])),
        DataElement::new(tags::MESSAGE_ID_BEING_RESPONDED_TO, VR::US, dicom_value!(U16, [message_id])),
        DataElement::new(tags::COMMAND_DATA_SET_TYPE, VR::US, dicom_value!(U16, [NO_DATA_SET])),
        DataElement::new(tags::STATUS, VR::US, dicom_value!(U16, [STATUS_SUCCESS])),
        DataElement::new(tags::AFFECTED_SOP_INSTANCE_UID, VR::UI, PrimitiveValue::from(sop_instance)),
    ])
}

/// A C-ECHO-RSP command (success).
pub(crate) fn echo_response(message_id: u16) -> InMemDicomObject {
    InMemDicomObject::command_from_element_iter([
        DataElement::new(tags::AFFECTED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(uids::VERIFICATION)),
        DataElement::new(tags::COMMAND_FIELD, VR::US, dicom_value!(U16, [C_ECHO_RSP])),
        DataElement::new(tags::MESSAGE_ID_BEING_RESPONDED_TO, VR::US, dicom_value!(U16, [message_id])),
        DataElement::new(tags::COMMAND_DATA_SET_TYPE, VR::US, dicom_value!(U16, [NO_DATA_SET])),
        DataElement::new(tags::STATUS, VR::US, dicom_value!(U16, [STATUS_SUCCESS])),
    ])
}
