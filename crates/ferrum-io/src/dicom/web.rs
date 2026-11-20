//! DICOMweb data source: QIDO-RS query and WADO-RS retrieval
//! ([design](../../../../docs/dicomweb.md), ADR 0021).
//!
//! [`DicomWebRepository`] implements [`VolumeRepository`] over PACS URLs:
//! `scan` interprets a `dicomweb://host/service-root?PatientID=…` path as a
//! QIDO series query, `load` retrieves the series' instances over WADO-RS
//! and assembles them through the same pipeline as the file reader
//! ([`super::assemble`]), so a retrieved study is voxel-identical to the
//! same study opened from files, in the canonical LPS frame.
//!
//! Retrieved data stays in memory; nothing is written to disk. Send-off
//! (STOW-RS) is host-only and not part of this repository (ADR 0021 §5).

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use dicom_core::ops::{AttributeSelector, AttributeSelectorStep};
use dicom_dictionary_std::tags;
use dicom_object::mem::InMemDicomObject;
use dicom_object::{FileDicomObject, Tag};
use dicom_web::{DicomWebClient, DicomWebError};
use ferrum_domain::{
    Dims3, LoadedSeries, ProgressSink, RepositoryError, SeriesDescriptor, SeriesMetadata, VolumeRepository,
};
use futures_util::StreamExt;

use super::header::{self, SliceHeader};
use super::{assemble, series, ScaledProgress};

/// URL scheme of a DICOMweb service over HTTP.
pub const SCHEME: &str = "dicomweb://";
/// URL scheme of a DICOMweb service over HTTPS.
pub const SECURE_SCHEME: &str = "dicomwebs://";
/// Default deadline of one PACS request (connection and body included).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Attributes requested from QIDO so a descriptor can be built without
/// retrieving pixels.
const INCLUDE_FIELDS: [Tag; 8] = [
    tags::MODALITY,
    tags::SERIES_DESCRIPTION,
    tags::SERIES_NUMBER,
    tags::NUMBER_OF_SERIES_RELATED_INSTANCES,
    tags::ROWS,
    tags::COLUMNS,
    tags::STUDY_INSTANCE_UID,
    tags::SERIES_INSTANCE_UID,
];

/// Reads series from a PACS over DICOMweb (QIDO-RS + WADO-RS).
///
/// Paths given to [`VolumeRepository::scan`] are PACS URLs
/// (`dicomweb://host[:port]/service-root?keyword=value&…`, `dicomwebs://`
/// for HTTPS); any other path is `NothingFound`, so the repository composes
/// with the file readers. Credentials are configured here, never parsed
/// from a URL (they would leak into logs and error strings).
///
/// Each operation blocks on its own current-thread tokio runtime: call it
/// from synchronous code, not from inside an ambient tokio runtime.
#[derive(Debug, Clone)]
pub struct DicomWebRepository {
    basic_auth: Option<(String, String)>,
    bearer_token: Option<String>,
    timeout: Duration,
}

impl Default for DicomWebRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl DicomWebRepository {
    /// An anonymous repository with the [`DEFAULT_TIMEOUT`] deadline.
    pub fn new() -> Self {
        Self { basic_auth: None, bearer_token: None, timeout: DEFAULT_TIMEOUT }
    }

    /// HTTP basic authentication for every request.
    pub fn with_basic_auth(mut self, username: &str, password: &str) -> Self {
        self.basic_auth = Some((username.to_string(), password.to_string()));
        self
    }

    /// A bearer token for every request (used when no basic auth is set).
    pub fn with_bearer_token(mut self, token: &str) -> Self {
        self.bearer_token = Some(token.to_string());
        self
    }

    /// The deadline of one PACS request; an expired deadline is an
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

