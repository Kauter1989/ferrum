//! JSON form of [`Provenance`], shared by annotation and segment documents.
//!
//! ```json
//! { "author": { "kind": "engine", "name": "nnInteractive", "version": "2.6", "research_only": true },
//!   "status": "proposed", "created": "2026-10-01T12:00:00Z", "reviewed_by": null, "reviewed": null }
//! ```

use ferrum_domain::{Author, Provenance, ReviewStatus, Timestamp};
use serde_json::{json, Value};

/// JSON object of `p`.
pub fn provenance_json(p: &Provenance) -> Value {
    let mut v = json!({
        "author": author_json(&p.author),
        "status": p.status.as_str(),
        "created": p.created.map(Timestamp::to_rfc3339),
        "reviewed_by": p.reviewed_by,
        "reviewed": p.reviewed.map(Timestamp::to_rfc3339),
    });
    if let Some(by) = &p.requested_by {
        v["requested_by"] = author_json(by);
    }
    v
}

fn author_json(a: &Author) -> Value {
    match a {
        Author::Human => json!({ "kind": "human" }),
        Author::Agent { id } => json!({ "kind": "agent", "id": id }),
        Author::Engine { name, version, research_only } => {
            json!({ "kind": "engine", "name": name, "version": version, "research_only": research_only })
        }
    }
}

fn author_from_json(a: &Value) -> Result<Author, String> {
    let text = |key: &str| a.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(match a.get("kind").and_then(Value::as_str) {
        None | Some("human") => Author::Human,
        Some("agent") => Author::Agent { id: text("id") },
        Some("engine") => Author::Engine {
            name: text("name").ok_or("provenance.author.name is missing")?,
            version: text("version").unwrap_or_default(),
            research_only: a.get("research_only").and_then(Value::as_bool).unwrap_or(false),
        },
        Some(other) => return Err(format!("unknown provenance author kind {other:?}")),
    })
}

/// Parses [`provenance_json`] output. A missing or `null` value is the
/// default (drawn by a person, confirmed), which is what documents written
/// before provenance existed describe.
pub fn provenance_from_json(v: &Value) -> Result<Provenance, String> {
    if v.is_null() {
        return Ok(Provenance::default());
    }
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
    let time = |key: &str| match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(t) => t
            .as_str()
            .and_then(Timestamp::parse_rfc3339)
            .map(Some)
            .ok_or_else(|| format!("provenance.{key}: expected an RFC 3339 time, got {t}")),
    };
    let author = author_from_json(v.get("author").unwrap_or(&Value::Null))?;
    let requested_by = match v.get("requested_by") {
        None | Some(Value::Null) => None,
        Some(a) => Some(author_from_json(a)?),
    };
    let status = match v.get("status").and_then(Value::as_str) {
        None => ReviewStatus::default(),
        Some(s) => ReviewStatus::parse(s).ok_or_else(|| format!("unknown review status {s:?}"))?,
    };
    Ok(Provenance {
        author,
        status,
        created: time("created")?,
        reviewed_by: text(v, "reviewed_by"),
        reviewed: time("reviewed")?,
        requested_by,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_author() {
        let mut reviewed = Provenance::engine("nnInteractive", "2.6", true, Timestamp(1_790_858_096));
        reviewed.review(ReviewStatus::Confirmed, Some("dr.k"), Timestamp(1_790_858_100));
        for p in [
            Provenance::default(),
            Provenance::agent(Some("run-1".into()), Timestamp(5)),
            Provenance::agent(None, Timestamp(5)),
            reviewed,
            Provenance::engine("nnInteractive", "2.6", true, Timestamp(9))
                .requested_by(Author::Agent { id: Some("run-2".into()) }),
        ] {
            assert_eq!(provenance_from_json(&provenance_json(&p)), Ok(p));
        }
        assert!(provenance_json(&Provenance::default()).get("requested_by").is_none(), "written only when set");
        let v = provenance_json(&Provenance::engine("E", "1", true, Timestamp(0)));
        assert_eq!(v["author"]["kind"], "engine");
        assert_eq!(v["status"], "proposed");
        assert_eq!(v["created"], "1970-01-01T00:00:00Z");
    }

    #[test]
    fn defaults_and_errors() {
        assert_eq!(provenance_from_json(&Value::Null), Ok(Provenance::default()));
        assert_eq!(provenance_from_json(&json!({})), Ok(Provenance::default()));
        for bad in [
            json!({ "author": { "kind": "robot" } }),
            json!({ "author": { "kind": "engine" } }),
            json!({ "status": "maybe" }),
            json!({ "created": "yesterday" }),
            json!({ "reviewed": 5 }),
            json!({ "requested_by": { "kind": "robot" } }),
        ] {
            assert!(provenance_from_json(&bad).is_err(), "{bad}");
        }
    }
}
