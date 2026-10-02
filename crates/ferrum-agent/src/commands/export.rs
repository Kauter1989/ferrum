//! `export bundle`: everything a harness hands on, in standard formats,
//! with hashes. Unconfirmed items are marked as such in every file.

use std::path::Path;

use ferrum_domain::{AnnotationReport, ReviewStatus};
use ferrum_io::provenance::provenance_json;
use serde_json::{json, Value};

use super::study::describe;
use super::Ctx;
use crate::envelope::{AgentError, Output};
use crate::params::Params;
use crate::study::{generator, Study};

/// Folder of the bundle inside the workspace.
pub const EXPORT_DIR: &str = "export";

/// Disclaimer written into every report.
pub const DISCLAIMER: &str = "Produced with FERRUM, research and engineering software that is not a medical device. \
Measurements and segmentations are proposals until a qualified person confirms them; items with status \
'proposed' are unconfirmed.";

fn file_entry(path: &Path) -> Result<Value, AgentError> {
    let (sha256, bytes) = ferrum_io::sha256_file(path)?;
    Ok(json!({ "path": path.to_string_lossy(), "bytes": bytes, "sha256": sha256 }))
}

fn write_json(path: &Path, v: &Value) -> Result<(), AgentError> {
    let text = serde_json::to_string_pretty(v).map_err(|e| AgentError::internal(e.to_string()))?;
    std::fs::write(path, text).map_err(|e| AgentError::internal(format!("{}: {e}", path.display())))
}

fn report(study: &Study, config: &crate::config::AgentConfig) -> (Value, usize) {
    let records =
        AnnotationReport::build(std::path::PathBuf::new(), Default::default(), &study.volume, &study.annotations);
    let measurements: Vec<Value> = records
        .annotations
        .iter()
        .map(|a| {
            json!({
                "id": a.id, "name": a.name, "type": a.kind, "value": a.value, "unit": a.unit, "text": a.text,
                "plane": a.plane.label().to_lowercase(), "slice_number": a.slice_index + 1,
                "confirmed": a.provenance.status == ReviewStatus::Confirmed,
                "provenance": provenance_json(&a.provenance),
            })
        })
        .collect();
    let set = &study.segments;
    let segments: Vec<Value> = set
        .segments()
        .iter()
        .map(|s| {
            json!({
                "label": s.label, "name": s.name, "voxels": set.voxel_count(s.label),
                "volume_ml": (set.volume_ml(s.label, study.volume.spacing()) * 1000.0).round() / 1000.0,
                "confirmed": s.provenance.status == ReviewStatus::Confirmed,
                "provenance": provenance_json(&s.provenance),
            })
        })
        .collect();
    let unconfirmed = measurements.iter().chain(&segments).filter(|i| i["confirmed"] == false).count();
    let doc = json!({
        "format": "ferrum-report",
        "version": 1,
        "generator": generator(),
        "created": ferrum_domain::Timestamp::now().to_rfc3339(),
        "disclaimer": DISCLAIMER,
        "study": describe(config, study).data,
        "measurements": measurements,
        "segments": segments,
        "unconfirmed_items": unconfirmed,
    });
    (doc, unconfirmed)
}

/// `export bundle`: report, annotations and segments in `export/`.
pub fn bundle(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let config = ctx.config;
    let study = ctx.study(p)?;
    let dir = study.workspace.path(EXPORT_DIR);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| AgentError::internal(e.to_string()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| AgentError::internal(e.to_string()))?;
    let (doc, unconfirmed) = report(study, config);
    let mut files = Vec::new();
    let report_path = dir.join("report.json");
    write_json(&report_path, &doc)?;
    files.push(file_entry(&report_path)?);
    if !study.annotations.is_empty() {
        let path = dir.join("annotations.json");
        let records = AnnotationReport::build(
            study.workspace.manifest().source.path.clone(),
            study.metadata.study.clone(),
            &study.volume,
            &study.annotations,
        );
        ferrum_io::write_annotation_report(&records, &generator(), &path)?;
        files.push(file_entry(&path)?);
    }
    if !study.segments.segments().is_empty() {
        let (nifti, meta) = (dir.join("segments.nii.gz"), dir.join("segments.json"));
        ferrum_io::write_label_nifti(study.segments.labels(), &study.volume, &nifti)?;
        ferrum_io::write_segments(&study.segments, study.volume.spacing(), &generator(), &meta)?;
        files.push(file_entry(&nifti)?);
        files.push(file_entry(&meta)?);
    }
    let mut out =
        Output::new(json!({ "directory": dir.to_string_lossy(), "files": files, "unconfirmed_items": unconfirmed }));
    if unconfirmed > 0 {
        out = out.warn(format!("{unconfirmed} item(s) are unconfirmed proposals; every file marks them as such"));
    }
    Ok(out)
}
