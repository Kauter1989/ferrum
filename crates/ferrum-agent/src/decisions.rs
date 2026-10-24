//! Review decisions made by a person in a host application, bound to what
//! was shown (16.7, issue #71).
//!
//! A host such as Medical Harness lets only a signed-in person confirm or
//! reject, through a channel no model tool reaches. It calls this library
//! API, **never an agent command**: there is no JSON Schema and no MCP tool
//! for it. FERRUM does not authenticate anyone: the host authenticates the
//! person, passes their name, and must never expose [`record`] as a model
//! tool. FERRUM records the name it is given.
//!
//! 1. When the host shows an item it reads its [`revision`]: a SHA-256 over
//!    the study's source hash, the open volume of a multi-volume series and
//!    the item's content (an annotation's kind, plane, slice, points, text,
//!    value and name; a segment's name and voxels). The encoding is fixed
//!    (`ferrum-review-revision/1`, `docs/workspace-format.md`); review state
//!    and display properties (colour, opacity, visibility) are not part of
//!    it — the host's own hash of what it showed covers the view.
//! 2. When the person decides, the host passes the revision back, with the
//!    person's name and a SHA-256 of what it showed (view state and values,
//!    in the host's own canonical form).
//! 3. [`record`] refuses with `source_changed` if the item changed since it
//!    was shown, and with `forbidden` if someone has decided on it already.
//!    It appends the audit entry (both hashes, the source hash, the volume)
//!    and then writes the decision like the desktop review queue. A
//!    workspace lock serialises decisions of several hosts.
//!
//! The agent command `review confirm|reject` and its operator switch stay
//! as they are, for harnesses that only have the command line. The audit
//! log is an append-only file, not tamper-evident.

use std::path::Path;

use ferrum_domain::analysis::MaskRegion;
use ferrum_domain::{ReviewStatus, Timestamp};
use ferrum_io::annotations::annotation_report_json;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode};
use crate::study::{Study, VolumeCache};

/// Tag of the revision encoding; a change of the encoding changes it.
pub const REVISION_FORMAT: &str = "ferrum-review-revision/1";

/// File in the workspace whose OS lock serialises decisions.
const LOCK_FILE: &str = ".decisions.lock";

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
    /// When it was recorded (also the audit entry's time).
    pub time: Timestamp,
}

/// The current revision of `item` in the workspace at `workspace` (resolved
/// like the agent's workspaces). A host that shows many items passes a
/// [`VolumeCache`] so the study is not loaded again for each.
pub fn revision(
    config: &AgentConfig,
    workspace: &Path,
    item: Item,
    cache: Option<&mut VolumeCache>,
) -> Result<String, AgentError> {
    let study = Study::load(config, &config.resolve_workspace(workspace)?, cache)?;
    item_revision(config, &study, item)
}

/// Records `d` in the workspace at `workspace` and its audit log; see the
/// module documentation.
pub fn record(
    config: &AgentConfig,
    workspace: &Path,
    d: &Decision,
    cache: Option<&mut VolumeCache>,
) -> Result<Recorded, AgentError> {
    let person = check_person(d.person)?;
    let shown = check_hash("shown_sha256", d.shown_sha256)?;
    let revision = check_hash("revision", d.revision)?;
    let root = config.resolve_workspace(workspace)?;
    let _lock = lock(&root)?;
    let mut study = Study::load(config, &root, cache)?;
    if item_revision(config, &study, d.item)? != revision {
        return Err(AgentError::new(
            ErrorCode::SourceChanged,
            format!("{} changed since it was shown; nothing was recorded", d.item.describe()),
        )
        .hint("show the item again, read its new revision, and let the person decide on that"));
    }
    refuse_decided(&study, d.item)?;
    let status = match d.verdict {
        Verdict::Confirm => ReviewStatus::Confirmed,
        Verdict::Reject => ReviewStatus::Rejected,
    };
    let time = Timestamp::now();
    let (kind, id) = match d.item {
        Item::Annotation(id) => ("annotation", id),
        Item::Segment(label) => ("segment", u64::from(label)),
    };
    let command = if status == ReviewStatus::Confirmed { "review confirm" } else { "review reject" };
    let volume = study.open_volume.as_ref().and_then(|v| v.volume.as_ref()).map(|v| v.label.clone());
    let mut entry = json!({
        "time": time.to_rfc3339(),
        "command": command,
        "source": "host",
        "params": {
            kind: id, "by": person, "revision": revision, "shown_sha256": shown,
            "source_sha256": study.source_sha256, "volume": volume,
        },
        "ok": true,
    });
    // the audit entry first: a decision is never in the workspace without it
    study.workspace.append_audit(entry.clone())?;
    if let Err(e) = apply(config, &mut study, d.item, status, &person, time) {
        entry["ok"] = json!(false);
        entry["error"] = json!(e.message);
        let _ = study.workspace.append_audit(entry);
        return Err(e);
    }
    Ok(Recorded { item: d.item, status, person, revision, shown_sha256: shown, time })
}

