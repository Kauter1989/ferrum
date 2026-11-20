//! STOW-RS send-off: storing de-identified SEG/SR exports to a PACS
//! ([design](../../../../docs/dicomweb.md) §9, ADR 0021 §5).
//!
//! **Host-only.** STOW-RS moves data off the workstation, so [`DicomWebStow`]
//! is called from a person's explicit action in a host and is **never
//! registered as an agent command** (the send-off boundary of ADR 0021 §5,
//! criterion 32.2-b). The agent surface (Stage 32, PR 3) wires only the
//! read-only query/retrieve/verify operations.
//!
//! The client is built on `dicom-web`'s STOW API (`store_instances`). It sends
//! the de-identified objects of a Stage 31 export ([`super::export`] with
//! `DicomExportOptions::deid`) and checks the response per instance:
//! success, warning or failure (PS3.18 §10.5.3, Store Instances Response
//! Module, Annex I).

use std::path::PathBuf;
use std::time::Duration;

use dicom_dictionary_std::tags;
use dicom_object::mem::InMemDicomObject;
use dicom_object::FileDicomObject;
use dicom_web::DicomWebClient;
use ferrum_domain::{ProgressSink, RepositoryError};

use super::header;
use super::web::{block_on_pacs, map_web_error, safe_url, split_authority, split_scheme, DEFAULT_TIMEOUT};

/// The outcome of one instance in a STOW-RS response (PS3.18 Annex I).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StowStatus {
    /// Stored successfully (Referenced SOP Sequence, status 0000).
    Success,
    /// Stored with a warning (Referenced SOP Sequence, warning reason).
    Warning,
    /// Not stored (Failed SOP Sequence, failure reason).
    Failure,
}

/// One instance's outcome in a STOW-RS response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StowInstance {
    /// Referenced SOP Class UID.
    pub sop_class_uid: String,
    /// Referenced SOP Instance UID.
    pub sop_instance_uid: String,
    /// Whether the instance was stored, stored with a warning, or failed.
    pub status: StowStatus,
    /// Warning Reason (0008,1196) or Failure Reason (0008,1197), when given.
    pub reason: Option<u16>,
}

/// The checked STOW-RS response: the per-instance outcomes the PACS reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StowReport {
    /// Every instance the response references, in success/warning/failure
    /// order.
    pub instances: Vec<StowInstance>,
}

impl StowReport {
    /// The number of instances with `status`.
    pub fn count(&self, status: StowStatus) -> usize {
        self.instances.iter().filter(|i| i.status == status).count()
    }

    /// `true` when no instance failed (warnings still count as stored).
    pub fn all_stored(&self) -> bool {
        self.instances.iter().all(|i| i.status != StowStatus::Failure)
    }
}

/// Stores DICOM objects to a PACS over STOW-RS. **Host-only** (ADR 0021 §5):
/// never an agent command.
///
/// Credentials are configured here, never parsed from a URL. Each operation
/// blocks on its own current-thread tokio runtime: call it from synchronous
/// code, not from inside an ambient tokio runtime.
#[derive(Debug, Clone)]
pub struct DicomWebStow {
    basic_auth: Option<(String, String)>,
    bearer_token: Option<String>,
    timeout: Duration,
}

impl Default for DicomWebStow {
    fn default() -> Self {
        Self::new()
    }
}

impl DicomWebStow {
    /// An anonymous client with the [`DEFAULT_TIMEOUT`] deadline.
    pub fn new() -> Self {
        Self { basic_auth: None, bearer_token: None, timeout: DEFAULT_TIMEOUT }
    }

    /// HTTP basic authentication for the store request.
    pub fn with_basic_auth(mut self, username: &str, password: &str) -> Self {
        self.basic_auth = Some((username.to_string(), password.to_string()));
        self
    }

    /// A bearer token for the store request (used when no basic auth is set).
    pub fn with_bearer_token(mut self, token: &str) -> Self {
        self.bearer_token = Some(token.to_string());
        self
    }