    /// Blocks on one PACS request under the configured deadline. The
    /// future maps its own errors; an expired deadline drops the request
    /// and reports a timeout naming the host only.
    fn run_request<T>(
        &self,
        host: &str,
        fut: impl Future<Output = Result<T, RepositoryError>>,
    ) -> Result<T, RepositoryError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| RepositoryError::Io(format!("PACS client runtime: {e}")))?;
        let timeout = self.timeout;
        let host = host.to_string();
        runtime.block_on(async move {
            match tokio::time::timeout(timeout, fut).await {
                Ok(result) => result,
                Err(_) => {
                    Err(RepositoryError::Io(format!("{host}: PACS request timed out after {} s", timeout.as_secs())))
                }
            }
        })
    }

    /// One QIDO series query: study-scoped when the URL filters by
    /// StudyInstanceUID, global otherwise.
    fn query_series(&self, url: &PacsUrl) -> Result<Vec<InMemDicomObject>, RepositoryError> {
        let client = self.client(&url.http_base);
        let mut request = match &url.study_uid {
            Some(uid) => client.query_series_in_study(uid),
            None => client.query_series(),
        };
        request = request.with_includefields(INCLUDE_FIELDS.to_vec());
        for (tag, value) in &url.filters {
            if *tag == tags::STUDY_INSTANCE_UID {
                continue; // already in the endpoint path
            }
            let selector = AttributeSelector::new([AttributeSelectorStep::Tag(*tag)])
                .ok_or_else(|| RepositoryError::Unsupported("PACS query: bad attribute selector".into()))?;
            request = request.with_filter(selector, value.clone());
        }
        if let Some(limit) = url.limit {
            request = request.with_limit(limit);
        }
        if let Some(offset) = url.offset {
            request = request.with_offset(offset);
        }
        let host = url.host.clone();
        self.run_request(&url.host, async move { request.run().await.map_err(|e| map_web_error(e, &host)) })
    }

    /// Retrieves the series' instances (whole Part 10 objects: metadata and
    /// pixel data) over WADO-RS, reporting progress per instance.
    fn retrieve(
        &self,
        target: &RetrievalTarget,
        expected: u32,
        progress: &dyn ProgressSink,
    ) -> Result<Vec<FileDicomObject<InMemDicomObject>>, RepositoryError> {
        let client = self.client(&target.http_base);
        let host = target.host.clone();
        let (study, series) = (target.study.clone(), target.series.clone());
        self.run_request(&target.host, async move {
            let stream = client.retrieve_series(&study, &series).run().await.map_err(|e| map_web_error(e, &host))?;
            let mut stream = Box::pin(stream);
            let mut objects = Vec::new();
            while let Some(item) = stream.next().await {
                if progress.is_cancelled() {
                    return Err(RepositoryError::Cancelled);
                }
                objects.push(item.map_err(|e| map_web_error(e, &host))?);
                let done = objects.len() as f32;
                let total = (expected as f32).max(done + 1.0);
                progress.report((done / total).min(0.95), "Retrieving PACS instances");
            }
            Ok(objects)
        })
    }
}

impl VolumeRepository for DicomWebRepository {
    fn name(&self) -> &str {
        "DICOMweb"
    }

