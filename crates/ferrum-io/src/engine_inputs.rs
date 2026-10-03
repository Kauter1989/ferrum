//! Engine inputs (`ferrum-engine-inputs` v1): the prompts that produced each
//! interactive engine segment of a workspace, so the object can be refined
//! later by replaying them (`docs/agent-segmentation.md` §5.3).
//!
//! ```json
//! { "format": "ferrum-engine-inputs", "version": 1,
//!   "objects": [ { "label": 7, "created": "2026-10-03T10:00:00Z",
//!                  "engine": "http://127.0.0.1:8765", "engine_name": "nnInteractive", "engine_version": "2.6.0",
//!                  "roi": { "min": [180, 140, 60], "max": [330, 290, 120] }, "revision": 2, "seeds": 0,
//!                  "prompts": [ { "type": "point", "positive": true, "voxel": [251, 198, 87] },
//!                               { "type": "box", "positive": true, "min": [200, 150, 87], "max": [300, 260, 88] },
//!                               { "type": "lasso", "positive": true, "min": [..], "max": [..], "runs": [0, 12, 40, 9] } ] } ] }
//! ```
//!
//! Prompt coordinates are voxels of the full volume grid; boxes are
//! half-open. Scribble and lasso masks are stored as `runs`: pairs of
//! (start, length) of the inside voxels in the box, `i` fastest. An entry
//! belongs to the segment with the same `label` and `created` time; entries
//! of deleted or replaced segments are ignored.

use std::path::Path;

use ferrum_domain::{Prompt, Timestamp, VoxelBox};
use glam::UVec3;
use serde_json::{json, Value};

use crate::error::IoError;

/// Value of the `format` field.
pub const ENGINE_INPUTS_FORMAT: &str = "ferrum-engine-inputs";
/// Version of the document layout.
pub const ENGINE_INPUTS_VERSION: u32 = 1;

/// The prompts behind one interactive engine segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInput {
    /// Segment label.
    pub label: u8,
    /// Creation time of the segment (from its provenance).
    pub created: Option<Timestamp>,
    /// Engine base URL.
    pub engine: String,
    /// Engine name.
    pub engine_name: String,
    /// Engine version.
    pub engine_version: String,
    /// Region of the volume uploaded to the engine.
    pub roi: VoxelBox,
    /// Number of times the object was computed.
    pub revision: u64,
    /// Leading prompts derived from an earlier mask (lasso seeds), which
    /// do not count against the agent's prompt limit.
    pub seeds: usize,
    /// Prompts in order, in voxels of the full grid.
    pub prompts: Vec<Prompt>,
}

fn box_json(bx: VoxelBox) -> (Value, Value) {
    (json!(bx.min.to_array()), json!(bx.max.to_array()))
}

/// Runs (start, length) of the non-zero values of `mask`.
pub fn mask_runs(mask: &[u8]) -> Vec<u64> {
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for (n, v) in mask.iter().chain(std::iter::once(&0)).enumerate() {
        match (start, *v != 0) {
            (None, true) => start = Some(n),
            (Some(s), false) => {
                runs.extend([s as u64, (n - s) as u64]);
                start = None;
            }
            _ => {}
        }
    }
    runs
}

/// The mask of `len` values described by `runs`.
pub fn mask_from_runs(runs: &[u64], len: usize) -> Result<Vec<u8>, String> {
    if !runs.len().is_multiple_of(2) {
        return Err("runs must be (start, length) pairs".into());
    }
    let mut mask = vec![0u8; len];
    for pair in runs.chunks(2) {
        let (s, n) = (pair[0] as usize, pair[1] as usize);
        let end = s.checked_add(n).filter(|e| *e <= len).ok_or("a run exceeds the box")?;
        mask[s..end].fill(1);
    }
    Ok(mask)
}

/// JSON of one prompt.
pub fn prompt_json(p: &Prompt) -> Value {
    match p {
        Prompt::Point { positive, voxel } => {
            json!({ "type": "point", "positive": positive, "voxel": voxel.to_array() })
        }
        Prompt::Box { positive, bx } => {
            let (min, max) = box_json(*bx);
            json!({ "type": "box", "positive": positive, "min": min, "max": max })
        }
        Prompt::Scribble { positive, bx, mask } | Prompt::Lasso { positive, bx, mask } => {
            let (min, max) = box_json(*bx);
            json!({ "type": p.kind().as_str(), "positive": positive, "min": min, "max": max, "runs": mask_runs(mask) })
        }
    }
}

