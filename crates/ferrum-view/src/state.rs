//! The state document of `ferrum-view/1`, the input a host may send and the
//! events it reads back.
//!
//! Stage 29: the state is the full domain [`ViewState`] — component, camera,
//! overlays, selected segment and tool included — read live from the
//! [`Viewer`] and serialised through the one JSON contract of `ferrum-io`
//! (ADR 0018). Every input becomes a partial state applied by
//! [`Viewer::apply_view_state`], so the host honours everything the contract
//! carries and the events are the domain's own.
//!
//! A state holds numbers and labels only: no path, UID, patient name or
//! value of the DICOM header ever appears here (criterion 25.5-a).

use ferrum_app::{InputKind, ProbeReading, ToolOutcome, Viewer};
use ferrum_domain::{
    RoiItem, SegmentStyle, SliceAxis, ViewComponent, ViewEvent, ViewProbe, ViewTool, WindowLevel, WindowPreset,
};
use ferrum_io::{event_to_json, state_to_json};
use glam::{UVec3, Vec2, Vec3};
use serde_json::{json, Value};

use crate::frame::{fit_rect, mpr_quarters, probe_on_volume, Quarter, MPR_SIZE, VOLUME_SIZE};
use crate::session::ViewError;

/// The full view state the host serves and applies (Stage 29): the domain
/// contract itself, not a 2D projection of it.
pub use ferrum_domain::ViewState;

/// The format string every state document carries.
pub const FORMAT: &str = "ferrum-view/1";

/// One camera operation: orbit by radians, zoom by an amount (positive zooms
/// in), or move the orbit target to a model-space point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CameraOp {
    /// Orbit by `dx`, `dy` radians around the camera's up and right axes.
    Orbit {
        /// Horizontal angle in radians.
        dx: f32,
        /// Vertical angle in radians.
        dy: f32,
    },
    /// Multiply the distance by `exp(-amount)` (positive zooms in).
    Zoom(f32),
    /// Move the orbit target to this model-space point.
    Target([f32; 3]),
}

/// A partial overlay change: only the given fields are set.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OverlayChange {
    /// Show or hide the overlay, if given.
    pub segments_shown: Option<bool>,
    /// The display's fill opacity, if given.
    pub fill_opacity: Option<f32>,
    /// The overlay style, if given.
    pub style: Option<SegmentStyle>,
}

/// One request from the person, as `POST /api/input` carries it. Exactly one
/// variant per request; the answer is the new state document.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Show slice `n` (0-based, clamped).
    Slice(u32),
    /// Move the slice by `n` (one wheel notch is ±1).
    Step(i32),
    /// Show another plane.
    Plane(SliceAxis),
    /// Set the window.
    Window {
        /// Window centre.
        center: f32,
        /// Window width (> 0).
        width: f32,
    },
    /// Apply a window preset by name.
    Preset(WindowPreset),
    /// Show or hide the segment overlay.
    Segments(bool),
    /// Read the voxel under a frame pixel of the current component.
    Probe {
        /// Pixel column of the frame.
        x: u32,
        /// Pixel row of the frame.
        y: u32,
    },
    /// Show another component (`slice`, `mpr` or `volume`).
    Component(ViewComponent),
    /// Move the 3D camera (orbit, zoom or target).
    Camera(CameraOp),
    /// Change the overlay's fill opacity and/or style.
    Overlays(OverlayChange),
    /// Select a segment by label (`None` clears the selection).
    SelectedSegment(Option<u8>),
    /// Activate a tool by its wire name.
    Tool(ViewTool),
    /// Show/hide one segment or set its opacity.
    Segment {
        /// The segment's label.
        label: u8,
        /// Its visibility, if given.
        visible: Option<bool>,
        /// Its opacity, if given.
        opacity: Option<f32>,
    },
    /// Draw an annotation with a measurement tool on the active plane's
    /// slice; a commit records `roi_drawn`. Points are pixels of that
    /// plane's native-resolution slice.
    Draw {
        /// The measurement tool (`distance`, `rect`, `angle` or `area`).
        tool: ViewTool,
        /// The clicked pixels, in order.
        points: Vec<(u32, u32)>,
    },
}

impl Input {
    /// Parses one input object: exactly one of `slice`, `step`, `plane`,
    /// `window`, `preset`, `segments`, `probe`, `component`, `camera`,
    /// `overlays`, `selected_segment`, `tool`, `segment`, `draw`.
    pub fn parse(body: &[u8]) -> Result<Self, ViewError> {
        let value: Value = serde_json::from_slice(body).map_err(|e| ViewError::BadRequest(format!("input: {e}")))?;
        let object = value.as_object().ok_or_else(|| ViewError::BadRequest("input: an object".into()))?;
        parse_input(object)
    }

