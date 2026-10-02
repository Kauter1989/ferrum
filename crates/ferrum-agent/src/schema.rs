//! JSON Schemas (draft 2020-12 subset) of every command's parameters.
//!
//! The MCP server publishes them as tool input schemas, `ferrum-cli schema`
//! prints them, and [`validate`] checks every call before it runs, so a
//! misspelt parameter is an error instead of being ignored.

use serde_json::{json, Map, Value};

const POINT_HINT: &str =
    r#"points are {"voxel": [i, j, k]}, {"patient_mm": [x, y, z]} or {"render": "r-0001", "pixel": [x, y]}"#;

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

fn numbers(n: usize, description: &str) -> Value {
    json!({ "type": "array", "items": { "type": "number" }, "minItems": n, "maxItems": n, "description": description })
}

/// A point in one of the three accepted forms.
pub fn point(description: &str) -> Value {
    json!({
        "description": description,
        "oneOf": [
            obj(json!({ "voxel": numbers(3, "0-based voxel (i, j, k) of the canonical LPS grid; integers are voxel centres") }), &["voxel"]),
            obj(json!({ "patient_mm": numbers(3, "LPS patient millimetres") }), &["patient_mm"]),
            obj(
                json!({
                    "render": { "type": "string", "pattern": "^r-[0-9]+$", "description": "id of an earlier render" },
                    "pixel": numbers(2, "pixel (x, y) of that render; pixel centres lie at +0.5"),
                }),
                &["render", "pixel"],
            ),
        ],
    })
}

fn points(min: usize, description: &str) -> Value {
    json!({ "type": "array", "items": point("a point"), "minItems": min, "description": description })
}

fn workspace() -> Value {
    json!({ "type": "string", "description": "workspace directory; relative paths go below the operator's workspace root" })
}

