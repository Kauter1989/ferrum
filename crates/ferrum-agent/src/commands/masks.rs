//! `segment shape|distance|components|compare|edit`: measurements and deterministic
//! edits of segments, computed from the label map (never from images),
//! and the quality checks every engine result carries (`checks`).

use ferrum_domain::analysis::{self, MaskRegion};
use ferrum_domain::{Author, Provenance, SegmentationSet, Timestamp, VoxelBox};
use glam::{DVec3, UVec3};
use serde_json::{json, Value};

use super::inspect::point;
use super::segment::{changeable_segment, segment_json};
use super::Ctx;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::points::{describe, nearest_voxel, voxel_to_patient};
use crate::study::Study;
use ferrum_radiomics::ValueUnit;

fn round(x: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (x * f).round() / f
}

/// The label in parameter `key`, which must name a segment.
pub fn segment_label(study: &Study, p: &Params, key: &str) -> Result<u8, AgentError> {
    let label = p.u64(key)?.ok_or_else(|| AgentError::bad_request(format!("{key} is required")))?;
    u8::try_from(label)
        .ok()
        .filter(|l| study.segments.segment(*l).is_some())
        .ok_or_else(|| AgentError::not_found(format!("unknown segment {label}")).hint("see segment list"))
}

/// Volume of one voxel in ml.
pub fn voxel_ml(study: &Study) -> f64 {
    let s = study.volume.spacing();
    f64::from(s.x) * f64::from(s.y) * f64::from(s.z) / 1000.0
}

/// The voxels of segment `label`, cropped to their box.
pub fn region(set: &SegmentationSet, label: u8) -> Option<MaskRegion> {
    MaskRegion::of_label(set.labels(), label)
}

fn box_json(bx: VoxelBox) -> Value {
    json!({ "min": bx.min.to_array(), "max": bx.max.to_array() })
}

/// A box parameter (two corner points, inclusive) as a half-open box.
pub fn box_param(study: &Study, v: &Value) -> Result<VoxelBox, AgentError> {
    let bp = Params::new(v)?;
    let vol = &study.volume;
    let (a, b) = (nearest_voxel(vol, point(study, &bp, "min")?), nearest_voxel(vol, point(study, &bp, "max")?));
    let lo = UVec3::new(a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2]));
    let hi = UVec3::new(a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])) + UVec3::ONE;
    Ok(VoxelBox::new(lo, hi))
}

fn diameter_json(study: &Study, d: &analysis::Diameter) -> Value {
    json!({
        "mm": round(d.mm, 2),
        "from": describe(&study.volume, d.from.as_dvec3()),
        "to": describe(&study.volume, d.to.as_dvec3()),
    })
}

fn slices_json(s: &analysis::Shape) -> Value {
    let range = |lo: u32, hi: u32, l: analysis::LargestSlice| json!({ "from": lo + 1, "to": hi, "largest": l.index + 1, "largest_voxels": l.voxels });
    let (b, l) = (s.bounds, s.largest_slices);
    json!({
        "axial": range(b.min.z, b.max.z, l[0]),
        "coronal": range(b.min.y, b.max.y, l[1]),
        "sagittal": range(b.min.x, b.max.x, l[2]),
    })
}

/// Shape measurements of a segment as JSON.
pub fn shape_json(study: &Study, label: u8, r: &MaskRegion) -> Value {
    let sp = study.volume.spacing();
    let s = analysis::shape(r, study.volume.dims(), sp);
    let in_plane = f64::from(sp.x.max(sp.y));
    let mut data = json!({
        "voxels": s.voxels,
        "volume_ml": round(s.voxels as f64 * voxel_ml(study), 3),
        "bounds": box_json(s.bounds),
        "centroid": describe(&study.volume, s.centroid),
        "extent_mm": s.extent_mm.map(|x| round(x, 2)),
        "slices": slices_json(&s),
        "long_axis": s.long_axis.as_ref().map(|d| diameter_json(study, d)),
        "short_axis": s.short_axis.as_ref().map(|d| diameter_json(study, d)),
        "axes_slice_number": s.largest_slices[0].index + 1,
        "uncertainty_mm": round(in_plane, 3),
        "touches_border": s.touches_border,
        "components": s.components,
        "largest_component_share": round(s.largest_component_share, 4),
    });
    if let Some(l) = laterality(study, label, r) {
        data["laterality"] = l;
    }
    data
}