    /// Applies the input to `viewer` through its use cases and returns the
    /// events the change produced (the domain's own; a field equal to the
    /// current one produces none).
    pub fn apply(self, viewer: &mut Viewer) -> Result<Vec<ViewEvent>, ViewError> {
        match self {
            Input::Slice(index) => Ok(apply_state(viewer, |t| t.slice_index = index)),
            Input::Step(delta) => Ok(apply_state(viewer, |t| {
                t.slice_index = (i64::from(t.slice_index) + i64::from(delta)).max(0) as u32;
            })),
            Input::Plane(axis) => {
                let index = viewer.slice_index(axis);
                Ok(apply_state(viewer, move |t| {
                    t.plane = axis;
                    t.slice_index = index;
                }))
            }
            Input::Window { center, width } => Ok(apply_state(viewer, |t| t.window = WindowLevel::new(center, width))),
            Input::Preset(preset) => {
                viewer.apply_window_preset(preset);
                let window = viewer.window();
                Ok(vec![ViewEvent::WindowChanged { center: window.center, width: window.width }])
            }
            Input::Segments(show) => Ok(apply_state(viewer, |t| t.overlays.segments_shown = show)),
            Input::Component(component) => Ok(apply_state(viewer, |t| t.component = component)),
            Input::Camera(op) => {
                let mut camera = viewer.volume.camera;
                match op {
                    CameraOp::Orbit { dx, dy } => camera.rotate(dx, dy),
                    CameraOp::Zoom(amount) => camera.zoom(amount),
                    CameraOp::Target(target) => camera.target = Vec3::from_array(target),
                }
                Ok(apply_state(viewer, move |t| t.camera = Some(camera)))
            }
            Input::Overlays(change) => Ok(apply_state(viewer, |t| {
                if let Some(shown) = change.segments_shown {
                    t.overlays.segments_shown = shown;
                }
                if let Some(opacity) = change.fill_opacity {
                    t.overlays.fill_opacity = opacity;
                }
                if let Some(style) = change.style {
                    t.overlays.style = style;
                }
            })),
            Input::SelectedSegment(label) => Ok(apply_state(viewer, |t| t.selected_segment = label)),
            Input::Tool(tool) => Ok(apply_state(viewer, |t| t.tool = tool)),
            Input::Segment { label, visible, opacity } => {
                let bad = |e: ferrum_domain::SegmentationError| ViewError::BadRequest(e.to_string());
                if let Some(visible) = visible {
                    viewer.set_segment_visible(label, visible).map_err(bad)?;
                }
                if let Some(opacity) = opacity {
                    viewer.set_segment_opacity(label, opacity).map_err(bad)?;
                }
                Ok(vec![ViewEvent::SegmentsChanged { shown: viewer.segmentation().show }])
            }
            Input::Probe { x, y } => {
                let probe = probe_input(viewer, x, y)?;
                viewer.probe = Some(ProbeReading { voxel: UVec3::from_array(probe.voxel), value: probe.value as f32 });
                Ok(vec![ViewEvent::PointPicked { probe }])
            }
            Input::Draw { tool, points } => draw_input(viewer, tool, &points),
        }
    }
}

/// Applies a partial change through the full applier: the target starts from
/// the viewer's current state, so untouched fields stay as they are.
fn apply_state(viewer: &mut Viewer, change: impl FnOnce(&mut ViewState)) -> Vec<ViewEvent> {
    let mut target = viewer.view_state();
    change(&mut target);
    viewer.apply_view_state(&target)
}