    /// The deadline of the store request; an expired deadline is an
    /// [`RepositoryError::Io`], never a hang.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn client(&self, http_base: &str) -> DicomWebClient {
        let mut client = DicomWebClient::with_single_url(http_base);
        if let Some((user, pass)) = &self.basic_auth {
            client.set_basic_auth(user, pass);
        } else if let Some(token) = &self.bearer_token {
            client.set_bearer_token(token);
        }
        client
    }

    /// Stores de-identified Part 10 objects (e.g. the SEG/SR of a Stage 31
    /// export) to the PACS service root `target`
    /// (`dicomweb://host[:port]/service-root`, `dicomwebs://` for HTTPS) and
    /// checks the response per instance. An empty `objects` is
    /// [`RepositoryError::NothingFound`].
    pub fn store(
        &self,
        target: &str,
        objects: &[FileDicomObject<InMemDicomObject>],
        progress: &dyn ProgressSink,
    ) -> Result<StowReport, RepositoryError> {
        if objects.is_empty() {
            return Err(RepositoryError::NothingFound);
        }
        let (http_base, host) = parse_target(target)?;
        let client = self.client(&http_base);
        let instances = objects.to_vec();
        let err_host = host.clone();
        let response = block_on_pacs(self.timeout, &host, async move {
            let stream = futures_util::stream::iter(instances);
            client.store_instances().with_instances(stream).run().await.map_err(|e| map_web_error(e, &err_host))
        })?;
        progress.report(1.0, "Stored PACS instances (STOW-RS)");
        Ok(parse_response(&response))
    }

    /// Stores de-identified Part 10 files (the `segmentation.dcm` /
    /// `measurements.dcm` of a Stage 31 de-identified export) to the PACS.
    /// This is the export → STOW hand-off: the exporter writes the
    /// de-identified objects, the host sends them.
    pub fn store_files(
        &self,
        target: &str,
        paths: &[PathBuf],
        progress: &dyn ProgressSink,
    ) -> Result<StowReport, RepositoryError> {
        let mut objects = Vec::with_capacity(paths.len());
        for path in paths {
            let obj = dicom_object::open_file(path)
                .map_err(|e| RepositoryError::Corrupt(format!("{}: {e}", path.display())))?;
            objects.push(obj);
        }
        self.store(target, &objects, progress)
    }
}

/// Parses a STOW target (the DICOMweb service root) into the `http(s)://`
/// base the client posts to and the authority used in errors. Errors name
/// the host only, never query values or credentials.
fn parse_target(text: &str) -> Result<(String, String), RepositoryError> {
    let (_, secure, rest) = split_scheme(text)
        .ok_or_else(|| RepositoryError::Unsupported(format!("{}: not a DICOMweb URL", safe_url(text))))?;
    let base = rest.split('?').next().unwrap_or(rest);
    let (host, _path) = split_authority(base, text)?;
    let http_base = format!("http{}://{base}", if secure { "s" } else { "" });
    Ok((http_base, host.to_string()))
}

/// Reads the per-instance outcomes of a STOW-RS response (PS3.18 Annex I,
/// Store Instances Response Module): the success, warning and failure
/// sequences.
fn parse_response(obj: &InMemDicomObject) -> StowReport {
    let mut instances = Vec::new();
    // Success and warning both live in the Referenced SOP Sequence (0008,1199):
    // an item carrying a non-zero Warning Reason (0008,1196) is a warning, any
    // other item is a success. (0008,1190) is the Retrieve URL — a single UR,
    // not an instance sequence (verified against a real Orthanc, PR 4; the
    // 0.10 dictionary names it RETRIEVE_URL).
    collect_referenced(obj, &mut instances);
    // Failure: Failed SOP Sequence (0008,1198).
    collect_sequence(obj, tags::FAILED_SOP_SEQUENCE, StowStatus::Failure, &mut instances);
    StowReport { instances }
}

/// Appends the Referenced SOP Sequence (0008,1199) items, splitting success
/// from warning by the presence of a non-zero Warning Reason (0008,1196).
fn collect_referenced(obj: &InMemDicomObject, out: &mut Vec<StowInstance>) {
    let Some(items) = obj.element_opt(tags::REFERENCED_SOP_SEQUENCE).ok().flatten().and_then(|e| e.items()) else { return };
    for item in items {
        let reason = header::int_of(item, tags::WARNING_REASON).map(|r| r as u16).filter(|&r| r != 0);
        let status = if reason.is_some() { StowStatus::Warning } else { StowStatus::Success };
        out.push(StowInstance {
            sop_class_uid: header::str_of(item, tags::REFERENCED_SOP_CLASS_UID).unwrap_or_default(),
            sop_instance_uid: header::str_of(item, tags::REFERENCED_SOP_INSTANCE_UID).unwrap_or_default(),
            status,
            reason,
        });
    }
}