/// `segment shape`: volume, extent, slice ranges, axial long and short
/// axis, border contact, components and laterality of a segment; with
/// `solid_min`, the same axes of its voxels at or above that value (the
/// solid part of a part-solid lesion, as a measurement).
pub fn shape(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let solid_min = p.f64("solid_min")?;
    let study = ctx.study(p)?;
    let label = segment_label(study, p, "segment")?;
    let r = region(&study.segments, label)
        .ok_or_else(|| AgentError::bad_request(format!("segment {label} has no voxels")))?;
    let mut data = json!({
        "segment": segment_json(study, label),
        "shape": shape_json(study, label, &r),
        "method": "from the label map: extents are voxel counts × spacing; the long axis is the longest distance between \
                   voxel centres on the largest axial slice, the short axis the longest extent perpendicular to it; \
                   uncertainty ± one in-plane voxel spacing",
    });
    let mut warnings = Vec::new();
    if let Some(t) = solid_min {
        if !t.is_finite() {
            return Err(AgentError::bad_request("solid_min must be a number"));
        }
        data["solid"] = solid_json(study, &r, t);
        if let ValueUnit::Unknown { modality } = ValueUnit::from_modality(&study.metadata.modality) {
            warnings.push(format!(
                "the voxel values are not known to be HU (modality {}): solid_min was compared with them as stored",
                if modality.is_empty() { "not declared" } else { modality.as_str() }
            ));
        }
    }
    let mut out = Output::new(data);
    for w in warnings {
        out = out.warn(w);
    }
    Ok(out)
}

/// The part of `r` whose values are at least `min`, with its axes.
fn solid_json(study: &Study, r: &MaskRegion, min: f64) -> Value {
    let vol = &study.volume;
    let size = r.size();
    let mut mask = vec![0u8; r.bx.voxel_count()];
    for (n, m) in mask.iter_mut().enumerate() {
        let (x, y, z) = (n as u32 % size.x, (n as u32 / size.x) % size.y, n as u32 / (size.x * size.y));
        let g = r.bx.min + UVec3::new(x, y, z);
        let value = vol.physical(g.x, g.y, g.z).map_or(f64::NEG_INFINITY, f64::from);
        *m = u8::from(r.contains(g) && value >= min);
    }
    let method = "voxels of the segment whose value is at least min_value, a threshold the caller chose; their axial \
                  long and short axis on their own largest axial slice, as for the whole segment. A measurement, not a \
                  classification";
    let total = r.count();
    let Some(solid) = MaskRegion::from_box_mask(r.bx, &mask) else {
        return json!({ "min_value": min, "voxels": 0, "volume_ml": 0.0, "share": 0.0, "long_axis": null, "short_axis": null, "method": method });
    };
    let s = analysis::shape(&solid, vol.dims(), vol.spacing());
    json!({
        "min_value": min,
        "voxels": s.voxels,
        "volume_ml": round(s.voxels as f64 * voxel_ml(study), 3),
        "share": round(s.voxels as f64 / total as f64, 4),
        "long_axis": s.long_axis.as_ref().map(|d| diameter_json(study, d)),
        "short_axis": s.short_axis.as_ref().map(|d| diameter_json(study, d)),
        "axes_slice_number": s.largest_slices[0].index + 1,
        "method": method,
    })
}

