//! No identifiers leave FERRUM through the agent: a CT series full of
//! patient data goes through every command, and every envelope, render and
//! exported file is scanned for it.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use ferrum_agent::{Agent, AgentConfig};
use serde_json::{json, Value};

const STUDY_UID: &str = "1.2.826.0.1.3680043.8.498.12345";
const SERIES_UID: &str = "1.2.826.0.1.3680043.8.498.67890";
/// Strings that must never appear in an output.
const SECRETS: [&str; 9] =
    ["DOE^JANE", "JANE", "MRN-4711", "19700101", "ACC-0815", "20240428", "St Example Hospital", STUDY_UID, SERIES_UID];

/// 16 × 16 × 8 CT in HU with a bright block, and patient data in the header.
fn write_ct(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for k in 0..8u32 {
        let pixels: Vec<u16> = (0..256u32)
            .map(|p| {
                let (i, j) = (p % 16, p / 16);
                // stored = HU + 1024
                if (5..11).contains(&i) && (5..11).contains(&j) && (2..6).contains(&k) {
                    1424
                } else {
                    24
                }
            })
            .collect();
        let text = |t, vr, v: &str| DataElement::new(t, vr, PrimitiveValue::from(v));
        let elements = vec![
            text(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
            text(tags::SOP_INSTANCE_UID, VR::UI, &format!("{SERIES_UID}.{}", k + 1)),
            text(tags::MODALITY, VR::CS, "CT"),
            text(tags::PATIENT_NAME, VR::PN, "DOE^JANE"),
            text(tags::PATIENT_ID, VR::LO, "MRN-4711"),
            text(tags::PATIENT_BIRTH_DATE, VR::DA, "19700101"),
            text(tags::ACCESSION_NUMBER, VR::SH, "ACC-0815"),
            text(tags::INSTITUTION_NAME, VR::LO, "St Example Hospital"),
            text(tags::STUDY_INSTANCE_UID, VR::UI, STUDY_UID),
            text(tags::SERIES_INSTANCE_UID, VR::UI, SERIES_UID),
            text(tags::STUDY_DATE, VR::DA, "20240428"),
            text(tags::STUDY_TIME, VR::TM, "101500"),
            text(tags::STUDY_DESCRIPTION, VR::LO, "Chest"),
            text(tags::SERIES_DESCRIPTION, VR::LO, "Axial 2.0"),
            text(tags::INSTANCE_NUMBER, VR::IS, &(k + 1).to_string()),
            text(tags::IMAGE_POSITION_PATIENT, VR::DS, &format!("-8\\-8\\{}", f64::from(k) * 2.0)),
            text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
            text(tags::PIXEL_SPACING, VR::DS, "1\\1"),
            text(tags::SLICE_THICKNESS, VR::DS, "2"),
            text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
            text(tags::RESCALE_SLOPE, VR::DS, "1"),
            text(tags::RESCALE_INTERCEPT, VR::DS, "-1024"),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(0u16)),
            DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::U16(pixels.into())),
        ];
        let obj = InMemDicomObject::from_element_iter(elements);
        let file = obj.with_meta(FileMetaTableBuilder::new().transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)).unwrap();
        file.write_to_file(dir.join(format!("{k:03}.dcm"))).unwrap();
    }
}

fn scan(what: &str, text: &str, found: &mut Vec<String>) {
    for s in SECRETS {
        if text.contains(s) {
            found.push(format!("{s:?} in {what}"));
        }
    }
}

/// Every command once; returns the envelopes.
fn session(agent: &Agent, ws: &str, src: &Path) -> Vec<Value> {
    let calls = [
        ("study scan", json!({ "paths": [src] })),
        ("study open", json!({ "workspace": ws, "path": src })),
        ("study info", json!({ "workspace": ws })),
        ("view slice", json!({ "workspace": ws, "plane": "axial", "slice_number": 4, "overlays": ["segments"] })),
        ("view montage", json!({ "workspace": ws, "plane": "coronal", "size": 128 })),
        ("view mpr", json!({ "workspace": ws, "at": { "voxel": [8, 8, 3] }, "size": 192 })),
        ("view volume", json!({ "workspace": ws, "mode": "isosurface", "threshold": 200, "size": 64 })),
        ("probe", json!({ "workspace": ws, "point": { "voxel": [8, 8, 3] } })),
        ("stats", json!({ "workspace": ws, "sphere": { "center": { "voxel": [8, 8, 3] }, "radius_mm": 3 } })),
        ("profile", json!({ "workspace": ws, "from": { "voxel": [0, 8, 3] }, "to": { "voxel": [15, 8, 3] } })),
        ("measure distance", json!({ "workspace": ws, "points": [{ "voxel": [5, 8, 3] }, { "voxel": [10, 8, 3] }] })),
        (
            "annotate add",
            json!({ "workspace": ws, "kind": "text", "plane": "axial", "text": "block", "points": [{ "voxel": [8, 8, 3] }] }),
        ),
        ("annotate list", json!({ "workspace": ws })),
        ("segment threshold", json!({ "workspace": ws, "seed": { "voxel": [8, 8, 3] }, "min": 300, "max": 500 })),
        ("segment list", json!({ "workspace": ws })),
        ("review list", json!({ "workspace": ws })),
        ("export bundle", json!({ "workspace": ws })),
    ];
    calls.iter().map(|(c, p)| agent.run(c, p)).collect()
}

#[test]
fn outputs_contain_no_identifiers() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("incoming/series");
    write_ct(&src);
    let ws = dir.path().join("ws/ct1");
    let ws_text = ws.to_string_lossy().into_owned();
    let envelopes = session(&Agent::default(), &ws_text, &src);
    let mut found = Vec::new();
    for e in &envelopes {
        assert_eq!(e["ok"], true, "{e:#}");
        let mut clean = e.clone();
        clean["provenance"]["params"] = Value::Null; // the harness's own input, echoed
        scan(&format!("envelope of {}", e["provenance"]["command"]), &clean.to_string(), &mut found);
    }
    assert_eq!(envelopes[2]["data"]["value_unit"], "HU", "a real CT series");
    for sub in ["renders", "export"] {
        for entry in std::fs::read_dir(ws.join(sub)).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            let bytes = if path.extension().is_some_and(|e| e == "gz") { gunzip(&bytes) } else { bytes };
            scan(&path.display().to_string(), &String::from_utf8_lossy(&bytes), &mut found);
        }
    }
    // the workspace's own annotation file follows the same rules
    scan("annotations.json", &std::fs::read_to_string(ws.join("annotations.json")).unwrap(), &mut found);
    assert!(found.is_empty(), "identifiers leaked: {found:#?}");

    // with the operator's consent, dates and accession numbers may be shared
    let open = AgentConfig {
        expose_dates: true,
        expose_identifiers: true,
        pseudonymise_uids: false,
        ..AgentConfig::default()
    };
    let info = Agent::new(open).run("study info", &json!({ "workspace": ws_text }));
    let ids = &info["data"]["study"];
    assert_eq!((ids["study_date"].as_str(), ids["accession_number"].as_str()), (Some("20240428"), Some("ACC-0815")));
    assert_eq!(ids["study_instance_uid"], STUDY_UID);
    assert!(!info.to_string().contains("DOE^JANE"), "patient names are never read");
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut out).unwrap();
    out
}