fn uvec(v: &Value, what: &str) -> Result<UVec3, String> {
    let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| format!("{what}: expected [i, j, k]"))?;
    let n: Vec<u32> = a.iter().filter_map(|x| x.as_u64().and_then(|x| u32::try_from(x).ok())).collect();
    <[u32; 3]>::try_from(n).map(UVec3::from).map_err(|_| format!("{what}: expected [i, j, k]"))
}

fn box_from(v: &Value) -> Result<VoxelBox, String> {
    Ok(VoxelBox::new(uvec(&v["min"], "min")?, uvec(&v["max"], "max")?))
}

/// Parses [`prompt_json`] output.
pub fn prompt_from_json(v: &Value) -> Result<Prompt, String> {
    let positive = v["positive"].as_bool().unwrap_or(true);
    match v["type"].as_str() {
        Some("point") => Ok(Prompt::Point { positive, voxel: uvec(&v["voxel"], "voxel")? }),
        Some("box") => Ok(Prompt::Box { positive, bx: box_from(v)? }),
        Some(kind @ ("scribble" | "lasso")) => {
            let bx = box_from(v)?;
            let runs: Vec<u64> =
                v["runs"].as_array().ok_or("runs: expected a list")?.iter().filter_map(Value::as_u64).collect();
            let mask = mask_from_runs(&runs, bx.voxel_count())?;
            Ok(if kind == "lasso" {
                Prompt::Lasso { positive, bx, mask }
            } else {
                Prompt::Scribble { positive, bx, mask }
            })
        }
        other => Err(format!("unknown prompt type {other:?}")),
    }
}