/// Holds the workspace's decision lock (released when dropped).
fn lock(root: &Path) -> Result<std::fs::File, AgentError> {
    let path = root.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| AgentError::internal(format!("{}: {e}", path.display())))?;
    file.lock().map_err(|e| AgentError::internal(format!("{}: {e}", path.display())))?;
    Ok(file)
}

/// Someone has decided already: a second decision would overwrite theirs.
fn refuse_decided(study: &Study, item: Item) -> Result<(), AgentError> {
    let p = match item {
        Item::Annotation(id) => study.annotations.provenance(id).cloned(),
        Item::Segment(label) => study.segments.segment(label).map(|s| s.provenance.clone()),
    }
    .ok_or_else(|| AgentError::not_found(format!("unknown {}", item.describe())))?;
    if p.is_pending() {
        return Ok(());
    }
    let by = p.reviewed_by.as_deref().unwrap_or("someone");
    let when = p.reviewed.map(|t| format!(" at {}", t.to_rfc3339())).unwrap_or_default();
    Err(AgentError::new(
        ErrorCode::Forbidden,
        format!("{} is already {} by {by}{when}; nothing was recorded", item.describe(), p.status.as_str()),
    )
    .hint("a decision is not overwritten; the item must be proposed again (changed) before anyone decides on it"))
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

/// Feeds `ferrum-review-revision/1` fields: strings length-prefixed (u64 LE),
/// integers as u64 LE.
struct Encoder(Sha256);

impl Encoder {
    fn str(&mut self, s: &str) {
        self.int(s.len() as u64);
        self.0.update(s.as_bytes());
    }

    fn int(&mut self, n: u64) {
        self.0.update(n.to_le_bytes());
    }

    fn hex(self) -> String {
        self.0.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

fn item_revision(config: &AgentConfig, study: &Study, item: Item) -> Result<String, AgentError> {
    let mut h = Encoder(Sha256::new());
    h.str(REVISION_FORMAT);
    h.str(&study.source_sha256);
    let volume = study.open_volume.as_ref().map(|v| v.id.clone()).unwrap_or_default();
    h.str(&volume);
    match item {
        Item::Annotation(id) => {
            let report = annotation_report_json(&study.annotation_report(config), "");
            let a = report["annotations"]
                .as_array()
                .and_then(|a| a.iter().find(|a| a["id"] == json!(id)))
                .ok_or_else(|| AgentError::not_found(format!("unknown annotation {id}")))?;
            h.str("annotation");
            h.int(id);
            let text = |k: &str| a[k].as_str().unwrap_or_default().to_owned();
            for key in ["type", "plane", "name", "text", "unit"] {
                h.str(&text(key));
            }
            h.int(a["slice_index"].as_u64().unwrap_or_default());
            h.str(&a["value"].as_f64().map_or_else(String::new, |v| format!("{v:.3}")));
            let points = a["points_voxel"].as_array().cloned().unwrap_or_default();
            h.int(points.len() as u64);
            for p in &points {
                let c = |i: usize| p[i].as_f64().unwrap_or_default();
                h.str(&format!("{:.3},{:.3},{:.3}", c(0), c(1), c(2)));
            }
        }
        Item::Segment(label) => {
            let segment = study
                .segments
                .segment(label)
                .ok_or_else(|| AgentError::not_found(format!("unknown segment {label}")))?;
            h.str("segment");
            h.int(u64::from(label));
            h.str(&segment.name);
            match MaskRegion::of_label(study.segments.labels(), label) {
                Some(r) => {
                    for c in r.bx.min.to_array().into_iter().chain(r.bx.max.to_array()) {
                        h.int(u64::from(c));
                    }
                    h.0.update(r.to_u8());
                }
                None => h.str("empty"),
            }
        }
    }
    Ok(h.hex())
}

fn check_person(person: &str) -> Result<String, AgentError> {
    let p = person.trim();
    // control and invisible formatting characters could hide or reorder a name in an audit viewer
    let hidden = |c: char| {
        c.is_control()
            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
    };
    if p.is_empty() || p.chars().count() > 200 || p.chars().any(hidden) {
        return Err(AgentError::bad_request(
            "person must name who decided (1 to 200 characters, no control or formatting characters)",
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

#[cfg(test)]
mod tests {
    use super::*;

    // covers 16.7-a
    #[test]
    fn the_revision_encoding_is_pinned() {
        // the same fields as a segment's revision; the expected value was computed independently
        // (Python: sha256 over u64-LE length-prefixed strings and u64-LE integers)
        let mut h = Encoder(Sha256::new());
        h.str(REVISION_FORMAT);
        h.str(&"ab".repeat(32));
        h.str("");
        h.str("segment");
        h.int(3);
        h.str("liver");
        for c in 1..=6 {
            h.int(c);
        }
        h.0.update([0u8, 1, 1, 0]);
        assert_eq!(
            h.hex(),
            "58f82d6d4e2c8591a39e847e6fc39fceca33d03072e3dc086dc8f00a8dfea80f",
            "changing the encoding needs a new REVISION_FORMAT"
        );
    }
}
