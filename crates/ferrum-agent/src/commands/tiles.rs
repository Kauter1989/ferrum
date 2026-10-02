//! Tiled renders: `view montage` (several slices of one plane) and
//! `view mpr` (three planes through a point). Each tile has its own
//! pixel mapping in the sidecar; a pixel of the composed image resolves
//! through the tile that contains it.

use ferrum_domain::{SliceAxis, Volume, WindowLevel};
use glam::DVec3;
use serde_json::{json, Value};

use super::inspect::point;
use super::view::{overlays, render_pixels, save_render, size_param, slice_json, window, Layout, NOTE};
use super::Ctx;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::points::{nearest_voxel, parse_plane, voxel_to_plane_mm};

/// Gap between tiles in pixels.
const GAP: u32 = 2;
/// Most tiles in one montage.
pub const MAX_TILES: u64 = 64;
/// Label and crosshair colour.
const MARK: [u8; 3] = [255, 214, 10];

/// 3 × 5 glyphs (rows of 3 bits) for slice numbers and plane letters.
fn glyph(c: char) -> Option<[u8; 5]> {
    Some(match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' | 'S' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 1, 1],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        'A' => [2, 5, 7, 5, 5],
        'C' => [7, 4, 4, 4, 7],
        _ => return None,
    })
}

