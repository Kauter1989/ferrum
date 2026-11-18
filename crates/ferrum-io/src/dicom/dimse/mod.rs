//! DIMSE data source: C-FIND query, C-MOVE retrieval and C-ECHO verification
//! over the classic DICOM upper-layer protocol ([design](../../../../docs/dicomdimse.md),
//! ADR 0021).
//!
//! [`DicomDimseRepository`] implements [`VolumeRepository`] over PACS URLs:
//! `scan` interprets a `dicom://host:port/CALLED-AE?PatientID=…` path as a
//! C-FIND query, `load` retrieves the series with C-MOVE — receiving the
//! sub-operations through a [`StoreScp`] C-STORE SCP — and assembles them
//! through the same pipeline as the file and DICOMweb readers
//! ([`super::assemble`]), so a retrieved study is voxel-identical to the same
//! study opened from files, in the canonical LPS frame.
//!
//! **Send-off boundary (ADR 0021 §5).** C-MOVE and the C-STORE SCP move data
//! off the workstation and open a network listener: they are **host-only**,
//! called from a person's explicit action in a host, and are never registered
//! as agent commands. C-ECHO ([`DicomDimseRepository::echo`]) and C-FIND
//! (`scan`) are read-only and may become agent commands (Stage 32, PR 3).
//!
//! Retrieved data stays in memory by default; nothing is written to disk
//! unless a receive directory is configured ([`DicomDimseRepository::with_receive_dir`]).

mod command;
mod scp;

use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use dicom_core::{DataDictionary, Tag};
use dicom_dictionary_std::{tags, uids};
use dicom_object::mem::InMemDicomObject;
use dicom_object::FileDicomObject;
use dicom_ul::association::client::ClientAssociationOptions;
use dicom_ul::association::Association;
use dicom_ul::ClientAssociation;
use ferrum_domain::{
    Dims3, LoadedSeries, ProgressSink, RepositoryError, SeriesDescriptor, SeriesMetadata, VolumeRepository,
};

use super::header::{self, SliceHeader};
use super::web::{keyword_tag, percent_decode, safe_url, split_authority};
use super::{assemble, series, ScaledProgress};
use command::{
    cmd_status, find_request, lookup_ts, map_dimse_error, move_request, receive_message, send_command, send_dataset,
    DimseError, STATUS_PENDING, STATUS_PENDING_WARNING, STATUS_SUCCESS,
};
pub use scp::{ScpHandle, StoreScp};

/// URL scheme of a DIMSE PACS node.
pub const SCHEME: &str = "dicom://";
/// Default deadline of one DIMSE socket operation (connect, read, write).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default calling AE title (this node, the SCU).
pub const DEFAULT_CALLING_AE: &str = "FERRUM-SCU";
/// Default C-STORE SCP AE title (the C-MOVE move destination).
pub const DEFAULT_STORE_AE: &str = "FERRUM-SCP";
/// Default C-STORE SCP listen address (ephemeral port on loopback).
pub const DEFAULT_STORE_ADDR: &str = "127.0.0.1:0";
/// Classic DIMSE TCP port, used when the URL carries no port.
const DEFAULT_PORT: u16 = 104;
/// Maximum PDU length this node admits.
const MAX_PDU: u32 = 16384;

/// Attributes requested from a series-level C-FIND so a descriptor can be
/// built without retrieving pixels (empty value = return key).
const SERIES_RETURN_KEYS: [Tag; 8] = [
    tags::MODALITY,
    tags::SERIES_DESCRIPTION,
    tags::SERIES_NUMBER,
    tags::NUMBER_OF_SERIES_RELATED_INSTANCES,
    tags::ROWS,
    tags::COLUMNS,
    tags::STUDY_INSTANCE_UID,
    tags::SERIES_INSTANCE_UID,
];

/// Attributes requested from a study-level C-FIND.
const STUDY_RETURN_KEYS: [Tag; 5] = [
    tags::STUDY_INSTANCE_UID,
    tags::STUDY_DATE,
    tags::STUDY_DESCRIPTION,
    tags::MODALITY,
    tags::NUMBER_OF_STUDY_RELATED_SERIES,
];

