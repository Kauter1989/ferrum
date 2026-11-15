//! The JSON contract of `ferrum-view/1` view state and events (Stage 28, #70).
//!
//! The domain types ([`ferrum_domain::ViewState`], [`ferrum_domain::ViewEvent`])
//! are pure; this module is the single place they become JSON, so the agent's
//! schema and the view host's parser cannot drift (ADR 0018). A state holds
//! numbers and labels only — no path, UID, name or header value (28.5-a).
//! See `docs/view-state.md`.

use ferrum_domain::{
    OrbitCamera, Overlays, RoiItem, SegmentStyle, SliceAxis, ViewComponent, ViewEvent, ViewProbe, ViewState, ViewTool,
    WindowLevel,
};
use glam::{Quat, Vec3};
use serde_json::{json, Value};

/// The format string every view-state document carries.
pub const VIEW_STATE_FORMAT: &str = "ferrum-view/1";

/// A saved view state: the state, its revision, and the study's source hash.
pub type SavedViewState = (ferrum_domain::ViewState, u64, String);

/// A saved event log: the events with their sequence numbers, the last
/// sequence number, and the count of evicted events.
pub type SavedViewEvents = (Vec<(u64, ferrum_domain::ViewEvent)>, u64, u64);

/// The state object of `ferrum-view/1` (§3.1 of the design), without the
/// `format`/`revision`/`source_hash` wrapper the workspace adds.
pub fn state_to_json(state: &ViewState) -> Value {
    json!({
        "component": state.component.as_str(),
        "plane": state.plane.as_str(),
        "slice_index": state.slice_index,
        "window": { "center": state.window.center, "width": state.window.width },
        "overlays": {
            "segments_shown": state.overlays.segments_shown,
            "fill_opacity": state.overlays.fill_opacity,
            "style": state.overlays.style.as_str(),
        },
        "selected_segment": state.selected_segment,
        "tool": state.tool.as_str(),
        "camera": state.camera.as_ref().map(camera_to_json),
        "probe": state.probe.as_ref().map(probe_to_json),
    })
}

/// Parses a state object. Missing fields take their default (a 2D-only state
/// stays valid); a bad enum or a malformed value is an error naming the field.
/// Unknown fields are ignored here — `view set` refuses them strictly (28.3).
pub fn state_from_json(value: &Value) -> Result<ViewState, String> {
    let obj = value.as_object().ok_or("state: an object")?;
    let mut state = ViewState::default();
    if let Some(v) = obj.get("component") {
        let s = v.as_str().ok_or("component: a string")?;
        state.component = ViewComponent::parse(s).ok_or_else(|| format!("component: unknown {s:?}"))?;
    }
    if let Some(v) = obj.get("plane") {
        let s = v.as_str().ok_or("plane: a string")?;
        state.plane = SliceAxis::parse(s).ok_or_else(|| format!("plane: unknown {s:?}"))?;
    }
    if let Some(v) = obj.get("slice_index") {
        let n = v.as_u64().ok_or("slice_index: a non-negative integer")?;
        state.slice_index = u32::try_from(n).map_err(|_| "slice_index: too large".to_string())?;
    }
    if let Some(v) = obj.get("window") {
        state.window = window_from_json(v)?;
    }
    if let Some(v) = obj.get("overlays") {
        state.overlays = overlays_from_json(v)?;
    }
    if let Some(v) = obj.get("selected_segment") {
        state.selected_segment = match v {
            Value::Null => None,
            _ => {
                let n = v.as_u64().ok_or("selected_segment: a label or null")?;
                Some(u8::try_from(n).map_err(|_| "selected_segment: too large".to_string())?)
            }
        };
    }
    if let Some(v) = obj.get("tool") {
        let s = v.as_str().ok_or("tool: a string")?;
        state.tool = ViewTool::parse(s).ok_or_else(|| format!("tool: unknown {s:?}"))?;
    }
    if let Some(v) = obj.get("camera") {
        state.camera = match v {
            Value::Null => None,
            _ => Some(camera_from_json(v)?),
        };
    }
    if let Some(v) = obj.get("probe") {
        state.probe = match v {
            Value::Null => None,
            _ => Some(probe_from_json(v)?),
        };
    }
    Ok(state)
}

fn window_from_json(value: &Value) -> Result<WindowLevel, String> {
    let w = value.as_object().ok_or("window: an object with center and width")?;
    // cast first: a huge f64 (1e300) becomes f32 infinity, which must be refused
    let center = w.get("center").and_then(Value::as_f64).ok_or("window: center")? as f32;
    let width = w.get("width").and_then(Value::as_f64).ok_or("window: width")? as f32;
    if !center.is_finite() || !width.is_finite() || width <= 0.0 {
        return Err("window: a finite centre and a width above 0".into());
    }
    Ok(WindowLevel::new(center, width))
}