    fn scan(&self, paths: &[PathBuf], progress: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        let mut out: Vec<SeriesDescriptor> = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            if progress.is_cancelled() {
                return Err(RepositoryError::Cancelled);
            }
            let Some(text) = path.to_str() else { continue };
            if split_scheme(text).is_none() {
                continue; // a file path, not a PACS query
            }
            let url = parse_pacs_url(text)?;
            for obj in self.query_series(&url)? {
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
        let url = series
            .sources
            .first()
            .and_then(|p| p.to_str())
            .ok_or_else(|| RepositoryError::Corrupt("DICOMweb series carries no retrieval URL".into()))?;
        let target = parse_retrieval_url(url)?;
        let objects = self.retrieve(&target, series.dims.z, &ScaledProgress::new(progress, 0.0, 0.5))?;
        if objects.is_empty() {
            return Err(RepositoryError::NothingFound);
        }
        let (headers, objects) = headers_of(&target, objects)?;
        let key = series::group(headers.clone())
            .into_iter()
            .find(|(k, _)| k.series_uid == target.series)
            .map(|(k, _)| k)
            .ok_or(RepositoryError::NothingFound)?;
        // Keep the requested group with its objects aligned (the decoder
        // indexes objects by header position).
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
        Ok(loaded_series(volume, &hs, &objs, &ordered, url))
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

/// Slice headers of retrieved instances, aligned with `objects`; an
/// instance without a readable image header aborts the load (the series
/// was advertised as loadable).
fn headers_of(
    target: &RetrievalTarget,
    objects: Vec<FileDicomObject<InMemDicomObject>>,
) -> Result<(Vec<SliceHeader>, Vec<FileDicomObject<InMemDicomObject>>), RepositoryError> {
    let mut headers = Vec::with_capacity(objects.len());
    for obj in &objects {
        let sop = header::str_of(obj, tags::SOP_INSTANCE_UID).unwrap_or_default();
        let path =
            PathBuf::from(format!("{}/studies/{}/series/{}/instances/{sop}", target.base, target.study, target.series));
        headers.push(SliceHeader::from_object(obj, &path)?);
    }
    Ok((headers, objects))
}

/// The descriptor of one QIDO series result; `None` when it carries no
/// SeriesInstanceUID (it could not be retrieved).
fn describe(url: &PacsUrl, obj: &InMemDicomObject) -> Option<SeriesDescriptor> {
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
        id: format!("{series_uid}#w:{}", url.host),
        format: "DICOMweb".into(),
        description: format!("{label} — {instances} instances, {size}"),
        modality,
        dims: Dims3::new(cols, rows, instances),
        sources: vec![PathBuf::from(format!("{}/studies/{study_uid}/series/{series_uid}", url.base))],
        volume: None,
    })
}

/// A parsed PACS query URL.
#[derive(Debug)]
struct PacsUrl {
    /// The URL without its query, in its original `dicomweb(s)://` scheme.
    base: String,
    /// The same base as an `http(s)://` URL for the DICOMweb client.
    http_base: String,
    /// Authority (`host[:port]`), for ids and error messages.
    host: String,
    /// QIDO matching attributes (keyword order preserved).
    filters: Vec<(Tag, String)>,
    /// StudyInstanceUID filter, served by the study-scoped endpoint.
    study_uid: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

/// `(scheme, secure, rest)` when `text` starts with a DICOMweb scheme.
fn split_scheme(text: &str) -> Option<(&'static str, bool, &str)> {
    if let Some(rest) = text.strip_prefix(SCHEME) {
        Some((SCHEME, false, rest))
    } else {
        text.strip_prefix(SECURE_SCHEME).map(|rest| (SECURE_SCHEME, true, rest))
    }
}

/// The URL quoted in errors: scheme, authority and path only — the query
/// may hold identifiers (PatientID, accession) and userinfo may hold
/// credentials; neither appears.
pub(crate) fn safe_url(text: &str) -> String {
    let no_query = text.split('?').next().unwrap_or(text);
    if let Some((scheme, rest)) = no_query.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        if let Some((_userinfo, host)) = authority.rsplit_once('@') {
            return if path.is_empty() { format!("{scheme}://{host}") } else { format!("{scheme}://{host}/{path}") };
        }
    }
    no_query.to_string()
}

/// Parses a PACS query URL. Errors name the problem and the host, never
/// query values.
fn parse_pacs_url(text: &str) -> Result<PacsUrl, RepositoryError> {
    let (scheme, secure, rest) = split_scheme(text)
        .ok_or_else(|| RepositoryError::Unsupported(format!("{}: not a DICOMweb URL", safe_url(text))))?;
    let (base, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, _path) = split_authority(base, text)?;
    let mut url = PacsUrl {
        base: format!("{scheme}{base}"),
        http_base: format!("http{}://{base}", if secure { "s" } else { "" }),
        host: host.to_string(),
        filters: Vec::new(),
        study_uid: None,
        limit: None,
        offset: None,
    };
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (keyword, value) = pair.split_once('=').unwrap_or((pair, ""));
        let keyword = percent_decode(keyword)?;
        let value = percent_decode(value)?;
        match keyword.as_str() {
            "limit" => url.limit = Some(parse_number(&keyword, &value)?),
            "offset" => url.offset = Some(parse_number(&keyword, &value)?),
            _ => {
                let tag = keyword_tag(&keyword).ok_or_else(|| {
                    RepositoryError::Unsupported(format!("{}: unknown PACS query keyword {keyword:?}", url.host))
                })?;
                if tag == tags::STUDY_INSTANCE_UID {
                    url.study_uid = Some(value.clone());
                }
                url.filters.push((tag, value));
            }
        }
    }
    Ok(url)
}

