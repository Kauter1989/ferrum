//! PACS query/retrieve/verify — the read-only agent surface (Stage 32, PR 3,
//! criteria 32.4-a…b). Send-off (STOW-RS, C-MOVE, C-STORE) moves data off the
//! workstation and is **host-only**: it is absent from this command set
//! (ADR 0021 §5). `pacs retrieve` uses WADO-RS only; retrieval over DIMSE
//! (C-MOVE) is a host operation.
//!
//! The agent is thin over the `ferrum-io` repositories. Every URL is checked
//! against the operator's PACS allowlist (`network.pacs`) before any network
//! call — the SSRF boundary. Query output is pseudonymised (no patient name,
//! ID, birth date or accession number).

use std::path::{Path, PathBuf};

use ferrum_domain::{NoProgress, RepositoryError, SeriesDescriptor, VolumeRepository};
use ferrum_io::{CompositeRepository, DicomDimseRepository};
use serde_json::{json, Value};

use super::study::{describe, scan_entry};
use super::Ctx;
use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::study::Study;

/// `pacs query`: a read-only series query — QIDO-RS over DICOMweb or C-FIND
/// over DIMSE. Returns pseudonymised series entries (as `study scan`).
pub fn query(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let url = p.req_str("url")?;
    check_pacs_allowed(ctx.config, url)?;
    let series: Vec<Value> = scan_pacs(url)?.iter().map(|d| scan_entry(ctx.config, d)).collect();
    Ok(Output::new(json!({ "series": series })))
}

/// `pacs retrieve`: retrieves a series from a DICOMweb PACS over WADO-RS into
/// a workspace (read-only; the pixels stay in memory). DIMSE retrieval
/// (C-MOVE) is host-only and refused here.
pub fn retrieve(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let url = p.req_str("url")?;
    check_pacs_allowed(ctx.config, url)?;
    if url.starts_with("dicom://") {
        return Err(AgentError::bad_request(
            "pacs retrieve uses WADO-RS (dicomweb://); retrieval over DIMSE is C-MOVE, which is host-only",
        )
        .hint("query a DICOMweb PACS, or ask the host to retrieve over DIMSE"));
    }
    if !url.starts_with("dicomweb://") && !url.starts_with("dicomwebs://") {
        return Err(AgentError::bad_request("pacs retrieve needs a dicomweb:// or dicomwebs:// URL"));
    }
    let root = ctx.workspace_path(p)?;
    let study = Study::open(ctx.config, &root, Path::new(url), p.str("series")?, ctx.cache.as_deref_mut())?;
    let mut out = describe(ctx.config, &study);
    for n in &study.notes {
        out = out.warn(n.clone());
    }
    ctx.study = Some(study);
    Ok(out)
}

/// `pacs echo`: verifies a classic PACS node with C-ECHO (read-only).
pub fn echo(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let url = p.req_str("url")?;
    check_pacs_allowed(ctx.config, url)?;
    if !url.starts_with("dicom://") {
        return Err(AgentError::bad_request("pacs echo verifies a DIMSE node (dicom://host[:port]/CALLED-AE)"));
    }
    DicomDimseRepository::new()
        .echo(url)
        .map_err(|e| AgentError::internal(e.to_string()).hint("check the PACS address and called AE title"))?;
    Ok(Output::new(json!({ "reachable": true })))
}

/// Queries a PACS URL through the composite repository (QIDO or C-FIND by
/// scheme), mapping failures to envelope errors that name no query values.
fn scan_pacs(url: &str) -> Result<Vec<SeriesDescriptor>, AgentError> {
    CompositeRepository::default().scan(&[PathBuf::from(url)], &NoProgress).map_err(|e| match e {
        RepositoryError::NothingFound => {
            AgentError::not_found("the PACS query returned no series").hint("check the query keywords and the PACS URL")
        }
        RepositoryError::Unsupported(m) => AgentError::bad_request(m),
        other => AgentError::internal(other.to_string()),
    })
}