fn parse_input(object: &serde_json::Map<String, Value>) -> Result<Input, ViewError> {
    if object.len() != 1 {
        return Err(ViewError::BadRequest(
            "input: one of slice, step, plane, window, preset, segments, probe, component, camera, overlays, \
             selected_segment, tool, segment, draw"
                .into(),
        ));
    }
    let (key, value) = object.iter().next().ok_or_else(|| ViewError::BadRequest("input: empty".into()))?;
    let bad = |what: &str| ViewError::BadRequest(format!("input {key:?}: {what}"));
    match key.as_str() {
        "slice" => {
            let n = value.as_u64().ok_or_else(|| bad("a non-negative integer"))?;
            Ok(Input::Slice(n.min(u64::from(u32::MAX)) as u32))
        }
        "step" => {
            let step = value.as_i64().ok_or_else(|| bad("an integer"))?;
            Ok(Input::Step(step.clamp(-4096, 4096) as i32))
        }
        "plane" => {
            let name = value.as_str().ok_or_else(|| bad("a string"))?;
            parse_plane(name).map(Input::Plane).ok_or_else(|| bad("axial, coronal or sagittal"))
        }
        "window" => parse_window(value).map(|(center, width)| Input::Window { center, width }),
        "preset" => {
            let name = value.as_str().ok_or_else(|| bad("a string"))?;
            WindowPreset::ALL
                .iter()
                .find(|p| same_name(p.label(), name))
                .copied()
                .map(Input::Preset)
                .ok_or_else(|| bad(&format!("unknown preset {name:?}")))
        }
        "segments" => value.as_bool().map(Input::Segments).ok_or_else(|| bad("true or false")),
        "probe" => {
            let (x, y) = parse_pixel(value)?;
            Ok(Input::Probe { x, y })
        }
        "component" => {
            let name = value.as_str().ok_or_else(|| bad("a string"))?;
            ViewComponent::parse(name).map(Input::Component).ok_or_else(|| bad("slice, mpr or volume"))
        }
        "camera" => parse_camera(value).map(Input::Camera),
        "overlays" => parse_overlays(value).map(Input::Overlays),
        "selected_segment" => match value {
            Value::Null => Ok(Input::SelectedSegment(None)),
            _ => {
                let n = value.as_u64().ok_or_else(|| bad("a label or null"))?;
                let label = u8::try_from(n).map_err(|_| bad("a label in 0..=255"))?;
                Ok(Input::SelectedSegment(Some(label)))
            }
        },
        "tool" => {
            let name = value.as_str().ok_or_else(|| bad("a string"))?;
            ViewTool::parse(name).map(Input::Tool).ok_or_else(|| bad(&format!("unknown tool {name:?}")))
        }
        "segment" => parse_segment(value).map(|(label, visible, opacity)| Input::Segment { label, visible, opacity }),
        "draw" => parse_draw(value),
        other => Err(ViewError::BadRequest(format!("input: unknown field {other:?}"))),
    }
}

fn parse_window(value: &Value) -> Result<(f32, f32), ViewError> {
    let w = value
        .as_object()
        .ok_or_else(|| ViewError::BadRequest("input \"window\": an object with center and width".into()))?;
    let center = w
        .get("center")
        .and_then(Value::as_f64)
        .ok_or_else(|| ViewError::BadRequest("input \"window\": center".into()))?;
    let width = w
        .get("width")
        .and_then(Value::as_f64)
        .ok_or_else(|| ViewError::BadRequest("input \"window\": width".into()))?;
    if !center.is_finite() || !width.is_finite() || width <= 0.0 {
        return Err(ViewError::BadRequest("input \"window\": a finite centre and a width above 0".into()));
    }
    Ok((center as f32, width as f32))
}

/// One frame pixel: an object with non-negative integer `x` and `y`.
fn parse_pixel(value: &Value) -> Result<(u32, u32), ViewError> {
    let p = value.as_object().ok_or_else(|| ViewError::BadRequest("a pixel: an object with x and y".into()))?;
    let x = p.get("x").and_then(Value::as_u64).ok_or_else(|| ViewError::BadRequest("pixel: x".into()))?;
    let y = p.get("y").and_then(Value::as_u64).ok_or_else(|| ViewError::BadRequest("pixel: y".into()))?;
    Ok((x.min(u64::from(u32::MAX)) as u32, y.min(u64::from(u32::MAX)) as u32))
}

fn finite_f32(value: &Value, what: &str) -> Result<f32, ViewError> {
    let f = value.as_f64().ok_or_else(|| ViewError::BadRequest(format!("{what}: a number")))?;
    if !f.is_finite() {
        return Err(ViewError::BadRequest(format!("{what}: finite")));
    }
    Ok(f as f32)
}

