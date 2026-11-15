//! Identifier spans for the host privacy layer (#74): positions and kinds
//! only, never values; tolerant matching in English and Russian; and the
//! scan is not an agent command.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use ferrum_agent::{mcp, scan_identifiers, Agent, AgentConfig, IdentifierKind, Span};
use ferrum_domain::{NoProgress, VolumeRepository};
use serde_json::json;

/// The values that must never appear in an output of the scan.
const SECRETS: [&str; 6] = ["IVANOV", "IVAN", "MRN-4711", "19700101", "ACC-0815", "Иванов"];

/// A DICOM series with patient data in the header, generated at run time.
fn write_series(dir: &Path, name: &str) {
    use dicom_core::{DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};

    std::fs::create_dir_all(dir).unwrap();
    for k in 0..4u32 {
        let pixels: Vec<u16> = vec![24; 64];
        let text = |t, vr, v: &str| DataElement::new(t, vr, PrimitiveValue::from(v));
        let elements = vec![
            text(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
            text(tags::SOP_INSTANCE_UID, VR::UI, &format!("1.2.826.0.1.3680043.8.498.777.{}", k + 1)),
            text(tags::SPECIFIC_CHARACTER_SET, VR::CS, "ISO_IR 192"),
            text(tags::MODALITY, VR::CS, "CT"),
            text(tags::PATIENT_NAME, VR::PN, name),
            text(tags::PATIENT_ID, VR::LO, "MRN-4711"),
            text(tags::PATIENT_BIRTH_DATE, VR::DA, "19700101"),
            text(tags::ACCESSION_NUMBER, VR::SH, "ACC-0815"),
            text(tags::STUDY_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.8.498.12345"),
            text(tags::SERIES_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.8.498.67890"),
            text(tags::INSTANCE_NUMBER, VR::IS, &(k + 1).to_string()),
            text(tags::IMAGE_POSITION_PATIENT, VR::DS, &format!("-4\\-4\\{}", f64::from(k) * 2.0)),
            text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
            text(tags::PIXEL_SPACING, VR::DS, "1\\1"),
            text(tags::SLICE_THICKNESS, VR::DS, "2"),
            text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
            text(tags::RESCALE_SLOPE, VR::DS, "1"),
            text(tags::RESCALE_INTERCEPT, VR::DS, "-1024"),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(8u16)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(8u16)),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(0u16)),
            DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::U16(pixels.into())),
        ];
        let obj = InMemDicomObject::from_element_iter(elements);
        let meta = FileMetaTableBuilder::new()
            .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
            .media_storage_sop_instance_uid(format!("1.2.826.0.1.3680043.8.498.777.{}", k + 1))
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN);
        obj.with_meta(meta).unwrap().write_to_file(dir.join(format!("s{k}.dcm"))).unwrap();
    }
}

/// A workspace over a generated series, and the configuration that reads it.
fn study(dir: &Path, name: &str) -> (AgentConfig, PathBuf) {
    let src = dir.join("incoming/series");
    write_series(&src, name);
    let repo = ferrum_io::CompositeRepository::default();
    let series = repo.scan(std::slice::from_ref(&src), &NoProgress).unwrap().remove(0);
    let ws = dir.join("ws/ct1");
    ferrum_io::Workspace::create(&ws, &src, &series, "field").unwrap();
    let config = AgentConfig {
        read_roots: vec![dir.join("incoming")],
        workspace_root: Some(dir.join("ws")),
        ..AgentConfig::default()
    };
    (config, ws)
}

/// The span of `needle` in `text`, with its kind.
fn at(text: &str, needle: &str, kind: IdentifierKind) -> Span {
    let start = text.find(needle).expect("the needle is in the text");
    Span { start, end: start + needle.len(), kind }
}

// covers 27.1-a
#[test]
fn english_text_gives_exact_spans_for_every_kind() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    let text = "Ivanov Ivan Ivanovich (MRN-4711), born 01.01.1970, accession ACC-0815.";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert_eq!(
        spans,
        vec![
            at(text, "Ivanov Ivan Ivanovich", IdentifierKind::PatientName),
            at(text, "MRN-4711", IdentifierKind::PatientId),
            at(text, "01.01.1970", IdentifierKind::BirthDate),
            at(text, "ACC-0815", IdentifierKind::AccessionNumber),
        ]
    );
}

