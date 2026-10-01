//! `review list|confirm|reject`: the items waiting for a person.
//!
//! Confirming is a clinical decision. A harness may confirm or reject only
//! if the operator allows it (`review.allow_harness_confirmation`) and must
//! name the person (`by`); the decision goes to the audit log. Otherwise a
//! person reviews in the desktop app.

use ferrum_domain::ReviewStatus;
use ferrum_io::provenance::provenance_json;
use serde_json::{json, Value};

use super::Ctx;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;

/// `review list`: proposed annotations and segments.
pub fn list(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let annotations: Vec<Value> = study
        .annotations
        .iter()
        .filter_map(|(id, key, a, name)| {
            let pv = study.annotations.provenance(id)?;
            pv.is_pending().then(|| {
                json!({
                    "id": id, "name": name, "type": a.kind(), "label": a.label(),
                    "plane": key.slice_axis().map(|s| s.label().to_lowercase()), "slice_number": key.index + 1,
                    "provenance": provenance_json(pv),
                })
            })
        })
        .collect();
    let segments: Vec<Value> = study
        .segments
        .segments()
        .iter()
        .filter(|s| s.provenance.is_pending())
        .map(|s| json!({ "label": s.label, "name": s.name, "provenance": provenance_json(&s.provenance) }))
        .collect();
    let mut out = Output::new(json!({ "annotations": annotations, "segments": segments }));
    if !nothing_pending(&out.data) {
        out = out.warn("proposed items are not findings until a person confirms them");
    }
    Ok(out)
}

fn nothing_pending(d: &Value) -> bool {
    d["annotations"].as_array().is_none_or(Vec::is_empty) && d["segments"].as_array().is_none_or(Vec::is_empty)
}

fn decide(ctx: &mut Ctx, p: &Params, status: ReviewStatus) -> Result<Output, AgentError> {
    if !ctx.config.allow_harness_confirmation {
        return Err(AgentError::new(ErrorCode::Forbidden, "the operator does not allow harnesses to review results")
            .hint("a person reviews proposals in the FERRUM desktop app"));
    }
    let by = p.req_str("by")?.trim().to_owned();
    if by.is_empty() {
        return Err(AgentError::bad_request("by must name the person who decided"));
    }
    let study = ctx.study(p)?;
    let now = ferrum_domain::Timestamp::now();
    match (p.u64("annotation")?, p.u64("segment")?) {
        (Some(id), None) => {
            if !study.annotations.review(id, status, Some(&by), now) {
                return Err(AgentError::not_found(format!("unknown annotation {id}")));
            }
            study.save_annotations()?;
            Ok(Output::new(json!({ "annotation": id, "status": status.as_str(), "by": by })))
        }
        (None, Some(label)) => {
            let label = u8::try_from(label).map_err(|_| AgentError::not_found(format!("unknown segment {label}")))?;
            study
                .segments
                .review(label, status, Some(&by), now)
                .map_err(|_| AgentError::not_found(format!("unknown segment {label}")))?;
            study.save_segments()?;
            Ok(Output::new(json!({ "segment": label, "status": status.as_str(), "by": by })))
        }
        _ => Err(AgentError::bad_request("give annotation or segment")),
    }
}

/// `review confirm`: see the module documentation.
pub fn confirm(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    decide(ctx, p, ReviewStatus::Confirmed)
}

/// `review reject`: see the module documentation.
pub fn reject(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    decide(ctx, p, ReviewStatus::Rejected)
}