/// `segment distance`: the smallest distance from a segment to the surface
/// of another, with the two closest voxels.
pub fn distance(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let a = segment_label(study, p, "segment")?;
    let b = segment_label(study, p, "to")?;
    if a == b {
        return Err(AgentError::bad_request("segment and to name the same segment"));
    }
    let empty = |l: u8| AgentError::bad_request(format!("segment {l} has no voxels"));
    let ra = region(&study.segments, a).ok_or_else(|| empty(a))?;
    let rb = region(&study.segments, b).ok_or_else(|| empty(b))?;
    let sp = study.volume.spacing();
    let near = analysis::distance_to_surface(&ra, &rb, sp)
        .ok_or_else(|| AgentError::internal(format!("segment {b} has no surface")))?;
    let overlap = (0..ra.bx.voxel_count())
        .map(|n| {
            let s = ra.size();
            ra.bx.min + UVec3::new(n as u32 % s.x, (n as u32 / s.x) % s.y, n as u32 / (s.x * s.y))
        })
        .filter(|g| ra.contains(*g) && rb.contains(*g))
        .count() as u64;
    let vol = &study.volume;
    Ok(Output::new(json!({
        "segment": segment_json(study, a),
        "to": segment_json(study, b),
        "distance": {
            "mm": round(near.mm, 2),
            "from": describe(vol, near.from.as_dvec3()),
            "to": describe(vol, near.to.as_dvec3()),
            "uncertainty_mm": round(f64::from(sp.max_element()), 3),
        },
        "overlap_voxels": overlap,
        "inside": overlap == ra.count(),
        "method": "from the label maps: the shortest distance between the centre of a voxel of segment and the centre \
                   of a surface voxel of to (a voxel of to with a face neighbour outside it), exact (Euclidean distance \
                   transform with the voxel spacing); 0 mm when they touch; uncertainty ± one voxel spacing (the \
                   largest)",
    })))
}

fn component_json(study: &Study, c: &analysis::Component, ml: f64) -> Value {
    json!({
        "voxels": c.voxels,
        "volume_ml": round(c.voxels as f64 * ml, 3),
        "bounds": box_json(c.bounds),
        "centroid": describe(&study.volume, c.centroid),
    })
}

/// `segment components`: 26-connected components of a segment, largest
/// first; with `split`, each further component of at least `min_ml`
/// becomes a new segment (the largest stays in the segment).
pub fn components(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let split = p.bool("split")?.unwrap_or(false);
    let study = ctx.study(p)?;
    let label = segment_label(study, p, "segment")?;
    let ml = voxel_ml(study);
    let min_voxels = (p.f64("min_ml")?.unwrap_or(0.0).max(0.0) / ml).ceil().max(1.0) as u64;
    let r = region(&study.segments, label)
        .ok_or_else(|| AgentError::bad_request(format!("segment {label} has no voxels")))?;
    let parts = analysis::split_components(&r, 1);
    let list: Vec<Value> = parts.iter().map(|(c, _)| component_json(study, c, ml)).collect();
    let mut data = json!({ "segment": segment_json(study, label), "components": list });
    if split {
        changeable_segment(study, label)?;
        let made = split_into_segments(study, label, &parts, min_voxels)?;
        study.save_segments()?;
        data["segment"] = segment_json(study, label);
        data["new_segments"] = json!(made.iter().map(|l| segment_json(study, *l)).collect::<Vec<_>>());
    }
    Ok(Output::new(data))
}

fn split_into_segments(
    study: &mut Study,
    label: u8,
    parts: &[(analysis::Component, MaskRegion)],
    min_voxels: u64,
) -> Result<Vec<u8>, AgentError> {
    let src = study.segments.segment(label).cloned().ok_or_else(|| AgentError::internal("segment vanished"))?;
    let mut made = Vec::new();
    for (n, (c, mask)) in parts.iter().enumerate().skip(1) {
        if c.voxels < min_voxels {
            continue;
        }
        let set = &mut study.segments;
        let new = set
            .add_segment(&format!("{} #{}", src.name, n + 1))
            .map_err(|e| AgentError::new(ErrorCode::Limit, e.to_string()))?;
        let provenance = Provenance { created: Some(Timestamp::now()), ..src.provenance.clone() };
        set.set_provenance(new, provenance).map_err(|e| AgentError::internal(e.to_string()))?;
        set.apply_mask(new, mask.bx, &mask.to_u8(), true).map_err(|e| AgentError::internal(e.to_string()))?;
        made.push(new);
    }
    Ok(made)
}

