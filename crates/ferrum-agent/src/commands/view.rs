//! `view slice`: renders one slice to a PNG with a sidecar JSON that maps
//! pixels to voxels and patient millimetres (`docs/agent-skill.md` §9).
//! The building blocks here also serve the tiled renders in [`super::tiles`].
//!
//! Rendering runs on the CPU, needs no GPU, and samples the nearest voxel
//! so that no value is invented between voxels.

use std::path::Path;

use ferrum_domain::{
    storage_histogram, SegmentStyle, SliceAxis, Volume, VoxelBox, WindowLevel, WindowPreset, AUTO_WINDOW_BINS,
};
use ferrum_engines::{HttpConfig, OcrBridge, OcrOutcome, OcrRegion};
use glam::{DVec2, DVec3, UVec3};
use serde_json::{json, Value};

use super::inspect::point;
use super::Ctx;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::points::{nearest_voxel, parse_plane, voxel_to_patient};
use crate::study::Study;

/// Default largest render side in pixels.
pub const DEFAULT_SIZE: u32 = 768;

/// Affine map of `(x, y, 1)` to three coordinates (rows).
pub(super) type Affine = [[f64; 3]; 3];

pub(super) fn apply(m: &Affine, x: f64, y: f64) -> DVec3 {
    DVec3::new(
        m[0][0] * x + m[0][1] * y + m[0][2],
        m[1][0] * x + m[1][1] * y + m[1][2],
        m[2][0] * x + m[2][1] * y + m[2][2],
    )
}

/// Geometry of one slice render.
pub(super) struct Layout {
    pub(super) plane: SliceAxis,
    pub(super) index: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixel_mm: f64,
    pub(super) to_voxel: Affine,
}

impl Layout {
    /// A slice of `plane` fitted into a square of `size` pixels.
    pub(super) fn new(v: &Volume, plane: SliceAxis, index: u32, size: u32) -> Self {
        let size_mm = plane.plane_size_mm(v).as_dvec2();
        let pixel_mm = size_mm.max_element() / f64::from(size);
        let width = (size_mm.x / pixel_mm).round().max(1.0) as u32;
        let height = (size_mm.y / pixel_mm).round().max(1.0) as u32;
        let s = plane.slice_position(v, index);
        let dims = v.dims().as_uvec3().as_dvec3();
        // pixel coordinates (x, y) cover in-plane millimetres (x, y) · pixel_mm
        let voxel = |x: f64, y: f64| {
            let uv = DVec2::new(x, y) * pixel_mm / size_mm;
            plane.tex_coord(uv.as_vec2(), s).as_dvec3() * dims - DVec3::splat(0.5)
        };
        let (o, ex, ey) = (voxel(0.0, 0.0), voxel(1.0, 0.0), voxel(0.0, 1.0));
        let (dx, dy) = (ex - o, ey - o);
        let mut to_voxel = [[0.0; 3]; 3];
        for (r, row) in to_voxel.iter_mut().enumerate() {
            *row = [dx[r], dy[r], o[r]];
        }
        // the normal coordinate is the slice index exactly
        to_voxel[plane.normal_axis()] = [0.0, 0.0, f64::from(index)];
        Self { plane, index, width, height, pixel_mm, to_voxel }
    }

    fn voxel_of_pixel(&self, v: &Volume, x: u32, y: u32) -> [u32; 3] {
        nearest_voxel(v, apply(&self.to_voxel, f64::from(x) + 0.5, f64::from(y) + 0.5))
    }

    pub(super) fn to_patient(&self, v: &Volume) -> Affine {
        let o = voxel_to_patient(v, apply(&self.to_voxel, 0.0, 0.0));
        let dx = voxel_to_patient(v, apply(&self.to_voxel, 1.0, 0.0)) - o;
        let dy = voxel_to_patient(v, apply(&self.to_voxel, 0.0, 1.0)) - o;
        let mut m = [[0.0; 3]; 3];
        for (r, row) in m.iter_mut().enumerate() {
            *row = [dx[r], dy[r], o[r]];
        }
        m
    }
}

/// The window the desktop app opens a series with when the data stores
/// none: 0.5 to 99.5 percent of the voxels.
fn auto_window(v: &Volume) -> WindowLevel {
    WindowLevel::auto(v.range(), &storage_histogram(v.data(), AUTO_WINDOW_BINS))
}

