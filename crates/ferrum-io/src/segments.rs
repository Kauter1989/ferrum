//! Segment metadata (`ferrum-segments`): names, colours, display settings
//! and provenance of the labels of a label map.
//!
//! The label map itself is a NIfTI file ([`crate::write_label_nifti`]);
//! this JSON sidecar keeps what NIfTI cannot hold.
//!
//! ```json
//! { "format": "ferrum-segments", "version": 1,
//!   "segments": [ { "label": 1, "name": "Liver", "color": [230, 85, 75], "visible": true,
//!                   "opacity": 0.5, "voxels": 18234, "volume_ml": 412.7, "provenance": { … } } ] }
//! ```

use std::path::Path;

use ferrum_domain::{Segment, SegmentationSet};
use glam::Vec3;
use serde_json::{json, Value};

use crate::error::IoError;
use crate::provenance::{provenance_from_json, provenance_json};

/// Value of the `format` field.
pub const SEGMENTS_FORMAT: &str = "ferrum-segments";
/// Version of the document layout.
pub const SEGMENTS_VERSION: u32 = 1;

/// Metadata document of the segments of `set` on a grid with `spacing`
/// (mm). Voxel counts and volumes are informative; readers ignore them.
pub fn segments_json(set: &SegmentationSet, spacing: Vec3, generator: &str) -> Value {
    let segments: Vec<Value> = set
        .segments()
        .iter()
        .map(|s| {
            json!({
                "label": s.label,
                "name": s.name,
                "color": s.color,
                "visible": s.visible,
                "opacity": (f64::from(s.opacity) * 1e3).round() / 1e3,
                "voxels": set.voxel_count(s.label),
                "volume_ml": (set.volume_ml(s.label, spacing) * 1e3).round() / 1e3,
                "provenance": provenance_json(&s.provenance),
            })
        })
        .collect();
    json!({ "format": SEGMENTS_FORMAT, "version": SEGMENTS_VERSION, "generator": generator, "segments": segments })
}

/// Parses a [`segments_json`] document into segments, in document order.
pub fn segments_from_json(doc: &Value) -> Result<Vec<Segment>, String> {
    if doc["format"] != SEGMENTS_FORMAT {
        return Err(format!("not a {SEGMENTS_FORMAT} document"));
    }
    if doc["version"].as_u64() != Some(u64::from(SEGMENTS_VERSION)) {
        return Err(format!("unsupported {SEGMENTS_FORMAT} version {}", doc["version"]));
    }
    let list = doc["segments"].as_array().ok_or("segments: expected a list")?;
    let mut out: Vec<Segment> = Vec::with_capacity(list.len());
    for (n, s) in list.iter().enumerate() {
        let fail = |m: &str| format!("segment {n}: {m}");
        let label = s["label"].as_u64().and_then(|l| u8::try_from(l).ok()).filter(|l| *l != 0);
        let label = label.ok_or_else(|| fail("label must be in 1..=255"))?;
        if out.iter().any(|o| o.label == label) {
            return Err(fail(&format!("label {label} appears twice")));
        }
        let mut seg = Segment::new(label, s["name"].as_str().unwrap_or_default().trim());
        if seg.name.is_empty() {
            seg.name = format!("Segment {label}");
        }
        if let Some(c) = s.get("color") {
            let rgb: Option<Vec<u8>> =
                c.as_array().map(|a| a.iter().filter_map(|v| v.as_u64().and_then(|v| u8::try_from(v).ok())).collect());
            seg.color = rgb.and_then(|v| <[u8; 3]>::try_from(v).ok()).ok_or_else(|| fail("color must be [r, g, b]"))?;
        }
        seg.visible = s["visible"].as_bool().unwrap_or(true);
        if let Some(o) = s["opacity"].as_f64() {
            seg.opacity = (o as f32).clamp(0.0, 1.0);
        }
        seg.provenance = provenance_from_json(&s["provenance"]).map_err(|m| fail(&m))?;
        out.push(seg);
    }
    Ok(out)
}

/// Writes [`segments_json`] to `path`.
pub fn write_segments(set: &SegmentationSet, spacing: Vec3, generator: &str, path: &Path) -> Result<(), IoError> {
    let text = serde_json::to_string_pretty(&segments_json(set, spacing, generator))
        .map_err(|e| IoError::invalid(path, e.to_string()))?;
    std::fs::write(path, text).map_err(|e| IoError::os(path, e))
}

/// Reads a document written by [`write_segments`].
pub fn read_segments(path: &Path) -> Result<Vec<Segment>, IoError> {
    let text = std::fs::read_to_string(path).map_err(|e| IoError::os(path, e))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| IoError::parse(path, e))?;
    segments_from_json(&doc).map_err(|m| IoError::invalid(path, m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::{Dims3, LabelMap, Provenance, Timestamp};

    fn set() -> SegmentationSet {
        let mut data = vec![0u8; 8];
        data[..3].fill(2);
        data[7] = 5;
        let mut set = SegmentationSet::from_labels(LabelMap::from_data(Dims3::new(2, 2, 2), data).unwrap(), vec![]);
        set.rename(2, "Liver");
        set.set_color(5, [1, 2, 3]).unwrap();
        set.set_visible(5, false).unwrap();
        set.set_opacity(5, 0.25).unwrap();
        set.set_provenance(5, Provenance::engine("TotalSegmentator", "2.18 (total)", false, Timestamp(7))).unwrap();
        set
    }

    #[test]
    fn round_trip() {
        let set = set();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("segments.json");
        write_segments(&set, Vec3::new(10.0, 10.0, 10.0), "FERRUM test", &path).unwrap();
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            (doc["segments"][0]["voxels"].as_u64(), doc["segments"][0]["volume_ml"].as_f64()),
            (Some(3), Some(3.0))
        );
        assert_eq!(read_segments(&path).unwrap(), set.segments());
        assert!(matches!(read_segments(&dir.path().join("none.json")), Err(IoError::Os { .. })));
        std::fs::write(&path, "[").unwrap();
        assert!(matches!(read_segments(&path), Err(IoError::Parse { .. })));
        std::fs::write(&path, "{}").unwrap();
        assert!(matches!(read_segments(&path), Err(IoError::Invalid { .. })));
    }

    #[test]
    fn defaults_and_errors() {
        let doc = |segments: Value| json!({ "format": SEGMENTS_FORMAT, "version": 1, "segments": segments });
        let minimal = segments_from_json(&doc(json!([{ "label": 4, "name": " " }]))).unwrap();
        assert_eq!(minimal, vec![Segment::new(4, "Segment 4")]);
        for bad in [
            json!({ "format": SEGMENTS_FORMAT, "version": 2, "segments": [] }),
            json!({ "format": SEGMENTS_FORMAT, "version": 1 }),
            doc(json!([{ "label": 0 }])),
            doc(json!([{ "label": 256 }])),
            doc(json!([{ "label": 1 }, { "label": 1 }])),
            doc(json!([{ "label": 1, "color": [1, 2] }])),
            doc(json!([{ "label": 1, "color": [1, 2, 300] }])),
            doc(json!([{ "label": 1, "provenance": { "status": "x" } }])),
        ] {
            assert!(segments_from_json(&bad).is_err(), "{bad}");
        }
    }
}