/// The segment `label` of another workspace on the same series.
fn other_region(ctx: &Ctx, study: &Study, ws: &str, label: u64) -> Result<Option<MaskRegion>, AgentError> {
    let root = ctx.config.resolve_workspace(std::path::Path::new(ws))?;
    if !root.join(ferrum_io::workspace::files::MANIFEST).exists() {
        return Err(AgentError::new(ErrorCode::NoStudy, format!("{} is not a workspace", root.display())));
    }
    let other = ferrum_io::Workspace::open(&root)?;
    if other.manifest().source.files != study.workspace.manifest().source.files {
        return Err(AgentError::bad_request("the other workspace holds another series")
            .hint("compare segments of workspaces opened on the same series"));
    }
    let set = other.load_segments(&study.volume)?.unwrap_or_else(|| SegmentationSet::new(study.volume.dims()));
    let label = u8::try_from(label).ok().filter(|l| set.segment(*l).is_some());
    let label = label.ok_or_else(|| AgentError::not_found("unknown segment in the other workspace"))?;
    Ok(region(&set, label))
}

/// Agreement of two masks as JSON.
pub fn agreement_json(study: &Study, a: Option<&MaskRegion>, b: Option<&MaskRegion>) -> Value {
    let g = analysis::compare(a, b, study.volume.spacing());
    let ml = voxel_ml(study);
    let opt = |x: Option<f64>| x.map(|v| round(v, 2));
    json!({
        "volume_ml_a": round(g.voxels_a as f64 * ml, 3),
        "volume_ml_b": round(g.voxels_b as f64 * ml, 3),
        "volume_difference_ml": round((g.voxels_b as f64 - g.voxels_a as f64) * ml, 3),
        "overlap_ml": round(g.intersection as f64 * ml, 3),
        "dice": round(g.dice, 4),
        "jaccard": round(g.jaccard, 4),
        "hd95_mm": opt(g.hd95_mm),
        "hausdorff_mm": opt(g.hausdorff_mm),
        "centroid_distance_mm": opt(g.centroid_distance_mm),
    })
}

/// `segment compare`: Dice, Jaccard, volume difference, Hausdorff distances
/// and centroid distance of two segments (the second may be in another
/// workspace of the same series).
pub fn compare(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    ctx.study(p)?;
    let study = ctx.study.as_ref().ok_or_else(|| AgentError::internal("study not loaded"))?;
    let a = segment_label(study, p, "a")?;
    let ra = region(&study.segments, a);
    let rb = match p.str("b_workspace")? {
        Some(ws) => other_region(ctx, study, ws, p.u64("b")?.ok_or_else(|| AgentError::bad_request("b is required"))?)?,
        None => region(&study.segments, segment_label(study, p, "b")?),
    };
    Ok(Output::new(json!({
        "a": segment_json(study, a),
        "b": p.u64("b")?,
        "agreement": agreement_json(study, ra.as_ref(), rb.as_ref()),
        "method": "voxel overlap on the volume grid; surface distances between boundary voxel centres (exact distance \
                   transform), HD95 is the larger of the two directed 95th percentiles",
    })))
}