fn parse_camera(value: &Value) -> Result<CameraOp, ViewError> {
    let c = value.as_object().ok_or_else(|| ViewError::BadRequest("input \"camera\": an object".into()))?;
    if c.len() != 1 {
        return Err(ViewError::BadRequest("input \"camera\": exactly one of orbit, zoom, target".into()));
    }
    let (key, v) = c.iter().next().ok_or_else(|| ViewError::BadRequest("input \"camera\": empty".into()))?;
    let bad = |what: &str| ViewError::BadRequest(format!("input \"camera\" {key:?}: {what}"));
    match key.as_str() {
        "orbit" => {
            let pair = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad("[dx, dy] in radians"))?;
            let dx = finite_f32(&pair[0], "orbit dx")?.clamp(-std::f32::consts::TAU, std::f32::consts::TAU);
            let dy = finite_f32(&pair[1], "orbit dy")?.clamp(-std::f32::consts::TAU, std::f32::consts::TAU);
            Ok(CameraOp::Orbit { dx, dy })
        }
        "zoom" => Ok(CameraOp::Zoom(finite_f32(v, "zoom")?.clamp(-10.0, 10.0))),
        "target" => {
            let t = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| bad("[x, y, z] in model space"))?;
            let mut target = [0.0f32; 3];
            for (out, item) in target.iter_mut().zip(t) {
                let f = finite_f32(item, "target")?;
                if f.abs() > 1e6 {
                    return Err(bad("a target of sane magnitude"));
                }
                *out = f;
            }
            Ok(CameraOp::Target(target))
        }
        other => Err(ViewError::BadRequest(format!("input \"camera\": unknown operation {other:?}"))),
    }
}

fn parse_overlays(value: &Value) -> Result<OverlayChange, ViewError> {
    let o = value.as_object().ok_or_else(|| ViewError::BadRequest("input \"overlays\": an object".into()))?;
    if o.is_empty() {
        return Err(ViewError::BadRequest("input \"overlays\": one of segments_shown, fill_opacity, style".into()));
    }
    let mut change = OverlayChange::default();
    for (key, v) in o {
        match key.as_str() {
            "segments_shown" => {
                change.segments_shown = Some(
                    v.as_bool()
                        .ok_or_else(|| ViewError::BadRequest("overlays: segments_shown is true or false".into()))?,
                );
            }
            "fill_opacity" => {
                let f = finite_f32(v, "overlays fill_opacity")?;
                if !(0.0..=1.0).contains(&f) {
                    return Err(ViewError::BadRequest("overlays: fill_opacity in 0..=1".into()));
                }
                change.fill_opacity = Some(f);
            }
            "style" => {
                let name = v.as_str().ok_or_else(|| ViewError::BadRequest("overlays: style is a string".into()))?;
                change.style = Some(
                    SegmentStyle::parse(name)
                        .ok_or_else(|| ViewError::BadRequest(format!("overlays: unknown style {name:?}")))?,
                );
            }
            other => return Err(ViewError::BadRequest(format!("overlays: unknown field {other:?}"))),
        }
    }
    Ok(change)
}

fn parse_segment(value: &Value) -> Result<(u8, Option<bool>, Option<f32>), ViewError> {
    let s = value.as_object().ok_or_else(|| ViewError::BadRequest("input \"segment\": an object".into()))?;
    let label = s
        .get("label")
        .and_then(Value::as_u64)
        .ok_or_else(|| ViewError::BadRequest("segment: label".into()))
        .and_then(|n| u8::try_from(n).map_err(|_| ViewError::BadRequest("segment: a label in 0..=255".into())))?;
    let mut visible = None;
    let mut opacity = None;
    for (key, v) in s {
        match key.as_str() {
            "label" => {}
            "visible" => {
                visible =
                    Some(v.as_bool().ok_or_else(|| ViewError::BadRequest("segment: visible is true or false".into()))?);
            }
            "opacity" => {
                let f = finite_f32(v, "segment opacity")?;
                if !(0.0..=1.0).contains(&f) {
                    return Err(ViewError::BadRequest("segment: opacity in 0..=1".into()));
                }
                opacity = Some(f);
            }
            other => return Err(ViewError::BadRequest(format!("segment: unknown field {other:?}"))),
        }
    }
    if visible.is_none() && opacity.is_none() {
        return Err(ViewError::BadRequest("segment: one of visible, opacity".into()));
    }
    Ok((label, visible, opacity))
}

fn parse_draw(value: &Value) -> Result<Input, ViewError> {
    let d = value.as_object().ok_or_else(|| ViewError::BadRequest("input \"draw\": an object".into()))?;
    let name = d.get("tool").and_then(Value::as_str).ok_or_else(|| ViewError::BadRequest("draw: tool".into()))?;
    let tool = ViewTool::parse(name).ok_or_else(|| ViewError::BadRequest(format!("draw: unknown tool {name:?}")))?;
    let needed = match tool {
        ViewTool::Distance | ViewTool::Rect => 2,
        ViewTool::Angle => 3,
        ViewTool::Area => 3,
        other => {
            return Err(ViewError::BadRequest(format!(
                "draw: {} draws no annotation; use distance, rect, angle or area",
                other.as_str()
            )))
        }
    };
    let points = d
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| ViewError::BadRequest("draw: points is an array of [x, y] pixels".into()))?;
    if points.len() < needed || points.len() > 1024 {
        return Err(ViewError::BadRequest(format!("draw: {} needs {needed}..1024 points", tool.as_str())));
    }
    let mut parsed = Vec::with_capacity(points.len());
    for point in points {
        let pair = point
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or_else(|| ViewError::BadRequest("draw: a point is [x, y]".into()))?;
        let x = pair[0].as_u64().ok_or_else(|| ViewError::BadRequest("draw: x is a non-negative integer".into()))?;
        let y = pair[1].as_u64().ok_or_else(|| ViewError::BadRequest("draw: y is a non-negative integer".into()))?;
        parsed.push((x.min(u64::from(u32::MAX)) as u32, y.min(u64::from(u32::MAX)) as u32));
    }
    Ok(Input::Draw { tool, points: parsed })
}