/// An RGB canvas.
struct Canvas {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self { width, height, rgb: vec![0; width as usize * height as usize * 3] }
    }

    fn set(&mut self, x: u32, y: u32, c: [u8; 3]) {
        if x < self.width && y < self.height {
            let i = (y as usize * self.width as usize + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    fn blit(&mut self, x0: u32, y0: u32, w: u32, h: u32, rgb: &[u8]) {
        for y in 0..h {
            let src = (y * w * 3) as usize;
            let dst = (((y0 + y) * self.width + x0) * 3) as usize;
            self.rgb[dst..dst + (w * 3) as usize].copy_from_slice(&rgb[src..src + (w * 3) as usize]);
        }
    }

    /// One glyph at 2× scale with its top-left corner at `(x0, y0)`.
    fn glyph(&mut self, x0: u32, y0: u32, g: &[u8; 5]) {
        for (row, bits) in g.iter().enumerate() {
            for col in (0..3u32).filter(|c| bits & (4 >> c) != 0) {
                let (px, py) = (x0 + col * 2, y0 + row as u32 * 2);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    self.set(px + dx, py + dy, MARK);
                }
            }
        }
    }

    /// Text at 2× scale on a dark box (no identifiers are ever drawn).
    fn label(&mut self, x0: u32, y0: u32, text: &str) {
        let chars: Vec<[u8; 5]> = text.chars().filter_map(glyph).collect();
        let w = chars.len() as u32 * 8 + 2;
        for y in 0..12 {
            for x in 0..w {
                self.set(x0 + x, y0 + y, [0, 0, 0]);
            }
        }
        for (n, g) in chars.iter().enumerate() {
            self.glyph(x0 + 2 + n as u32 * 8, y0 + 1, g);
        }
    }
}

/// One rendered tile and where it sits.
struct Tile {
    layout: Layout,
    origin: [u32; 2],
}

fn tiles_json(v: &Volume, tiles: &[Tile]) -> Value {
    Value::Array(
        tiles
            .iter()
            .map(|t| {
                let mut j = slice_json(v, &t.layout);
                j["origin"] = json!(t.origin);
                j
            })
            .collect(),
    )
}

/// `view montage`: slices `from`..=`to` (1-based) every `step` of one plane.
pub fn montage(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let max = ctx.config.max_render_px;
    let study = ctx.study(p)?;
    let plane = parse_plane(p.req_str("plane")?)?;
    let count = u64::from(plane.slice_count(&study.volume));
    let from = p.u64("from")?.unwrap_or(1);
    let to = p.u64("to")?.unwrap_or(count);
    if from == 0 || from > to || to > count {
        return Err(AgentError::new(ErrorCode::OutOfVolume, format!("slices {from}..{to} of {count}"))
            .hint(format!("slice numbers run from 1 to {count}")));
    }
    let span = to - from + 1;
    let step = p.u64("step")?.unwrap_or_else(|| span.div_ceil(16)).max(1);
    let indices: Vec<u32> = (from..=to).step_by(step as usize).map(|n| (n - 1) as u32).collect();
    if indices.len() as u64 > MAX_TILES {
        return Err(AgentError::new(ErrorCode::Limit, format!("{} tiles; at most {MAX_TILES}", indices.len()))
            .hint("raise step or narrow from/to"));
    }
    let n = indices.len() as u32;
    let columns =
        p.u64("columns")?.map_or_else(|| (f64::from(n).sqrt().ceil()) as u32, |c| c.clamp(1, u64::from(n)) as u32);
    let rows = n.div_ceil(columns);
    let size = size_param(p, max, 1024)?;
    let tile = ((size - GAP * (columns.max(rows) - 1)) / columns.max(rows)).max(16);
    let w = window(study, p)?;
    let outlines = overlays(p)?.contains(&"segments");
    let layouts: Vec<Layout> = indices.iter().map(|i| Layout::new(&study.volume, plane, *i, tile)).collect();
    let (tw, th) = (layouts[0].width, layouts[0].height);
    let tiles: Vec<Tile> = layouts
        .into_iter()
        .enumerate()
        .map(|(k, layout)| {
            let (c, r) = (k as u32 % columns, k as u32 / columns);
            Tile { layout, origin: [c * (tw + GAP), r * (th + GAP)] }
        })
        .collect();
    let mut canvas = Canvas::new(columns * (tw + GAP) - GAP, rows * (th + GAP) - GAP);
    for t in &tiles {
        let rgb = render_pixels(study, &t.layout, w, outlines);
        canvas.blit(t.origin[0], t.origin[1], tw, th, &rgb);
        canvas.label(t.origin[0] + 2, t.origin[1] + 2, &(t.layout.index + 1).to_string());
    }
    let v = &study.volume;
    let (width, height) = (canvas.width, canvas.height);
    save_render(study, width, height, canvas.rgb, |id| {
        json!({
            "render": id,
            "kind": "montage",
            "plane": plane.label().to_lowercase(),
            "slice_numbers": indices.iter().map(|i| i + 1).collect::<Vec<_>>(),
            "size": [width, height],
            "window": { "center": w.center, "width": w.width },
            "tiles": tiles_json(v, &tiles),
            "note": format!("{NOTE} Each tile is labelled with its slice number; its maps take pixel coordinates relative to the tile origin."),
        })
    })
}

fn window_json(w: WindowLevel) -> Value {
    json!({ "center": w.center, "width": w.width })
}

/// Draws a crosshair through `at` (continuous voxel) on a tile, leaving a
/// gap around the point itself.
fn crosshair(canvas: &mut Canvas, v: &Volume, t: &Tile, at: DVec3) {
    let mm = voxel_to_plane_mm(v, t.layout.plane, at);
    let (cx, cy) = (f64::from(mm.x) / t.layout.pixel_mm, f64::from(mm.y) / t.layout.pixel_mm);
    let (w, h) = (t.layout.width, t.layout.height);
    let gap = 6.0;
    for x in 0..w {
        if (f64::from(x) + 0.5 - cx).abs() > gap {
            canvas.set(t.origin[0] + x, t.origin[1] + cy.clamp(0.0, f64::from(h - 1)) as u32, MARK);
        }
    }
    for y in 0..h {
        if (f64::from(y) + 0.5 - cy).abs() > gap {
            canvas.set(t.origin[0] + cx.clamp(0.0, f64::from(w - 1)) as u32, t.origin[1] + y, MARK);
        }
    }
}

/// `view mpr`: axial, coronal and sagittal slices through a point, side by
/// side, with a crosshair at the point.
pub fn mpr(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let max = ctx.config.max_render_px;
    let study = ctx.study(p)?;
    let at = point(study, p, "at")?;
    let size = size_param(p, max, 1024)?;
    let tile = ((size - 2 * GAP) / 3).max(16);
    let w = window(study, p)?;
    let outlines = overlays(p)?.contains(&"segments");
    let v = &study.volume;
    let idx = nearest_voxel(v, at);
    let mut x = 0;
    let mut tiles = Vec::new();
    for plane in [SliceAxis::Axial, SliceAxis::Coronal, SliceAxis::Sagittal] {
        let layout = Layout::new(v, plane, idx[plane.normal_axis()], tile);
        let width = layout.width;
        tiles.push(Tile { layout, origin: [x, 0] });
        x += width + GAP;
    }
    let height = tiles.iter().map(|t| t.layout.height).max().unwrap_or(16);
    let mut canvas = Canvas::new(x - GAP, height);
    for (t, letter) in tiles.iter().zip(["A", "C", "S"]) {
        let rgb = render_pixels(study, &t.layout, w, outlines);
        canvas.blit(t.origin[0], t.origin[1], t.layout.width, t.layout.height, &rgb);
        crosshair(&mut canvas, v, t, at);
        canvas.label(t.origin[0] + 2, t.origin[1] + 2, &format!("{letter}{}", t.layout.index + 1));
    }
    let point = crate::points::describe(v, at);
    let (width, height) = (canvas.width, canvas.height);
    save_render(study, width, height, canvas.rgb, |id| {
        json!({
            "render": id,
            "kind": "mpr",
            "point": point,
            "size": [width, height],
            "window": window_json(w),
            "tiles": tiles_json(v, &tiles),
            "note": format!("{NOTE} Tiles: axial (A), coronal (C), sagittal (S) with their slice numbers; a crosshair marks the point. Tile maps take pixel coordinates relative to the tile origin."),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_and_labels() {
        assert!("0123456789ACS".chars().all(|c| glyph(c).is_some()));
        assert!(glyph('x').is_none());
        let mut c = Canvas::new(40, 20);
        c.label(1, 1, "A12");
        assert!(c.rgb.chunks(3).any(|p| p == MARK));
        c.set(99, 99, MARK); // outside: ignored
    }
}
