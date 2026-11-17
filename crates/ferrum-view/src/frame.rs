//! Frame sources of the local view host.
//!
//! A frame source draws the component of a [`Viewer`] the state names
//! (Stage 29): `slice` is one slice at the volume's native resolution — one
//! pixel per voxel, no letterbox and nearest sampling, so that the pixels of
//! a frame and the voxels of a pick map one to one; `volume` is the 3D view
//! and `mpr` the four-quarter grid, both at a fixed output size. The GPU and
//! CPU backends draw each part with the same renderer and tile the MPR grid
//! with the one compositor below, so they stay comparable pixel by pixel
//! ([`docs/view-host.md`](../../../docs/view-host.md)).

use ferrum_app::Viewer;
use ferrum_domain::{SliceAxis, ViewComponent, ViewProbe, WindowLevel};
use glam::{UVec2, UVec3, Vec2};
use serde_json::{json, Value};

use crate::session::ViewError;

/// Which backend drew a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The wgpu renderer ([`crate::GpuFrameSource`]).
    Gpu,
    /// The CPU reference path ([`crate::CpuFrameSource`]).
    Cpu,
}

impl Backend {
    /// `"gpu"` or `"cpu"`, as the host reports it.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Gpu => "gpu",
            Backend::Cpu => "cpu",
        }
    }
}

/// What a host may ask for when it starts: a GPU, the CPU reference, or
/// whichever works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendPreference {
    /// Use the GPU when an adapter exists, otherwise the CPU.
    Auto,
    /// Require the GPU; failing to get one is an error.
    Gpu,
    /// Use the CPU reference path.
    Cpu,
}

/// One slice drawn at native resolution: RGBA8, top row first.
#[derive(Debug, Clone)]
pub struct RawFrame {
    /// Width in pixels (the plane's width in voxels).
    pub width: u32,
    /// Height in pixels (the plane's height in voxels).
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// The window the frame was drawn with.
    pub window: WindowLevel,
    /// Where the slice's pixels lie in the volume, as the sidecar of a
    /// render gives it.
    pub mapping: Value,
}

impl RawFrame {
    /// The slice's RGBA pixels as a PNG.
    pub fn png(&self) -> Result<Vec<u8>, ViewError> {
        let image = image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
            .ok_or_else(|| ViewError::Internal("frame size does not match its pixels".into()))?;
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).map_err(|e| ViewError::Internal(e.to_string()))?;
        Ok(out.into_inner())
    }
}

/// A backend that draws the view components.
///
/// `viewer` carries the whole view state — the component, the slice indices,
/// the window, the camera, the overlay — and the source reads it as one
/// state, no request. The GPU backend pushes the viewer's data through
/// [`Viewer::sync_gpu`] first, so the trait takes the viewer mutably.
pub trait FrameSource: Send {
    /// Which backend this is.
    fn backend(&self) -> Backend;

    /// Draws the frame of `component` as the viewer currently shows it.
    fn render(&mut self, viewer: &mut Viewer, component: ViewComponent) -> Result<RawFrame, ViewError>;
}

/// The parameters every backend draws with: the slice's size in pixels, the
/// window in normalised intensity, the slice position and the overlay.
pub(crate) struct SliceSetup {
    pub(crate) size: UVec2,
    pub(crate) params: ferrum_render::SliceParams,
}

/// Reads the viewer's slice state as the single set of parameters both
/// backends use: the full native rectangle, nearest sampling and an opaque
/// background, so a frame is exactly one pixel per voxel.
pub(crate) fn slice_setup(viewer: &Viewer, axis: SliceAxis) -> Result<SliceSetup, ViewError> {
    let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
    let (width, height) = axis.plane_dims(&dataset.volume);
    let size = UVec2::new(width.max(1), height.max(1));
    let full = (Vec2::ZERO, size.as_vec2());
    let mut params = viewer
        .slice_params(axis, full, 1.0)
        .ok_or_else(|| ViewError::Internal("the viewer has no slice parameters".into()))?;
    params.rect = full;
    params.nearest = true;
    params.background = [0.0, 0.0, 0.0, 1.0];
    Ok(SliceSetup { size, params })
}