fn overlays_from_json(value: &Value) -> Result<Overlays, String> {
    let o = value.as_object().ok_or("overlays: an object")?;
    let mut overlays = Overlays::default();
    if let Some(v) = o.get("segments_shown") {
        overlays.segments_shown = v.as_bool().ok_or("overlays: segments_shown is true or false")?;
    }
    if let Some(v) = o.get("fill_opacity") {
        let f = v.as_f64().ok_or("overlays: fill_opacity is a number")?;
        if !f.is_finite() || !(0.0..=1.0).contains(&f) {
            return Err("overlays: fill_opacity in 0..=1".into());
        }
        overlays.fill_opacity = f as f32;
    }
    if let Some(v) = o.get("style") {
        let s = v.as_str().ok_or("overlays: style is a string")?;
        overlays.style = SegmentStyle::parse(s).ok_or_else(|| format!("overlays: unknown style {s:?}"))?;
    }
    Ok(overlays)
}

/// The camera as JSON: orientation quaternion `x,y,z,w`, distance, target, fov.
pub fn camera_to_json(camera: &OrbitCamera) -> Value {
    let q = camera.orientation;
    let t = camera.target;
    json!({
        "orientation": [q.x, q.y, q.z, q.w],
        "distance": camera.distance,
        "target": [t.x, t.y, t.z],
        "fov_y": camera.fov_y,
    })
}

fn camera_from_json(value: &Value) -> Result<OrbitCamera, String> {
    let c = value.as_object().ok_or("camera: an object")?;
    let orientation = c.get("orientation").and_then(Value::as_array).ok_or("camera: orientation")?;
    if orientation.len() != 4 {
        return Err("camera: orientation is four numbers".into());
    }
    let q: Vec<f64> = orientation.iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect();
    if q.iter().any(|v| !v.is_finite()) {
        return Err("camera: orientation is finite".into());
    }
    let distance = c.get("distance").and_then(Value::as_f64).ok_or("camera: distance")?;
    let target = c.get("target").and_then(Value::as_array).ok_or("camera: target")?;
    if target.len() != 3 {
        return Err("camera: target is three numbers".into());
    }
    let t: Vec<f64> = target.iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect();
    let fov_y = c.get("fov_y").and_then(Value::as_f64).ok_or("camera: fov_y")?;
    if !distance.is_finite() || t.iter().any(|v| !v.is_finite()) || !fov_y.is_finite() {
        return Err("camera: finite distance, target and fov_y".into());
    }
    // cast first, then validate: a huge f64 becomes f32 infinity (GPU device-loss
    // risk, L40); a zero quaternion normalizes to NaN, so it is refused.
    let orientation = Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32);
    let target = Vec3::new(t[0] as f32, t[1] as f32, t[2] as f32);
    let distance = distance as f32;
    let fov_y = fov_y as f32;
    if !orientation.is_finite() || orientation.length() < 1e-6 {
        return Err("camera: orientation is a finite, non-zero quaternion".into());
    }
    if !target.is_finite() || !distance.is_finite() || !fov_y.is_finite() {
        return Err("camera: finite distance, target and fov_y".into());
    }
    if !(0.0..std::f32::consts::PI).contains(&fov_y) {
        return Err("camera: fov_y in (0, pi)".into());
    }
    Ok(OrbitCamera {
        orientation: orientation.normalize(),
        distance: distance.clamp(OrbitCamera::MIN_DISTANCE, OrbitCamera::MAX_DISTANCE),
        target,
        fov_y,
    })
}

fn probe_to_json(probe: &ViewProbe) -> Value {
    json!({
        "voxel": probe.voxel,
        "patient_mm": probe.patient_mm,
        "value": probe.value,
        "unit": probe.unit,
    })
}

fn probe_from_json(value: &Value) -> Result<ViewProbe, String> {
    let p = value.as_object().ok_or("probe: an object")?;
    let voxel = p.get("voxel").and_then(Value::as_array).ok_or("probe: voxel")?;
    let mm = p.get("patient_mm").and_then(Value::as_array).ok_or("probe: patient_mm")?;
    if voxel.len() != 3 || mm.len() != 3 {
        return Err("probe: voxel and patient_mm are three numbers".into());
    }
    let mut v = [0u32; 3];
    for (i, x) in voxel.iter().enumerate() {
        v[i] = u32::try_from(x.as_u64().ok_or("probe: voxel is integers")?).map_err(|_| "probe: voxel too large")?;
    }
    let mut m = [0f64; 3];
    for (i, x) in mm.iter().enumerate() {
        m[i] = x.as_f64().ok_or("probe: patient_mm is numbers")?;
    }
    let value = p.get("value").and_then(Value::as_f64).ok_or("probe: value")?;
    let unit = p.get("unit").and_then(Value::as_str).ok_or("probe: unit")?.to_string();
    Ok(ViewProbe { voxel: v, patient_mm: m, value, unit })
}

