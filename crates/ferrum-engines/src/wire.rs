//! Wire format of `ferrum-engine/1` (see `docs/engine-protocol.md`): JSON
//! messages and voxel arrays, shared by the client and the reference server.

use base64::Engine as _;
use ferrum_domain::{
    Dims3, EngineCapabilities, EngineError, EngineInfo, EngineLabel, Geometry, JobState, JobStatus, Prompt, PromptKind,
    PromptResult, Volume, VoxelBox, ENGINE_PROTOCOL,
};
use glam::{Mat3, UVec3, Vec3};
use serde_json::{json, Value};

/// Voxel data type of an uploaded volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dtype {
    /// Signed 16-bit integers (CT in Hounsfield units).
    Int16,
    /// Unsigned 16-bit integers.
    Uint16,
    /// 32-bit floats.
    Float32,
}

impl Dtype {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Dtype::Int16 => "int16",
            Dtype::Uint16 => "uint16",
            Dtype::Float32 => "float32",
        }
    }

    /// Parses a wire name.
    pub fn parse(s: &str) -> Option<Dtype> {
        [Dtype::Int16, Dtype::Uint16, Dtype::Float32].into_iter().find(|d| d.as_str() == s)
    }

    /// Bytes per voxel.
    pub fn size(self) -> usize {
        match self {
            Dtype::Int16 | Dtype::Uint16 => 2,
            Dtype::Float32 => 4,
        }
    }
}

/// Session declaration (`POST /v1/sessions`).
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeHeader {
    /// Grid size.
    pub dims: Dims3,
    /// Voxel type of the upload.
    pub dtype: Dtype,
    /// Voxel size in mm.
    pub spacing: Vec3,
    /// Patient placement (LPS).
    pub geometry: Geometry,
    /// DICOM modality (may be empty).
    pub modality: String,
    /// `"HU"` for CT, otherwise empty.
    pub value_unit: String,
}

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadRequest(msg.into())
}

fn protocol(msg: impl Into<String>) -> EngineError {
    EngineError::Protocol(msg.into())
}

fn u32_array<const N: usize>(v: &Value, what: &str) -> Result<[u32; N], EngineError> {
    let arr = v.as_array().filter(|a| a.len() == N).ok_or_else(|| bad(format!("{what}: expected {N} integers")))?;
    let mut out = [0u32; N];
    for (o, x) in out.iter_mut().zip(arr) {
        *o = x.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(format!("{what}: not an integer")))?;
    }
    Ok(out)
}

fn f32_array(v: &Value, what: &str) -> Result<Vec3, EngineError> {
    let arr = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| bad(format!("{what}: expected 3 numbers")))?;
    let n = |i: usize| arr[i].as_f64().map(|x| x as f32).ok_or_else(|| bad(format!("{what}: not a number")));
    Ok(Vec3::new(n(0)?, n(1)?, n(2)?))
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// Encodes a box as `{min, max}`.
pub fn box_to_json(b: VoxelBox) -> Value {
    json!({ "min": b.min.to_array(), "max": b.max.to_array() })
}

/// Decodes `{min, max}`.
pub fn box_from_json(v: &Value) -> Result<VoxelBox, EngineError> {
    let min = u32_array::<3>(v.get("min").unwrap_or(&Value::Null), "min")?;
    let max = u32_array::<3>(v.get("max").unwrap_or(&Value::Null), "max")?;
    Ok(VoxelBox::new(UVec3::from(min), UVec3::from(max)))
}

/// Formats a box for the `box=` query parameter.
pub fn box_query(b: VoxelBox) -> String {
    format!("{},{},{},{},{},{}", b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z)
}

/// Parses the `box=` query parameter.
pub fn parse_box_query(s: &str) -> Result<VoxelBox, EngineError> {
    let n: Vec<u32> = s
        .split(',')
        .map(|x| x.trim().parse::<u32>())
        .collect::<Result<_, _>>()
        .map_err(|_| bad("box: expected 6 integers"))?;
    match n.as_slice() {
        [a, b, c, d, e, f] => Ok(VoxelBox::new(UVec3::new(*a, *b, *c), UVec3::new(*d, *e, *f))),
        _ => Err(bad("box: expected 6 integers")),
    }
}