fn text(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn integer(min: u64, description: &str) -> Value {
    json!({ "type": "integer", "minimum": min, "description": description })
}

fn number(description: &str) -> Value {
    json!({ "type": "number", "description": description })
}

fn plane() -> Value {
    json!({ "type": "string", "enum": ["axial", "coronal", "sagittal"], "description": "slice orientation" })
}

fn agent() -> Value {
    text("id of the agent or harness session, recorded in the provenance of what it creates")
}

fn ws_only() -> Value {
    obj(json!({ "workspace": workspace() }), &["workspace"])
}

fn view_slice() -> (&'static str, Value) {
    (
            "Renders one slice to a PNG with a sidecar mapping pixels to voxels and patient mm. Look with images; take numbers from probe, stats and measure.",
            obj(
                json!({
                    "workspace": workspace(),
                    "plane": plane(),
                    "slice_number": integer(1, "1-based slice number"),
                    "at": point("the slice through this point (instead of slice_number)"),
                    "window": {
                        "description": "window preset or explicit window (default: the series' own)",
                        "oneOf": [
                            { "type": "string", "enum": ["full_range", "brain", "soft_tissue", "lung", "bone"] },
                            obj(json!({ "center": number("window centre"), "width": number("window width") }), &["center", "width"]),
                        ],
                    },
                    "size": integer(16, "largest image side in pixels (capped by the operator)"),
                    "overlays": { "type": "array", "items": { "type": "string", "enum": ["segments"] }, "description": "drawn on top: segment outlines" },
                }),
                &["workspace", "plane"],
            ),
    )
}

fn window_param() -> Value {
    json!({
        "description": "window preset or explicit window (default: the series' own)",
        "oneOf": [
            { "type": "string", "enum": ["full_range", "brain", "soft_tissue", "lung", "bone"] },
            obj(json!({ "center": number("window centre"), "width": number("window width") }), &["center", "width"]),
        ],
    })
}

fn overlays_param() -> Value {
    json!({ "type": "array", "items": { "type": "string", "enum": ["segments"] }, "description": "drawn on top: segment outlines" })
}

type Described = (&'static str, Value);

fn views() -> (Described, Described) {
    let montage = (
        "Renders several slices of one plane as a labelled grid (PNG + sidecar with a mapping per tile). Use it to find where something is, then view slice for detail.",
        obj(
            json!({
                "workspace": workspace(),
                "plane": plane(),
                "from": integer(1, "first slice number (default 1)"),
                "to": integer(1, "last slice number (default: the last)"),
                "step": integer(1, "every n-th slice (default: at most 16 tiles)"),
                "columns": integer(1, "tiles per row (default: a square grid)"),
                "window": window_param(),
                "size": integer(16, "largest image side in pixels (capped by the operator)"),
                "overlays": overlays_param(),
            }),
            &["workspace", "plane"],
        ),
    );
    let mpr = (
        "Renders axial, coronal and sagittal slices through a point side by side, with a crosshair at the point (PNG + sidecar with a mapping per tile).",
        obj(
            json!({
                "workspace": workspace(),
                "at": point("the point the three planes pass through"),
                "window": window_param(),
                "size": integer(16, "largest image side in pixels (capped by the operator)"),
                "overlays": overlays_param(),
            }),
            &["workspace", "at"],
        ),
    );
    (montage, mpr)
}

fn volume_view() -> (&'static str, Value) {
    (
            "Renders the volume in 3D on the CPU from a standard viewpoint (PNG). Shows shape and context; its pixels do not map to voxels, so measure on slices.",
            obj(
                json!({
                    "workspace": workspace(),
                    "mode": { "type": "string", "enum": ["mip", "isosurface", "transfer_function"], "description": "maximum intensity projection (default), shaded isosurface at threshold, or a CT transfer-function preset" },
                    "threshold": number("isosurface value, e.g. 300 for bone in HU"),
                    "preset": { "type": "string", "enum": ["soft_tissue_bone", "lung_vessels", "bone"], "description": "CT transfer-function preset (mode transfer_function)" },
                    "view": { "type": "string", "enum": ["anterior", "posterior", "left", "right", "superior", "inferior"], "description": "viewpoint (default anterior)" },
                    "size": integer(16, "image side in pixels (default 512, capped by the operator)"),
                    "overlays": overlays_param(),
                }),
                &["workspace"],
            ),
    )
}

fn stats() -> (&'static str, Value) {
    (
        "Statistics of the values in one region: voxels, volume in ml, mean, std, min, max, percentiles.",
        obj(
            json!({
                "workspace": workspace(),
                "box": obj(json!({ "min": point("one corner"), "max": point("the opposite corner") }), &["min", "max"]),
                "sphere": obj(json!({ "center": point("centre"), "radius_mm": number("radius in mm") }), &["center", "radius_mm"]),
                "segment": integer(1, "segment label"),
                "annotation": integer(0, "id of an area or rectangle annotation"),
            }),
            &["workspace"],
        ),
    )
}

/// Description and parameter schema of `command`, or `None` if unknown.
pub fn command_schema(command: &str) -> Option<(&'static str, Value)> {
    let measure = |n: usize, what: &'static str| {
        (what, obj(json!({ "workspace": workspace(), "points": points(n, "the points") }), &["workspace", "points"]))
    };
    Some(match command {
        "study scan" => (
            "Lists the series in files and folders: pseudonymised id, format, modality, size, description.",
            obj(json!({ "paths": { "type": "array", "items": text("a DICOM folder or file, or a NIfTI file"), "minItems": 1 } }), &["paths"]),
        ),
        "study open" => (
            "Opens a series into a workspace (created if needed; the source files are hashed) and describes it.",
            obj(
                json!({ "workspace": workspace(), "path": text("DICOM folder or NIfTI file"), "series": text("series from study scan, when the source holds several") }),
                &["workspace", "path"],
            ),
        ),
        "study info" => ("Describes the open study: size, spacing, patient geometry, value unit, window presets, annotation and segment counts.", ws_only()),
        "view slice" => view_slice(),
        "view montage" => views().0,
        "view mpr" => views().1,
        "view volume" => volume_view(),
        "profile" => (
            "Values along a line between two points (nearest voxel at evenly spaced samples), with distances in mm.",
            obj(
                json!({
                    "workspace": workspace(),
                    "from": point("start"),
                    "to": point("end"),
                    "samples": { "type": "integer", "minimum": 2, "maximum": 2000, "description": "number of samples (default: one per smallest voxel spacing)" },
                }),
                &["workspace", "from", "to"],
            ),
        ),
        "export bundle" => (
            "Writes export/ in the workspace: report.json (study, measurements, segments with volumes; unconfirmed items marked), annotations.json, segments.nii.gz + segments.json, with SHA-256 of every file.",
            ws_only(),
        ),
        "probe" => (
            "Value of the voxel nearest to a point, with unit (HU for CT).",
            obj(json!({ "workspace": workspace(), "point": point("where to probe") }), &["workspace", "point"]),
        ),
        "stats" => stats(),
        "measure distance" => measure(2, "Distance between two points in mm, with an uncertainty of one voxel spacing."),
        "measure angle" => measure(3, "Angle at the second of three points, in degrees."),
        "measure area" => measure(3, "Area of a planar polygon through the points, in mm²."),
        "annotate add" => (
            "Adds an annotation on one slice, proposed by the agent for review.",
            obj(
                json!({
                    "workspace": workspace(),
                    "kind": { "type": "string", "enum": ["distance", "angle", "area", "rectangle", "text"] },
                    "plane": plane(),
                    "points": points(1, "points on one slice: distance 2, angle 3, area ≥ 3, rectangle 2 opposite corners, text 1"),
                    "name": text("name shown in the viewer"),
                    "text": text("text of a text annotation"),
                    "agent": agent(),
                }),
                &["workspace", "kind", "plane", "points"],
            ),
        ),
        "annotate list" => ("Lists annotations with values, points and provenance.", ws_only()),
        "annotate rename" => (
            "Renames an annotation the agent created.",
            obj(json!({ "workspace": workspace(), "id": integer(0, "annotation id"), "name": text("new name") }), &["workspace", "id", "name"]),
        ),
        "annotate delete" => (
            "Deletes an annotation the agent created.",
            obj(json!({ "workspace": workspace(), "id": integer(0, "annotation id") }), &["workspace", "id"]),
        ),
        "segment list" => ("Lists segments with voxel count, volume in ml and provenance.", ws_only()),
        "segment threshold" => (
            "Region growing (6-connected) from a seed within a value range; the result is a new segment proposed by the agent. Existing segments are kept.",
            obj(
                json!({
                    "workspace": workspace(),
                    "seed": point("seed point; probe it first"),
                    "min": number("lowest value (e.g. HU)"),
                    "max": number("highest value"),
                    "max_ml": number("refuse regions larger than this"),
                    "name": text("segment name"),
                    "agent": agent(),
                }),
                &["workspace", "seed", "min", "max"],
            ),
        ),
        "segment rename" => (
            "Renames a segment the agent created.",
            obj(json!({ "workspace": workspace(), "label": integer(1, "segment label"), "name": text("new name") }), &["workspace", "label", "name"]),
        ),
        "segment delete" => (
            "Deletes a segment the agent created.",
            obj(json!({ "workspace": workspace(), "label": integer(1, "segment label") }), &["workspace", "label"]),
        ),
        "review list" => ("Lists the annotations and segments waiting for a person's review.", ws_only()),
        "review confirm" | "review reject" => (
            if command == "review confirm" {
                "Confirms an item in the name of a person; only if the operator allows harness review."
            } else {
                "Rejects an item in the name of a person; only if the operator allows harness review."
            },
            obj(
                json!({
                    "workspace": workspace(),
                    "annotation": integer(0, "annotation id"),
                    "segment": integer(1, "segment label"),
                    "by": text("the person who decided"),
                }),
                &["workspace", "by"],
            ),
        ),
        _ => return None,
    })
}

/// Checks `value` against `schema`; the error names the offending path.
pub fn validate(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(alts) = schema["oneOf"].as_array() {
        let ok = alts.iter().filter(|s| validate(s, value, path).is_ok()).count();
        return match ok {
            1 => Ok(()),
            0 if alts.iter().any(|a| a["properties"].get("voxel").is_some()) => {
                Err(format!("{path}: not a point; {POINT_HINT}"))
            }
            0 => Err(format!("{path}: does not match any allowed form")),
            _ => Err(format!("{path}: matches several forms")),
        };
    }
    if let Some(t) = schema["type"].as_str() {
        let fits = match t {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.as_f64().is_some_and(f64::is_finite),
            "integer" => value.is_u64() || value.is_i64(),
            "boolean" => value.is_boolean(),
            _ => true,
        };
        if !fits {
            return Err(format!("{path}: expected {t}, got {value}"));
        }
    }
    if let Some(e) = schema["enum"].as_array() {
        if !e.contains(value) {
            return Err(format!("{path}: {value} is not one of {}", Value::Array(e.clone())));
        }
    }
    if let (Some(min), Some(x)) = (schema["minimum"].as_f64(), value.as_f64()) {
        if x < min {
            return Err(format!("{path}: {x} is below the minimum {min}"));
        }
    }
    if let (Some(max), Some(x)) = (schema["maximum"].as_f64(), value.as_f64()) {
        if x > max {
            return Err(format!("{path}: {x} is above the maximum {max}"));
        }
    }
    if let Some(pattern_ok) = schema["pattern"].as_str().map(|_| value.as_str().is_some_and(is_render_id)) {
        if !pattern_ok {
            return Err(format!("{path}: {value} is not a render id like r-0001"));
        }
    }
    if let Some(a) = value.as_array() {
        validate_array(schema, a, path)?;
    }
    if let Some(o) = value.as_object() {
        validate_object(schema, o, path)?;
    }
    Ok(())
}

fn is_render_id(s: &str) -> bool {
    s.strip_prefix("r-").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn validate_array(schema: &Value, a: &[Value], path: &str) -> Result<(), String> {
    if let Some(min) = schema["minItems"].as_u64() {
        if (a.len() as u64) < min {
            return Err(format!("{path}: needs at least {min} items, got {}", a.len()));
        }
    }
    if let Some(max) = schema["maxItems"].as_u64() {
        if (a.len() as u64) > max {
            return Err(format!("{path}: allows at most {max} items, got {}", a.len()));
        }
    }
    if schema.get("items").is_some() {
        for (i, item) in a.iter().enumerate() {
            validate(&schema["items"], item, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

fn validate_object(schema: &Value, o: &Map<String, Value>, path: &str) -> Result<(), String> {
    for r in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if o.get(r).is_none_or(Value::is_null) {
            return Err(format!("{path}.{r} is required"));
        }
    }
    let props = schema["properties"].as_object();
    for (k, v) in o {
        match props.and_then(|p| p.get(k)) {
            Some(_) if v.is_null() => {}
            Some(s) => validate(s, v, &format!("{path}.{k}"))?,
            None if schema["additionalProperties"] == false => {
                let known: Vec<&str> = props.map(|p| p.keys().map(String::as_str).collect()).unwrap_or_default();
                return Err(format!("{path}.{k} is not a parameter; known: {}", known.join(", ")));
            }
            None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_a_schema() {
        for name in crate::Agent::commands() {
            let (description, schema) = command_schema(name).unwrap_or_else(|| panic!("{name}"));
            assert!(!description.is_empty());
            assert_eq!(schema["type"], "object", "{name}");
            assert_eq!(schema["additionalProperties"], false, "{name}");
        }
        assert!(command_schema("frobnicate").is_none());
    }

    #[test]
    fn validation_errors_name_the_path() {
        let (_, s) = command_schema("measure distance").unwrap();
        let ok =
            json!({ "workspace": "w", "points": [{ "voxel": [0, 0, 0] }, { "render": "r-0001", "pixel": [1.5, 2] }] });
        assert_eq!(validate(&s, &ok, "params"), Ok(()));
        let cases = [
            (json!({ "points": [] }), "params.workspace is required"),
            (
                json!({ "workspace": "w", "points": [{ "voxel": [0, 0] }, { "voxel": [0, 0, 0] }] }),
                "params.points[0]: not a point",
            ),
            (json!({ "workspace": "w", "points": [{ "voxel": [0, 0, 0] }] }), "needs at least 2 items"),
            (json!({ "workspace": "w", "point": [] , "points": [] }), "params.point is not a parameter"),
            (json!({ "workspace": 3, "points": [] }), "params.workspace: expected string"),
            (
                json!({ "workspace": "w", "points": [{ "render": "x", "pixel": [1, 2] }, { "voxel": [0, 0, 0] }] }),
                "not a point",
            ),
        ];
        for (v, expected) in cases {
            let e = validate(&s, &v, "params").unwrap_err();
            assert!(e.contains(expected), "{e} does not contain {expected}");
        }
        let (_, slice) = command_schema("view slice").unwrap();
        let v = |extra: Value| {
            let mut p = json!({ "workspace": "w", "plane": "axial", "slice_number": 1 });
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            validate(&slice, &p, "params")
        };
        assert!(v(json!({ "window": "lung" })).is_ok());
        assert!(v(json!({ "window": { "center": -600, "width": 1500 } })).is_ok());
        assert!(v(json!({ "window": "neon" })).is_err());
        assert!(v(json!({ "slice_number": 0 })).unwrap_err().contains("below the minimum"));
        assert!(v(json!({ "plane": "oblique" })).unwrap_err().contains("is not one of"));
        assert!(v(json!({ "size": 1.5 })).unwrap_err().contains("expected integer"));
        assert!(v(json!({ "overlays": ["labels"] })).is_err());
        assert!(v(json!({ "at": null })).is_ok(), "null means absent");
        assert!(validate(&json!({ "type": "boolean" }), &json!(1), "x").is_err());
        assert!(validate(&json!({ "type": "array", "maxItems": 1 }), &json!([1, 2]), "x").is_err());
        assert!(validate(&json!({ "oneOf": [{ "type": "number" }, { "type": "integer" }] }), &json!(1), "x")
            .unwrap_err()
            .contains("several"));
        assert!(validate(&json!({ "oneOf": [{ "type": "string" }] }), &json!(1), "x")
            .unwrap_err()
            .contains("any allowed form"));
    }
}