/// One event as JSON (`{"seq", "kind", …}`, §3.2 of the design).
pub fn event_to_json(seq: u64, event: &ViewEvent) -> Value {
    let mut object = match event {
        ViewEvent::SliceChanged { plane, slice_index } => {
            json!({ "plane": plane.as_str(), "slice_index": slice_index })
        }
        ViewEvent::PlaneChanged { plane, slice_index } => {
            json!({ "plane": plane.as_str(), "slice_index": slice_index })
        }
        ViewEvent::WindowChanged { center, width } => json!({ "center": center, "width": width }),
        ViewEvent::SegmentsChanged { shown } => json!({ "shown": shown }),
        ViewEvent::CameraChanged { camera } => json!({ "camera": camera_to_json(camera) }),
        ViewEvent::RoiDrawn { item } => match item {
            RoiItem::Annotation(id) => json!({ "item": { "kind": "annotation", "id": id } }),
            RoiItem::Segment(label) => json!({ "item": { "kind": "segment", "label": label } }),
        },
        ViewEvent::PointPicked { probe } => probe_to_json(probe),
    };
    if let Value::Object(m) = &mut object {
        m.insert("seq".into(), json!(seq));
        m.insert("kind".into(), json!(event.kind()));
    }
    object
}

/// Parses one event object into its sequence number and the event.
pub fn event_from_json(value: &Value) -> Result<(u64, ViewEvent), String> {
    let obj = value.as_object().ok_or("event: an object")?;
    let seq = obj.get("seq").and_then(Value::as_u64).ok_or("event: seq")?;
    let kind = obj.get("kind").and_then(Value::as_str).ok_or("event: kind")?;
    let event = match kind {
        "slice_changed" => ViewEvent::SliceChanged { plane: plane_field(obj)?, slice_index: index_field(obj)? },
        "plane_changed" => ViewEvent::PlaneChanged { plane: plane_field(obj)?, slice_index: index_field(obj)? },
        "window_changed" => {
            let center = obj.get("center").and_then(Value::as_f64).ok_or("window_changed: center")? as f32;
            let width = obj.get("width").and_then(Value::as_f64).ok_or("window_changed: width")? as f32;
            ViewEvent::WindowChanged { center, width }
        }
        "segments_changed" => {
            let shown = obj.get("shown").and_then(Value::as_bool).ok_or("segments_changed: shown")?;
            ViewEvent::SegmentsChanged { shown }
        }
        "camera_changed" => {
            let camera = camera_from_json(obj.get("camera").ok_or("camera_changed: camera")?)?;
            ViewEvent::CameraChanged { camera }
        }
        "roi_drawn" => {
            let item = obj.get("item").and_then(Value::as_object).ok_or("roi_drawn: item")?;
            let roi = match item.get("kind").and_then(Value::as_str).ok_or("roi_drawn: item.kind")? {
                "annotation" => {
                    RoiItem::Annotation(item.get("id").and_then(Value::as_u64).ok_or("roi_drawn: item.id")?)
                }
                "segment" => {
                    let label = item.get("label").and_then(Value::as_u64).ok_or("roi_drawn: item.label")?;
                    RoiItem::Segment(u8::try_from(label).map_err(|_| "roi_drawn: label too large")?)
                }
                other => return Err(format!("roi_drawn: unknown item kind {other:?}")),
            };
            ViewEvent::RoiDrawn { item: roi }
        }
        "point_picked" => ViewEvent::PointPicked { probe: probe_from_json(value)? },
        other => return Err(format!("event: unknown kind {other:?}")),
    };
    Ok((seq, event))
}

fn plane_field(obj: &serde_json::Map<String, Value>) -> Result<SliceAxis, String> {
    let s = obj.get("plane").and_then(Value::as_str).ok_or("event: plane")?;
    SliceAxis::parse(s).ok_or_else(|| format!("event: unknown plane {s:?}"))
}

