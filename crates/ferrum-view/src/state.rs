//! The state document of `ferrum-view/1`, the input a host may send and the
//! events it reads back.
//!
//! A state holds numbers and labels only: no path, UID, patient name or
//! value of the DICOM header ever appears here (criterion 25.5-a).

use ferrum_app::Viewer;
use ferrum_domain::{SliceAxis, WindowLevel, WindowPreset};
use glam::DVec3;
use serde_json::{json, Value};

use crate::session::ViewError;

/// The format string every state document carries.
pub const FORMAT: &str = "ferrum-view/1";

/// What the host currently shows and what the person last read.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    /// The plane shown.
    pub axis: SliceAxis,
    /// The slice index (0-based).
    pub index: u32,
    /// The window in the volume's units.
    pub window: WindowLevel,
    /// Whether the segment overlay is on.
    pub segments: bool,
    /// Incremented on every change; a frame carries the value it was drawn
    /// for.
    pub rev: u64,
    /// The value under the cursor after a pick.
    pub probe: Option<Probe>,
}

/// A value read from the voxel under a pick.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    /// The voxel of the pick.
    pub voxel: [u32; 3],
    /// Its centre in patient (LPS) millimetres.
    pub patient_mm: [f64; 3],
    /// The voxel's value in the volume's units.
    pub value: f64,
    /// `HU` for a CT, `raw` otherwise.
    pub unit: &'static str,
}

/// One thing the person did, as the agent reads it back.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// 1-based, increasing within one host process.
    pub seq: u64,
    /// `slice_changed`, `plane_changed`, `window_changed`,
    /// `segments_changed` or `point_picked`.
    pub kind: &'static str,
    /// The values of the change.
    pub data: Value,
}

impl Event {
    /// The event as JSON (`{"seq", "kind", …}`).
    pub fn to_json(&self) -> Value {
        let mut object = match &self.data {
            Value::Object(m) => m.clone(),
            other => {
                let mut m = serde_json::Map::new();
                m.insert("value".into(), other.clone());
                m
            }
        };
        object.insert("seq".into(), json!(self.seq));
        object.insert("kind".into(), json!(self.kind));
        Value::Object(object)
    }
}

/// One request from the person, as `POST /api/input` carries it.
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
    /// Read the voxel under a frame pixel returned by the last frame.
    Probe {
        /// Pixel column of the frame.
        x: u32,
        /// Pixel row of the frame.
        y: u32,
    },
}

impl Input {
    /// Parses one input object: exactly one of `slice`, `step`, `plane`,
    /// `window`, `preset`, `segments`, `probe`.
    pub fn parse(body: &[u8]) -> Result<Self, ViewError> {
        let value: Value = serde_json::from_slice(body).map_err(|e| ViewError::BadRequest(format!("input: {e}")))?;
        let object = value.as_object().ok_or_else(|| ViewError::BadRequest("input: an object".into()))?;
        parse_input(object)
    }

    /// Applies the input to `viewer` and `state`, and returns the event to
    /// record (all of them change the state).
    pub fn apply(self, viewer: &mut Viewer, state: &mut ViewState) -> Result<(&'static str, Value), ViewError> {
        let (kind, data) = match self {
            Input::Slice(index) => {
                viewer.set_slice_index(state.axis, index);
                state.index = viewer.slice_index(state.axis);
                ("slice_changed", json!({ "plane": plane_name(state.axis), "slice_index": state.index }))
            }
            Input::Step(delta) => {
                viewer.step_slice(state.axis, delta);
                state.index = viewer.slice_index(state.axis);
                ("slice_changed", json!({ "plane": plane_name(state.axis), "slice_index": state.index }))
            }
            Input::Plane(axis) => {
                state.axis = axis;
                state.index = viewer.slice_index(axis);
                ("plane_changed", json!({ "plane": plane_name(axis), "slice_index": state.index }))
            }
            Input::Window { center, width } => {
                let window = WindowLevel::new(center, width);
                viewer.set_window(window);
                state.window = window;
                ("window_changed", json!({ "center": window.center, "width": window.width }))
            }
            Input::Preset(preset) => {
                viewer.apply_window_preset(preset);
                state.window = viewer.window();
                (
                    "window_changed",
                    json!({ "center": state.window.center, "width": state.window.width, "preset": preset.label() }),
                )
            }
            Input::Segments(show) => {
                viewer.set_segments_shown(show);
                state.segments = show;
                ("segments_changed", json!({ "shown": show }))
            }
            Input::Probe { x, y } => {
                let probe = probe_at(viewer, state, x, y)?;
                let data = probe_json(&probe);
                state.probe = Some(probe);
                ("point_picked", data)
            }
        };
        state.rev += 1;
        Ok((kind, data))
    }
}