/// Encodes engine information (`GET /v1/info`).
pub fn info_to_json(info: &EngineInfo) -> Value {
    let c = &info.capabilities;
    json!({
        "protocol": info.protocol,
        "name": info.name,
        "version": info.version,
        "vendor": info.vendor,
        "device": info.device,
        "capabilities": {
            "interactive": c.interactive,
            "automatic": c.automatic,
            "prompts": c.prompts.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
            "planar_boxes_only": c.planar_boxes_only,
            "undo": c.undo,
        },
        "modalities": info.modalities,
        "labels": info.labels.iter().map(|l| {
            let mut v = json!({ "value": l.value, "name": l.name });
            if let Some(c) = l.color {
                v["color"] = json!(c);
            }
            v
        }).collect::<Vec<_>>(),
        "research_only": info.research_only,
        "license": info.license,
        "limits": { "max_voxels": info.max_voxels, "session_ttl_s": info.session_ttl_s },
    })
}

fn label_from_json(v: &Value) -> Option<EngineLabel> {
    let color = v.get("color").and_then(Value::as_array).filter(|c| c.len() == 3).and_then(|c| {
        let ch = |i: usize| c[i].as_u64().and_then(|x| u8::try_from(x).ok());
        Some([ch(0)?, ch(1)?, ch(2)?])
    });
    Some(EngineLabel {
        value: v.get("value")?.as_u64().and_then(|x| u16::try_from(x).ok())?,
        name: str_of(v, "name"),
        color,
    })
}

/// Decodes engine information. Unknown fields and prompt kinds are ignored;
/// another protocol version is an error.
pub fn info_from_json(v: &Value) -> Result<EngineInfo, EngineError> {
    let protocol_name = str_of(v, "protocol");
    if protocol_name != ENGINE_PROTOCOL {
        return Err(protocol(format!("engine speaks {protocol_name:?}, FERRUM needs {ENGINE_PROTOCOL}")));
    }
    let c = v.get("capabilities").unwrap_or(&Value::Null);
    let flag = |key: &str| c.get(key).and_then(Value::as_bool).unwrap_or(false);
    let limits = v.get("limits").unwrap_or(&Value::Null);
    let limit = |key: &str| limits.get(key).and_then(Value::as_u64).unwrap_or(0);
    Ok(EngineInfo {
        protocol: protocol_name,
        name: str_of(v, "name"),
        version: str_of(v, "version"),
        vendor: str_of(v, "vendor"),
        device: str_of(v, "device"),
        capabilities: EngineCapabilities {
            interactive: flag("interactive"),
            automatic: flag("automatic"),
            prompts: c
                .get("prompts")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|k| k.as_str().and_then(PromptKind::parse)).collect())
                .unwrap_or_default(),
            planar_boxes_only: flag("planar_boxes_only"),
            undo: flag("undo"),
        },
        modalities: v
            .get("modalities")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|m| m.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
        labels: v
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(label_from_json).collect())
            .unwrap_or_default(),
        research_only: v.get("research_only").and_then(Value::as_bool).unwrap_or(false),
        license: str_of(v, "license"),
        max_voxels: limit("max_voxels"),
        session_ttl_s: limit("session_ttl_s"),
    })
}

/// Encodes a prompt.
pub fn prompt_to_json(p: &Prompt) -> Value {
    let b64 = |m: &[u8]| base64::engine::general_purpose::STANDARD.encode(m);
    match p {
        Prompt::Point { positive, voxel } => {
            json!({ "type": "point", "positive": positive, "voxel": voxel.to_array() })
        }
        Prompt::Box { positive, bx } => {
            json!({ "type": "box", "positive": positive, "min": bx.min.to_array(), "max": bx.max.to_array() })
        }
        Prompt::Scribble { positive, bx, mask } | Prompt::Lasso { positive, bx, mask } => json!({
            "type": p.kind().as_str(),
            "positive": positive,
            "min": bx.min.to_array(),
            "max": bx.max.to_array(),
            "mask": b64(mask),
        }),
    }
}