/// `(authority, path)` of the URL body; refuses userinfo and a bad port.
/// The path keeps its leading `/` (empty when the URL has no path).
pub(crate) fn split_authority<'a>(base: &'a str, whole: &str) -> Result<(&'a str, &'a str), RepositoryError> {
    let authority = base.split('/').next().unwrap_or(base);
    let path = &base[authority.len()..];
    let bad = |reason: &str| RepositoryError::Unsupported(format!("{}: {reason}", safe_url(whole)));
    if authority.is_empty() {
        return Err(bad("PACS URL has no host"));
    }
    if authority.contains('@') {
        return Err(bad("credentials in a PACS URL are refused; configure them on the repository"));
    }
    if authority.chars().any(|c| c.is_whitespace()) {
        return Err(bad("PACS host contains whitespace"));
    }
    if let Some(port) = authority.rsplit_once(':').map(|(_, p)| p) {
        if port.parse::<u16>().is_err() {
            return Err(bad("PACS URL has an invalid port"));
        }
    }
    Ok((authority, path))
}

fn parse_number(keyword: &str, value: &str) -> Result<u32, RepositoryError> {
    value
        .parse::<u32>()
        .map_err(|_| RepositoryError::Unsupported(format!("PACS query: {keyword} must be a non-negative integer")))
}

/// The QIDO matching attribute a query keyword stands for.
pub(crate) fn keyword_tag(keyword: &str) -> Option<Tag> {
    Some(match keyword {
        "PatientID" => tags::PATIENT_ID,
        "PatientName" => tags::PATIENT_NAME,
        "AccessionNumber" => tags::ACCESSION_NUMBER,
        "StudyInstanceUID" => tags::STUDY_INSTANCE_UID,
        "SeriesInstanceUID" => tags::SERIES_INSTANCE_UID,
        "StudyDate" => tags::STUDY_DATE,
        "Modality" => tags::MODALITY,
        "StudyDescription" => tags::STUDY_DESCRIPTION,
        "SeriesDescription" => tags::SERIES_DESCRIPTION,
        "SeriesNumber" => tags::SERIES_NUMBER,
        _ => return None,
    })
}

/// Percent-decoding of one URL component (`%XX`; `+` stays literal).
pub(crate) fn percent_decode(text: &str) -> Result<String, RepositoryError> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| RepositoryError::Unsupported("PACS URL: truncated percent-encoding".into()))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| RepositoryError::Unsupported("PACS URL: query is not UTF-8".into()))
}

/// Where one series is retrieved from: the parsed `sources[0]` URL.
#[derive(Debug)]
struct RetrievalTarget {
    base: String,
    http_base: String,
    host: String,
    study: String,
    series: String,
}

/// Parses a retrieval URL
/// `dicomweb(s)://host/root/studies/{study}/series/{series}`.
fn parse_retrieval_url(text: &str) -> Result<RetrievalTarget, RepositoryError> {
    let bad = || RepositoryError::Corrupt(format!("{}: malformed DICOMweb retrieval URL", safe_url(text)));
    let (scheme, secure, rest) = split_scheme(text).ok_or_else(bad)?;
    let (host, path) = split_authority(rest, text).map_err(|_| bad())?;
    let (root, tail) = path.split_once("/studies/").ok_or_else(bad)?;
    let (study, series) = tail.split_once("/series/").ok_or_else(bad)?;
    if study.is_empty() || series.is_empty() || study.contains('/') || series.contains('/') {
        return Err(bad());
    }
    Ok(RetrievalTarget {
        base: format!("{scheme}{host}{root}"),
        http_base: format!("http{}://{host}{root}", if secure { "s" } else { "" }),
        host: host.to_string(),
        study: study.to_string(),
        series: series.to_string(),
    })
}