// covers 27.1-a
#[test]
fn russian_initials_and_inflections_are_found() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    let text = "Иванов И.И. пришёл на приём";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "Иванов И.И.", IdentifierKind::PatientName)]);

    let text = "Иванова Ивана Ивановича выписали";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "Иванова Ивана Ивановича", IdentifierKind::PatientName)]);

    let text = "выписка: И. Иванов";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "И. Иванов", IdentifierKind::PatientName)]);

    // the full middle name after an initial must not stay unmasked
    let text = "Иванов И. Иванович";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "Иванов И. Иванович", IdentifierKind::PatientName)]);
}

// covers 27.1-a
#[test]
fn diacritics_and_yo_match_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "MÜLLER^HANS");
    let text = "Müller Hans kam am Montag";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "Müller Hans", IdentifierKind::PatientName)]);

    let dir2 = tempfile::tempdir().unwrap();
    let (config2, _) = study(dir2.path(), "ИВАНОВ^ПЁТР");
    let text = "Иванов Петр на приёме";
    let spans = scan_identifiers(&config2, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "Иванов Петр", IdentifierKind::PatientName)]);
}

// covers 27.1-a
#[test]
fn latin_and_cyrillic_match_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    let text = "пациент Иванов Иван";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "Иванов Иван", IdentifierKind::PatientName)]);

    // the other direction: a Cyrillic header, Latin text
    let dir2 = tempfile::tempdir().unwrap();
    let (config2, _) = study(dir2.path(), "ИВАНОВ^ИВАН^ИВАНОВИЧ");
    let text = "patient Ivanov Ivan";
    let spans = scan_identifiers(&config2, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "Ivanov Ivan", IdentifierKind::PatientName)]);
}

// covers 27.1-a
#[test]
fn the_birth_date_matches_the_common_formats() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    for form in [
        "19700101",
        "1970-01-01",
        "1970.01.01",
        "01.01.1970",
        "01/01/1970",
        "01-01-1970",
        "1 January 1970",
        "January 1, 1970",
        "1 jan 1970",
        "1 января 1970",
        "1 январь 1970",
        "1.1.1970",
        "1/1/1970",
        "1970-1-1",
        "01 января 1970",
    ] {
        let text = format!("дата рождения: {form}.");
        let spans = scan_identifiers(&config, Path::new("ct1"), &text).unwrap();
        assert_eq!(spans, vec![at(&text, form, IdentifierKind::BirthDate)], "format {form}");
    }
}

// covers 27.1-b
#[test]
fn the_output_carries_positions_and_kinds_only() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    let text = "Ivanov Ivan Ivanovich, MRN-4711, 19700101, ACC-0815, Иванов И.И.";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert!(!spans.is_empty());
    let debug = format!("{spans:?}");
    let json = json!(spans
        .iter()
        .map(|s| json!({"start": s.start, "end": s.end, "kind": format!("{:?}", s.kind)}))
        .collect::<Vec<_>>())
    .to_string();
    for secret in SECRETS {
        assert!(!debug.contains(secret), "a value leaked into the debug output: {debug}");
        assert!(!json.contains(secret), "a value leaked into the JSON output: {json}");
    }
}

// covers 27.1-c
#[test]
fn the_scan_is_not_an_agent_command() {
    // "study scan" is an existing command; the privacy scan must not appear
    // as a command of its own under any name
    for name in Agent::commands() {
        assert!(!name.contains("identifier") && !name.starts_with("scan"), "{name}");
    }
    for tool in mcp::tools() {
        let name = tool["name"].as_str().unwrap();
        assert!(!name.contains("identifier") && !name.starts_with("scan"), "{name}");
    }
    let schemas =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills/ferrum/schemas/commands.json"))
            .unwrap();
    let schemas: serde_json::Value = serde_json::from_str(&schemas).unwrap();
    for key in schemas.as_object().unwrap().keys() {
        assert!(!key.contains("identifier") && !key.starts_with("scan"), "{key}");
    }
}