/// Decodes a prompt (structure only; see [`Prompt::validate`] for bounds).
pub fn prompt_from_json(v: &Value) -> Result<Prompt, EngineError> {
    let kind = v.get("type").and_then(Value::as_str).ok_or_else(|| bad("prompt without type"))?;
    let kind = PromptKind::parse(kind).ok_or_else(|| EngineError::Unsupported(format!("{kind} prompts")))?;
    let positive = v.get("positive").and_then(Value::as_bool).unwrap_or(true);
    let mask = || -> Result<Vec<u8>, EngineError> {
        let s = v.get("mask").and_then(Value::as_str).ok_or_else(|| bad("mask missing"))?;
        base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| bad(format!("mask: {e}")))
    };
    Ok(match kind {
        PromptKind::Point => Prompt::Point {
            positive,
            voxel: UVec3::from(u32_array::<3>(v.get("voxel").unwrap_or(&Value::Null), "voxel")?),
        },
        PromptKind::Box => Prompt::Box { positive, bx: box_from_json(v)? },
        PromptKind::Scribble => Prompt::Scribble { positive, bx: box_from_json(v)?, mask: mask()? },
        PromptKind::Lasso => Prompt::Lasso { positive, bx: box_from_json(v)?, mask: mask()? },
    })
}

/// Encodes a prompt result.
pub fn result_to_json(r: &PromptResult) -> Value {
    json!({ "revision": r.revision, "changed": r.changed.map(box_to_json), "empty": r.empty })
}

/// Decodes a prompt result.
pub fn result_from_json(v: &Value) -> Result<PromptResult, EngineError> {
    let revision = v.get("revision").and_then(Value::as_u64).ok_or_else(|| protocol("result without revision"))?;
    let changed = match v.get("changed") {
        None | Some(Value::Null) => None,
        Some(b) => Some(box_from_json(b).map_err(|e| protocol(e.to_string()))?),
    };
    Ok(PromptResult { revision, changed, empty: v.get("empty").and_then(Value::as_bool).unwrap_or(false) })
}

/// Encodes a job status (`GET /v1/jobs/{id}`).
pub fn job_to_json(j: &JobStatus) -> Value {
    json!({ "state": j.state.as_str(), "progress": j.progress, "message": j.message })
}

/// Decodes a job status.
pub fn job_from_json(v: &Value) -> Result<JobStatus, EngineError> {
    let state = v
        .get("state")
        .and_then(Value::as_str)
        .and_then(JobState::parse)
        .ok_or_else(|| protocol("job without a valid state"))?;
    let progress = v.get("progress").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 1.0) as f32;
    Ok(JobStatus { state, progress, message: str_of(v, "message") })
}

/// Encodes a segmentation request (`POST …/segment`).
pub fn labels_to_json(labels: Option<&[String]>) -> Value {
    json!({ "labels": labels })
}

/// Decodes a segmentation request; `None` means all labels.
pub fn labels_from_json(v: &Value) -> Result<Option<Vec<String>>, EngineError> {
    match v.get("labels") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(str::to_owned).ok_or_else(|| bad("labels must be strings")))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(bad("labels must be an array or null")),
    }
}