fn same_name(label: &str, given: &str) -> bool {
    label.eq_ignore_ascii_case(given) || label.replace(' ', "_").eq_ignore_ascii_case(given)
}

/// The plane of `name` (case-insensitive, also the plural).
pub fn parse_plane(name: &str) -> Option<SliceAxis> {
    SliceAxis::ALL.into_iter().find(|axis| same_name(axis.label(), name))
}

/// The lower-case name of a plane, as every document of this host writes it.
pub fn plane_name(axis: SliceAxis) -> &'static str {
    match axis {
        SliceAxis::Axial => "axial",
        SliceAxis::Coronal => "coronal",
        SliceAxis::Sagittal => "sagittal",
    }
}

/// The unit of a value: HU for a CT, raw values otherwise (MR, PET without a
/// unit, an image with no declared modality).
pub fn value_unit(viewer: &Viewer) -> &'static str {
    ferrum_app::value_unit(viewer)
}

/// The state document (`ferrum-view/1`): the full domain view state read
/// live from the viewer, through the one JSON contract of `ferrum-io`, plus
/// the host's revision, slice count, segment list and volume description.
pub fn state_json(viewer: &Viewer, rev: u64) -> Result<Value, ViewError> {
    let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
    let volume = &dataset.volume;
    let view = viewer.view_state();
    let dims = volume.dims().as_uvec3();
    let spacing = volume.spacing();
    let range = volume.range();
    let mut doc = state_to_json(&view);
    doc["format"] = json!(FORMAT);
    doc["rev"] = json!(rev);
    doc["slice_count"] = json!(view.plane.slice_count(volume));
    doc["segments"] = json!({ "shown": view.overlays.segments_shown, "items": segment_items(viewer) });
    doc["volume"] = json!({
        "dims": [dims.x, dims.y, dims.z],
        "spacing_mm": [spacing.x, spacing.y, spacing.z],
        "value_unit": value_unit(viewer),
        "value_range": [range.min, range.max],
    });
    Ok(doc)
}