// covers 27.1-a
#[test]
fn a_nifti_study_has_no_identifiers_and_clean_text_has_no_spans() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("incoming/phantom.nii");
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    let dims = ferrum_domain::Dims3::new(8, 8, 4);
    let vals: Vec<f32> = (0..dims.voxel_count()).map(|i| if i % 2 == 0 { 0.0 } else { 100.0 }).collect();
    let volume = ferrum_domain::Volume::from_physical(dims, glam::Vec3::ONE, &vals).unwrap();
    ferrum_io::write_nifti(&volume, &src).unwrap();
    let repo = ferrum_io::CompositeRepository::default();
    let series = repo.scan(std::slice::from_ref(&src), &NoProgress).unwrap().remove(0);
    let ws = dir.path().join("ws/n1");
    ferrum_io::Workspace::create(&ws, &src, &series, "field").unwrap();
    let config = AgentConfig {
        read_roots: vec![dir.path().join("incoming")],
        workspace_root: Some(dir.path().join("ws")),
        ..AgentConfig::default()
    };
    let spans = scan_identifiers(&config, Path::new("n1"), "Ivanov Ivan, MRN-4711, 01.01.1970").unwrap();
    assert!(spans.is_empty(), "a NIfTI study carries no identifiers: {spans:?}");

    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    let spans = scan_identifiers(&config, Path::new("ct1"), "The patient arrived on Tuesday.").unwrap();
    assert!(spans.is_empty(), "clean text has no spans: {spans:?}");
}

// covers 27.1-a
#[test]
fn a_missing_workspace_and_a_foreign_source_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    let err = scan_identifiers(&config, Path::new("missing"), "Ivanov").unwrap_err();
    assert_eq!(err.code, ferrum_agent::ErrorCode::NoStudy);

    let closed = AgentConfig { read_roots: vec![dir.path().join("elsewhere")], ..config.clone() };
    let err = scan_identifiers(&closed, Path::new("ct1"), "Ivanov").unwrap_err();
    assert_eq!(err.code, ferrum_agent::ErrorCode::Forbidden);
}

// covers 27.1-a (security review of #110: the masking is driven by the
// workspace's recorded, hash-verified files, not by a fresh folder walk)
#[test]
fn a_planted_file_does_not_drive_the_masking() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    let src = dir.path().join("incoming/series");
    // junk that sorts first, and a valid DICOM of another patient
    std::fs::write(src.join("000-junk.dcm"), b"not a dicom file").unwrap();
    let other = dir.path().join("other");
    write_series(&other, "OTHER^PERSON");
    std::fs::copy(other.join("s0.dcm"), src.join("aaa-other.dcm")).unwrap();

    let text = "Ivanov Ivan and Other Person";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "Ivanov Ivan", IdentifierKind::PatientName)]);
}

// covers 27.1-a
#[test]
fn a_swapped_source_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    std::fs::write(dir.path().join("incoming/series/s0.dcm"), b"swapped").unwrap();
    let err = scan_identifiers(&config, Path::new("ct1"), "Ivanov").unwrap_err();
    assert_eq!(err.code, ferrum_agent::ErrorCode::SourceChanged);
}

// covers 27.1-a (security review of #110: a symlink must not lead the
// reader outside the operator's read roots — the #82 vulnerability class)
#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_roots_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = study(dir.path(), "IVANOV^IVAN^IVANOVICH");
    let outside = dir.path().join("outside");
    write_series(&outside, "OUTSIDE^SECRET");
    let src = dir.path().join("incoming/series");
    std::os::unix::fs::symlink(outside.join("s0.dcm"), src.join("a.dcm")).unwrap();

    // the io folder walk skips the symlink itself
    let ids = ferrum_io::read_patient_identifiers(&src).unwrap();
    assert_eq!(ids.name, "IVANOV^IVAN^IVANOVICH");

    // and the scan masks the real patient, not the linked one
    let text = "Ivanov Ivan and Outside Secret";
    let spans = scan_identifiers(&config, Path::new("ct1"), text).unwrap();
    assert_eq!(spans, vec![at(text, "Ivanov Ivan", IdentifierKind::PatientName)]);
}