fn index_field(obj: &serde_json::Map<String, Value>) -> Result<u32, String> {
    let n = obj.get("slice_index").and_then(Value::as_u64).ok_or("event: slice_index")?;
    u32::try_from(n).map_err(|_| "event: slice_index too large".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_state() -> ViewState {
        ViewState {
            component: ViewComponent::Volume,
            plane: SliceAxis::Coronal,
            slice_index: 142,
            window: WindowLevel::new(-600.0, 1500.0),
            overlays: Overlays { segments_shown: true, fill_opacity: 0.4, style: SegmentStyle::FillAndOutline },
            selected_segment: Some(3),
            tool: ViewTool::Probe,
            camera: Some(OrbitCamera::default()),
            probe: Some(ViewProbe {
                voxel: [251, 198, 142],
                patient_mm: [12.4, -40.1, 88.1],
                value: 55.0,
                unit: "HU".into(),
            }),
        }
    }

    // covers 28.1-a
    #[test]
    fn a_full_state_round_trips() {
        let state = full_state();
        let json = state_to_json(&state);
        assert_eq!(state_from_json(&json).unwrap(), state);
    }

    // covers 28.1-a
    #[test]
    fn a_two_d_only_state_is_valid_and_defaults_the_rest() {
        let json = json!({ "plane": "axial", "slice_index": 12, "window": { "center": 40, "width": 400 } });
        let state = state_from_json(&json).unwrap();
        assert_eq!(state.component, ViewComponent::Slice);
        assert_eq!(state.plane, SliceAxis::Axial);
        assert_eq!(state.slice_index, 12);
        assert_eq!(state.tool, ViewTool::Pan);
        assert!(state.camera.is_none());
    }

    // covers 28.1-a
    #[test]
    fn bad_enums_and_values_are_refused() {
        assert!(state_from_json(&json!({ "component": "3d" })).is_err());
        assert!(state_from_json(&json!({ "plane": "oblique" })).is_err());
        assert!(state_from_json(&json!({ "tool": "wand" })).is_err());
        assert!(state_from_json(&json!({ "window": { "center": 0, "width": 0 } })).is_err());
        assert!(state_from_json(&json!({ "overlays": { "style": "glow" } })).is_err());
        assert!(state_from_json(&json!({ "selected_segment": 300 })).is_err());
    }

    // covers 28.1-a — crafted values must not reach the renderer (L40)
    #[test]
    fn crafted_camera_and_window_are_refused_or_sanitized() {
        let cam = |orientation: Value, distance: f64, fov_y: f64| json!({ "camera": { "orientation": orientation, "distance": distance, "target": [0, 0, 0], "fov_y": fov_y } });
        // a zero quaternion normalizes to NaN
        assert!(state_from_json(&cam(json!([0, 0, 0, 0]), 2.0, 0.8)).is_err());
        // a distance beyond f32 range becomes infinity (1e30 is still finite in f32)
        assert!(state_from_json(&cam(json!([0, 0, 0, 1]), 1e40, 0.8)).is_err());
        // fov outside (0, pi)
        assert!(state_from_json(&cam(json!([0, 0, 0, 1]), 2.0, 4.0)).is_err());
        // a window of 1e300 becomes f32 infinity
        assert!(state_from_json(&json!({ "window": { "center": 1e300, "width": 1.0 } })).is_err());
        assert!(state_from_json(&json!({ "window": { "center": 0.0, "width": 1e300 } })).is_err());
        // a valid camera is normalized and clamped, not refused
        let state = state_from_json(&cam(json!([0, 0, 0, 2]), 100.0, 0.8)).unwrap();
        let camera = state.camera.unwrap();
        assert!((camera.orientation.length() - 1.0).abs() < 1e-5, "normalized");
        assert!(camera.distance <= OrbitCamera::MAX_DISTANCE, "clamped");
    }

    // covers 28.1-b
    #[test]
    fn every_event_kind_round_trips() {
        let events = [
            ViewEvent::SliceChanged { plane: SliceAxis::Axial, slice_index: 142 },
            ViewEvent::PlaneChanged { plane: SliceAxis::Sagittal, slice_index: 7 },
            ViewEvent::WindowChanged { center: -600.0, width: 1500.0 },
            ViewEvent::SegmentsChanged { shown: true },
            ViewEvent::CameraChanged { camera: OrbitCamera::default() },
            ViewEvent::RoiDrawn { item: RoiItem::Segment(3) },
            ViewEvent::RoiDrawn { item: RoiItem::Annotation(99) },
            ViewEvent::PointPicked {
                probe: ViewProbe { voxel: [1, 2, 3], patient_mm: [0.0; 3], value: 55.0, unit: "HU".into() },
            },
        ];
        for (i, e) in events.iter().enumerate() {
            let seq = (i + 1) as u64;
            let json = event_to_json(seq, e);
            let (got_seq, got) = event_from_json(&json).unwrap();
            assert_eq!(got_seq, seq);
            assert_eq!(&got, e);
        }
        assert!(event_from_json(&json!({ "seq": 1, "kind": "teleported" })).is_err());
    }
}