/// The SSRF boundary: refuses PACS URLs the operator does not allow. With no
/// configuration file only loopback hosts are allowed; link-local and cloud
/// metadata addresses are always refused, even when allow-listed. The check
/// judges the scheme + authority (`host[:port]`) — the network destination —
/// before any request is made.
fn check_pacs_allowed(config: &AgentConfig, url: &str) -> Result<(), AgentError> {
    let (scheme, authority, host) = pacs_url_parts(url)?;
    if is_blocked_host(&host) {
        return Err(AgentError::new(
            ErrorCode::Forbidden,
            format!("PACS host {host} is a link-local, unspecified or metadata address and is always refused"),
        ));
    }
    let allowed = if config.pacs.is_empty() {
        config.is_default && is_loopback_host(&host)
    } else {
        config
            .pacs
            .iter()
            .any(|entry| pacs_url_parts(entry).map(|(s, a, _)| s == scheme && a == authority).unwrap_or(false))
    };
    if !allowed {
        return Err(AgentError::new(ErrorCode::Forbidden, format!("PACS {scheme}://{authority} is not allowed"))
            .hint("the operator configuration lists the PACS FERRUM may contact (network.pacs)"));
    }
    Ok(())
}

/// `(scheme, authority, host)` of a PACS URL. Refuses a non-PACS scheme,
/// userinfo and an empty host. Error messages name the scheme and host only
/// — never the query (it may hold identifiers) or credentials.
fn pacs_url_parts(url: &str) -> Result<(String, String, String), AgentError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| AgentError::bad_request("not a PACS URL (dicomweb://, dicomwebs:// or dicom://)"))?;
    if !matches!(scheme, "dicomweb" | "dicomwebs" | "dicom") {
        return Err(AgentError::bad_request(format!("unsupported PACS scheme {scheme:?}")));
    }
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(AgentError::bad_request(format!("{scheme}://: PACS URL has no host")));
    }
    if authority.contains('@') {
        return Err(AgentError::bad_request("credentials in a PACS URL are refused; configure them on the host"));
    }
    if authority.chars().any(|c| c.is_whitespace()) {
        return Err(AgentError::bad_request("PACS host contains whitespace"));
    }
    Ok((scheme.to_string(), authority.to_string(), strip_port(authority).to_string()))
}

/// The host without its port (IPv6 brackets removed).
fn strip_port(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    authority.rsplit_once(':').map_or(
        authority,
        |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                h
            } else {
                authority
            }
        },
    )
}