fn segment_items(viewer: &Viewer) -> Vec<Value> {
    viewer
        .segmentation()
        .set()
        .map(|s| {
            s.segments()
                .iter()
                .map(|g| {
                    json!({
                        "label": g.label,
                        "name": g.name,
                        "color": g.color,
                        "visible": g.visible,
                        "opacity": g.opacity,
                        "voxels": s.voxel_count(g.label),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The events after `since`, as the `ferrum-view/1` event document.
pub fn events_json(events: &[(u64, ViewEvent)], seq: u64, dropped: u64, since: Option<u64>) -> Value {
    let since = since.unwrap_or(0);
    let list: Vec<Value> = events.iter().filter(|(s, _)| *s > since).map(|(s, e)| event_to_json(*s, e)).collect();
    json!({ "format": FORMAT, "events": list, "next": seq, "dropped": dropped })
}

/// The annotations document (`ferrum-view/1`): the measurements the viewer
/// holds, read live from [`Viewer::annotations`] — the host's tool machine
/// commits them (the `draw` input) and a panel displays their values
/// (Stage 29, criterion 29.2-c). The value, unit and label are the domain's
/// own ([`ferrum_domain::Annotation`]); the points are in-plane millimetres,
/// the same frame the `draw` input takes pixels for. Numbers and labels only:
/// no path, UID, patient name or header value appears here (25.5-a).
pub fn annotations_json(viewer: &Viewer) -> Value {
    let items: Vec<Value> = viewer
        .annotations()
        .iter()
        .map(|(id, key, annotation, name)| {
            json!({
                "id": id,
                "name": name,
                "kind": annotation.kind(),
                "plane": key.slice_axis().map(plane_name),
                "slice_index": key.index,
                "value": annotation.value(),
                "unit": annotation.unit(),
                "label": annotation.label(),
                "points_mm": annotation.points().iter().map(|p| [p.x, p.y]).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "format": FORMAT, "annotations": items })
}

/// Reads the voxel under a frame pixel of the current component: the 2D
/// affine on a slice, the camera projection in 3D, and the quarter's own
/// mapping in the MPR grid.
fn probe_input(viewer: &Viewer, x: u32, y: u32) -> Result<ViewProbe, ViewError> {
    let view = viewer.view_state();
    match view.component {
        ViewComponent::Slice => probe_on_slice(viewer, view.plane, view.slice_index, x, y),
        ViewComponent::Volume => probe_pixel_3d(viewer, x, y, VOLUME_SIZE),
        ViewComponent::Mpr => {
            let size = MPR_SIZE;
            if x >= size || y >= size {
                return Err(ViewError::BadRequest(format!("probe {x},{y} is outside the {size}×{size} MPR frame")));
            }
            for (quarter, rect) in mpr_quarters() {
                if x < rect.0.x || y < rect.0.y || x >= rect.1.x || y >= rect.1.y {
                    continue;
                }
                let local = (x - rect.0.x, y - rect.0.y);
                let half = rect.1 - rect.0;
                return match quarter {
                    Quarter::Volume => probe_pixel_3d(viewer, local.0, local.1, half.x),
                    Quarter::Slice(axis) => {
                        let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
                        let plane = axis.plane_dims(&dataset.volume);
                        let image = fit_rect((glam::UVec2::ZERO, half), plane);
                        if local.0 < image.0.x || local.1 < image.0.y || local.0 >= image.1.x || local.1 >= image.1.y {
                            return Err(ViewError::BadRequest(format!(
                                "probe {x},{y} is outside the {} quarter",
                                plane_name(axis)
                            )));
                        }
                        let px = (local.0 - image.0.x) * plane.0 / (image.1.x - image.0.x);
                        let py = (local.1 - image.0.y) * plane.1 / (image.1.y - image.0.y);
                        probe_on_slice(viewer, axis, viewer.slice_index(axis), px, py)
                    }
                };
            }
            Err(ViewError::BadRequest(format!("probe {x},{y} is outside the MPR frame")))
        }
    }
}

/// The probe of a native-resolution slice pixel (one pixel per voxel).
fn probe_on_slice(viewer: &Viewer, axis: SliceAxis, index: u32, x: u32, y: u32) -> Result<ViewProbe, ViewError> {
    let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
    let volume = &dataset.volume;
    let voxel = axis
        .voxel_at(volume, index, x, y)
        .ok_or_else(|| ViewError::BadRequest(format!("probe {x},{y} is outside the {}-slice", plane_name(axis))))?;
    probe_of_voxel(viewer, voxel)
}

/// The probe of a 3D frame pixel: the camera ray through it, resolved by
/// [`Viewer::pick`] (the same visibility model the frame was drawn with).
fn probe_pixel_3d(viewer: &Viewer, x: u32, y: u32, size: u32) -> Result<ViewProbe, ViewError> {
    probe_on_volume(viewer, ndc_of(x, y, size, size), 1.0)
        .map_err(|_| ViewError::BadRequest(format!("probe {x},{y} hits nothing in the 3D view")))
}

/// Normalised device coordinates of a frame pixel (`+y` up, pixel centres).
pub(crate) fn ndc_of(x: u32, y: u32, width: u32, height: u32) -> Vec2 {
    Vec2::new(
        ((f64::from(x) + 0.5) / f64::from(width.max(1))) as f32 * 2.0 - 1.0,
        1.0 - ((f64::from(y) + 0.5) / f64::from(height.max(1))) as f32 * 2.0,
    )
}

/// The reading of a voxel: its centre in patient millimetres and its value.
pub(crate) fn probe_of_voxel(viewer: &Viewer, voxel: UVec3) -> Result<ViewProbe, ViewError> {
    let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
    let volume = &dataset.volume;
    let centre = glam::DVec3::new(f64::from(voxel.x) + 0.5, f64::from(voxel.y) + 0.5, f64::from(voxel.z) + 0.5);
    let mm = volume.voxel_to_patient_f64(centre);
    let value = volume.physical(voxel.x, voxel.y, voxel.z).unwrap_or(0.0);
    Ok(ViewProbe {
        voxel: [voxel.x, voxel.y, voxel.z],
        patient_mm: [mm.x, mm.y, mm.z],
        value: f64::from(value),
        unit: value_unit(viewer).to_string(),
    })
}

/// Draws an annotation through the tool machine ([`Viewer::slice_input`]):
/// the clicked pixels become in-plane millimetres of the active plane's
/// slice, the tool is selected through the full applier, and a commit is
/// recorded as `roi_drawn` and saved into the workspace.
fn draw_input(viewer: &mut Viewer, tool: ViewTool, points: &[(u32, u32)]) -> Result<Vec<ViewEvent>, ViewError> {
    let view = viewer.view_state();
    let axis = view.plane;
    let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
    let (w, h) = axis.plane_dims(&dataset.volume);
    let image_mm = axis.plane_size_mm(&dataset.volume);
    let mut mm = Vec::with_capacity(points.len());
    for &(x, y) in points {
        if x >= w || y >= h {
            return Err(ViewError::BadRequest(format!(
                "draw: pixel {x},{y} is outside the {}-slice",
                plane_name(axis)
            )));
        }
        mm.push(Vec2::new(
            ((f64::from(x) + 0.5) / f64::from(w)) as f32 * image_mm.x,
            ((f64::from(y) + 0.5) / f64::from(h)) as f32 * image_mm.y,
        ));
    }
    apply_state(viewer, |t| t.tool = tool);
    let mut events = Vec::new();
    // The viewport equals the image in millimetres, so the default slice view
    // maps screen points to millimetres one to one (the host draws at native
    // resolution, without pan or zoom).
    let press = |viewer: &mut Viewer, events: &mut Vec<ViewEvent>, kind: InputKind, at: Vec2| {
        if let ToolOutcome::Committed(id) = viewer.slice_input(axis, kind, at, image_mm) {
            events.push(ViewEvent::RoiDrawn { item: RoiItem::Annotation(id) });
        }
    };
    match tool {
        ViewTool::Distance | ViewTool::Rect => {
            press(viewer, &mut events, InputKind::Press, mm[0]);
            press(viewer, &mut events, InputKind::Release, mm[1]);
        }
        ViewTool::Angle => {
            for &at in &mm {
                press(viewer, &mut events, InputKind::Press, at);
            }
        }
        ViewTool::Area => {
            for &at in &mm {
                press(viewer, &mut events, InputKind::Press, at);
            }
            // Click the first point again to close the polygon.
            press(viewer, &mut events, InputKind::Press, mm[0]);
        }
        _ => return Err(ViewError::BadRequest("draw: not a measurement tool".into())),
    }
    if events.iter().any(|e| matches!(e, ViewEvent::RoiDrawn { .. })) {
        if let Err(e) = viewer.save_workspace() {
            log::warn!("ferrum-view: the annotation was drawn but could not be saved: {e}");
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::{Overlays, ViewState as DomainView};

    fn viewer() -> Viewer {
        Viewer::new(std::sync::Arc::new(ferrum_io::CompositeRepository::default()))
    }

    // covers 29.1-a — every input variant parses, and one object sets one thing
    #[test]
    fn the_new_inputs_parse_and_bad_ones_are_refused() {
        let ok = |body: &str| Input::parse(body.as_bytes()).unwrap();
        assert_eq!(ok(r#"{"component":"mpr"}"#), Input::Component(ViewComponent::Mpr));
        assert_eq!(ok(r#"{"camera":{"orbit":[0.3,-0.2]}}"#), Input::Camera(CameraOp::Orbit { dx: 0.3, dy: -0.2 }));
        assert_eq!(ok(r#"{"camera":{"zoom":1.5}}"#), Input::Camera(CameraOp::Zoom(1.5)));
        assert_eq!(ok(r#"{"camera":{"target":[0.1,0.2,0.3]}}"#), Input::Camera(CameraOp::Target([0.1, 0.2, 0.3])));
        assert_eq!(
            ok(r#"{"overlays":{"fill_opacity":0.4,"style":"outline"}}"#),
            Input::Overlays(OverlayChange {
                fill_opacity: Some(0.4),
                style: Some(SegmentStyle::Outline),
                ..Default::default()
            })
        );
        assert_eq!(ok(r#"{"selected_segment":3}"#), Input::SelectedSegment(Some(3)));
        assert_eq!(ok(r#"{"selected_segment":null}"#), Input::SelectedSegment(None));
        assert_eq!(ok(r#"{"tool":"distance"}"#), Input::Tool(ViewTool::Distance));
        assert_eq!(
            ok(r#"{"segment":{"label":1,"visible":false}}"#),
            Input::Segment { label: 1, visible: Some(false), opacity: None }
        );
        assert_eq!(
            ok(r#"{"draw":{"tool":"distance","points":[[4,4],[7,6]]}}"#),
            Input::Draw { tool: ViewTool::Distance, points: vec![(4, 4), (7, 6)] }
        );
        for bad in [
            r#"{"component":"3d"}"#,
            r#"{"camera":{"orbit":[0.3]}}"#,
            r#"{"camera":{"orbit":[0.3,0.2],"zoom":1}}"#,
            r#"{"camera":{"zoom":"far"}}"#,
            r#"{"camera":{"target":[0,0]}}"#,
            r#"{"overlays":{}}"#,
            r#"{"overlays":{"fill_opacity":1.5}}"#,
            r#"{"overlays":{"style":"glow"}}"#,
            r#"{"overlays":{"gamma":1}}"#,
            r#"{"selected_segment":300}"#,
            r#"{"tool":"wand"}"#,
            r#"{"segment":{"label":1}}"#,
            r#"{"segment":{"label":300,"visible":true}}"#,
            r#"{"segment":{"label":1,"opacity":2}}"#,
            r#"{"draw":{"tool":"pan","points":[[0,0],[1,1]]}}"#,
            r#"{"draw":{"tool":"distance","points":[[0,0]]}}"#,
            r#"{"draw":{"tool":"angle","points":[[0,0],[1,1]]}}"#,
            r#"{"draw":{"tool":"area","points":[[0,0],[1,1]]}}"#,
            r#"{"draw":{"tool":"distance","points":[[0,0],[1]]}}"#,
            r#"{"draw":{"tool":"distance","points":[{"x":0,"y":0},{"x":1,"y":1}]}}"#,
            r#"{"component":"mpr","tool":"pan"}"#,
        ] {
            assert!(Input::parse(bad.as_bytes()).is_err(), "{bad} must be refused");
        }
    }

    // covers 29.1-a — the full applier honours every field; no unhonoured list
    #[test]
    fn a_full_state_is_applied_through_the_viewer() {
        let mut v = viewer();
        let view = DomainView {
            plane: SliceAxis::Coronal,
            window: WindowLevel::new(-600.0, 1500.0),
            overlays: Overlays { segments_shown: true, fill_opacity: 0.4, style: SegmentStyle::Outline },
            component: ViewComponent::Volume,
            camera: Some(Default::default()),
            selected_segment: Some(3),
            tool: ViewTool::Probe,
            ..Default::default()
        };
        let events = v.apply_view_state(&view);
        let back = v.view_state();
        assert_eq!(back.component, ViewComponent::Volume);
        assert_eq!(back.plane, SliceAxis::Coronal);
        assert_eq!(back.window, WindowLevel::new(-600.0, 1500.0));
        assert_eq!(back.overlays, view.overlays);
        assert_eq!(back.selected_segment, Some(3));
        assert_eq!(back.tool, ViewTool::Probe);
        assert!(back.camera.is_some());
        let kinds: Vec<&str> = events.iter().map(ViewEvent::kind).collect();
        assert!(kinds.contains(&"plane_changed"), "{kinds:?}");
        assert!(kinds.contains(&"window_changed"), "{kinds:?}");
        assert!(kinds.contains(&"segments_changed"), "{kinds:?}");
    }

    // covers 29.1-a — a camera input orbits the viewer's camera and reports it
    #[test]
    fn a_camera_input_orbits_and_events() {
        let mut v = viewer();
        let before = v.volume.camera;
        let events = Input::Camera(CameraOp::Orbit { dx: 0.5, dy: 0.2 }).apply(&mut v).unwrap();
        assert_ne!(v.volume.camera, before);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), "camera_changed");
        // the same camera again produces no event
        let same = Input::Camera(CameraOp::Target([
            v.volume.camera.target.x,
            v.volume.camera.target.y,
            v.volume.camera.target.z,
        ]))
        .apply(&mut v)
        .unwrap();
        assert!(same.is_empty(), "{same:?}");
    }

    // covers 29.1-a — overlays merge into the current state, not the default
    #[test]
    fn an_overlay_change_keeps_the_untouched_fields() {
        let mut v = viewer();
        v.set_segments_shown(true);
        v.set_segment_fill_opacity(0.8);
        let events = Input::Overlays(OverlayChange { style: Some(SegmentStyle::Fill), ..Default::default() })
            .apply(&mut v)
            .unwrap();
        let overlays = v.view_state().overlays;
        assert!(overlays.segments_shown, "shown stays");
        assert_eq!(overlays.fill_opacity, 0.8, "fill opacity stays");
        assert_eq!(overlays.style, SegmentStyle::Fill);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), "segments_changed");
    }
}
