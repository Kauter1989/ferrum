//! JSON documents of annotations (`ferrum-annotations`).
//!
//! The document is self-describing (`format`, `version`) and unambiguous:
//! every annotation carries its plane, slice index, in-plane millimetre
//! points, the same points as voxel coordinates of the volume and, since
//! version 2, its provenance (author and review status). Versions 1 and 2
//! can be read back into an [`AnnotationSet`].

use std::path::Path;

use ferrum_domain::{Annotation, AnnotationReport, AnnotationSet, SliceAxis, SliceKey};
use glam::Vec2;
use serde_json::{json, Value};

use crate::error::IoError;
use crate::provenance::{provenance_from_json, provenance_json};

/// Value of the `format` field of exported documents.
pub const ANNOTATIONS_FORMAT: &str = "ferrum-annotations";
/// Version of the exported document layout.
pub const ANNOTATIONS_VERSION: u32 = 2;

/// Converts `report` to the JSON document written by
/// [`write_annotation_report`]. `generator` names the producing
/// application (e.g. `"FERRUM 0.1.0"`).
pub fn annotation_report_json(report: &AnnotationReport, generator: &str) -> Value {
    let s = &report.study;
    let source_name = report.source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let annotations: Vec<Value> = report
        .annotations
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "name": a.name,
                "type": a.kind,
                "plane": a.plane.label().to_lowercase(),
                "slice_index": a.slice_index,
                "slice_number": a.slice_index + 1,
                "value": a.value.map(round3),
                "unit": a.unit,
                "text": a.text,
                "points_mm": a.points_mm.iter().map(|p| [round3(p.x), round3(p.y)]).collect::<Vec<_>>(),
                "points_voxel": a.points_voxel.iter().map(|p| [round3(p.x), round3(p.y), round3(p.z)]).collect::<Vec<_>>(),
                "provenance": provenance_json(&a.provenance),
            })
        })
        .collect();
    json!({
        "format": ANNOTATIONS_FORMAT,
        "version": ANNOTATIONS_VERSION,
        "generator": generator,
        "source": {
            "name": source_name,
            "path": report.source.to_string_lossy(),
        },
        "study": {
            "study_instance_uid": s.study_instance_uid,
            "series_instance_uid": s.series_instance_uid,
            "study_date": s.study_date,
            "study_time": s.study_time,
            "study_description": s.study_description,
            "series_description": s.series_description,
            "series_number": s.series_number,
            "accession_number": s.accession_number,
            "modality": s.modality,
        },
        "volume": {
            "dims": report.dims,
            "spacing_mm": [round6(report.spacing.x), round6(report.spacing.y), round6(report.spacing.z)],
            "frame": "LPS voxel grid: i towards patient left, j posterior, k superior",
        },
        "coordinates": {
            "points_mm": "in-plane millimetres from the top-left corner of the displayed slice; x right, y down",
            "points_voxel": "continuous (i, j, k); integer values are voxel centres",
            "slice_index": "zero-based; slice_number is the one-based number shown in the viewer",
        },
        "annotations": annotations,
    })
}

/// Reads a `ferrum-annotations` document (version 1 or 2) written by
/// [`write_annotation_report`]. Ids, names, slices, points and provenance
/// are restored; version 1 annotations count as drawn by a person. If
/// `dims` is given, the document must describe a volume of that size.
pub fn read_annotations(path: &Path, dims: Option<[u32; 3]>) -> Result<AnnotationSet, IoError> {
    let text = std::fs::read_to_string(path).map_err(|e| IoError::os(path, e))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| IoError::parse(path, e))?;
    annotations_from_json(&doc, dims).map_err(|m| IoError::invalid(path, m))
}

