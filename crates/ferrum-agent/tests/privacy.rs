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
/// The last slice is 4 mm from its neighbour instead of 2, so that loading
/// warns (covers 2.7-f).
fn write_ct(dir: &Path) {
    write_images(dir, None);
}

/// [`write_ct`], as the echo `echo` of a multi-echo series when given
/// (Echo Number and Echo Time set, files and instance UIDs of their own).
fn write_images(dir: &Path, echo: Option<u32>) {
    std::fs::create_dir_all(dir).unwrap();
    let n = echo.unwrap_or(0);
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
            text(tags::SOP_INSTANCE_UID, VR::UI, &format!("{SERIES_UID}.{}", k + 1 + 100 * n)),
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
            text(tags::IMAGE_POSITION_PATIENT, VR::DS, &format!("-8\\-8\\{}", f64::from(k + k / 7) * 2.0)),
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
        let mut obj = InMemDicomObject::from_element_iter(elements);
        if let Some(e) = echo {
            obj.put(text(tags::ECHO_NUMBERS, VR::IS, &e.to_string()));
            obj.put(text(tags::ECHO_TIME, VR::DS, &(20 * e).to_string()));
        }
        let file = obj.with_meta(FileMetaTableBuilder::new().transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)).unwrap();
        file.write_to_file(dir.join(format!("{n}-{k:03}.dcm"))).unwrap();
    }
}

fn scan_text(what: &str, text: &str, found: &mut Vec<String>) {
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
        ("segment metrics", json!({ "workspace": ws, "segment": 1, "describe": true })), // covers 18.4-d
        ("segment metrics", json!({ "workspace": ws, "segment": 1, "profile": "clinical", "describe": true })),
        ("segment list", json!({ "workspace": ws })),
        ("review list", json!({ "workspace": ws })),
        ("export bundle", json!({ "workspace": ws, "formats": ["ferrum", "dicom"] })),
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
        scan_text(&format!("envelope of {}", e["provenance"]["command"]), &clean.to_string(), &mut found);
    }
    assert_eq!(envelopes[2]["data"]["value_unit"], "HU", "a real CT series");
    assert!(envelopes[1]["warnings"].to_string().contains("slice spacing is irregular"), "{:#}", envelopes[1]);
    for sub in ["renders", "export"] {
        for entry in std::fs::read_dir(ws.join(sub)).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            let bytes = if path.extension().is_some_and(|e| e == "gz") { gunzip(&bytes) } else { bytes };
            scan_text(&path.display().to_string(), &String::from_utf8_lossy(&bytes), &mut found);
        }
    }
    // the workspace's own annotation file follows the same rules
    scan_text("annotations.json", &std::fs::read_to_string(ws.join("annotations.json")).unwrap(), &mut found);
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

    // DICOM objects for the PACS carry the patient only with that consent
    let open_agent = Agent::new(AgentConfig {
        expose_identifiers: true,
        expose_dates: true,
        pseudonymise_uids: false,
        ..AgentConfig::default()
    });
    let env = open_agent.run("export bundle", &json!({ "workspace": ws_text, "formats": ["dicom"] }));
    assert!(!env.to_string().contains("DOE^JANE"), "the envelope still carries no names");
    let seg = std::fs::read(ws.join("export/segmentation.dcm")).unwrap();
    let text = String::from_utf8_lossy(&seg);
    assert!(text.contains("MRN-4711") && text.contains(STUDY_UID) && text.contains(SERIES_UID), "filed with the study");
}

#[test]
fn volume_ids_and_labels_contain_no_identifiers() {
    // covers 20.3-e
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("incoming/echoes");
    write_images(&src, Some(1));
    write_images(&src, Some(2));
    let ws = dir.path().join("ws/me").to_string_lossy().into_owned();
    let agent = Agent::default();
    let scan = agent.run("study scan", &json!({ "paths": [src] }));
    let entries = scan["data"]["series"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{scan:#}");
    assert_eq!(entries[1]["volume"]["label"], "echo 2, TE = 40 ms");
    let second = entries[1]["series"].clone();
    let mut envelopes = vec![scan.clone()];
    envelopes.extend(session(&agent, &ws, &src));
    envelopes.push(agent.run("study open", &json!({ "workspace": ws, "path": src, "series": second })));
    envelopes.push(agent.run("probe", &json!({ "workspace": ws, "point": { "voxel": [8, 8, 3] } })));
    let mut found = Vec::new();
    for e in &envelopes {
        assert_eq!(e["ok"], true, "{e:#}");
        let mut clean = e.clone();
        clean["provenance"]["params"] = Value::Null;
        scan_text(&format!("envelope of {}", e["provenance"]["command"]), &clean.to_string(), &mut found);
    }
    assert_eq!(envelopes.last().unwrap()["provenance"]["volume"]["label"], "echo 2, TE = 40 ms");
    assert!(found.is_empty(), "identifiers leaked: {found:#?}");
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut out).unwrap();
    out
}