/// Appends the instances of one response sequence with their status.
fn collect_sequence(obj: &InMemDicomObject, tag: dicom_core::Tag, status: StowStatus, out: &mut Vec<StowInstance>) {
    let Some(items) = obj.element_opt(tag).ok().flatten().and_then(|e| e.items()) else { return };
    let reason_tag = if status == StowStatus::Failure { tags::FAILURE_REASON } else { tags::WARNING_REASON };
    for item in items {
        out.push(StowInstance {
            sop_class_uid: header::str_of(item, tags::REFERENCED_SOP_CLASS_UID).unwrap_or_default(),
            sop_instance_uid: header::str_of(item, tags::REFERENCED_SOP_INSTANCE_UID).unwrap_or_default(),
            status,
            reason: header::int_of(item, reason_tag).map(|r| r as u16),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::{DataElement, PrimitiveValue, VR};

    fn referenced(class: &str, instance: &str) -> InMemDicomObject {
        InMemDicomObject::from_element_iter([
            DataElement::new(tags::REFERENCED_SOP_CLASS_UID, VR::UI, PrimitiveValue::from(class)),
            DataElement::new(tags::REFERENCED_SOP_INSTANCE_UID, VR::UI, PrimitiveValue::from(instance)),
        ])
    }

    #[test]
    fn targets_parse_into_base_and_host() {
        // covers 32.2-a: the service root becomes the STOW endpoint base
        let (base, host) = parse_target("dicomweb://pacs.example:8042/dicom-web").unwrap();
        assert_eq!(base, "http://pacs.example:8042/dicom-web");
        assert_eq!(host, "pacs.example:8042");
        let (base, _) = parse_target("dicomwebs://h/root").unwrap();
        assert_eq!(base, "https://h/root");
    }

    #[test]
    fn malformed_targets_are_refused_without_credentials() {
        // covers 32.2-b: a STOW target is a DICOMweb URL, no userinfo
        for bad in ["http://h/root", "/data/x", "dicomweb://user:pass@h/r", "dicomweb://"] {
            let err = parse_target(bad).unwrap_err();
            assert!(matches!(err, RepositoryError::Unsupported(_)), "{bad}: {err}");
            assert!(!err.to_string().contains("pass"), "{bad}: {err}");
        }
    }

    #[test]
    fn response_reports_success_warning_and_failure_per_instance() {
        // covers 32.2-a: the response is checked per instance
        let ok = referenced("1.2.840.10008.5.1.4.1.1.66.4", "1.2.3.1");
        let mut warn = referenced("1.2.840.10008.5.1.4.1.1.66.4", "1.2.3.2");
        warn.put(DataElement::new(tags::WARNING_REASON, VR::US, PrimitiveValue::from(0xB000u16)));
        let mut fail = referenced("1.2.840.10008.5.1.4.1.1.66.4", "1.2.3.3");
        fail.put(DataElement::new(tags::FAILURE_REASON, VR::US, PrimitiveValue::from(0xA700u16)));
        // success and warning share the Referenced SOP Sequence (0008,1199)
        let response = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::REFERENCED_SOP_SEQUENCE,
                VR::SQ,
                dicom_core::value::DataSetSequence::from(vec![ok, warn]),
            ),
            DataElement::new(tags::FAILED_SOP_SEQUENCE, VR::SQ, dicom_core::value::DataSetSequence::from(vec![fail])),
        ]);
        let report = parse_response(&response);
        assert_eq!(report.instances.len(), 3);
        assert_eq!(report.count(StowStatus::Success), 1);
        assert_eq!(report.count(StowStatus::Warning), 1);
        assert_eq!(report.count(StowStatus::Failure), 1);
        assert!(!report.all_stored());
        assert_eq!(report.instances[0].sop_instance_uid, "1.2.3.1");
        assert_eq!(report.instances[1].reason, Some(0xB000));
        assert_eq!(report.instances[2].reason, Some(0xA700));
    }

    #[test]
    fn an_all_success_response_is_stored() {
        // covers 32.2-a
        let response = InMemDicomObject::from_element_iter([DataElement::new(
            tags::REFERENCED_SOP_SEQUENCE,
            VR::SQ,
            dicom_core::value::DataSetSequence::from(vec![referenced("1.2", "1.2.3")]),
        )]);
        let report = parse_response(&response);
        assert!(report.all_stored());
        assert_eq!(report.count(StowStatus::Success), 1);
        // an empty response references nothing
        assert!(parse_response(&InMemDicomObject::new_empty()).instances.is_empty());
    }
}
