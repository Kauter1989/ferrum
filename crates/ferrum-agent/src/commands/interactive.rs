//! `segment interactive`: prompts to an interactive engine
//! (`docs/agent-segmentation.md` §5.2–5.3).
//!
//! - A new object becomes a segment proposed by the engine, requested by
//!   the agent. With `segment`, further prompts refine an object made
//!   earlier (`append` adds to its stored prompts, `undo` drops the last
//!   one); with `from_segment`, a segment the agent made is redone by the
//!   engine, seeded with lasso prompts from its own mask.
//! - The prompts of every object are stored in the workspace
//!   (`engine_inputs.json`). Each call replays them on a fresh session; a
//!   long-running server reuses an open session when its prompts are a
//!   prefix of the new list. Both give the same result.
//! - Only a region of interest is uploaded: the prompts' bounding box plus
//!   the operator's `roi_margin_mm`, unless the call gives `roi` or asks
//!   for the `whole_volume`.

use std::time::Duration;

use ferrum_domain::analysis::{self, MaskRegion};
use ferrum_domain::{
    EngineInfo, Geometry, Prompt, Provenance, ReviewStatus, SegmentationEngine, SliceAxis, Timestamp, Volume, VoxelBox,
};
use ferrum_io::EngineInput;
use glam::{DVec3, UVec3};
use serde_json::{json, Value};

use super::engine::{engine, engine_error, engine_url, info_json, research_allowed, research_warning};
use super::inspect::point;
use super::masks::{box_param, checks, region, requesting_agent, segment_label, CheckInput};
use super::segment::{changeable_segment, segment_json};
use super::Ctx;
use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::points::{nearest_voxel, parse_plane, resolve, PointInput};
use crate::sessions::{CachedSession, GpuLock, SessionKey};
use crate::study::{generator, Study, VolumeCache};

fn bad(msg: impl Into<String>) -> AgentError {
    AgentError::bad_request(msg)
}

fn corner_box(a: [u32; 3], b: [u32; 3]) -> VoxelBox {
    let lo = UVec3::new(a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2]));
    let hi = UVec3::new(a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])) + UVec3::ONE;
    VoxelBox::new(lo, hi)
}

fn resolve_points(study: &Study, v: &Value) -> Result<Vec<DVec3>, AgentError> {
    let list = v["points"].as_array().ok_or_else(|| bad("points is required"))?;
    list.iter().map(|p| resolve(&study.volume, &PointInput::parse(p)?, &study.renders_dir())).collect()
}

/// The plane all `points` lie on (same nearest slice index), axial first,
/// and that slice index.
fn common_plane(study: &Study, points: &[DVec3], wanted: Option<SliceAxis>) -> Result<(SliceAxis, u32), AgentError> {
    let idx: Vec<[u32; 3]> = points.iter().map(|p| nearest_voxel(&study.volume, *p)).collect();
    let order = [SliceAxis::Axial, SliceAxis::Coronal, SliceAxis::Sagittal];
    order
        .into_iter()
        .filter(|a| wanted.is_none_or(|w| w == *a))
        .find_map(|a| {
            let n = a.normal_axis();
            idx.iter().all(|v| v[n] == idx[0][n]).then_some((a, idx[0][n]))
        })
        .ok_or_else(|| {
            bad("the points of a lasso or scribble must lie on one slice").hint("point at one render, or give plane")
        })
}

/// In-plane axes of `plane` (first, second) as grid axes.
fn plane_axes(plane: SliceAxis) -> (usize, usize) {
    match plane {
        SliceAxis::Axial => (0, 1),
        SliceAxis::Coronal => (0, 2),
        SliceAxis::Sagittal => (1, 2),
    }
}

