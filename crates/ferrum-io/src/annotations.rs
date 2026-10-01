//! JSON export of annotation reports.
//!
//! The document is self-describing (`format`, `version`) and unambiguous:
//! every annotation carries its plane, slice index, in-plane millimetre
//! points and the same points as voxel coordinates of the volume.

use std::path::Path;

use ferrum_domain::AnnotationReport;
use serde_json::{json, Value};

use crate::error::IoError;

/// Value of the `format` field of exported documents.
pub const ANNOTATIONS_FORMAT: &str = "ferrum-annotations";
/// Version of the exported document layout.
pub const ANNOTATIONS_VERSION: u32 = 1;

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
    use ferrum_domain::{Annotation, AnnotationSet, Dims3, SliceAxis, SliceKey, StudyInfo, Volume};
    use glam::{Vec2, Vec3};
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