pub(super) fn window(study: &Study, p: &Params) -> Result<WindowLevel, AgentError> {
    let range = study.volume.range();
    match p.get("window") {
        None => Ok(study.metadata.default_window.unwrap_or_else(|| auto_window(&study.volume))),
        Some(Value::String(name)) => WindowPreset::ALL
            .iter()
            .find(|w| w.label().eq_ignore_ascii_case(name) || w.label().replace(' ', "_").eq_ignore_ascii_case(name))
            .map(|w| w.window(range))
            .ok_or_else(|| {
                AgentError::bad_request(format!("unknown window {name:?}"))
                    .hint("presets: full_range, brain, soft_tissue, lung, bone; or {\"center\": c, \"width\": w}")
            }),
        Some(w) => {
            let wp = Params::new(w)?;
            Ok(WindowLevel::new(wp.req_f64("center")? as f32, wp.req_f64("width")? as f32))
        }
    }
}

fn slice_index(study: &Study, p: &Params, plane: SliceAxis) -> Result<u32, AgentError> {
    let count = plane.slice_count(&study.volume);
    let index = match (p.u64("slice_number")?, p.get("at")) {
        (Some(n), None) => n.checked_sub(1).ok_or_else(|| AgentError::bad_request("slice_number is 1-based"))?,
        (None, Some(_)) => point(study, p, "at")?[plane.normal_axis()].round() as u64,
        _ => return Err(AgentError::bad_request("give slice_number (1-based) or at (a point)")),
    };
    if index >= u64::from(count) {
        return Err(AgentError::new(ErrorCode::OutOfVolume, format!("slice {} of {count}", index + 1))
            .hint(format!("{} slice numbers run from 1 to {count}", plane.label().to_lowercase())));
    }
    Ok(index as u32)
}

fn next_render_id(dir: &Path) -> String {
    let n = std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    name.strip_prefix("r-")?.strip_suffix(".json")?.parse::<u32>().ok()
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    format!("r-{:04}", n + 1)
}

/// How segments are drawn on a render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentOverlay {
    /// Outline, fill or both.
    pub style: SegmentStyle,
    /// Fill opacity for every segment; `None`: each segment's own opacity.
    pub opacity: Option<f32>,
}

/// The segment overlay of a render call: `overlays` containing `segments`,
/// with `segment_style` (default `outline`) and `segment_opacity`.
pub(super) fn segment_overlay(p: &Params) -> Result<Option<SegmentOverlay>, AgentError> {
    if !overlays(p)?.contains(&"segments") {
        return Ok(None);
    }
    let style = match p.str("segment_style")? {
        None => SegmentStyle::Outline,
        Some(s) => SegmentStyle::parse(s).ok_or_else(|| {
            AgentError::bad_request(format!("unknown segment_style {s:?}")).hint("outline, fill or fill_outline")
        })?,
    };
    let opacity = p.f64("segment_opacity")?;
    if opacity.is_some_and(|o| !(0.0..=1.0).contains(&o)) {
        return Err(AgentError::bad_request("segment_opacity must lie in [0, 1]"));
    }
    Ok(Some(SegmentOverlay { style, opacity: opacity.map(|o| o as f32) }))
}

/// Width of a segment outline in pixels: about a third of a voxel on
/// screen, 1 to 3 pixels, so it stays visible when slices are enlarged.
fn outline_width(study: &Study, l: &Layout) -> usize {
    let voxel_mm = f64::from(l.plane.plane_spacing(&study.volume).min_element());
    ((voxel_mm / l.pixel_mm) / 3.0).round().clamp(1.0, 3.0) as usize
}

/// `true` if pixel `i` of label `l0` lies within `t` pixels (along a row or
/// column) of another label or of the image border.
fn on_edge(lbl: &[u8], wd: usize, ht: usize, i: usize, t: usize) -> bool {
    let (x, y, l0) = (i % wd, i / wd, lbl[i]);
    (1..=t).any(|d| {
        x < d
            || y < d
            || x + d >= wd
            || y + d >= ht
            || lbl[i - d] != l0
            || lbl[i + d] != l0
            || lbl[i - d * wd] != l0
            || lbl[i + d * wd] != l0
    })
}

