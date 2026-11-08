//! Frame sources of the local view host.
//!
//! A frame source draws one slice of a [`Viewer`] at the volume's native
//! resolution: one pixel per voxel, no letterbox and nearest sampling, so
//! that the pixels of a frame and the voxels of a pick map one to one and
//! the GPU and CPU backends can be compared exactly
//! ([`docs/view-host.md`](../../../docs/view-host.md)).

use ferrum_app::Viewer;
use ferrum_domain::{SliceAxis, WindowLevel};
use glam::{UVec2, Vec2};
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

/// A backend that draws a slice.
///
/// `viewer` carries the volume, the slice index, the window and the segment
/// overlay; the host keeps it in step with [`crate::ViewState`] before it
/// asks for a frame, so a source reads one state and no request.
pub trait FrameSource: Send {
    /// Which backend this is.
    fn backend(&self) -> Backend;

    /// Draws the slice of `axis` the viewer currently shows.
    fn render(&mut self, viewer: &Viewer, axis: SliceAxis) -> Result<RawFrame, ViewError>;
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