/// `segment edit`: deterministic clean-up of a segment the agent made or
/// asked an engine for.
pub fn edit(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let label = segment_label(study, p, "segment")?;
    changeable_segment(study, label)?;
    let op = p.req_str("op")?;
    let before = study.segments.voxel_count(label);
    let r = region(&study.segments, label)
        .ok_or_else(|| AgentError::bad_request(format!("segment {label} has no voxels")))?;
    let edited = match op {
        "keep_largest" => analysis::keep_largest(&r),
        "fill_holes" => {
            let labels = study.segments.labels();
            analysis::fill_holes(&r, |g| labels.label(g.x, g.y, g.z) == Some(0))
        }
        "restrict_to_box" => {
            let bx = box_param(study, p.req("box")?)?;
            r.restricted(bx).map_or_else(|| MaskRegion::empty(r.bx), |m| m.expanded(r.bx))
        }
        "remove_small" => {
            let min = (p.f64("min_ml")?.unwrap_or(0.0) / voxel_ml(study)).ceil() as u64;
            let mut out = MaskRegion::empty(r.bx);
            for (_, m) in analysis::split_components(&r, min.max(1)) {
                out = union(&out, &m.expanded(r.bx));
            }
            out
        }
        other => return Err(AgentError::bad_request(format!("unknown op {other:?}"))),
    };
    let set = &mut study.segments;
    set.apply_mask(label, edited.bx, &edited.to_u8(), false).map_err(|e| AgentError::internal(e.to_string()))?;
    study.save_segments()?;
    let ml = voxel_ml(study);
    Ok(Output::new(json!({
        "segment": segment_json(study, label),
        "op": op,
        "volume_ml_before": round(before as f64 * ml, 3),
        "volume_ml_after": round(study.segments.voxel_count(label) as f64 * ml, 3),
    })))
}

fn union(a: &MaskRegion, b: &MaskRegion) -> MaskRegion {
    MaskRegion { bx: a.bx, inside: a.inside.iter().zip(&b.inside).map(|(x, y)| *x || *y).collect() }
}