/// The mapping block of a frame: pixel to voxel and to patient millimetres,
/// shared by both backends and by the state document.
pub(crate) fn mapping_of(viewer: &Viewer, axis: SliceAxis, size: UVec2) -> Result<Value, ViewError> {
    let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
    let index = viewer.slice_index(axis);
    let plane_mm = axis.plane_size_mm(&dataset.volume).as_dvec2();
    let pixel_mm = (plane_mm.x / f64::from(size.x)).max(f64::from(f32::EPSILON));
    let dims = dataset.volume.dims().as_uvec3().as_dvec3();
    let slice_position = f64::from(axis.slice_position(&dataset.volume, index));
    // Pixel (x, y) covers in-plane millimetres x * pixel_mm, y * pixel_mm;
    // the slice shader samples `axis.tex_coord(uv, position)`, so a pixel
    // centre maps to the same voxel `SliceAxis::voxel_at` gives.
    let uv = |x: f64, y: f64| Vec2::new((x / f64::from(size.x)) as f32, (y / f64::from(size.y)) as f32);
    let voxel =
        |x: f64, y: f64| axis.tex_coord(uv(x, y), slice_position as f32).as_dvec3() * dims - glam::DVec3::splat(0.5);
    let (origin, ex, ey) = (voxel(0.0, 0.0), voxel(1.0, 0.0), voxel(0.0, 1.0));
    let to_voxel = affine(origin, ex - origin, ey - origin);
    let patient = |x: f64, y: f64| dataset.volume.voxel_to_patient_f64(voxel(x, y));
    let (p0, px, py) = (patient(0.0, 0.0), patient(1.0, 0.0), patient(0.0, 1.0));
    let to_patient = affine(p0, px - p0, py - p0);
    let [left, right, top, bottom] = axis.edge_labels();
    Ok(json!({
        "plane": axis.label().to_lowercase(),
        "slice_index": index,
        "size": [size.x, size.y],
        "pixel_mm": round4(pixel_mm),
        "pixel_to_voxel": round_affine(to_voxel),
        "pixel_to_patient_mm": round_affine(to_patient),
        "orientation": { "left": left, "right": right, "top": top, "bottom": bottom },
    }))
}

/// Fixed output side of the 3D frame: the CPU reference's time grows with
/// the pixel count, so the host draws the 3D view at one agreed size (the
/// agent's `view volume` default), not at the volume's resolution.
pub(crate) const VOLUME_SIZE: u32 = 512;

/// Fixed output side of the MPR composite: four quarters of half that side.
pub(crate) const MPR_SIZE: u32 = 512;

/// One quarter of the MPR grid: a slice of one plane, or the 3D view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Quarter {
    /// The slice of this plane at the viewer's current index.
    Slice(SliceAxis),
    /// The 3D view with the viewer's camera.
    Volume,
}

/// The four quarters of the MPR frame as `(kind, rect)` in composite pixels,
/// mirroring the desktop's grid (the `ferrum-ui` widget): axial top-left,
/// 3D top-right, coronal bottom-left, sagittal bottom-right. The rects tile
/// the frame exactly.
pub(crate) fn mpr_quarters() -> [(Quarter, (UVec2, UVec2)); 4] {
    let h = MPR_SIZE / 2;
    [
        (Quarter::Slice(SliceAxis::Axial), (UVec2::ZERO, UVec2::new(h, h))),
        (Quarter::Volume, (UVec2::new(h, 0), UVec2::new(MPR_SIZE, h))),
        (Quarter::Slice(SliceAxis::Coronal), (UVec2::new(0, h), UVec2::new(h, MPR_SIZE))),
        (Quarter::Slice(SliceAxis::Sagittal), (UVec2::new(h, h), UVec2::splat(MPR_SIZE))),
    ]
}

/// The largest box inside `rect`, centred, with the pixel shape of a plane
/// `plane` wide and high, at an integer scale (the composite blits nearest,
/// so a voxel stays a whole number of pixels). A plane larger than the rect
/// takes scale 1 and is cropped by the blit, never stretched.
pub(crate) fn fit_rect(rect: (UVec2, UVec2), plane: (u32, u32)) -> (UVec2, UVec2) {
    let (bw, bh) = (rect.1.x - rect.0.x, rect.1.y - rect.0.y);
    let (pw, ph) = (plane.0.max(1), plane.1.max(1));
    let scale = (bw / pw).min(bh / ph).max(1);
    let (w, h) = (pw * scale, ph * scale);
    let origin = rect.0 + UVec2::new((bw.saturating_sub(w)) / 2, (bh.saturating_sub(h)) / 2);
    (origin, origin + UVec2::new(w.min(bw), h.min(bh)))
}

/// The probe of a 3D frame pixel: [`Viewer::pick`] resolves it with the same
/// visibility model the frame was drawn with. The pick lands on the rendered
/// surface, which trilinear sampling places between voxels (often in the
/// empty cell in front of the material); the probe names the voxel one step
/// behind it along the ray, so a click on a structure reads that structure's
/// value. `None`-hit is a [`ViewError::BadRequest`].
pub(crate) fn probe_on_volume(viewer: &Viewer, ndc: Vec2, aspect: f32) -> Result<ViewProbe, ViewError> {
    let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
    let hit = viewer
        .pick(ndc, aspect)
        .ok_or_else(|| ViewError::BadRequest("the probe hits nothing in the 3D view".into()))?;
    let dims = dataset.volume.dims().as_vec3();
    let dir_tex = (viewer.volume.camera.view_dir() / dataset.volume.model_extent() * dims).normalize_or_zero();
    let t = hit + dir_tex / dims;
    let v = (t * dims).floor();
    let axis = |c: f32, n: u32| (c as i64).clamp(0, i64::from(n) - 1) as u32;
    let voxel = UVec3::new(
        axis(v.x, dataset.volume.dims().x),
        axis(v.y, dataset.volume.dims().y),
        axis(v.z, dataset.volume.dims().z),
    );
    crate::state::probe_of_voxel(viewer, voxel)
}