/// Document holding `inputs`.
pub fn engine_inputs_json(inputs: &[EngineInput], generator: &str) -> Value {
    let objects: Vec<Value> = inputs
        .iter()
        .map(|e| {
            let (min, max) = box_json(e.roi);
            json!({
                "label": e.label,
                "created": e.created.map(Timestamp::to_rfc3339),
                "engine": e.engine,
                "engine_name": e.engine_name,
                "engine_version": e.engine_version,
                "roi": { "min": min, "max": max },
                "revision": e.revision,
                "seeds": e.seeds,
                "prompts": e.prompts.iter().map(prompt_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "format": ENGINE_INPUTS_FORMAT, "version": ENGINE_INPUTS_VERSION, "generator": generator, "objects": objects })
}

fn input_from_json(o: &Value) -> Result<EngineInput, String> {
    let text = |k: &str| o[k].as_str().map(str::to_owned).unwrap_or_default();
    let created = match &o["created"] {
        Value::Null => None,
        t => Some(t.as_str().and_then(Timestamp::parse_rfc3339).ok_or("created: expected an RFC 3339 time")?),
    };
    let prompts = o["prompts"].as_array().ok_or("prompts: expected a list")?;
    Ok(EngineInput {
        label: o["label"]
            .as_u64()
            .and_then(|l| u8::try_from(l).ok())
            .filter(|l| *l != 0)
            .ok_or("label must be in 1..=255")?,
        created,
        engine: text("engine"),
        engine_name: text("engine_name"),
        engine_version: text("engine_version"),
        roi: box_from(&o["roi"])?,
        revision: o["revision"].as_u64().unwrap_or(1),
        seeds: o["seeds"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(0),
        prompts: prompts.iter().map(prompt_from_json).collect::<Result<_, _>>()?,
    })
}

/// Parses an [`engine_inputs_json`] document.
pub fn engine_inputs_from_json(doc: &Value) -> Result<Vec<EngineInput>, String> {
    if doc["format"] != ENGINE_INPUTS_FORMAT {
        return Err(format!("not a {ENGINE_INPUTS_FORMAT} document"));
    }
    if doc["version"].as_u64() != Some(u64::from(ENGINE_INPUTS_VERSION)) {
        return Err(format!("unsupported {ENGINE_INPUTS_FORMAT} version {}", doc["version"]));
    }
    let list = doc["objects"].as_array().ok_or("objects: expected a list")?;
    list.iter().enumerate().map(|(n, o)| input_from_json(o).map_err(|m| format!("object {n}: {m}"))).collect()
}

/// Writes [`engine_inputs_json`] to `path`.
pub fn write_engine_inputs(inputs: &[EngineInput], generator: &str, path: &Path) -> Result<(), IoError> {
    let text = serde_json::to_string_pretty(&engine_inputs_json(inputs, generator))
        .map_err(|e| IoError::invalid(path, e.to_string()))?;
    std::fs::write(path, text).map_err(|e| IoError::os(path, e))
}

/// Reads a document written by [`write_engine_inputs`].
pub fn read_engine_inputs(path: &Path) -> Result<Vec<EngineInput>, IoError> {
    let text = std::fs::read_to_string(path).map_err(|e| IoError::os(path, e))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| IoError::parse(path, e))?;
    engine_inputs_from_json(&doc).map_err(|m| IoError::invalid(path, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EngineInput {
        let bx = VoxelBox::new(UVec3::new(1, 2, 3), UVec3::new(4, 6, 4));
        let mask: Vec<u8> = (0..bx.voxel_count()).map(|n| u8::from(n % 5 < 2)).collect();
        EngineInput {
            label: 7,
            created: Some(Timestamp(1_790_000_000)),
            engine: "http://127.0.0.1:8765".into(),
            engine_name: "nnInteractive".into(),
            engine_version: "2.6".into(),
            roi: VoxelBox::new(UVec3::ZERO, UVec3::splat(10)),
            revision: 3,
            seeds: 0,
            prompts: vec![
                Prompt::Point { positive: true, voxel: UVec3::new(2, 3, 3) },
                Prompt::Box { positive: false, bx },
                Prompt::Lasso { positive: true, bx, mask: mask.clone() },
                Prompt::Scribble { positive: false, bx, mask },
            ],
        }
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("engine_inputs.json");
        write_engine_inputs(&[sample()], "FERRUM test", &path).unwrap();
        assert_eq!(read_engine_inputs(&path).unwrap(), vec![sample()]);
        std::fs::write(&path, "{").unwrap();
        assert!(matches!(read_engine_inputs(&path), Err(IoError::Parse { .. })));
        assert!(matches!(read_engine_inputs(&dir.path().join("none")), Err(IoError::Os { .. })));
    }

    #[test]
    fn runs_and_errors() {
        assert_eq!(mask_runs(&[0, 1, 1, 0, 1]), vec![1, 2, 4, 1]);
        assert_eq!(mask_from_runs(&[1, 2, 4, 1], 5).unwrap(), vec![0, 1, 1, 0, 1]);
        assert!(mask_from_runs(&[1], 5).is_err() && mask_from_runs(&[4, 2], 5).is_err());
        let doc = |objects: Value| json!({ "format": ENGINE_INPUTS_FORMAT, "version": 1, "objects": objects });
        let good = engine_inputs_json(&[sample()], "g");
        assert_eq!(engine_inputs_from_json(&good).unwrap().len(), 1);
        for bad in [
            json!({ "format": "x", "version": 1, "objects": [] }),
            json!({ "format": ENGINE_INPUTS_FORMAT, "version": 2, "objects": [] }),
            json!({ "format": ENGINE_INPUTS_FORMAT, "version": 1 }),
            doc(json!([{ "label": 0 }])),
            doc(json!([{ "label": 1, "roi": { "min": [0, 0], "max": [1, 1, 1] }, "prompts": [] }])),
            doc(
                json!([{ "label": 1, "roi": { "min": [0, 0, 0], "max": [1, 1, 1] }, "prompts": [{ "type": "blob" }] }]),
            ),
            doc(json!([{ "label": 1, "created": 5, "roi": { "min": [0, 0, 0], "max": [1, 1, 1] }, "prompts": [] }])),
            doc(json!([{ "label": 1, "roi": { "min": [0, 0, 0], "max": [1, 1, 1] } }])),
            doc(
                json!([{ "label": 1, "roi": { "min": [0, 0, 0], "max": [1, 1, 1] }, "prompts": [{ "type": "lasso", "min": [0, 0, 0], "max": [1, 1, 1] }] }]),
            ),
        ] {
            assert!(engine_inputs_from_json(&bad).is_err(), "{bad}");
        }
    }
}