/// Encodes a label map as little-endian `uint16`.
pub fn encode_label_map(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Decodes a little-endian `uint16` label map of `n` voxels.
pub fn decode_label_map(bytes: &[u8], n: usize) -> Result<Vec<u16>, EngineError> {
    if bytes.len() != n * 2 {
        return Err(protocol(format!("label map has {} bytes, expected {}", bytes.len(), n * 2)));
    }
    Ok(bytes.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect())
}

/// Encodes a session declaration.
pub fn header_to_json(h: &VolumeHeader) -> Value {
    json!({
        "dims": [h.dims.x, h.dims.y, h.dims.z],
        "dtype": h.dtype.as_str(),
        "spacing": h.spacing.to_array(),
        "origin": h.geometry.origin.to_array(),
        "direction": h.geometry.direction_rows(),
        "modality": h.modality,
        "value_unit": h.value_unit,
    })
}

/// Decodes a session declaration; optional fields get defaults.
pub fn header_from_json(v: &Value) -> Result<VolumeHeader, EngineError> {
    let [x, y, z] = u32_array::<3>(v.get("dims").unwrap_or(&Value::Null), "dims")?;
    let dims = Dims3::new(x, y, z);
    if dims.is_empty() {
        return Err(bad("dims must be non-zero"));
    }
    let dtype = v
        .get("dtype")
        .and_then(Value::as_str)
        .and_then(Dtype::parse)
        .ok_or_else(|| bad("dtype must be int16, uint16 or float32"))?;
    let spacing = f32_array(v.get("spacing").unwrap_or(&Value::Null), "spacing")?;
    let origin = match v.get("origin") {
        None | Some(Value::Null) => Vec3::ZERO,
        Some(o) => f32_array(o, "origin")?,
    };
    let direction = match v.get("direction").and_then(Value::as_array) {
        Some(rows) if rows.len() == 3 => {
            let r = |i: usize| f32_array(&rows[i], "direction");
            Mat3::from_cols(r(0)?, r(1)?, r(2)?)
        }
        _ => Mat3::IDENTITY,
    };
    Ok(VolumeHeader {
        dims,
        dtype,
        spacing,
        geometry: Geometry { origin, direction },
        modality: str_of(v, "modality"),
        value_unit: str_of(v, "value_unit"),
    })
}

/// Encodes `volume` for upload: CT whose range fits `int16` goes as
/// rounded Hounsfield units, everything else as `float32` physical values.
pub fn encode_volume(volume: &Volume, modality: &str) -> (VolumeHeader, Vec<u8>) {
    let r = volume.range();
    let ct = modality.eq_ignore_ascii_case("CT");
    let fits_i16 = r.min >= f32::from(i16::MIN) && r.max <= f32::from(i16::MAX);
    let dtype = if ct && fits_i16 { Dtype::Int16 } else { Dtype::Float32 };
    let mut bytes = Vec::with_capacity(volume.data().len() * dtype.size());
    for &s in volume.data() {
        let v = r.from_storage(s);
        match dtype {
            Dtype::Int16 => bytes.extend_from_slice(&(v.round() as i16).to_le_bytes()),
            _ => bytes.extend_from_slice(&v.to_le_bytes()),
        }
    }
    let header = VolumeHeader {
        dims: volume.dims(),
        dtype,
        spacing: volume.spacing(),
        geometry: volume.geometry(),
        modality: modality.to_owned(),
        value_unit: if ct { "HU".into() } else { String::new() },
    };
    (header, bytes)
}

/// Decodes uploaded voxels into a volume.
pub fn decode_volume(h: &VolumeHeader, bytes: &[u8]) -> Result<Volume, EngineError> {
    let n = h.dims.voxel_count();
    if bytes.len() != n * h.dtype.size() {
        return Err(bad(format!("expected {} bytes of {}, got {}", n * h.dtype.size(), h.dtype.as_str(), bytes.len())));
    }
    let values: Vec<f32> = match h.dtype {
        Dtype::Int16 => bytes.as_chunks::<2>().0.iter().map(|b| f32::from(i16::from_le_bytes(*b))).collect(),
        Dtype::Uint16 => bytes.as_chunks::<2>().0.iter().map(|b| f32::from(u16::from_le_bytes(*b))).collect(),
        Dtype::Float32 => bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect(),
    };
    Volume::from_physical(h.dims, h.spacing, &values)
        .map(|v| v.with_geometry(h.geometry))
        .map_err(|e| bad(e.to_string()))
}

/// HTTP status and protocol error code of an engine error.
pub fn error_status(e: &EngineError) -> (u16, &'static str) {
    match e {
        EngineError::BadRequest(_) | EngineError::Protocol(_) => (400, "bad_request"),
        EngineError::Unauthorized => (401, "unauthorized"),
        EngineError::NotFound => (404, "not_found"),
        EngineError::TooLarge(_) => (413, "too_large"),
        EngineError::Unsupported(_) => (422, "unsupported_prompt"),
        EngineError::Busy { .. } => (503, "busy"),
        EngineError::Unreachable(_) | EngineError::Internal(_) => (500, "internal"),
    }
}

/// Error body `{error: {code, message}}`.
pub fn error_body(code: &str, message: &str) -> Value {
    json!({ "error": { "code": code, "message": message } })
}

/// Maps an HTTP error response to an engine error.
pub fn error_from_response(status: u16, body: &str, retry_after_s: Option<u64>) -> EngineError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let err = parsed.as_ref().and_then(|v| v.get("error"));
    let code = err.and_then(|e| e.get("code")).and_then(Value::as_str).unwrap_or("");
    let message = err.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or(body).to_owned();
    match (code, status) {
        ("bad_request", _) | ("", 400) => EngineError::BadRequest(message),
        ("unauthorized", _) | ("", 401) => EngineError::Unauthorized,
        ("not_found", _) | ("", 404) => EngineError::NotFound,
        ("no_volume", _) | ("", 409) => EngineError::Protocol(format!("no volume uploaded: {message}")),
        ("too_large", _) | ("", 413) => EngineError::TooLarge(message),
        ("unsupported_prompt", _) | ("", 422) => EngineError::Unsupported(message),
        ("busy", _) | ("", 503) => EngineError::Busy { retry_after_s: retry_after_s.unwrap_or(1) },
        _ => EngineError::Internal(format!("HTTP {status}: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::IntensityRange;

    #[test]
    fn info_roundtrips_and_checks_the_protocol() {
        let info = EngineInfo {
            protocol: ENGINE_PROTOCOL.into(),
            name: "x".into(),
            capabilities: EngineCapabilities {
                interactive: true,
                prompts: vec![PromptKind::Point, PromptKind::Lasso],
                undo: true,
                ..Default::default()
            },
            modalities: vec!["CT".into()],
            labels: vec![
                EngineLabel { value: 1, name: "liver".into(), color: Some([1, 2, 3]) },
                EngineLabel { value: 2, name: "spleen".into(), color: None },
            ],
            research_only: true,
            max_voxels: 10,
            session_ttl_s: 60,
            ..Default::default()
        };
        assert_eq!(info_from_json(&info_to_json(&info)).unwrap(), info);
        let mut v = info_to_json(&info);
        v["capabilities"]["prompts"] = json!(["point", "polygon"]);
        v["unknown"] = json!(1);
        assert_eq!(info_from_json(&v).unwrap().capabilities.prompts, vec![PromptKind::Point]);
        v["protocol"] = json!("ferrum-engine/2");
        assert!(matches!(info_from_json(&v), Err(EngineError::Protocol(_))));
    }

    #[test]
    fn prompts_and_results_roundtrip() {
        let bx = VoxelBox::new(UVec3::new(1, 2, 3), UVec3::new(3, 4, 4));
        for p in [
            Prompt::Point { positive: false, voxel: UVec3::new(5, 6, 7) },
            Prompt::Box { positive: true, bx },
            Prompt::Scribble { positive: true, bx, mask: vec![0, 1, 1, 0] },
            Prompt::Lasso { positive: false, bx, mask: vec![1; 4] },
        ] {
            assert_eq!(prompt_from_json(&prompt_to_json(&p)).unwrap(), p);
        }
        assert!(matches!(prompt_from_json(&json!({"type": "polygon"})), Err(EngineError::Unsupported(_))));
        assert!(matches!(prompt_from_json(&json!({})), Err(EngineError::BadRequest(_))));
        assert!(prompt_from_json(&json!({"type": "lasso", "min": [0, 0, 0], "max": [1, 1, 1], "mask": "!!"})).is_err());
        assert!(prompt_from_json(&json!({"type": "point", "voxel": [1, -2, 3]})).is_err());
        for r in [
            PromptResult { revision: 3, changed: Some(bx), empty: false },
            PromptResult { revision: 4, changed: None, empty: true },
        ] {
            assert_eq!(result_from_json(&result_to_json(&r)).unwrap(), r);
        }
        assert!(result_from_json(&json!({})).is_err());
        assert_eq!(parse_box_query(&box_query(bx)).unwrap(), bx);
        assert!(parse_box_query("1,2,3").is_err());
        assert!(parse_box_query("a,2,3,4,5,6").is_err());
    }

    #[test]
    fn volumes_roundtrip_as_int16_or_float32() {
        let dims = Dims3::new(3, 2, 2);
        let vals: Vec<f32> = (0..12).map(|i| i as f32 * 100.0 - 1000.0).collect();
        let g = Geometry { origin: Vec3::new(-5.0, 2.0, 30.0), direction: Mat3::from_cols(Vec3::X, -Vec3::Z, Vec3::Y) };
        let v = Volume::from_physical(dims, Vec3::new(0.5, 0.5, 2.0), &vals).unwrap().with_geometry(g);
        for (modality, dtype) in [("CT", Dtype::Int16), ("MR", Dtype::Float32)] {
            let (h, bytes) = encode_volume(&v, modality);
            assert_eq!(h.dtype, dtype);
            let h = header_from_json(&header_to_json(&h)).unwrap();
            let back = decode_volume(&h, &bytes).unwrap();
            assert_eq!(back.geometry(), g);
            assert_eq!(back.spacing(), v.spacing());
            for (i, &expect) in vals.iter().enumerate() {
                let got = back.range().from_storage(back.data()[i]);
                assert!((got - expect).abs() < 0.1, "{modality}: {got} vs {expect}");
            }
            assert!(decode_volume(&h, &bytes[1..]).is_err());
        }
        let r = IntensityRange::new(0.0, 70_000.0).unwrap();
        let big = Volume::new(dims, Vec3::ONE, r, vec![65535; 12]).unwrap();
        assert_eq!(encode_volume(&big, "CT").0.dtype, Dtype::Float32);
        let h = header_from_json(&json!({"dims": [2, 2, 1], "dtype": "uint16", "spacing": [1, 1, 1]})).unwrap();
        assert_eq!(h.geometry, Geometry::default());
        assert!(decode_volume(&h, &[1, 0, 2, 0, 3, 0, 4, 0]).is_ok());
        assert!(header_from_json(&json!({"dims": [0, 2, 1], "dtype": "uint16", "spacing": [1, 1, 1]})).is_err());
        assert!(header_from_json(&json!({"dims": [2, 2, 1], "dtype": "int8", "spacing": [1, 1, 1]})).is_err());
    }

    #[test]
    fn jobs_labels_and_label_maps_roundtrip() {
        let j = JobStatus { state: JobState::Running, progress: 0.25, message: "liver".into() };
        assert_eq!(job_from_json(&job_to_json(&j)).unwrap(), j);
        assert!(job_from_json(&json!({"state": "paused"})).is_err());
        assert_eq!(job_from_json(&json!({"state": "done", "progress": 7})).unwrap().progress, 1.0);
        let names = vec!["liver".to_string(), "spleen".to_string()];
        assert_eq!(labels_from_json(&labels_to_json(Some(&names))).unwrap(), Some(names));
        assert_eq!(labels_from_json(&labels_to_json(None)).unwrap(), None);
        assert_eq!(labels_from_json(&json!({})).unwrap(), None);
        assert!(labels_from_json(&json!({"labels": [1]})).is_err());
        assert!(labels_from_json(&json!({"labels": "liver"})).is_err());
        let values = vec![0u16, 1, 117, 65535];
        assert_eq!(decode_label_map(&encode_label_map(&values), 4).unwrap(), values);
        assert!(decode_label_map(&[1, 2, 3], 2).is_err());
    }

    #[test]
    fn errors_map_both_ways() {
        let cases = [
            EngineError::BadRequest("m".into()),
            EngineError::Unauthorized,
            EngineError::NotFound,
            EngineError::TooLarge("m".into()),
            EngineError::Unsupported("m".into()),
            EngineError::Busy { retry_after_s: 7 },
        ];
        for e in cases {
            let (status, code) = error_status(&e);
            let body = error_body(code, "m").to_string();
            assert_eq!(error_from_response(status, &body, Some(7)), e);
        }
        assert_eq!(error_status(&EngineError::Internal("x".into())), (500, "internal"));
        assert!(matches!(error_from_response(409, "{}", None), EngineError::Protocol(_)));
        assert!(matches!(error_from_response(418, "teapot", None), EngineError::Internal(_)));
        assert_eq!(error_from_response(503, "", None), EngineError::Busy { retry_after_s: 1 });
    }
}