/// Grey values with an optional segment overlay. A pixel is on a segment's
/// outline when another label (or the image border) lies within the
/// outline width along its row or column, so outlines are closed on every
/// side. `volume` is the data drawn — the study's own, or a masked copy
/// (Clean Pixel Data): masking is a pixel transform on the data, so the
/// sampling and the overlay rule are exactly the same.
pub(super) fn render_pixels(
    study: &Study,
    volume: &Volume,
    l: &Layout,
    w: WindowLevel,
    overlay: Option<SegmentOverlay>,
) -> Vec<u8> {
    let v = volume;
    let (wd, ht) = (l.width as usize, l.height as usize);
    let mut voxels = Vec::with_capacity(wd * ht);
    for y in 0..l.height {
        for x in 0..l.width {
            voxels.push(l.voxel_of_pixel(v, x, y));
        }
    }
    let mut rgb = Vec::with_capacity(wd * ht * 3);
    for vox in &voxels {
        let value = v.physical(vox[0], vox[1], vox[2]).unwrap_or(0.0);
        let g = (w.apply(value) * 255.0).round() as u8;
        rgb.extend_from_slice(&[g, g, g]);
    }
    if let Some(o) = overlay {
        let labels = study.segments.labels();
        let lbl: Vec<u8> = voxels.iter().map(|p| labels.label(p[0], p[1], p[2]).unwrap_or(0)).collect();
        let t = outline_width(study, l);
        for (i, &l0) in lbl.iter().enumerate() {
            let Some(seg) = study.segments.segment(l0).filter(|s| s.visible && l0 != 0) else { continue };
            let edge = o.style.outlines() && on_edge(&lbl, wd, ht, i, t);
            let grey = [rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]];
            let alpha = o.opacity.unwrap_or(seg.opacity);
            rgb[i * 3..i * 3 + 3].copy_from_slice(&o.style.blend(grey, seg.color, alpha, edge));
        }
    }
    rgb
}

/// The solid fill a masked render writes into the masked voxels (stored
/// value `0` = the range minimum: black/neutral under a typical window).
pub(super) const MASK_FILL: u16 = 0;

/// The mask one render call applies (Clean Pixel Data, criterion 31.3-c):
/// explicit `masks` boxes, else the `BurnedInAnnotation` flag of the DICOM
/// sources (the conservative whole-volume default), else none. A requested
/// mask is always reported in the sidecar — never silently passed.
pub(super) struct MaskPlan {
    /// The call asked for masking (`masked`, or a `masks` region).
    pub(super) requested: bool,
    /// The voxel boxes to mask (empty: nothing to mask).
    pub(super) boxes: Vec<VoxelBox>,
    /// Where the boxes came from: `regions`, `burned_in` or `none`.
    pub(super) source: &'static str,
}

impl MaskPlan {
    /// The sidecar block: what was requested and what was applied.
    fn json(&self) -> Value {
        json!({
            "requested": self.requested,
            "applied": !self.boxes.is_empty(),
            "source": self.source,
            "fill": MASK_FILL,
            "boxes": self.boxes.iter().map(|b| json!({ "min": b.min.to_array(), "max": b.max.to_array() })).collect::<Vec<_>>(),
        })
    }
}

/// Resolves the mask of a render call: the `masks` boxes (the same corner
/// points as `stats`/`segment` boxes), else the flag default.
pub(super) fn mask_plan(study: &Study, p: &Params) -> Result<MaskPlan, AgentError> {
    let requested = p.bool("masked")?.unwrap_or(false) || p.list("masks")?.is_some_and(|l| !l.is_empty());
    if !requested {
        return Ok(MaskPlan { requested: false, boxes: Vec::new(), source: "none" });
    }
    let mut boxes = Vec::new();
    for b in p.list("masks")?.into_iter().flatten() {
        boxes.push(super::masks::box_param(study, b)?);
    }
    if !boxes.is_empty() {
        return Ok(MaskPlan { requested: true, boxes, source: "regions" });
    }
    // No configured region: a flagged study masks the whole volume (the
    // text's location is unknown); an unflagged one has nothing to mask.
    let sources = study.workspace.manifest().source.file_paths();
    if ferrum_io::dicom::clean_pixel::sources_burned_in(&sources) {
        return Ok(MaskPlan { requested: true, boxes: vec![VoxelBox::full(study.volume.dims())], source: "burned_in" });
    }
    Ok(MaskPlan { requested: true, boxes: Vec::new(), source: "none" })
}

