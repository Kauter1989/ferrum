//! Review decisions made by a person in a host application, bound to what
//! was shown (16.7, issue #71).
//!
//! A host such as Medical Harness lets only a signed-in person confirm or
//! reject, through a channel no model tool reaches. It calls this library
//! API, **never an agent command**: there is no JSON Schema and no MCP tool
//! for it, so an agent cannot reach it.
//!
//! 1. When the host shows an item it reads its [`revision`]: a SHA-256 over
//!    the study's source hash and the item's content (an annotation's
//!    geometry, text and name; a segment's voxels and name). Review state is
//!    not part of it.
//! 2. When the person decides, the host passes the revision back, with the
//!    person it authenticated and a SHA-256 of what it showed (view state
//!    and values, in the host's own canonical form).
//! 3. [`record`] refuses with `source_changed` if the item changed since it
//!    was shown, writes the decision into the workspace like the desktop
//!    review queue, and appends an audit entry that holds both hashes.
//!
//! The agent command `review confirm|reject` and its operator switch stay
//! as they are, for harnesses that only have the command line.

use std::path::Path;

use ferrum_domain::analysis::MaskRegion;
use ferrum_domain::{ReviewStatus, Timestamp};
use ferrum_io::annotations::annotation_report_json;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode};
use crate::study::Study;

/// The item a decision is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// An annotation by id.
    Annotation(u64),
    /// A segment by label.
    Segment(u8),
}

impl Item {
    fn describe(self) -> String {
        match self {
            Item::Annotation(id) => format!("annotation {id}"),
            Item::Segment(label) => format!("segment {label}"),
        }
    }
}

/// What the person decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The item is right.
    Confirm,
    /// The item is wrong.
    Reject,
}

/// A decision as the host passes it.
#[derive(Debug, Clone, Copy)]
pub struct Decision<'a> {
    /// The item decided on.
    pub item: Item,
    /// Confirm or reject.
    pub verdict: Verdict,
    /// The named person the host authenticated.
    pub person: &'a str,
    /// The item's [`revision`] when it was shown.
    pub revision: &'a str,
    /// SHA-256 (64 hex digits) of what the host showed: view state and values.
    pub shown_sha256: &'a str,
}

/// A decision as it was recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    /// The item.
    pub item: Item,
    /// Its review status now.
    pub status: ReviewStatus,
    /// Who decided.
    pub person: String,
    /// The revision decided on.
    pub revision: String,
    /// The host's hash of what was shown.
    pub shown_sha256: String,
    /// When it was recorded.
    pub time: Timestamp,
}

/// The current revision of `item` in the workspace at `workspace` (resolved
/// like the agent's workspaces).
pub fn revision(config: &AgentConfig, workspace: &Path, item: Item) -> Result<String, AgentError> {
    let study = Study::load(config, &config.resolve_workspace(workspace)?, None)?;
    item_revision(config, &study, item)
}

/// Records `d` in the workspace at `workspace` and its audit log; see the
/// module documentation.
pub fn record(config: &AgentConfig, workspace: &Path, d: &Decision) -> Result<Recorded, AgentError> {
    let person = check_person(d.person)?;
    let shown = check_hash("shown_sha256", d.shown_sha256)?;
    let revision = check_hash("revision", d.revision)?;
    let mut study = Study::load(config, &config.resolve_workspace(workspace)?, None)?;
    let now = item_revision(config, &study, d.item)?;
    if now != revision {
        return Err(AgentError::new(
            ErrorCode::SourceChanged,
            format!("{} changed since it was shown; nothing was recorded", d.item.describe()),
        )
        .hint("show the item again, read its new revision, and let the person decide on that"));
    }
    let status = match d.verdict {
        Verdict::Confirm => ReviewStatus::Confirmed,
        Verdict::Reject => ReviewStatus::Rejected,
    };
    let time = Timestamp::now();
    apply(config, &mut study, d.item, status, &person, time)?;
    let (kind, id) = match d.item {
        Item::Annotation(id) => ("annotation", id),
        Item::Segment(label) => ("segment", u64::from(label)),
    };
    let command = if status == ReviewStatus::Confirmed { "review confirm" } else { "review reject" };
    study.workspace.append_audit(json!({
        "command": command,
        "source": "host",
        "params": { kind: id, "by": person, "revision": revision, "shown_sha256": shown },
        "ok": true,
    }))?;
    Ok(Recorded { item: d.item, status, person, revision, shown_sha256: shown, time })
}

fn apply(
    config: &AgentConfig,
    study: &mut Study,
    item: Item,
    status: ReviewStatus,
    person: &str,
    at: Timestamp,
) -> Result<(), AgentError> {
    match item {
        Item::Annotation(id) => {
            if !study.annotations.review(id, status, Some(person), at) {
                return Err(AgentError::not_found(format!("unknown annotation {id}")));
            }
            study.save_annotations(config)
        }
        Item::Segment(label) => {
            study
                .segments
                .review(label, status, Some(person), at)
                .map_err(|_| AgentError::not_found(format!("unknown segment {label}")))?;
            study.save_segments()
        }
    }
}

fn item_revision(config: &AgentConfig, study: &Study, item: Item) -> Result<String, AgentError> {
    let mut h = Sha256::new();
    h.update(b"ferrum-review-revision/1\n");
    h.update(study.source_sha256.as_bytes());
    match item {
        Item::Annotation(id) => {
            let report = annotation_report_json(&study.annotation_report(config), "");
            let mut entry = report["annotations"]
                .as_array()
                .and_then(|a| a.iter().find(|a| a["id"] == json!(id)))
                .cloned()
                .ok_or_else(|| AgentError::not_found(format!("unknown annotation {id}")))?;
            // review state is not content: a decision does not change what was shown
            if let Value::Object(m) = &mut entry {
                m.remove("provenance");
            }
            h.update(b"annotation\n");
            h.update(entry.to_string().as_bytes());
        }
        Item::Segment(label) => {
            let segment = study
                .segments
                .segment(label)
                .ok_or_else(|| AgentError::not_found(format!("unknown segment {label}")))?;
            h.update(format!("segment\n{label}\n{}\n", segment.name).as_bytes());
            if let Some(r) = MaskRegion::of_label(study.segments.labels(), label) {
                h.update(format!("{:?}\n", r.bx).as_bytes());
                h.update(r.to_u8());
            }
        }
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn check_person(person: &str) -> Result<String, AgentError> {
    let p = person.trim();
    if p.is_empty() || p.chars().count() > 200 || p.chars().any(char::is_control) {
        return Err(AgentError::bad_request(
            "person must name who decided (1 to 200 characters, no control characters)",
        ));
    }
    Ok(p.to_owned())
}

fn check_hash(what: &str, h: &str) -> Result<String, AgentError> {
    if h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(h.to_ascii_lowercase())
    } else {
        Err(AgentError::bad_request(format!("{what} must be a SHA-256 in 64 hex digits")))
    }
}