/// A loopback host (`localhost`, `127.0.0.0/8`, `::1`).
fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Hosts that are never a legitimate PACS and are classic SSRF targets:
/// link-local (169.254.0.0/16, incl. the 169.254.169.254 metadata endpoint,
/// and IPv6 fe80::/10), unspecified (0.0.0.0, ::), broadcast, and the GCE
/// metadata hostname. Private/on-prem ranges (10/8, 192.168/16) are NOT
/// blocked here — a site PACS lives there; the operator allowlist governs
/// them (`docs/agent-cli.md`).
fn is_blocked_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("metadata.google.internal") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast(),
        Ok(std::net::IpAddr::V6(v6)) => {
            let link_local = (v6.segments()[0] & 0xffc0) == 0xfe80;
            let mapped_bad = v6
                .to_ipv4_mapped()
                .map(|v4| v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast())
                .unwrap_or(false);
            link_local || v6.is_unspecified() || mapped_bad
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pacs: &[&str], is_default: bool) -> AgentConfig {
        AgentConfig { pacs: pacs.iter().map(|s| s.to_string()).collect(), is_default, ..AgentConfig::default() }
    }

    #[test]
    fn url_parts_parse_scheme_authority_and_host() {
        // covers 32.4-a: the destination is parsed before any request
        let (s, a, h) = pacs_url_parts("dicomweb://pacs.example:8042/dicom-web?PatientID=1").unwrap();
        assert_eq!((s.as_str(), a.as_str(), h.as_str()), ("dicomweb", "pacs.example:8042", "pacs.example"));
        let (_, _, h) = pacs_url_parts("dicom://127.0.0.1:104/PACS").unwrap();
        assert_eq!(h, "127.0.0.1");
        let (_, _, h) = pacs_url_parts("dicomwebs://[::1]:8042/r").unwrap();
        assert_eq!(h, "::1");
        let (_, a, _) = pacs_url_parts("dicom://host/PACS").unwrap();
        assert_eq!(a, "host");
    }

    #[test]
    fn bad_urls_are_refused_without_their_values() {
        // covers 32.4-b: a query value never reaches an error
        for bad in [
            "http://h/r?PatientID=secret",
            "ftp://h",
            "dicomweb://",
            "dicomweb://user:pass@h/r",
            "dicomweb://ho st/r",
            "not a url",
        ] {
            let err = pacs_url_parts(bad).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadRequest, "{bad}: {err}");
            assert!(!err.to_string().contains("secret"), "{bad}: {err}");
            assert!(!err.to_string().contains("pass"), "{bad}: {err}");
        }
    }

    #[test]
    fn default_config_allows_only_loopback() {
        // covers 32.4-a: without configuration only loopback PACS are allowed
        let c = config(&[], true);
        assert!(check_pacs_allowed(&c, "dicomweb://127.0.0.1:8042/dicom-web").is_ok());
        assert!(check_pacs_allowed(&c, "dicom://localhost:104/PACS").is_ok());
        assert!(check_pacs_allowed(&c, "dicomwebs://[::1]:8042/r").is_ok());
        assert_eq!(check_pacs_allowed(&c, "dicomweb://pacs.example/r").unwrap_err().code, ErrorCode::Forbidden);
        // a configured (non-default) empty allowlist refuses everything
        let strict = config(&[], false);
        assert_eq!(check_pacs_allowed(&strict, "dicomweb://127.0.0.1/r").unwrap_err().code, ErrorCode::Forbidden);
    }

    #[test]
    fn allowlist_matches_scheme_and_authority() {
        // covers 32.4-a: the operator allowlist is the boundary
        let c = config(&["dicomweb://pacs.example:8042/dicom-web", "dicom://10.0.0.5:104/PACS"], false);
        assert!(check_pacs_allowed(&c, "dicomweb://pacs.example:8042/dicom-web?PatientID=1").is_ok());
        assert!(check_pacs_allowed(&c, "dicom://10.0.0.5:104/OTHER-AE").is_ok());
        // wrong port, wrong scheme, wrong host are refused
        assert_eq!(check_pacs_allowed(&c, "dicomweb://pacs.example:9999/r").unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(check_pacs_allowed(&c, "dicomwebs://pacs.example:8042/r").unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(check_pacs_allowed(&c, "dicomweb://evil.example/r").unwrap_err().code, ErrorCode::Forbidden);
    }

    #[test]
    fn link_local_and_metadata_hosts_are_always_refused() {
        // covers 32.4-a (SSRF): even an allow-listed metadata host is refused
        let c = config(
            &[
                "dicomweb://169.254.169.254/latest",
                "dicomweb://169.254.1.1/r",
                "dicomweb://[fe80::1]/r",
                "dicomweb://metadata.google.internal/r",
                "dicomweb://0.0.0.0/r",
                "dicomweb://[::]/r",
                "dicomweb://[::ffff:169.254.169.254]/r",
            ],
            false,
        );
        for url in [
            "dicomweb://169.254.169.254/latest/meta-data",
            "dicomweb://169.254.1.1/r",
            "dicomweb://[fe80::1]/r",
            "dicomweb://metadata.google.internal/r",
            "dicomweb://0.0.0.0/r",
            "dicomweb://[::]/r",
            "dicomweb://[::ffff:169.254.169.254]/r",
        ] {
            assert_eq!(check_pacs_allowed(&c, url).unwrap_err().code, ErrorCode::Forbidden, "{url}");
        }
        // a normal private PACS is not blocked by the host rule (allowlist governs)
        assert!(!is_blocked_host("10.0.0.5"));
        assert!(!is_blocked_host("192.168.1.10"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(!is_loopback_host("10.0.0.5"));
    }
}