/// The mapping of a 3D frame: a projection (camera + viewport), not the 2D
/// affine. A client resolves any pixel `(x, y)` to a ray with the camera's
/// `ray(ndc, aspect)` at `ndc = (2(x+0.5)/w − 1, 1 − 2(y+0.5)/h)` and
/// `aspect = w/h`; `pick_ray` is that ray through the frame's centre and
/// `pick` the voxel it hits (the same visibility model the frame was drawn
/// with), so a client has an anchor without doing the math.
pub(crate) fn projection_mapping(viewer: &Viewer, size: UVec2) -> Result<Value, ViewError> {
    let camera = viewer.volume.camera;
    let aspect = size.x as f32 / size.y.max(1) as f32;
    let ray = camera.ray(Vec2::ZERO, aspect);
    let pick = probe_on_volume(viewer, Vec2::ZERO, aspect).ok().map(|p| probe_json(&p));
    Ok(json!({
        "kind": "projection",
        "size": [size.x, size.y],
        "camera": ferrum_io::view_state::camera_to_json(&camera),
        "pick_ray": { "origin": ray.origin.to_array(), "direction": ray.direction.to_array() },
        "pick": pick,
    }))
}

fn probe_json(probe: &ViewProbe) -> Value {
    json!({
        "voxel": probe.voxel,
        "patient_mm": probe.patient_mm,
        "value": probe.value,
        "unit": probe.unit,
    })
}

/// Draws the MPR grid: each quarter through `draw` (the backend's own slice
/// or 3D render), tiled into one composite frame by the one blit below. The
/// layout mirrors the desktop's grid; a slice keeps its plane's aspect
/// (letterboxed, never stretched) and the 3D quarter fills its rect.
pub(crate) fn composite_mpr(
    viewer: &Viewer,
    mut draw: impl FnMut(Quarter, UVec2) -> Result<RawFrame, ViewError>,
) -> Result<RawFrame, ViewError> {
    let size = UVec2::splat(MPR_SIZE);
    let mut rgba = vec![0u8; (size.x * size.y * 4) as usize];
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    let mut quarters = Vec::new();
    for (quarter, rect) in mpr_quarters() {
        let half = rect.1 - rect.0;
        let sub = draw(quarter, half)?;
        let entry = match quarter {
            Quarter::Slice(axis) => {
                let image = fit_rect(rect, (sub.width, sub.height));
                blit(&mut rgba, size, &sub, image);
                json!({
                    "component": "slice",
                    "plane": crate::state::plane_name(axis),
                    "rect": rect_json(rect),
                    "image_rect": rect_json(image),
                    "mapping": sub.mapping,
                })
            }
            Quarter::Volume => {
                blit(&mut rgba, size, &sub, rect);
                json!({ "component": "volume", "rect": rect_json(rect), "mapping": sub.mapping })
            }
        };
        quarters.push(entry);
    }
    Ok(RawFrame {
        width: size.x,
        height: size.y,
        rgba,
        window: viewer.window(),
        mapping: json!({ "kind": "mpr", "size": [size.x, size.y], "quarters": quarters }),
    })
}

fn rect_json(rect: (UVec2, UVec2)) -> Value {
    json!([rect.0.x, rect.0.y, rect.1.x, rect.1.y])
}

/// Copies `src` into `dst` at `at`, nearest (each source pixel becomes a
/// whole block at the integer scales [`fit_rect`] chooses). This is a
/// display blit, not image formation: both backends tile their MPR frames
/// with this one rule, after drawing each quarter with the shared slice and
/// volume renderers.
fn blit(dst: &mut [u8], dst_size: UVec2, src: &RawFrame, at: (UVec2, UVec2)) {
    let (w, h) = (at.1.x - at.0.x, at.1.y - at.0.y);
    if w == 0 || h == 0 || src.width == 0 || src.height == 0 {
        return;
    }
    for y in 0..h {
        let sy = y * src.height / h;
        for x in 0..w {
            let sx = x * src.width / w;
            let s = ((sy * src.width + sx) * 4) as usize;
            let d = (((at.0.y + y) * dst_size.x + at.0.x + x) * 4) as usize;
            dst[d..d + 4].copy_from_slice(&src.rgba[s..s + 4]);
        }
    }
}

/// Rows of the affine map `(x, y, 1) -> origin + x·ex + y·ey`.
fn affine(origin: glam::DVec3, ex: glam::DVec3, ey: glam::DVec3) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for (row, out) in m.iter_mut().enumerate() {
        *out = [ex[row], ey[row], origin[row]];
    }
    m
}

fn round4(value: f64) -> f64 {
    (value * 1e4).round() / 1e4
}

fn round_affine(m: [[f64; 3]; 3]) -> Value {
    json!(m.map(|row| row.map(round4)))
}