/// Voxels of one slice covered by a closed polygon (`fill`) or a polyline.
fn rasterize(
    study: &Study,
    points: &[DVec3],
    plane: SliceAxis,
    slice: u32,
    fill: bool,
) -> Result<(VoxelBox, Vec<u8>), AgentError> {
    let (a, b) = plane_axes(plane);
    let n = plane.normal_axis();
    let uv: Vec<[f64; 2]> = points.iter().map(|p| [p[a], p[b]]).collect();
    let dims = study.volume.dims().as_uvec3();
    let clamp = |x: f64, axis: usize| x.round().clamp(0.0, f64::from(dims[axis] - 1)) as u32;
    let lo = [
        clamp(uv.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), a),
        clamp(uv.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min), b),
    ];
    let hi = [
        clamp(uv.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max), a),
        clamp(uv.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max), b),
    ];
    let (w, h) = ((hi[0] - lo[0] + 1) as usize, (hi[1] - lo[1] + 1) as usize);
    let mut pixels = vec![0u8; w * h];
    if fill {
        for y in 0..h {
            for x in 0..w {
                let c = [f64::from(lo[0]) + x as f64, f64::from(lo[1]) + y as f64];
                pixels[x + w * y] = u8::from(inside_polygon(&uv, c));
            }
        }
    }
    let mut edges: Vec<[[f64; 2]; 2]> = uv.windows(2).map(|w| [w[0], w[1]]).collect();
    if fill {
        edges.push([uv[uv.len() - 1], uv[0]]);
    }
    for [p0, p1] in edges {
        let steps = (((p1[0] - p0[0]).abs() + (p1[1] - p0[1]).abs()) * 4.0).ceil().max(1.0) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let (x, y) = (clamp(p0[0] + t * (p1[0] - p0[0]), a) - lo[0], clamp(p0[1] + t * (p1[1] - p0[1]), b) - lo[1]);
            pixels[x as usize + w * y as usize] = 1;
        }
    }
    let mut min = UVec3::ZERO;
    let mut max = UVec3::ZERO;
    min[a] = lo[0];
    min[b] = lo[1];
    min[n] = slice;
    max[a] = hi[0] + 1;
    max[b] = hi[1] + 1;
    max[n] = slice + 1;
    // pixels are (first, second) in-plane with first fastest; the box is i fastest, which matches
    // for every plane because the first in-plane axis is always the lower grid axis
    Ok((VoxelBox::new(min, max), pixels))
}