fn parse_input(object: &serde_json::Map<String, Value>) -> Result<Input, ViewError> {
    if object.len() != 1 {
        return Err(ViewError::BadRequest("input: one of slice, step, plane, window, preset, segments, probe".into()));
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
        "window" => {
            let w = value.as_object().ok_or_else(|| bad("an object with center and width"))?;
            let center = w.get("center").and_then(Value::as_f64).ok_or_else(|| bad("center"))?;
            let width = w.get("width").and_then(Value::as_f64).ok_or_else(|| bad("width"))?;
            if !center.is_finite() || !width.is_finite() || width <= 0.0 {
                return Err(bad("a finite centre and a width above 0"));
            }
            Ok(Input::Window { center: center as f32, width: width as f32 })
        }
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
            let p = value.as_object().ok_or_else(|| bad("an object with x and y"))?;
            let x = p.get("x").and_then(Value::as_u64).ok_or_else(|| bad("x"))?;
            let y = p.get("y").and_then(Value::as_u64).ok_or_else(|| bad("y"))?;
            Ok(Input::Probe { x: x.min(u64::from(u32::MAX)) as u32, y: y.min(u64::from(u32::MAX)) as u32 })
        }
        other => Err(ViewError::BadRequest(format!("input: unknown field {other:?}"))),
    }
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

fn probe_at(viewer: &Viewer, state: &ViewState, x: u32, y: u32) -> Result<Probe, ViewError> {
    let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
    let volume = &dataset.volume;
    let voxel = state.axis.voxel_at(volume, state.index, x, y).ok_or_else(|| {
        ViewError::BadRequest(format!("probe {x},{y} is outside the {}-slice", plane_name(state.axis)))
    })?;
    let centre = DVec3::new(f64::from(voxel.x) + 0.5, f64::from(voxel.y) + 0.5, f64::from(voxel.z) + 0.5);
    let mm = volume.voxel_to_patient_f64(centre);
    let value = volume.physical(voxel.x, voxel.y, voxel.z).unwrap_or(0.0);
    Ok(Probe {
        voxel: [voxel.x, voxel.y, voxel.z],
        patient_mm: [mm.x, mm.y, mm.z],
        value: f64::from(value),
        unit: value_unit(viewer),
    })
}

/// The unit of a value: HU for a CT, raw values otherwise (MR, PET without a
/// unit, an image with no declared modality).
pub fn value_unit(viewer: &Viewer) -> &'static str {
    let modality = viewer.dataset().map(|d| d.metadata.modality.trim().to_ascii_uppercase()).unwrap_or_default();
    if modality == "CT" {
        "HU"
    } else {
        "raw"
    }
}

/// The state document (`ferrum-view/1`).
pub fn state_json(viewer: &Viewer, state: &ViewState) -> Result<Value, ViewError> {
    let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
    let volume = &dataset.volume;
    let dims = volume.dims().as_uvec3();
    let spacing = volume.spacing();
    let range = volume.range();
    let segments = viewer.segmentation().set();
    let list: Vec<Value> = segments
        .map(|s| {
            s.segments()
                .iter()
                .map(|g| {
                    json!({
                        "label": g.label,
                        "name": g.name,
                        "visible": g.visible,
                        "opacity": g.opacity,
                        "voxels": s.voxel_count(g.label),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "format": FORMAT,
        "rev": state.rev,
        "view": "slice",
        "plane": plane_name(state.axis),
        "slice_index": state.index,
        "slice_count": state.axis.slice_count(volume),
        "window": { "center": state.window.center, "width": state.window.width },
        "segments": {
            "shown": state.segments,
            "items": list,
        },
        "volume": {
            "dims": [dims.x, dims.y, dims.z],
            "spacing_mm": [spacing.x, spacing.y, spacing.z],
            "value_unit": value_unit(viewer),
            "value_range": [range.min, range.max],
        },
        "probe": state.probe.as_ref().map(probe_json),
    }))
}

fn probe_json(probe: &Probe) -> Value {
    json!({
        "voxel": probe.voxel,
        "patient_mm": probe.patient_mm.map(|v| (v * 1e4).round() / 1e4),
        "value": (probe.value * 1e4).round() / 1e4,
        "unit": probe.unit,
    })
}