/// `view slice`: plane, slice_number or at, window, size, overlays, masked,
/// ocr. With `ocr`, the optional OCR bridge check runs on the unmasked
/// render; its detected regions extend the mask when masking is requested.
/// The sidecar reports the mask and the OCR outcome — never silently passed.
pub fn slice(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let max = ctx.config.max_render_px;
    let ocr_bridge = ctx.config.ocr_bridge.clone();
    let study = ctx.study(p)?;
    let plane = parse_plane(p.req_str("plane")?)?;
    let index = slice_index(study, p, plane)?;
    let w = window(study, p)?;
    let size = size_param(p, max, DEFAULT_SIZE)?;
    let overlay = segment_overlay(p)?;
    let layout = Layout::new(&study.volume, plane, index, size);
    let mut mask = mask_plan(study, p)?;
    let ocr_requested = p.bool("ocr")?.unwrap_or(false);
    // The unmasked render: the OCR check's input, and the final image when
    // nothing is masked.
    let base =
        (ocr_requested || mask.boxes.is_empty()).then(|| render_pixels(study, &study.volume, &layout, w, overlay));
    let mut ocr_json = None;
    if ocr_requested {
        let rgb = base.as_ref().ok_or_else(|| AgentError::internal("no base render"))?;
        let png = png_bytes(layout.width, layout.height, rgb)?;
        let bridge = ocr_bridge.as_deref().map(|url| OcrBridge::new(HttpConfig::new(url)));
        let outcome = ferrum_engines::ocr::check(bridge.as_ref(), &png);
        if mask.requested {
            if let OcrOutcome::Detected { regions } = &outcome {
                if mask.boxes.is_empty() {
                    mask.source = "ocr";
                }
                mask.boxes.extend(regions.iter().map(|r| ocr_region_box(&layout, study.volume.dims(), r)));
            }
        }
        ocr_json = Some(outcome.json());
    }
    let rgb = if mask.boxes.is_empty() {
        base.ok_or_else(|| AgentError::internal("no render"))?
    } else {
        let masked = study.volume.masked_copy(&mask.boxes, MASK_FILL);
        render_pixels(study, &masked, &layout, w, overlay)
    };
    save_render(study, layout.width, layout.height, rgb, |id| sidecar(study, &layout, id, w, overlay, &mask, ocr_json))
}