fn inside_polygon(poly: &[[f64; 2]], c: [f64; 2]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (pi, pj) = (poly[i], poly[j]);
        if (pi[1] > c[1]) != (pj[1] > c[1]) && c[0] < (pj[0] - pi[0]) * (c[1] - pi[1]) / (pj[1] - pi[1]) + pi[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// One prompt of the call in full-grid voxels.
fn parse_prompt(study: &Study, v: &Value) -> Result<Prompt, AgentError> {
    let p = Params::new(v)?;
    let positive = p.bool("positive")?.unwrap_or(true);
    let vol = &study.volume;
    match p.req_str("type")? {
        "point" => Ok(Prompt::Point { positive, voxel: UVec3::from(nearest_voxel(vol, point(study, &p, "point")?)) }),
        "box" => {
            let (a, b) = (nearest_voxel(vol, point(study, &p, "min")?), nearest_voxel(vol, point(study, &p, "max")?));
            Ok(Prompt::Box { positive, bx: corner_box(a, b) })
        }
        kind @ ("lasso" | "scribble") => {
            let points = resolve_points(study, v)?;
            let need = if kind == "lasso" { 3 } else { 2 };
            if points.len() < need {
                return Err(bad(format!("a {kind} needs at least {need} points")));
            }
            let wanted = p.str("plane")?.map(parse_plane).transpose()?;
            let (plane, slice) = common_plane(study, &points, wanted)?;
            let (bx, mask) = rasterize(study, &points, plane, slice, kind == "lasso")?;
            Ok(if kind == "lasso" {
                Prompt::Lasso { positive, bx, mask }
            } else {
                Prompt::Scribble { positive, bx, mask }
            })
        }
        other => Err(bad(format!("unknown prompt type {other:?}")).hint("prompt types: point, box, scribble, lasso")),
    }
}

/// Lasso prompts from segment `label`'s mask on its largest slice of each
/// plane.
fn seed_prompts(study: &Study, label: u8) -> Result<Vec<Prompt>, AgentError> {
    let r = region(&study.segments, label).ok_or_else(|| bad(format!("segment {label} has no voxels")))?;
    let s = analysis::shape(&r, study.volume.dims(), study.volume.spacing());
    let mut out = Vec::new();
    for (plane, largest) in
        [SliceAxis::Axial, SliceAxis::Coronal, SliceAxis::Sagittal].into_iter().zip(s.largest_slices)
    {
        let n = plane.normal_axis();
        let (mut min, mut max) = (r.bx.min, r.bx.max);
        min[n] = largest.index;
        max[n] = largest.index + 1;
        let bx = VoxelBox::new(min, max);
        let Some(slice) = r.restricted(bx) else { continue };
        let slice = slice.expanded(bx);
        out.push(Prompt::Lasso { positive: true, bx, mask: slice.to_u8() });
    }
    Ok(out)
}

fn prompt_bounds(p: &Prompt) -> VoxelBox {
    match p {
        Prompt::Point { voxel, .. } => VoxelBox::new(*voxel, *voxel + UVec3::ONE),
        Prompt::Box { bx, .. } | Prompt::Scribble { bx, .. } | Prompt::Lasso { bx, .. } => *bx,
    }
}

/// The prompts' bounding box plus `margin_mm`, clamped to the volume.
pub fn default_roi(volume: &Volume, prompts: &[Prompt], margin_mm: f64) -> VoxelBox {
    let dims = volume.dims().as_uvec3();
    let b = prompts.iter().map(prompt_bounds).reduce(VoxelBox::union).unwrap_or(VoxelBox::full(volume.dims()));
    let sp = volume.spacing().as_dvec3();
    let m = UVec3::new(
        (margin_mm / sp.x).ceil() as u32,
        (margin_mm / sp.y).ceil() as u32,
        (margin_mm / sp.z).ceil() as u32,
    );
    VoxelBox::new(b.min.saturating_sub(m), (b.max + m).min(dims))
}

/// The voxels of `roi` as a volume placed at the same patient position.
pub fn crop(volume: &Volume, roi: VoxelBox) -> Result<Volume, AgentError> {
    if roi == VoxelBox::full(volume.dims()) {
        return Ok(volume.clone());
    }
    let d = volume.dims();
    let s = roi.size();
    let mut data = Vec::with_capacity(roi.voxel_count());
    for k in roi.min.z..roi.max.z {
        for j in roi.min.y..roi.max.y {
            let at = d.index(roi.min.x, j, k);
            data.extend_from_slice(&volume.data()[at..at + s.x as usize]);
        }
    }
    let geometry = Geometry { origin: volume.voxel_to_patient(roi.min.as_vec3()), ..volume.geometry() };
    Volume::new(ferrum_domain::Dims3::new(s.x, s.y, s.z), volume.spacing(), volume.range(), data)
        .map(|v| v.with_geometry(geometry))
        .map_err(|e| AgentError::internal(e.to_string()))
}

/// `p` moved into the voxels of `roi`.
fn to_roi(p: &Prompt, roi: VoxelBox) -> Result<Prompt, AgentError> {
    let b = prompt_bounds(p);
    if !(b.min.cmpge(roi.min).all() && b.max.cmple(roi.max).all()) {
        return Err(AgentError::new(ErrorCode::OutOfVolume, "a prompt lies outside the region of interest")
            .hint("leave out roi (the default covers every prompt) or make it larger"));
    }
    let o = roi.min;
    let shift = |bx: &VoxelBox| VoxelBox::new(bx.min - o, bx.max - o);
    Ok(match p {
        Prompt::Point { positive, voxel } => Prompt::Point { positive: *positive, voxel: *voxel - o },
        Prompt::Box { positive, bx } => Prompt::Box { positive: *positive, bx: shift(bx) },
        Prompt::Scribble { positive, bx, mask } => {
            Prompt::Scribble { positive: *positive, bx: shift(bx), mask: mask.clone() }
        }
        Prompt::Lasso { positive, bx, mask } => {
            Prompt::Lasso { positive: *positive, bx: shift(bx), mask: mask.clone() }
        }
    })
}

/// What one call asks the engine for.
struct Plan {
    url: String,
    info: EngineInfo,
    /// Segment to write into (`None`: a new one).
    target: Option<u8>,
    /// Name of a new segment.
    name: String,
    prompts: Vec<Prompt>,
    roi: VoxelBox,
    revision: u64,
    /// This call seeds the object from a segment's mask.
    seeded: bool,
    /// Leading lasso prompts derived from a mask; not counted against the
    /// prompt limit.
    seeds: usize,
    modality: String,
}

/// Stored prompts of the object being refined.
struct Refinement {
    prompts: Vec<Prompt>,
    roi: VoxelBox,
    revision: u64,
    seeds: usize,
}

fn stored_input(study: &Study, label: u8) -> Result<Option<EngineInput>, AgentError> {
    let created = study.segments.segment(label).and_then(|s| s.provenance.created);
    let inputs = study.workspace.load_engine_inputs()?;
    Ok(inputs.into_iter().find(|e| e.label == label && e.created == created))
}

/// Prompts, target and revision of a refinement of segment `label`.
fn refinement(study: &Study, p: &Params, url: &str, label: u8, new: Vec<Prompt>) -> Result<Refinement, AgentError> {
    changeable_segment(study, label)?;
    let stored = stored_input(study, label)?.ok_or_else(|| {
        bad(format!("segment {label} has no stored engine prompts"))
            .hint("refine only segments made by segment interactive; use from_segment to redo another of your segments")
    })?;
    if stored.engine != url {
        return Err(
            bad(format!("segment {label} was made by {}", stored.engine)).hint("refine it with the same engine")
        );
    }
    let keep = p.bool("append")?.unwrap_or(false) || p.bool("undo")?.unwrap_or(false);
    let (mut prompts, seeds) = if keep { (stored.prompts, stored.seeds) } else { (Vec::new(), 0) };
    if p.bool("undo")?.unwrap_or(false) {
        if !new.is_empty() {
            return Err(bad("undo takes no new prompts"));
        }
        if prompts.len() <= seeds.max(1) {
            return Err(bad("there is no prompt to undo").hint("delete the segment instead"));
        }
        prompts.pop();
    }
    prompts.extend(new);
    Ok(Refinement { prompts, roi: stored.roi, revision: stored.revision + 1, seeds })
}

fn plan(study: &Study, config: &AgentConfig, p: &Params) -> Result<Plan, AgentError> {
    let url = engine_url(config, p)?;
    let info = engine(&url).info().map_err(engine_error)?;
    research_allowed(config, &info)?;
    let new: Vec<Prompt> = match p.list("prompts")? {
        Some(l) => l.iter().map(|v| parse_prompt(study, v)).collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    let (target, mut prompts, stored_roi, revision, seeds) = match (p.u64("segment")?, p.u64("from_segment")?) {
        (Some(_), Some(_)) => return Err(bad("give segment or from_segment, not both")),
        (Some(_), None) => {
            let label = segment_label(study, p, "segment")?;
            let r = refinement(study, p, &url, label, new)?;
            (Some(label), r.prompts, Some(r.roi), r.revision, r.seeds)
        }
        (None, Some(_)) => {
            let label = segment_label(study, p, "from_segment")?;
            changeable_segment(study, label)?;
            let mut prompts = seed_prompts(study, label)?;
            let seeds = prompts.len();
            prompts.extend(new);
            (Some(label), prompts, None, 1, seeds)
        }
        (None, None) => (None, new, None, 1, 0),
    };
    let seeded = p.u64("from_segment")?.is_some();
    if prompts.is_empty() {
        return Err(bad("prompts is required"));
    }
    check_prompts(config, &info, &mut prompts, seeds)?;
    let roi = choose_roi(study, config, p, &prompts, stored_roi)?;
    let name = p.str("name")?.map_or_else(|| format!("{} object", info.name), str::to_owned);
    let modality = p.str("modality")?.map_or_else(|| study.metadata.modality.clone(), str::to_owned);
    Ok(Plan { url, info, target, name, prompts, roi, revision, seeded, seeds, modality })
}

fn check_prompts(
    config: &AgentConfig,
    info: &EngineInfo,
    prompts: &mut [Prompt],
    seeds: usize,
) -> Result<(), AgentError> {
    let limit = config.max_prompts_per_object as usize;
    let counted = prompts.len().saturating_sub(seeds);
    if counted > limit {
        return Err(AgentError::new(
            ErrorCode::Limit,
            format!("{counted} prompts exceed the limit of {limit} per object"),
        )
        .hint("report the object as not converged instead of prompting further"));
    }
    if let Some(k) = prompts.iter().map(Prompt::kind).find(|k| !info.supports(*k)) {
        return Err(bad(format!("{} does not take {} prompts", info.name, k.as_str())).hint("see engine info: prompts"));
    }
    let thick = |bx: &VoxelBox| bx.size().cmpgt(UVec3::ONE).all();
    if info.capabilities.planar_boxes_only && prompts.iter().any(|p| matches!(p, Prompt::Box { bx, .. } if thick(bx))) {
        return Err(bad(format!("{} takes boxes on one slice only", info.name))
            .hint("give both corners on the same slice, e.g. two pixels of one render"));
    }
    Ok(())
}

fn choose_roi(
    study: &Study,
    config: &AgentConfig,
    p: &Params,
    prompts: &[Prompt],
    stored: Option<VoxelBox>,
) -> Result<VoxelBox, AgentError> {
    if p.bool("whole_volume")?.unwrap_or(false) {
        return Ok(VoxelBox::full(study.volume.dims()));
    }
    if let Some(b) = p.get("roi") {
        return box_param(study, b);
    }
    let around = default_roi(&study.volume, prompts, config.roi_margin_mm);
    Ok(stored.map_or(around, |s| s.union(around)))
}

/// The engine's mask of the object as a region of the full grid.
struct EngineResult {
    mask: Option<MaskRegion>,
    session: CachedSession,
}

fn session_key(study: &Study, plan: &Plan, label: u8, created: Option<Timestamp>) -> SessionKey {
    SessionKey {
        workspace: study.workspace.root().to_path_buf(),
        source_sha256: study.source_sha256.clone(),
        engine: plan.url.clone(),
        version: plan.info.version.clone(),
        label,
        created,
        roi: plan.roi,
    }
}

fn run_engine(
    study: &Study,
    plan: &Plan,
    cached: Option<CachedSession>,
    key: SessionKey,
) -> Result<EngineResult, AgentError> {
    let reusable =
        cached.filter(|c| c.prompts.len() <= plan.prompts.len() && c.prompts[..] == plan.prompts[..c.prompts.len()]);
    let mut s = match reusable {
        Some(c) => c,
        None => {
            let sub = crop(&study.volume, plan.roi)?;
            let session = engine(&plan.url).open_session(&sub, &plan.modality).map_err(engine_error)?;
            CachedSession { key, session, prompts: Vec::new(), extent: None }
        }
    };
    for pr in plan.prompts[s.prompts.len()..].iter() {
        let r = s.session.prompt(&to_roi(pr, plan.roi)?).map_err(engine_error)?;
        if let Some(bx) = r.changed {
            s.extent = Some(s.extent.map_or(bx, |e| e.union(bx)));
        }
        s.prompts.push(pr.clone());
    }
    let mask = match s.extent {
        Some(bx) => {
            let m = s.session.mask(bx).map_err(engine_error)?;
            let global = VoxelBox::new(bx.min + plan.roi.min, bx.max + plan.roi.min);
            MaskRegion::from_box_mask(global, &m)
        }
        None => None,
    };
    Ok(EngineResult { mask, session: s })
}

/// Voxels of `mask` that belong to segments other than `label`.
fn overlap(study: &Study, mask: Option<&MaskRegion>, label: Option<u8>) -> u64 {
    let Some(m) = mask else { return 0 };
    let labels = study.segments.labels();
    let s = m.bx.size();
    let mut n = 0;
    for (idx, inside) in m.inside.iter().enumerate() {
        let l = UVec3::new(
            (idx % s.x as usize) as u32,
            ((idx / s.x as usize) % s.y as usize) as u32,
            (idx / (s.x as usize * s.y as usize)) as u32,
        );
        let g = m.bx.min + l;
        let v = labels.label(g.x, g.y, g.z).unwrap_or(0);
        if *inside && v != 0 && Some(v) != label {
            n += 1;
        }
    }
    n
}

/// Writes the engine's mask into the target (or a new segment) and returns
/// its label.
fn write(study: &mut Study, p: &Params, plan: &Plan, mask: Option<&MaskRegion>) -> Result<u8, AgentError> {
    let agent = requesting_agent(p)?;
    let i = &plan.info;
    let set = &mut study.segments;
    let fresh = Provenance::engine(&i.name, &i.version, i.research_only, Timestamp::now()).requested_by(agent);
    let label = match plan.target {
        Some(l) => {
            let old = set.segment(l).map(|s| s.provenance.clone()).unwrap_or_default();
            let keep = !plan.seeded && matches!(old.author, ferrum_domain::Author::Engine { .. });
            let mut prov = if keep { Provenance { created: old.created, ..fresh } } else { fresh };
            prov.review(ReviewStatus::Proposed, None, Timestamp::now());
            set.set_provenance(l, prov).map_err(|e| AgentError::internal(e.to_string()))?;
            if let Some(n) = p.str("name")? {
                set.rename(l, n);
            }
            l
        }
        None => {
            let l = set.add_segment(&plan.name).map_err(|e| AgentError::new(ErrorCode::Limit, e.to_string()))?;
            set.set_provenance(l, fresh).map_err(|e| AgentError::internal(e.to_string()))?;
            l
        }
    };
    let old = region(&study.segments, label);
    let bx = match (&old, mask) {
        (Some(o), Some(m)) => o.bx.union(m.bx),
        (Some(o), None) => o.bx,
        (None, Some(m)) => m.bx,
        (None, None) => return Ok(label),
    };
    let new = mask.map_or_else(|| MaskRegion::empty(bx), |m| m.expanded(bx));
    study.segments.apply_mask(label, bx, &new.to_u8(), false).map_err(|e| AgentError::internal(e.to_string()))?;
    Ok(label)
}

fn save_input(study: &Study, plan: &Plan, label: u8) -> Result<(), AgentError> {
    let mut inputs = study.workspace.load_engine_inputs()?;
    // drop entries of this label and of segments that no longer exist as recorded
    inputs.retain(|e| {
        e.label != label && study.segments.segment(e.label).is_some_and(|s| s.provenance.created == e.created)
    });
    inputs.push(EngineInput {
        label,
        created: study.segments.segment(label).and_then(|s| s.provenance.created),
        engine: plan.url.clone(),
        engine_name: plan.info.name.clone(),
        engine_version: plan.info.version.clone(),
        roi: plan.roi,
        revision: plan.revision,
        seeds: plan.seeds,
        prompts: plan.prompts.clone(),
    });
    inputs.sort_by_key(|e| e.label);
    Ok(study.workspace.save_engine_inputs(&inputs, &generator())?)
}

fn take_cached(cache: Option<&mut &mut VolumeCache>, config: &AgentConfig, key: &SessionKey) -> Option<CachedSession> {
    let c = cache?;
    if let Some(group) = config.gpu_group(&key.engine) {
        let others: Vec<String> =
            config.gpu_groups.iter().filter(|(g, _)| g == group).flat_map(|(_, l)| l.clone()).collect();
        c.engines.close_engines(&others);
        return None;
    }
    c.engines.take(key)
}

/// `segment interactive`: see the module documentation.
pub fn interactive(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    ctx.study(p)?;
    let Ctx { config, study, cache, .. } = ctx;
    let study = study.as_mut().ok_or_else(|| AgentError::internal("study not loaded"))?;
    let plan = plan(study, config, p)?;
    let previous = plan.target.map(|l| region(&study.segments, l));
    let created = plan.target.and_then(|l| study.segments.segment(l)).and_then(|s| s.provenance.created);
    // a new object's session is cached under its label once it exists; before that nothing matches
    let key = session_key(study, &plan, plan.target.unwrap_or(0), if plan.seeded { None } else { created });
    let cached = take_cached(cache.as_mut(), config, &key);
    let result = {
        let _gpu = GpuLock::acquire(config, &plan.url, Duration::from_secs(config.engine_job_timeout_s))?;
        run_engine(study, &plan, cached, key)?
    };
    if result.mask.is_none() && plan.target.is_none() {
        return Ok(research_warning(
            Output::new(json!({ "segment": null, "engine": info_json(config, &plan.url, &plan.info) })),
            &plan.info,
        )
        .warn("the engine found no object for these prompts"));
    }
    let overlap_voxels = overlap(study, result.mask.as_ref(), plan.target);
    let label = write(study, p, &plan, result.mask.as_ref())?;
    study.save_segments()?;
    save_input(study, &plan, label)?;
    if let Some(c) = cache.as_mut().filter(|_| config.gpu_group(&plan.url).is_none()) {
        let mut s = result.session;
        s.key.label = label;
        s.key.created = study.segments.segment(label).and_then(|x| x.provenance.created);
        c.engines.put(s);
    }
    let input = CheckInput {
        overlap_voxels,
        previous: previous.filter(|_| !plan.seeded),
        min_ml: p.f64("min_ml")?,
        max_ml: p.f64("max_ml")?,
        research_only: plan.info.research_only,
        roi: Some(plan.roi),
    };
    let (checks, warnings) = checks(study, label, &input);
    let mut out = Output::new(json!({
        "segment": segment_json(study, label),
        "revision": plan.revision,
        "prompts": plan.prompts.len(),
        "roi": { "min": plan.roi.min.to_array(), "max": plan.roi.max.to_array(), "voxels": plan.roi.voxel_count() },
        "checks": checks,
        "engine": info_json(config, &plan.url, &plan.info),
    }));
    for w in warnings {
        out = out.warn(w);
    }
    if !plan.info.capabilities.deterministic {
        out = out.warn(format!("{} does not guarantee the same result when prompts are replayed", plan.info.name));
    }
    Ok(research_warning(out, &plan.info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::Dims3;
    use glam::Vec3;

    fn volume() -> Volume {
        let dims = Dims3::new(10, 8, 6);
        Volume::from_physical(
            dims,
            Vec3::new(1.0, 1.0, 2.0),
            &(0..dims.voxel_count()).map(|n| n as f32).collect::<Vec<_>>(),
        )
        .unwrap()
        .with_geometry(Geometry { origin: Vec3::new(-5.0, -4.0, 10.0), ..Geometry::default() })
    }

    #[test]
    fn crop_keeps_values_and_position() {
        let v = volume();
        let roi = VoxelBox::new(UVec3::new(2, 1, 3), UVec3::new(5, 4, 6));
        let c = crop(&v, roi).unwrap();
        assert_eq!(c.dims(), Dims3::new(3, 3, 3));
        assert_eq!(c.physical(0, 0, 0), v.physical(2, 1, 3));
        assert_eq!(c.physical(2, 2, 2), v.physical(4, 3, 5));
        assert_eq!(c.voxel_to_patient(Vec3::ZERO), v.voxel_to_patient(Vec3::new(2.0, 1.0, 3.0)));
        assert_eq!(crop(&v, VoxelBox::full(v.dims())).unwrap(), v);
    }

    #[test]
    fn default_roi_adds_a_margin_in_mm() {
        let v = volume();
        let p = [Prompt::Point { positive: true, voxel: UVec3::new(5, 4, 3) }];
        assert_eq!(default_roi(&v, &p, 2.0), VoxelBox::new(UVec3::new(3, 2, 2), UVec3::new(8, 7, 5)));
        assert_eq!(default_roi(&v, &p, 100.0), VoxelBox::full(v.dims()));
        let shifted = to_roi(&p[0], VoxelBox::new(UVec3::new(3, 2, 2), UVec3::new(8, 7, 5))).unwrap();
        assert_eq!(shifted, Prompt::Point { positive: true, voxel: UVec3::new(2, 2, 1) });
        let outside = to_roi(&p[0], VoxelBox::new(UVec3::ZERO, UVec3::ONE)).unwrap_err();
        assert_eq!(outside.code, ErrorCode::OutOfVolume);
    }

    #[test]
    fn polygons_contain_their_inside() {
        let square = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        assert!(inside_polygon(&square, [2.0, 2.0]));
        assert!(!inside_polygon(&square, [5.0, 2.0]));
        assert_eq!(plane_axes(SliceAxis::Coronal), (0, 2));
    }
}