/// Reads series from a classic PACS over DIMSE (C-FIND + C-MOVE/C-STORE).
///
/// Paths given to [`VolumeRepository::scan`] are PACS URLs
/// (`dicom://host[:port]/CALLED-AE?keyword=value&…`); any other path is
/// `NothingFound`, so the repository composes with the file readers. The
/// calling AE title, the C-STORE SCP address and the move destination are
/// configured here, never parsed from a URL.
///
/// Each operation is synchronous (blocking sockets with a deadline): call it
/// from synchronous code. C-MOVE retrieval (`load`) is host-only (ADR 0021 §5).
#[derive(Debug, Clone)]
pub struct DicomDimseRepository {
    calling_ae_title: String,
    move_destination: Option<String>,
    store_scp_addr: String,
    receive_dir: Option<PathBuf>,
    timeout: Duration,
}

impl Default for DicomDimseRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl DicomDimseRepository {
    /// A repository with the default AE titles, an ephemeral loopback C-STORE
    /// SCP and the [`DEFAULT_TIMEOUT`] deadline.
    pub fn new() -> Self {
        Self {
            calling_ae_title: DEFAULT_CALLING_AE.into(),
            move_destination: None,
            store_scp_addr: DEFAULT_STORE_ADDR.into(),
            receive_dir: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// This node's calling AE title (the SCU identity presented to the PACS).
    pub fn with_calling_ae_title(mut self, ae_title: &str) -> Self {
        self.calling_ae_title = ae_title.to_string();
        self
    }

    /// The C-MOVE move destination AE title (defaults to the C-STORE SCP AE
    /// title, [`DEFAULT_STORE_AE`]). The PACS must resolve it to the SCP address.
    pub fn with_move_destination(mut self, ae_title: &str) -> Self {
        self.move_destination = Some(ae_title.to_string());
        self
    }

    /// The address the C-STORE SCP binds for C-MOVE sub-operations (default
    /// [`DEFAULT_STORE_ADDR`], an ephemeral loopback port). A host configures
    /// the PACS's move destination AE with the bound address.
    pub fn with_store_scp_addr(mut self, addr: &str) -> Self {
        self.store_scp_addr = addr.to_string();
        self
    }

    /// Also write received instances to `dir` as Part 10 files (a persistent
    /// storage SCP, e.g. a workspace receive area). By default received data
    /// stays in memory and nothing is written to disk (ADR 0021 §2).
    pub fn with_receive_dir(mut self, dir: PathBuf) -> Self {
        self.receive_dir = Some(dir);
        self
    }

    /// The deadline of one DIMSE socket operation; an expired deadline is an
    /// [`RepositoryError::Io`], never a hang.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The move destination AE title (the C-STORE SCP's identity).
    fn move_ae(&self) -> String {
        self.move_destination.clone().unwrap_or_else(|| DEFAULT_STORE_AE.into())
    }

    /// C-ECHO verification of a PACS node (read-only). `target` is a
    /// `dicom://host[:port]/CALLED-AE` URL. Establishes an association,
    /// exchanges a C-ECHO and checks the success status.
    pub fn echo(&self, target: &str) -> Result<(), RepositoryError> {
        let url = parse_dimse_url(target)?;
        self.echo_at(&url).map_err(|e| map_dimse_error(e, &url.host))
    }

    /// The C-ECHO exchange on a parsed URL.
    fn echo_at(&self, url: &DimseUrl) -> Result<(), DimseError> {
        let mut assoc = self.establish(url, uids::VERIFICATION)?;
        let pc = assoc.presentation_contexts().first().cloned().ok_or(DimseError::Missing("presentation context"))?;
        send_command(&mut assoc, pc.id, &command::echo_request(1))?;
        let (rsp, _) = receive_message(&mut assoc, &command::command_ts())?;
        let status = cmd_status(&rsp);
        let _ = assoc.release();
        if status != STATUS_SUCCESS {
            return Err(DimseError::Status { operation: "C-ECHO", code: status });
        }
        Ok(())
    }

    /// Establishes an association proposing one abstract syntax with the
    /// little-endian transfer syntaxes.
    fn establish(&self, url: &DimseUrl, abstract_syntax: &str) -> Result<ClientAssociation<TcpStream>, DimseError> {
        let assoc = ClientAssociationOptions::new()
            .calling_ae_title(self.calling_ae_title.clone())
            .called_ae_title(url.called_ae.clone())
            .with_presentation_context(
                abstract_syntax,
                vec![uids::EXPLICIT_VR_LITTLE_ENDIAN.to_string(), uids::IMPLICIT_VR_LITTLE_ENDIAN.to_string()],
            )
            .max_pdu_length(MAX_PDU)
            .read_timeout(self.timeout)
            .write_timeout(self.timeout)
            .connection_timeout(self.timeout)
            .establish(url.addr.as_str())?;
        Ok(assoc)
    }

    /// One C-FIND query at the given level, collecting the pending match
    /// identifiers until the success status.
    fn c_find(
        &self,
        url: &DimseUrl,
        query: &InMemDicomObject,
        progress: &dyn ProgressSink,
    ) -> Result<Vec<InMemDicomObject>, DimseError> {
        let mut assoc = self.establish(url, uids::STUDY_ROOT_QUERY_RETRIEVE_INFORMATION_MODEL_FIND)?;
        let pc = assoc.presentation_contexts().first().cloned().ok_or(DimseError::Missing("presentation context"))?;
        let ts = lookup_ts(&pc.transfer_syntax).ok_or(DimseError::Missing("transfer syntax"))?;
        send_command(&mut assoc, pc.id, &find_request(uids::STUDY_ROOT_QUERY_RETRIEVE_INFORMATION_MODEL_FIND, 1))?;
        send_dataset(&mut assoc, pc.id, query, &ts)?;
        let mut results = Vec::new();
        loop {
            if progress.is_cancelled() {
                let _ = assoc.abort();
                return Err(DimseError::Cancelled);
            }
            let (rsp, data) = receive_message(&mut assoc, &ts)?;
            match cmd_status(&rsp) {
                STATUS_SUCCESS => break,
                STATUS_PENDING | STATUS_PENDING_WARNING => {
                    if let Some(obj) = data {
                        results.push(obj);
                        progress.report(0.5, "Querying PACS (C-FIND)");
                    }
                }
                code => {
                    let _ = assoc.abort();
                    return Err(DimseError::Status { operation: "C-FIND", code });
                }
            }
        }
        let _ = assoc.release();
        Ok(results)
    }

    /// Issues a series-level C-MOVE; the instances arrive on the C-STORE SCP.
    /// Blocks until the move reports success (or a failure status).
    fn c_move(
        &self,
        url: &DimseUrl,
        study: &str,
        series_uid: &str,
        progress: &dyn ProgressSink,
    ) -> Result<(), DimseError> {
        let mut assoc = self.establish(url, uids::STUDY_ROOT_QUERY_RETRIEVE_INFORMATION_MODEL_MOVE)?;
        let pc = assoc.presentation_contexts().first().cloned().ok_or(DimseError::Missing("presentation context"))?;
        let ts = lookup_ts(&pc.transfer_syntax).ok_or(DimseError::Missing("transfer syntax"))?;
        send_command(&mut assoc, pc.id, &move_request(uids::STUDY_ROOT_QUERY_RETRIEVE_INFORMATION_MODEL_MOVE, &self.move_ae(), 1))?;
        let query = series_identifier(study, series_uid);
        send_dataset(&mut assoc, pc.id, &query, &ts)?;
        loop {
            if progress.is_cancelled() {
                let _ = assoc.abort();
                return Err(DimseError::Cancelled);
            }
            let (rsp, _) = receive_message(&mut assoc, &ts)?;
            match cmd_status(&rsp) {
                STATUS_SUCCESS => break,
                STATUS_PENDING | STATUS_PENDING_WARNING => {
                    progress.report(move_progress(&rsp), "Retrieving PACS instances (C-MOVE)");
                }
                code => {
                    let _ = assoc.abort();
                    return Err(DimseError::Status { operation: "C-MOVE", code });
                }
            }
        }
        let _ = assoc.release();
        Ok(())
    }
}

impl VolumeRepository for DicomDimseRepository {
    fn name(&self) -> &str {
        "DIMSE"
    }

    fn scan(&self, paths: &[PathBuf], progress: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        let mut out: Vec<SeriesDescriptor> = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            if progress.is_cancelled() {
                return Err(RepositoryError::Cancelled);
            }
            let Some(text) = path.to_str() else { continue };
            if !text.starts_with(SCHEME) {
                continue; // a file path, not a PACS query
            }
            let url = parse_dimse_url(text)?;
            for obj in self.find_series(&url, progress).map_err(|e| map_dimse_error(e, &url.host))? {
                if let Some(d) = describe(&url, &obj) {
                    if !out.iter().any(|existing| existing.id == d.id) {
                        out.push(d);
                    }
                }
            }
            progress.report((i + 1) as f32 / paths.len() as f32, "Querying PACS");
        }
        if out.is_empty() {
            return Err(RepositoryError::NothingFound);
        }
        Ok(out)
    }

    fn load(&self, series: &SeriesDescriptor, progress: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        let url_text = series
            .sources
            .first()
            .and_then(|p| p.to_str())
            .ok_or_else(|| RepositoryError::Corrupt("DIMSE series carries no retrieval URL".into()))?;
        let url = parse_dimse_url(url_text).map_err(|_| RepositoryError::Corrupt(format!("{}: malformed DIMSE retrieval URL", safe_url(url_text))))?;
        let study = url.study_uid.clone().ok_or_else(|| RepositoryError::Corrupt("DIMSE retrieval URL has no StudyInstanceUID".into()))?;
        let series_uid = url.series_uid.clone().ok_or_else(|| RepositoryError::Corrupt("DIMSE retrieval URL has no SeriesInstanceUID".into()))?;

        // Receive the move sub-operations on a C-STORE SCP (host-only).
        let scp = StoreScp::bind(self.store_scp_addr.as_str(), &self.move_ae(), self.receive_dir.clone())?;
        let handle = scp.spawn();
        let move_result = self.c_move(&url, &study, &series_uid, &ScaledProgress::new(progress, 0.0, 0.5));
        let objects = handle.finish();
        move_result.map_err(|e| map_dimse_error(e, &url.host))?;
        if objects.is_empty() {
            return Err(RepositoryError::NothingFound);
        }
        let (headers, objects) = headers_of(&url, &study, &series_uid, objects)?;
        let key = series::group(headers.clone())
            .into_iter()
            .find(|(k, _)| k.series_uid == series_uid)
            .map(|(k, _)| k)
            .ok_or(RepositoryError::NothingFound)?;
        let mut hs = Vec::new();
        let mut objs = Vec::new();
        for (h, o) in headers.into_iter().zip(objects) {
            if series::SeriesKey::of(&h) == key {
                hs.push(h);
                objs.push(o);
            }
        }
        let ordered = series::order(&hs);
        let volume = assemble::assemble_with(&hs, &ordered, &ScaledProgress::new(progress, 0.5, 1.0), |i, frames| {
            assemble::decode_object_frames(&objs[i], frames, &hs[i].path)
        })?;
        Ok(loaded_series(volume, &hs, &objs, &ordered, url_text))
    }
}

impl DicomDimseRepository {
    /// Resolves a query URL to series-level match identifiers: a series-level
    /// C-FIND within the study when the URL names one, otherwise a study-level
    /// C-FIND followed by a series-level C-FIND per study.
    fn find_series(&self, url: &DimseUrl, progress: &dyn ProgressSink) -> Result<Vec<InMemDicomObject>, DimseError> {
        if let Some(study) = &url.study_uid {
            let query = series_query(study, &url.filters);
            return self.c_find(url, &query, progress);
        }
        let study_query = study_query(&url.filters);
        let studies = self.c_find(url, &study_query, progress)?;
        let mut series = Vec::new();
        for study_obj in studies {
            let Some(study) = header::str_of(&study_obj, tags::STUDY_INSTANCE_UID).filter(|s| !s.is_empty()) else {
                continue;
            };
            let query = series_query(&study, &[]);
            series.extend(self.c_find(url, &query, progress)?);
        }
        Ok(series)
    }
}

/// A series-level C-FIND identifier: the study UID plus matching filters and
/// the series return keys (empty values).
fn series_query(study: &str, filters: &[(Tag, String)]) -> InMemDicomObject {
    let mut obj = InMemDicomObject::new_empty();
    obj.put(dicom_core::DataElement::new(tags::QUERY_RETRIEVE_LEVEL, dicom_core::VR::CS, dicom_core::PrimitiveValue::from("SERIES")));
    obj.put(dicom_core::DataElement::new(tags::STUDY_INSTANCE_UID, dicom_core::VR::UI, dicom_core::PrimitiveValue::from(study)));
    for tag in SERIES_RETURN_KEYS {
        obj.put(dicom_core::DataElement::new(tag, dicom_core::VR::CS, dicom_core::PrimitiveValue::Empty));
    }
    apply_filters(&mut obj, filters);
    obj
}

/// A study-level C-FIND identifier: matching filters plus study return keys.
fn study_query(filters: &[(Tag, String)]) -> InMemDicomObject {
    let mut obj = InMemDicomObject::new_empty();
    obj.put(dicom_core::DataElement::new(tags::QUERY_RETRIEVE_LEVEL, dicom_core::VR::CS, dicom_core::PrimitiveValue::from("STUDY")));
    for tag in STUDY_RETURN_KEYS {
        obj.put(dicom_core::DataElement::new(tag, dicom_core::VR::CS, dicom_core::PrimitiveValue::Empty));
    }
    apply_filters(&mut obj, filters);
    obj
}

/// A series-level C-MOVE identifier (the series to retrieve).
fn series_identifier(study: &str, series_uid: &str) -> InMemDicomObject {
    let mut obj = InMemDicomObject::new_empty();
    obj.put(dicom_core::DataElement::new(tags::QUERY_RETRIEVE_LEVEL, dicom_core::VR::CS, dicom_core::PrimitiveValue::from("SERIES")));
    obj.put(dicom_core::DataElement::new(tags::STUDY_INSTANCE_UID, dicom_core::VR::UI, dicom_core::PrimitiveValue::from(study)));
    obj.put(dicom_core::DataElement::new(tags::SERIES_INSTANCE_UID, dicom_core::VR::UI, dicom_core::PrimitiveValue::from(series_uid)));
    obj
}

/// Sets the matching attributes (skipping the study UID, already in the query).
fn apply_filters(obj: &mut InMemDicomObject, filters: &[(Tag, String)]) {
    for (tag, value) in filters {
        if *tag == tags::STUDY_INSTANCE_UID {
            continue;
        }
        let vr = dicom_dictionary_std::StandardDataDictionary
            .by_tag(*tag)
            .and_then(|e| e.vr.exact())
            .unwrap_or(dicom_core::VR::LO);
        obj.put(dicom_core::DataElement::new(*tag, vr, dicom_core::PrimitiveValue::from(value.as_str())));
    }
}

/// C-MOVE progress from the sub-operation counts of a pending response.
fn move_progress(rsp: &InMemDicomObject) -> f32 {
    let completed = header::int_of(rsp, tags::NUMBER_OF_COMPLETED_SUB_OPERATIONS).unwrap_or(0).max(0) as f32;
    let remaining = header::int_of(rsp, tags::NUMBER_OF_REMAINING_SUB_OPERATIONS).unwrap_or(0).max(0) as f32;
    let total = completed + remaining;
    if total > 0.0 {
        (completed / total).clamp(0.0, 0.95)
    } else {
        0.5
    }
}

/// Builds the metadata of a retrieved series, as the file reader does.
fn loaded_series(
    volume: ferrum_domain::Volume,
    headers: &[SliceHeader],
    objects: &[FileDicomObject<InMemDicomObject>],
    ordered: &series::OrderedSeries,
    url: &str,
) -> LoadedSeries {
    let first = &headers[0];
    let mut attributes = header::describe(&objects[0]);
    attributes.push(("Slices".into(), ordered.planes.len().to_string()));
    attributes.push(("Slice spacing".into(), format!("{:.3} mm", ordered.slice_spacing)));
    attributes.push(("Ordering".into(), format!("{:?}", ordered.method)));
    let mut warnings = ordered.warnings.clone();
    if first.pixel_spacing.is_none() {
        warnings.push("the series carries no pixel spacing: 1 mm per pixel is assumed".into());
    }
    LoadedSeries {
        volume,
        metadata: SeriesMetadata {
            modality: first.modality.clone(),
            description: first.series_description.clone(),
            default_window: first.window,
            attributes,
            study: header::study_info(&objects[0]),
            source: PathBuf::from(url),
            warnings,
        },
    }
}

/// Slice headers of retrieved instances, aligned with `objects`; the synthetic
/// per-instance URL is the error-reporting "path".
fn headers_of(
    url: &DimseUrl,
    study: &str,
    series_uid: &str,
    objects: Vec<FileDicomObject<InMemDicomObject>>,
) -> Result<(Vec<SliceHeader>, Vec<FileDicomObject<InMemDicomObject>>), RepositoryError> {
    let mut headers = Vec::with_capacity(objects.len());
    for obj in &objects {
        let sop = header::str_of(obj, tags::SOP_INSTANCE_UID).unwrap_or_default();
        let path = PathBuf::from(format!("{}/studies/{study}/series/{series_uid}/instances/{sop}", url.base));
        headers.push(SliceHeader::from_object(obj, &path)?);
    }
    Ok((headers, objects))
}

/// The descriptor of one series-level C-FIND result; `None` when it carries no
/// SeriesInstanceUID (it could not be retrieved).
fn describe(url: &DimseUrl, obj: &InMemDicomObject) -> Option<SeriesDescriptor> {
    let series_uid = header::str_of(obj, tags::SERIES_INSTANCE_UID).filter(|s| !s.is_empty())?;
    let study_uid = header::str_of(obj, tags::STUDY_INSTANCE_UID).unwrap_or_default();
    let instances = header::int_of(obj, tags::NUMBER_OF_SERIES_RELATED_INSTANCES).unwrap_or(0).max(0) as u32;
    let rows = header::int_of(obj, tags::ROWS).unwrap_or(0).max(0) as u32;
    let cols = header::int_of(obj, tags::COLUMNS).unwrap_or(0).max(0) as u32;
    let modality = header::str_of(obj, tags::MODALITY).unwrap_or_default();
    let label = header::str_of(obj, tags::SERIES_DESCRIPTION)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unnamed series".into());
    let size = if rows > 0 && cols > 0 { format!("{cols}×{rows}") } else { "size unknown until retrieved".into() };
    Some(SeriesDescriptor {
        id: format!("{series_uid}#d:{}", url.host),
        format: "DIMSE".into(),
        description: format!("{label} — {instances} instances, {size}"),
        modality,
        dims: Dims3::new(cols, rows, instances),
        sources: vec![PathBuf::from(format!("{}?StudyInstanceUID={study_uid}&SeriesInstanceUID={series_uid}", url.base))],
        volume: None,
    })
}

/// A parsed DIMSE PACS URL.
#[derive(Debug)]
struct DimseUrl {
    /// The URL without its query: `dicom://host[:port]/CALLED-AE`.
    base: String,
    /// Authority (`host[:port]`), for ids and error messages.
    host: String,
    /// Socket address (`host:port`, port 104 when the URL omits it).
    addr: String,
    /// The called AE title (the PACS node), `ANY-SCP` when omitted.
    called_ae: String,
    /// C-FIND matching attributes.
    filters: Vec<(Tag, String)>,
    /// StudyInstanceUID filter / retrieval target.
    study_uid: Option<String>,
    /// SeriesInstanceUID filter / retrieval target.
    series_uid: Option<String>,
}

/// Parses a DIMSE PACS URL. Errors name the problem and the host, never query
/// values. Reuses the DICOMweb URL helpers (one implementation, L23).
fn parse_dimse_url(text: &str) -> Result<DimseUrl, RepositoryError> {
    let rest = text
        .strip_prefix(SCHEME)
        .ok_or_else(|| RepositoryError::Unsupported(format!("{}: not a DIMSE URL", safe_url(text))))?;
    let (base, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = split_authority(base, text)?;
    let called_ae = path.trim_matches('/');
    if called_ae.contains('/') {
        return Err(RepositoryError::Unsupported(format!("{}: malformed DIMSE URL path", safe_url(text))));
    }
    let called_ae = if called_ae.is_empty() { "ANY-SCP" } else { called_ae };
    let addr = if authority.contains(':') { authority.to_string() } else { format!("{authority}:{DEFAULT_PORT}") };
    let mut url = DimseUrl {
        base: format!("{SCHEME}{authority}/{called_ae}"),
        host: authority.to_string(),
        addr,
        called_ae: called_ae.to_string(),
        filters: Vec::new(),
        study_uid: None,
        series_uid: None,
    };
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (keyword, value) = pair.split_once('=').unwrap_or((pair, ""));
        let keyword = percent_decode(keyword)?;
        let value = percent_decode(value)?;
        let tag = keyword_tag(&keyword)
            .ok_or_else(|| RepositoryError::Unsupported(format!("{}: unknown PACS query keyword {keyword:?}", url.host)))?;
        if tag == tags::STUDY_INSTANCE_UID {
            url.study_uid = Some(value.clone());
        }
        if tag == tags::SERIES_INSTANCE_UID {
            url.series_uid = Some(value.clone());
        }
        url.filters.push((tag, value));
    }
    Ok(url)
}