/// The PNG bytes of an RGB render (the OCR bridge's input).
fn png_bytes(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, AgentError> {
    let img =
        image::RgbImage::from_raw(width, height, rgb.to_vec()).ok_or_else(|| AgentError::internal("image size"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| AgentError::internal(e.to_string()))?;
    Ok(out.into_inner())
}

/// The voxel box covering an OCR region of the render: the region's corners
/// mapped through the render's pixel-to-voxel affine, rounded outwards
/// (conservative — a privacy mask may cover more, never less), on the
/// render's own slice.
fn ocr_region_box(l: &Layout, dims: ferrum_domain::Dims3, r: &OcrRegion) -> VoxelBox {
    let p0 = apply(&l.to_voxel, f64::from(r.x), f64::from(r.y));
    // add in f64: a crafted OCR region (x = u32::MAX, w = 1) must not overflow
    let p1 = apply(&l.to_voxel, f64::from(r.x) + f64::from(r.w), f64::from(r.y) + f64::from(r.h));
    let (lo, hi) = (p0.min(p1), p0.max(p1));
    let d = dims.as_uvec3();
    let clamp = |v: f64, n: u32| v.clamp(0.0, f64::from(n)) as u32;
    let mut min = UVec3::new(clamp(lo.x.floor(), d.x), clamp(lo.y.floor(), d.y), clamp(lo.z.floor(), d.z));
    let mut max = UVec3::new(clamp(hi.x.ceil(), d.x), clamp(hi.y.ceil(), d.y), clamp(hi.z.ceil(), d.z));
    // the render shows one slice: the box stays on it
    let n = l.plane.normal_axis();
    min[n] = l.index;
    max[n] = l.index + 1;
    // at least one voxel along each in-plane axis
    for a in [l.plane.screen_axis(0), l.plane.screen_axis(1)] {
        if max[a] <= min[a] {
            max[a] = (min[a] + 1).min(d[a]);
        }
    }
    VoxelBox::new(min, max)
}

/// Size parameter: largest side in pixels, capped by the operator.
pub(super) fn size_param(p: &Params, max: u32, default: u32) -> Result<u32, AgentError> {
    Ok(p.u64("size")?.map_or(default.min(max), |s| s.clamp(16, u64::from(max)) as u32))
}

/// The `overlays` parameter (only `segments` exists).
pub(super) fn overlays<'a>(p: &Params<'a>) -> Result<Vec<&'a str>, AgentError> {
    let overlays: Vec<&str> =
        p.list("overlays")?.map(|l| l.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if let Some(bad) = overlays.iter().find(|o| **o != "segments") {
        return Err(AgentError::bad_request(format!("unknown overlay {bad:?}")).hint("overlays: segments"));
    }
    Ok(overlays)
}

/// Writes `renders/<id>.png` and `<id>.json` (the sidecar built for the
/// new id) and returns the sidecar with the file paths.
pub(super) fn save_render(
    study: &Study,
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    sidecar: impl FnOnce(&str) -> Value,
) -> Result<Output, AgentError> {
    let dir = study.renders_dir();
    std::fs::create_dir_all(&dir).map_err(|e| AgentError::internal(e.to_string()))?;
    let id = next_render_id(&dir);
    let png = dir.join(format!("{id}.png"));
    let img = image::RgbImage::from_raw(width, height, rgb).ok_or_else(|| AgentError::internal("image size"))?;
    img.save_with_format(&png, image::ImageFormat::Png).map_err(|e| AgentError::internal(e.to_string()))?;
    let sidecar = sidecar(&id);
    let json_path = dir.join(format!("{id}.json"));
    let text = serde_json::to_string_pretty(&sidecar).map_err(|e| AgentError::internal(e.to_string()))?;
    std::fs::write(&json_path, text).map_err(|e| AgentError::internal(e.to_string()))?;
    let mut data = sidecar;
    data["image"] = json!(path_text(&png));
    data["sidecar"] = json!(path_text(&json_path));
    Ok(Output::new(data))
}

fn path_text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Note written into every sidecar.
pub(super) const NOTE: &str =
    "pixel centres lie at +0.5; maps take (x, y, 1). Take numbers from probe, stats and measure, not from the image.";

/// Plane, slice and mapping of one slice image (a whole render or a tile).
pub(super) fn slice_json(v: &Volume, l: &Layout) -> Value {
    let r = |m: Affine| m.map(|row| row.map(|x| (x * 1e4).round() / 1e4));
    let [left, right, top, bottom] = l.plane.edge_labels();
    json!({
        "plane": l.plane.label().to_lowercase(),
        "slice_number": l.index + 1,
        "slice_index": l.index,
        "slice_count": l.plane.slice_count(v),
        "size": [l.width, l.height],
        "pixel_mm": (l.pixel_mm * 1e4).round() / 1e4,
        "pixel_to_voxel": r(l.to_voxel),
        "pixel_to_patient_mm": r(l.to_patient(v)),
        "orientation": { "left": left, "right": right, "top": top, "bottom": bottom },
    })
}

fn sidecar(
    study: &Study,
    l: &Layout,
    id: &str,
    w: WindowLevel,
    overlay: Option<SegmentOverlay>,
    mask: &MaskPlan,
    ocr: Option<Value>,
) -> Value {
    let mut s = json!({ "render": id, "kind": "slice" });
    if let (Some(o), Value::Object(m)) = (s.as_object_mut(), slice_json(&study.volume, l)) {
        o.extend(m);
    }
    s["window"] = json!({ "center": w.center, "width": w.width });
    s["overlays"] = json!(overlay.map_or_else(Vec::new, |_| vec!["segments"]));
    if let Some(o) = overlay {
        s["segment_style"] = json!(o.style.as_str());
        s["segment_opacity"] = json!(o.opacity);
    }
    if mask.requested {
        s["mask"] = mask.json();
    }
    if let Some(o) = ocr {
        s["ocr"] = o;
    }
    s["note"] = json!(NOTE);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers 31.3-b — a crafted OCR region must not overflow (review of #133):
    // x + w is added in f64, so x = u32::MAX, w = 1 cannot panic or wrap.
    #[test]
    fn a_crafted_ocr_region_does_not_overflow() {
        let dims = ferrum_domain::Dims3::new(8, 8, 4);
        let v = Volume::from_physical(dims, glam::Vec3::ONE, &vec![0.0; dims.voxel_count()]).unwrap();
        let l = Layout::new(&v, ferrum_domain::SliceAxis::Axial, 2, 64);
        let r = OcrRegion { x: u32::MAX, y: u32::MAX, w: 1, h: 1 };
        let bx = ocr_region_box(&l, dims, &r);
        assert!(bx.max.x <= dims.x && bx.max.y <= dims.y, "the box is clamped to the grid: {bx:?}");
    }
}