/// Maps a `dicom-web` failure to a repository error. Messages name the host
/// and the failure kind; the request URL (its query may hold identifiers)
/// and credentials never appear.
fn map_web_error(e: DicomWebError, host: &str) -> RepositoryError {
    match e {
        DicomWebError::RequestFailed { source, .. } => {
            if source.is_connect() {
                RepositoryError::Io(format!("{host}: PACS connection failed"))
            } else if source.is_timeout() {
                RepositoryError::Io(format!("{host}: PACS request timed out"))
            } else if source.is_body() || source.is_decode() {
                RepositoryError::Corrupt(format!("{host}: PACS response body failed"))
            } else {
                RepositoryError::Io(format!("{host}: PACS request failed"))
            }
        }
        DicomWebError::HttpStatusFailure { status_code } => {
            if status_code.as_u16() == 401 || status_code.as_u16() == 403 {
                RepositoryError::Io(format!("{host}: PACS authentication failed (HTTP {status_code})"))
            } else {
                RepositoryError::Io(format!("{host}: PACS returned HTTP {status_code}"))
            }
        }
        DicomWebError::EmptyResponse => RepositoryError::NothingFound,
        DicomWebError::DeserializationFailed { .. }
        | DicomWebError::UnexpectedContentType { .. }
        | DicomWebError::MissingContentTypeHeader
        | DicomWebError::ContentTypeParseFailed { .. } => {
            RepositoryError::Corrupt(format!("{host}: PACS response is not DICOM JSON"))
        }
        DicomWebError::MultipartReaderFailed { .. }
        | DicomWebError::UnexpectedMultipartType { .. }
        | DicomWebError::DicomReaderFailed { .. } => {
            RepositoryError::Corrupt(format!("{host}: PACS response is not DICOM"))
        }
        // The message is dropped: a future dicom-web Display could carry request
        // detail; the host and the failure kind are all a repository error names.
        DicomWebError::Other { .. } => RepositoryError::Io(format!("{host}: PACS request failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacs_urls_parse_into_base_and_filters() {
        // covers 32.1-a: the path-shaped query becomes a QIDO request
        let url =
            parse_pacs_url("dicomweb://pacs.example:8042/dicom-web?PatientID=123&StudyDate=20240101&limit=50").unwrap();
        assert_eq!(url.http_base, "http://pacs.example:8042/dicom-web");
        assert_eq!(url.base, "dicomweb://pacs.example:8042/dicom-web");
        assert_eq!(url.host, "pacs.example:8042");
        assert_eq!(url.filters, vec![(tags::PATIENT_ID, "123".into()), (tags::STUDY_DATE, "20240101".into())]);
        assert_eq!(url.limit, Some(50));
        assert_eq!(url.offset, None);
        assert_eq!(url.study_uid, None);
    }

    #[test]
    fn secure_scheme_and_study_scope() {
        // covers 32.1-a
        let url = parse_pacs_url("dicomwebs://host/root?StudyInstanceUID=1.2.3&Modality=CT").unwrap();
        assert_eq!(url.http_base, "https://host/root");
        assert_eq!(url.study_uid.as_deref(), Some("1.2.3"));
        assert_eq!(url.filters.len(), 2);
    }

    #[test]
    fn values_are_percent_decoded_once() {
        // covers 32.1-a
        let url = parse_pacs_url("dicomweb://h/r?PatientName=DOE%5EJOHN&SeriesDescription=a%20b").unwrap();
        assert_eq!(url.filters[0].1, "DOE^JOHN");
        assert_eq!(url.filters[1].1, "a b");
    }

    #[test]
    fn unknown_keywords_and_bad_numbers_are_refused_without_their_values() {
        // covers 32.1-c: the refusal names the keyword, not the value
        let err = parse_pacs_url("dicomweb://h/r?Secret=patient-id-123").unwrap_err();
        assert!(matches!(err, RepositoryError::Unsupported(_)), "{err}");
        assert!(err.to_string().contains("Secret"), "{err}");
        assert!(!err.to_string().contains("patient-id-123"), "{err}");
        let err = parse_pacs_url("dicomweb://h/r?limit=many").unwrap_err();
        assert!(err.to_string().contains("limit"), "{err}");
        assert!(!err.to_string().contains("many"), "{err}");
    }

    #[test]
    fn userinfo_and_malformed_hosts_are_refused() {
        // covers 32.1-c: credentials never ride in a URL
        for bad in [
            "dicomweb://user:pass@host/r",
            "dicomweb://",
            "dicomweb://host:notaport/r",
            "dicomweb://ho st/r",
            "dicomweb://h/r?PatientID=%zz",
            "dicomweb://h/r?PatientID=%2",
        ] {
            let err = parse_pacs_url(bad).unwrap_err();
            assert!(matches!(err, RepositoryError::Unsupported(_)), "{bad}: {err}");
            assert!(!err.to_string().contains("pass"), "{bad}: {err}");
        }
    }

    #[test]
    fn file_paths_are_not_pacs_urls() {
        // covers 32.1-a: composite safety — a file path is NothingFound
        assert!(split_scheme("/data/ct").is_none());
        assert!(split_scheme("C:\\data\\ct").is_none());
        assert!(split_scheme("http://host/r").is_none());
        assert!(split_scheme("dicomweb://host/r").is_some());
        assert!(split_scheme("dicomwebs://host/r").is_some());
        let repo = DicomWebRepository::new();
        let err = repo.scan(&[PathBuf::from("/data/ct")], &ferrum_domain::NoProgress).unwrap_err();
        assert_eq!(err, RepositoryError::NothingFound);
        let err = repo.scan(&[], &ferrum_domain::NoProgress).unwrap_err();
        assert_eq!(err, RepositoryError::NothingFound);
    }

    #[test]
    fn retrieval_urls_round_trip() {
        // covers 32.1-a: sources[0] carries everything load needs
        use dicom_core::{DataElement, PrimitiveValue, VR};
        let url = parse_pacs_url("dicomweb://h:8042/dicom-web?PatientID=1").unwrap();
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, PrimitiveValue::from("1.2.3")),
            DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, PrimitiveValue::from("1.2")),
            DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(tags::NUMBER_OF_SERIES_RELATED_INSTANCES, VR::IS, PrimitiveValue::from("7")),
        ]);
        let d = describe(&url, &obj).unwrap();
        assert_eq!(d.id, "1.2.3#w:h:8042");
        assert_eq!(d.format, "DICOMweb");
        assert_eq!(d.modality, "CT");
        assert_eq!(d.dims, Dims3::new(0, 0, 7));
        let target = parse_retrieval_url(d.sources[0].to_str().unwrap()).unwrap();
        assert_eq!(target.http_base, "http://h:8042/dicom-web");
        assert_eq!(target.study, "1.2");
        assert_eq!(target.series, "1.2.3");
        // a result without a series uid cannot be retrieved
        let empty = InMemDicomObject::new_empty();
        assert!(describe(&url, &empty).is_none());
    }

    #[test]
    fn malformed_retrieval_urls_are_corrupt() {
        // covers 32.1-c
        for bad in ["dicomweb://h/r", "dicomweb://h/r/studies//series/1", "dicomweb://h/r/studies/1/series/", "/data/x"]
        {
            assert!(matches!(parse_retrieval_url(bad), Err(RepositoryError::Corrupt(_))), "{bad}");
        }
    }

    #[test]
    fn error_text_drops_the_query() {
        // covers 32.1-c: a PatientID in the query never reaches an error
        assert_eq!(safe_url("dicomweb://h/r?PatientID=secret"), "dicomweb://h/r");
        let err = map_web_error(DicomWebError::EmptyResponse, "pacs.example");
        assert_eq!(err, RepositoryError::NothingFound);
        let err = map_web_error(DicomWebError::MissingContentTypeHeader, "pacs.example");
        assert!(matches!(err, RepositoryError::Corrupt(_)), "{err}");
        assert!(err.to_string().contains("pacs.example"), "{err}");
    }
}