/// Parses a `ferrum-annotations` document; see [`read_annotations`].
pub fn annotations_from_json(doc: &Value, dims: Option<[u32; 3]>) -> Result<AnnotationSet, String> {
    if doc["format"] != ANNOTATIONS_FORMAT {
        return Err(format!("not a {ANNOTATIONS_FORMAT} document"));
    }
    match doc["version"].as_u64() {
        Some(v) if (1..=u64::from(ANNOTATIONS_VERSION)).contains(&v) => {}
        v => return Err(format!("unsupported {ANNOTATIONS_FORMAT} version {v:?}")),
    }
    if let Some(expected) = dims {
        let found: Option<Vec<u32>> = doc["volume"]["dims"]
            .as_array()
            .map(|a| a.iter().filter_map(|d| d.as_u64().and_then(|d| u32::try_from(d).ok())).collect());
        if found.as_deref() != Some(&expected[..]) {
            return Err(format!("annotations belong to a volume of {found:?} voxels, not {expected:?}"));
        }
    }
    let mut set = AnnotationSet::default();
    for (n, a) in doc["annotations"].as_array().ok_or("annotations: expected a list")?.iter().enumerate() {
        let fail = |m: String| format!("annotation {n}: {m}");
        let id = a["id"].as_u64().ok_or_else(|| fail("id is missing".into()))?;
        let plane =
            a["plane"].as_str().and_then(|p| SliceAxis::ALL.into_iter().find(|s| s.label().eq_ignore_ascii_case(p)));
        let plane = plane.ok_or_else(|| fail(format!("unknown plane {}", a["plane"])))?;
        let index = a["slice_index"]
            .as_u64()
            .and_then(|i| u32::try_from(i).ok())
            .ok_or_else(|| fail("slice_index is missing".into()))?;
        let annotation = annotation_from_json(a).map_err(fail)?;
        let provenance = provenance_from_json(&a["provenance"]).map_err(fail)?;
        let name = a["name"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map_or_else(|| format!("{} {}", annotation.kind(), id + 1), str::to_owned);
        if !set.insert(id, SliceKey::new(plane, index), annotation, &name, provenance) {
            return Err(fail(format!("duplicate id {id}")));
        }
    }
    Ok(set)
}

/// The geometry of one annotation from its `type`, `points_mm` and `text`.
fn annotation_from_json(a: &Value) -> Result<Annotation, String> {
    let points: Vec<Vec2> = a["points_mm"]
        .as_array()
        .ok_or("points_mm is missing")?
        .iter()
        .map(|p| match (p[0].as_f64(), p[1].as_f64()) {
            (Some(x), Some(y)) => Ok(Vec2::new(x as f32, y as f32)),
            _ => Err(format!("bad point {p}")),
        })
        .collect::<Result<_, _>>()?;
    let kind = a["type"].as_str().unwrap_or_default();
    let need = |n: usize| {
        if points.len() == n {
            Ok(())
        } else {
            Err(format!("{kind} needs {n} points, got {}", points.len()))
        }
    };
    Ok(match kind {
        "Distance" => need(2).map(|()| Annotation::Distance { a: points[0], b: points[1] })?,
        "Rectangle" => need(2).map(|()| Annotation::Rect { a: points[0], b: points[1] })?,
        "Angle" => need(3).map(|()| Annotation::Angle { a: points[0], vertex: points[1], b: points[2] })?,
        "Text" => need(1)
            .map(|()| Annotation::Text { pos: points[0], text: a["text"].as_str().unwrap_or_default().to_owned() })?,
        "Area" if points.len() >= 3 => Annotation::Polygon { points },
        "Area" => return Err(format!("Area needs at least 3 points, got {}", points.len())),
        other => return Err(format!("unknown annotation type {other:?}")),
    })
}

/// Rounds to 0.001 (a micrometre for millimetre values) so the file shows
/// the measured precision instead of `f32` noise.
fn round3(v: f32) -> f64 {
    (f64::from(v) * 1e3).round() / 1e3
}

/// Rounds voxel spacing to 1e-6 mm.
fn round6(v: f32) -> f64 {
    (f64::from(v) * 1e6).round() / 1e6
}

/// Writes `report` as pretty-printed JSON to `path`.
pub fn write_annotation_report(report: &AnnotationReport, generator: &str, path: &Path) -> Result<(), IoError> {
    let doc = annotation_report_json(report, generator);
    let text = serde_json::to_string_pretty(&doc).map_err(|e| IoError::invalid(path, e.to_string()))?;
    std::fs::write(path, text).map_err(|e| IoError::os(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::{Dims3, StudyInfo, Volume};
    use glam::Vec3;
    use std::path::PathBuf;

    #[test]
    fn writes_study_and_named_annotations() {
        let dims = Dims3::new(8, 8, 4);
        let volume = Volume::from_physical(dims, Vec3::ONE, &vec![0.0; dims.voxel_count()]).unwrap();
        let mut set = AnnotationSet::default();
        let id =
            set.add(SliceKey::new(SliceAxis::Axial, 1), Annotation::Distance { a: Vec2::ZERO, b: Vec2::new(0.0, 4.0) });
        set.rename(id, "Nodule A");
        let study = StudyInfo {
            study_date: "20240428".into(),
            study_instance_uid: "1.2.3".into(),
            modality: "CT".into(),
            ..Default::default()
        };
        let report = AnnotationReport::build(PathBuf::from("/data/lungs"), study, &volume, &set);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        write_annotation_report(&report, "FERRUM test", &path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["format"], ANNOTATIONS_FORMAT);
        assert_eq!(v["source"]["name"], "lungs");
        assert_eq!(v["study"]["study_date"], "20240428");
        assert_eq!(v["study"]["study_instance_uid"], "1.2.3");
        assert_eq!(v["volume"]["dims"], json!([8, 8, 4]));
        let a = &v["annotations"][0];
        assert_eq!(a["name"], "Nodule A");
        assert_eq!(a["type"], "Distance");
        assert_eq!(a["plane"], "axial");
        assert_eq!(a["slice_index"], 1);
        assert_eq!(a["slice_number"], 2);
        assert_eq!(a["value"], 4.0);
        assert_eq!(a["unit"], "mm");
        assert_eq!(a["points_mm"], json!([[0.0, 0.0], [0.0, 4.0]]));
        assert_eq!(a["points_voxel"][1], json!([-0.5, 3.5, 1.0]));
        assert_eq!(a["provenance"]["author"]["kind"], "human");
        assert_eq!(a["provenance"]["status"], "confirmed");
    }

    fn every_kind() -> (Volume, AnnotationSet) {
        let dims = Dims3::new(8, 8, 4);
        let volume = Volume::from_physical(dims, Vec3::ONE, &vec![0.0; dims.voxel_count()]).unwrap();
        let mut set = AnnotationSet::default();
        let k = SliceKey::new(SliceAxis::Coronal, 3);
        let at = ferrum_domain::Timestamp(1_790_858_096);
        set.add(k, Annotation::Distance { a: Vec2::ZERO, b: Vec2::new(0.25, 4.0) });
        let gone = set.add(k, Annotation::Rect { a: Vec2::ONE, b: Vec2::new(3.0, 2.0) });
        set.remove(gone);
        set.add(
            SliceKey::new(SliceAxis::Sagittal, 0),
            Annotation::Angle { a: Vec2::X, vertex: Vec2::ZERO, b: Vec2::Y },
        );
        let area = Annotation::Polygon { points: vec![Vec2::ZERO, Vec2::X, Vec2::ONE] };
        set.add_with(k, area, ferrum_domain::Provenance::agent(Some("run-1".into()), at));
        let text = set.add_with(
            k,
            Annotation::Text { pos: Vec2::ONE, text: "see prior".into() },
            ferrum_domain::Provenance::engine("E", "1", true, at),
        );
        set.rename(text, "Comment");
        set.add(SliceKey::new(SliceAxis::Axial, 2), Annotation::Rect { a: Vec2::ONE, b: Vec2::new(3.0, 2.0) });
        (volume, set)
    }

    #[test]
    fn round_trips_every_kind_with_ids_and_provenance() {
        let (volume, set) = every_kind();
        let report = AnnotationReport::build(PathBuf::from("/data/x"), StudyInfo::default(), &volume, &set);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        write_annotation_report(&report, "FERRUM test", &path).unwrap();
        let back = read_annotations(&path, Some([8, 8, 4])).unwrap();
        assert_eq!(back, set);
        assert!(matches!(read_annotations(&path, Some([8, 8, 5])), Err(IoError::Invalid { .. })));
        assert!(matches!(read_annotations(&dir.path().join("missing.json"), None), Err(IoError::Os { .. })));
        std::fs::write(&path, "{").unwrap();
        assert!(matches!(read_annotations(&path, None), Err(IoError::Parse { .. })));
    }

    #[test]
    fn reads_version_1_and_rejects_bad_documents() {
        let v1 = json!({ "format": "ferrum-annotations", "version": 1, "annotations": [
            { "id": 4, "name": "", "type": "Distance", "plane": "axial", "slice_index": 1, "points_mm": [[0, 0], [3, 4]] }
        ]});
        let set = annotations_from_json(&v1, None).unwrap();
        assert_eq!((set.name(4), set.get(4).unwrap().value()), (Some("Distance 5"), Some(5.0)));
        assert_eq!(set.provenance(4), Some(&ferrum_domain::Provenance::default()));
        let base = |a: Value| json!({ "format": "ferrum-annotations", "version": 2, "annotations": [a] });
        let ok = json!({ "id": 0, "type": "Text", "plane": "sagittal", "slice_index": 0, "points_mm": [[1, 1]], "text": "t" });
        assert!(annotations_from_json(&base(ok.clone()), None).is_ok());
        let mut bad = vec![
            json!({ "format": "other", "version": 1, "annotations": [] }),
            json!({ "format": "ferrum-annotations", "version": 3, "annotations": [] }),
            json!({ "format": "ferrum-annotations", "version": 2 }),
            json!({ "format": "ferrum-annotations", "version": 2, "annotations": [ok.clone(), ok.clone()] }),
        ];
        for (key, value) in [
            ("id", Value::Null),
            ("plane", json!("oblique")),
            ("slice_index", json!(-1)),
            ("type", json!("Ellipse")),
            ("points_mm", json!([[1, 1], [2, 2]])),
            ("points_mm", json!([["a", 1]])),
            ("points_mm", Value::Null),
            ("provenance", json!({ "status": "maybe" })),
        ] {
            let mut a = ok.clone();
            a[key] = value;
            bad.push(base(a));
        }
        bad.push(base(
            json!({ "id": 0, "type": "Area", "plane": "axial", "slice_index": 0, "points_mm": [[1, 1], [2, 2]] }),
        ));
        for doc in bad {
            assert!(annotations_from_json(&doc, None).is_err(), "{doc}");
        }
    }

    #[test]
    fn unwritable_path_is_an_error() {
        let report = AnnotationReport {
            source: PathBuf::new(),
            study: StudyInfo::default(),
            dims: [1, 1, 1],
            spacing: Vec3::ONE,
            annotations: vec![],
        };
        let err = write_annotation_report(&report, "x", Path::new("/nonexistent-dir/a.json"));
        assert!(err.is_err());
    }
}
