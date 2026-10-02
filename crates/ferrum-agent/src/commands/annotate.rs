//! `annotate add|list|rename|delete`: named annotations, saved in the
//! workspace as `ferrum-annotations` v2 with agent provenance.

use ferrum_domain::{Annotation, AnnotationReport, Author, Provenance, SliceKey, Timestamp};
use ferrum_io::provenance::provenance_json;
use glam::Vec3;
use serde_json::{json, Value};

use super::inspect::points;
use super::Ctx;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::points::{describe, parse_plane, voxel_to_plane_mm};
use crate::study::Study;

/// Provenance of things an agent creates: proposed, with the agent id the
/// harness passed (`agent`), if any.
pub fn agent_provenance(p: &Params) -> Result<Provenance, AgentError> {
    Ok(Provenance::agent(p.str("agent")?.map(str::to_owned), Timestamp::now()))
}

/// `annotate add`: kind, plane, points (one slice), optional name and text.
pub fn add(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let config = ctx.config;
    let provenance = agent_provenance(p)?;
    let study = ctx.study(p)?;
    let plane = parse_plane(p.req_str("plane")?)?;
    let pts = points(study, p, "points")?;
    let axis = plane.normal_axis();
    let index = pts.first().map(|q| q[axis].round()).unwrap_or(0.0);
    if pts.iter().any(|q| q[axis].round() != index) {
        return Err(AgentError::bad_request(format!(
            "the points lie on different {} slices",
            plane.label().to_lowercase()
        ))
        .hint("an annotation lies on one slice: give points with the same slice coordinate"));
    }
    let mm: Vec<_> = pts.iter().map(|q| voxel_to_plane_mm(&study.volume, plane, *q)).collect();
    let n = mm.len();
    let need = |k: usize, what: &str| {
        if n == k {
            Ok(())
        } else {
            Err(AgentError::bad_request(format!("{what} needs {k} points, got {n}")))
        }
    };
    let kind = p.req_str("kind")?;
    let annotation = match kind {
        "distance" => need(2, "a distance").map(|()| Annotation::Distance { a: mm[0], b: mm[1] })?,
        "rectangle" => need(2, "a rectangle (opposite corners)").map(|()| Annotation::Rect { a: mm[0], b: mm[1] })?,
        "angle" => need(3, "an angle").map(|()| Annotation::Angle { a: mm[0], vertex: mm[1], b: mm[2] })?,
        "area" if n >= 3 => Annotation::Polygon { points: mm },
        "area" => return Err(AgentError::bad_request(format!("an area needs at least 3 points, got {n}"))),
        "text" => {
            need(1, "a text")?;
            Annotation::Text { pos: mm[0], text: p.req_str("text")?.to_owned() }
        }
        other => {
            return Err(AgentError::bad_request(format!("unknown kind {other:?}"))
                .hint("kinds are distance, angle, area, rectangle and text"))
        }
    };
    let id = study.annotations.add_with(SliceKey::new(plane, index as u32), annotation, provenance);
    if let Some(name) = p.str("name")? {
        study.annotations.rename(id, name);
    }
    study.save_annotations(config)?;
    Ok(Output::new(json!({ "annotation": entry(study, id) })))
}

/// Records of all annotations (points as voxel coordinates).
fn report(study: &Study) -> AnnotationReport {
    AnnotationReport::build(std::path::PathBuf::new(), Default::default(), &study.volume, &study.annotations)
}

/// One annotation as JSON (`null` for an unknown id).
fn entry(study: &Study, id: u64) -> Value {
    report(study).annotations.iter().find(|a| a.id == id).map_or(Value::Null, |a| record(study, a))
}

fn record(study: &Study, a: &ferrum_domain::AnnotationRecord) -> Value {
    let r = |x: f32| (f64::from(x) * 1000.0).round() / 1000.0;
    json!({
        "id": a.id,
        "name": a.name,
        "type": a.kind,
        "plane": a.plane.label().to_lowercase(),
        "slice_number": a.slice_index + 1,
        "value": a.value.map(r),
        "unit": a.unit,
        "text": a.text,
        "points": a.points_voxel.iter().map(|v: &Vec3| describe(&study.volume, v.as_dvec3())).collect::<Vec<_>>(),
        "provenance": provenance_json(&a.provenance),
    })
}

/// `annotate list`: every annotation with its provenance.
pub fn list(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let items: Vec<Value> = report(study).annotations.iter().map(|a| record(study, a)).collect();
    Ok(Output::new(json!({ "annotations": items })))
}

/// Checks that annotation `id` exists and was created by an agent: agents
/// never change what a person drew or an engine proposed.
fn own_annotation(study: &Study, id: u64) -> Result<(), AgentError> {
    match study.annotations.provenance(id) {
        None => Err(AgentError::not_found(format!("unknown annotation {id}")).hint("see annotate list")),
        Some(pv) if !matches!(pv.author, Author::Agent { .. }) => Err(AgentError::new(
            ErrorCode::Forbidden,
            format!(
                "annotation {id} was created by a {}; agents may change only their own annotations",
                pv.author.kind()
            ),
        )),
        Some(_) => Ok(()),
    }
}

fn id_param(p: &Params) -> Result<u64, AgentError> {
    p.u64("id")?.ok_or_else(|| AgentError::bad_request("id is required"))
}

/// `annotate rename`: renames an annotation the agent created.
pub fn rename(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let config = ctx.config;
    let study = ctx.study(p)?;
    let id = id_param(p)?;
    own_annotation(study, id)?;
    if !study.annotations.rename(id, p.req_str("name")?) {
        return Err(AgentError::bad_request("the name must not be empty"));
    }
    study.save_annotations(config)?;
    Ok(Output::new(json!({ "annotation": entry(study, id) })))
}

/// `annotate delete`: deletes an annotation the agent created.
pub fn delete(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let config = ctx.config;
    let study = ctx.study(p)?;
    let id = id_param(p)?;
    own_annotation(study, id)?;
    study.annotations.remove(id);
    study.save_annotations(config)?;
    Ok(Output::new(json!({ "deleted": id })))
}