/// Side of a segment's name: `Some(true)` for left, `Some(false)` for right.
fn named_side(name: &str) -> Option<bool> {
    let words: Vec<String> = name.split(|c: char| !c.is_ascii_alphanumeric()).map(str::to_ascii_lowercase).collect();
    match (words.iter().any(|w| w == "left"), words.iter().any(|w| w == "right")) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Midline reference (patient x in mm) and what it is: the centroid of a
/// midline structure among the segments, else the centre of the volume.
fn midline(study: &Study) -> (f64, String) {
    const MIDLINE: [&str; 6] = ["spinal_cord", "spinal cord", "vertebra", "sacrum", "sternum", "spine"];
    for s in study.segments.segments() {
        let n = s.name.to_ascii_lowercase();
        if MIDLINE.iter().any(|m| n.contains(m)) && named_side(&s.name).is_none() {
            if let Some(r) = region(&study.segments, s.label) {
                return (voxel_to_patient(&study.volume, r.centroid()).x, format!("centroid of segment {}", s.name));
            }
        }
    }
    let centre = (study.volume.dims().as_uvec3().as_dvec3() - DVec3::ONE) / 2.0;
    (voxel_to_patient(&study.volume, centre).x, "centre of the volume".into())
}

/// Laterality check for segments named left or right: in LPS, patient left
/// is `+x`.
fn laterality(study: &Study, label: u8, r: &MaskRegion) -> Option<Value> {
    let side = named_side(&study.segments.segment(label)?.name)?;
    let x = voxel_to_patient(&study.volume, r.centroid()).x;
    let (mid, reference) = midline(study);
    let observed_left = x > mid;
    Some(json!({
        "named": if side { "left" } else { "right" },
        "observed": if observed_left { "left" } else { "right" },
        "consistent": observed_left == side,
        "centroid_x_mm": round(x, 1),
        "midline_x_mm": round(mid, 1),
        "reference": reference,
    }))
}

/// What the engine command knows beyond the label map.
#[derive(Debug, Clone, Default)]
pub struct CheckInput {
    /// Voxels the engine marked that another segment owns (kept there).
    pub overlap_voxels: u64,
    /// The segment's voxels before this command (refinement).
    pub previous: Option<Option<MaskRegion>>,
    /// Smallest plausible volume (ml).
    pub min_ml: Option<f64>,
    /// Largest plausible volume (ml).
    pub max_ml: Option<f64>,
    /// The engine is for research use only.
    pub research_only: bool,
    /// Region uploaded to an interactive engine: an object touching one
    /// of its inner faces may be cut off.
    pub roi: Option<VoxelBox>,
}

/// `true` if `r` reaches a face of `roi` that is not a face of the grid.
fn touches_roi(r: &MaskRegion, roi: VoxelBox, dims: UVec3) -> bool {
    (0..3).any(|a| (roi.min[a] > 0 && r.bx.min[a] == roi.min[a]) || (roi.max[a] < dims[a] && r.bx.max[a] == roi.max[a]))
}

/// Agreement of successive revisions below which a refinement has not
/// converged.
pub const STABLE_DICE: f64 = 0.95;
/// Smallest share of the largest component for a single-object segment.
pub const LARGEST_SHARE: f64 = 0.9;

/// Quality checks of segment `label` (`docs/agent-segmentation.md` §4) and
/// a warning for every failed one.
pub fn checks(study: &Study, label: u8, input: &CheckInput) -> (Value, Vec<String>) {
    let name = study.segments.segment(label).map(|s| s.name.clone()).unwrap_or_default();
    let mut failed: Vec<(&str, String)> = Vec::new();
    let mut out = json!({ "research_only": input.research_only, "overlap_voxels": input.overlap_voxels });
    let Some(r) = region(&study.segments, label) else {
        out["empty"] = json!(true);
        out["failed"] = json!(["empty"]);
        return (out, vec![format!("{name}: the engine found nothing")]);
    };
    out["empty"] = json!(false);
    let ml = r.count() as f64 * voxel_ml(study);
    let s = analysis::shape(&r, study.volume.dims(), study.volume.spacing());
    out["volume_ml"] = json!(round(ml, 3));
    out["components"] = json!(s.components);
    out["largest_component_share"] = json!(round(s.largest_component_share, 4));
    out["touches_border"] = json!(s.touches_border);
    if s.largest_component_share < LARGEST_SHARE {
        failed.push((
            "components",
            format!(
                "{name}: {} components, the largest holds {:.0} %",
                s.components,
                s.largest_component_share * 100.0
            ),
        ));
    }
    if input.roi.is_some_and(|roi| touches_roi(&r, roi, study.volume.dims().as_uvec3())) {
        failed.push(("roi", format!("{name}: reaches the edge of the region sent to the engine; it may be cut off (give a larger roi or whole_volume)")));
    }
    if s.touches_border {
        failed.push((
            "border",
            format!("{name}: touches the edge of the volume; it may be cut off (volume is a lower bound)"),
        ));
    }
    if input.min_ml.is_some_and(|m| ml < m) || input.max_ml.is_some_and(|m| ml > m) {
        failed.push(("size", format!("{name}: {ml:.3} ml is outside the expected range")));
    }
    if input.overlap_voxels as f64 > 0.01 * r.count() as f64 {
        failed.push((
            "overlap",
            format!(
                "{name}: {} voxels the engine marked belong to other segments and were kept there",
                input.overlap_voxels
            ),
        ));
    }
    if let Some(l) = laterality(study, label, &r) {
        if l["consistent"] == false {
            failed.push((
                "laterality",
                format!(
                    "{name}: named {} but lies on the patient's {} ({})",
                    l["named"].as_str().unwrap_or(""),
                    l["observed"].as_str().unwrap_or(""),
                    l["reference"].as_str().unwrap_or("")
                ),
            ));
        }
        out["laterality"] = l;
    }
    if let Some(prev) = &input.previous {
        let dice = analysis::compare(prev.as_ref(), Some(&r), study.volume.spacing()).dice;
        out["stability"] = json!(round(dice, 4));
        if dice < STABLE_DICE {
            failed.push((
                "stability",
                format!(
                    "{name}: changed by this refinement (Dice {dice:.3} to the previous revision); not converged yet"
                ),
            ));
        }
    }
    out["failed"] = json!(failed.iter().map(|(k, _)| *k).collect::<Vec<_>>());
    (out, failed.into_iter().map(|(_, w)| w).collect())
}

/// The agent of a call (`agent` parameter) as an author.
pub fn requesting_agent(p: &Params) -> Result<Author, AgentError> {
    Ok(Author::Agent { id: p.str("agent")?.map(str::to_owned) })
}
