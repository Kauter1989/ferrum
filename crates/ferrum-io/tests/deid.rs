//! Stage 31 PR 1 — DICOM de-identification (PS3.15 Basic Profile) tests.
//!
//! A synthetic object carrying **every** Table E.1-1 attribute is generated at
//! run time (no patient/volume data is committed). The pinned fixture
//! (`deid/fixture.rs`) fixes the action of every row; the agreement test fails
//! when the committed table / generated rules disagree with it.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod common;
#[path = "deid/fixture.rs"]
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry};
use dicom_core::header::Header;
use dicom_core::value::DataSetSequence;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::{tags, uids, StandardDataDictionary};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use ferrum_domain::{NoProgress, VolumeRepository};
use ferrum_io::dicom::deid::{deidentify, rules, DeidError, Opt, Profile, ProfileOptions, UidMap};
use ferrum_io::dicom::export::uid_from;
use ferrum_io::DicomRepository;

type Elem = DataElement<InMemDicomObject>;

const SALT: &str = "test-salt";
const CSV: &str = include_str!("../src/dicom/deid_table.csv");
const OPT_NAMES: [&str; 6] = [
    "retain_uids",
    "retain_device_identity",
    "retain_institution_identity",
    "retain_patient_characteristics",
    "retain_dates_modified",
    "clean_descriptors",
];
const METHOD_TAGS: [Tag; 4] = [
    tags::PATIENT_IDENTITY_REMOVED,
    tags::DEIDENTIFICATION_METHOD,
    tags::DEIDENTIFICATION_METHOD_CODE_SEQUENCE,
    tags::LONGITUDINAL_TEMPORAL_INFORMATION_MODIFIED,
];

fn tag(s: &str) -> Tag {
    s.parse().unwrap()
}

fn tag_str(t: Tag) -> String {
    format!("({:04X},{:04X})", t.group(), t.element())
}

fn vr_of(t: Tag) -> VR {
    StandardDataDictionary.by_tag(t).and_then(|e| e.vr().exact()).unwrap_or(VR::LO)
}

fn str_at(obj: &InMemDicomObject, t: Tag) -> String {
    obj.element(t).unwrap().to_str().unwrap().trim_end_matches(['\0', ' ']).to_string()
}

fn present(obj: &InMemDicomObject, t: Tag) -> bool {
    obj.element_opt(t).unwrap().is_some()
}

fn is_zero(obj: &InMemDicomObject, t: Tag) -> bool {
    let Ok(Some(e)) = obj.element_opt(t) else { return false };
    if e.vr() == VR::SQ {
        return e.items().map(|i| i.is_empty()).unwrap_or(true);
    }
    match e.to_str() {
        Ok(s) => s.trim_end_matches(['\0', ' ']).is_empty(),
        Err(_) => true,
    }
}

/// The primary branch of a compound action code (mirrors the engine, independently).
fn effective(action: &str) -> &'static str {
    match action {
        "D" => "D",
        "Z" | "Z/D" => "Z",
        "X" | "X/Z" | "X/D" | "X/Z/D" | "X/Z/U*" => "X",
        "U" => "U",
        "C" => "C",
        "K" => "K",
        other => panic!("unknown action {other}"),
    }
}

/// A non-empty sample value of the right VR for `t` (unique per tag).
fn sample(t: Tag) -> Elem {
    let vr = vr_of(t);
    let seed = u32::from(t.group()) * 65536 + u32::from(t.element());
    if vr == VR::SQ {
        return DataElement::new(t, VR::SQ, DataSetSequence::from(vec![nested_item()]));
    }
    let pv = match vr {
        VR::PN => PrimitiveValue::from("DOE^JOHN"),
        VR::DA => PrimitiveValue::from("20240428"),
        VR::TM => PrimitiveValue::from("102417"),
        VR::DT => PrimitiveValue::from("20240428102417"),
        VR::UI => PrimitiveValue::from(format!("1.2.826.0.1.3680043.2.1125.{seed}")),
        VR::DS => PrimitiveValue::from("1.5"),
        VR::IS => PrimitiveValue::from("7"),
        VR::AS => PrimitiveValue::from("055Y"),
        VR::CS => PrimitiveValue::from("SAMPLE"),
        VR::AE => PrimitiveValue::from("SAMPLEAE"),
        VR::UR => PrimitiveValue::from("http://example.org/"),
        VR::US => PrimitiveValue::from(7u16),
        VR::UL => PrimitiveValue::from(7u32),
        VR::SS => PrimitiveValue::from(7i16),
        VR::SL => PrimitiveValue::from(7i32),
        VR::UV => PrimitiveValue::from(7u64),
        VR::SV => PrimitiveValue::from(7i64),
        VR::FL => PrimitiveValue::from(1.5f32),
        VR::FD => PrimitiveValue::from(1.5f64),
        VR::OB | VR::UN | VR::OW | VR::OD | VR::OF | VR::OL | VR::OV => PrimitiveValue::from(vec![1u8, 2, 3, 4]),
        _ => PrimitiveValue::from(format!("SAMPLE {seed}")),
    };
    DataElement::new(t, vr, pv)
}

/// A sequence item carrying nested table attributes, to exercise recursion.
fn nested_item() -> InMemDicomObject {
    InMemDicomObject::from_element_iter([
        DataElement::new(tags::PATIENT_NAME, VR::PN, PrimitiveValue::from("NESTED^NAME")),
        DataElement::new(tags::PATIENT_ID, VR::LO, PrimitiveValue::from("NESTED-ID-999")),
    ])
}

/// Every concrete Table E.1-1 attribute, plus private/curve/overlay group elements.
fn full_object() -> InMemDicomObject {
    let mut elems: Vec<Elem> = Vec::new();
    for (t, _action, option) in fixture::FIXTURE {
        if !option.is_empty() {
            continue;
        }
        elems.push(sample(tag(t)));
    }
    // Wildcard/group rows: a private element, a curve element, overlay comments/data.
    elems.push(DataElement::new(Tag(0x0009, 0x0010), VR::LO, PrimitiveValue::from("PRIVATE CREATOR")));
    elems.push(DataElement::new(Tag(0x0009, 0x1001), VR::LO, PrimitiveValue::from("private value")));
    elems.push(DataElement::new(Tag(0x5000, 0x0100), VR::OW, PrimitiveValue::from(vec![1u8, 2])));
    elems.push(DataElement::new(Tag(0x6000, 0x4000), VR::LT, PrimitiveValue::from("overlay comment")));
    elems.push(DataElement::new(Tag(0x6000, 0x3000), VR::OW, PrimitiveValue::from(vec![1u8, 2])));
    InMemDicomObject::from_element_iter(elems)
}

/// Independent parse of the committed table: `(tag, keyword, action, option)`.
/// The tag `(gggg,eeee)` contains a comma, so split after its closing paren.
fn csv_rows() -> Vec<(String, String, String, String)> {
    CSV.lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (tag_part, rest) = l.split_once(')').unwrap();
            let tag = format!("{tag_part})");
            let rest = rest.strip_prefix(',').unwrap_or(rest);
            let mut f = rest.split(',');
            (tag, f.next().unwrap().to_string(), f.next().unwrap().to_string(), f.next().unwrap().to_string())
        })
        .collect()
}

fn is_concrete(t: &str) -> bool {
    !t.contains(['x', 'g'])
}

fn profile_with(opt: &str) -> Profile {
    let mut options = ProfileOptions { uid_salt: SALT.to_string(), ..Default::default() };
    options.enable(opt).unwrap();
    Profile { options }
}

fn deid_default(obj: &InMemDicomObject) -> InMemDicomObject {
    let mut out = obj.clone();
    let profile = Profile { options: ProfileOptions { uid_salt: SALT.to_string(), ..Default::default() } };
    deidentify(&mut out, &profile, &mut UidMap::new(SALT));
    out
}

// covers 31.1-a — the generated rules, the committed table and the pinned
// fixture agree; keywords match the standard dictionary.
#[test]
fn table_generator_and_fixture_agree() {
    let rows = csv_rows();
    let concrete: Vec<(String, String, String)> =
        rows.iter().filter(|(t, ..)| is_concrete(t)).map(|(t, _kw, a, o)| (t.clone(), a.clone(), o.clone())).collect();

    // Keywords match dicom_dictionary_std for every tag it knows.
    for (t, kw, _a, _o) in &rows {
        if !is_concrete(t) {
            continue;
        }
        if let Some(entry) = StandardDataDictionary.by_tag(tag(t)) {
            assert_eq!(kw, entry.alias(), "keyword of {t}");
        }
    }

    // The four wildcard/group rows are present with action X.
    for wild in ["(50xx,xxxx)", "(60xx,4000)", "(60xx,3000)", "(gggg,eeee)"] {
        let row = rows.iter().find(|(t, ..)| t == wild).unwrap_or_else(|| panic!("missing wildcard {wild}"));
        assert_eq!(row.2, "X");
    }

    // Expand the engine's generated rules to rows and compare with the fixture.
    let mut generated: Vec<(String, String, String)> = Vec::new();
    for (t, rule) in rules() {
        generated.push((tag_str(*t), rule.base.as_str().to_string(), String::new()));
        for (opt, action) in &rule.overrides {
            generated.push((tag_str(*t), action.as_str().to_string(), opt.as_str().to_string()));
        }
    }
    let mut pinned: Vec<(String, String, String)> =
        fixture::FIXTURE.iter().map(|(t, a, o)| (t.to_string(), a.to_string(), o.to_string())).collect();
    generated.sort();
    pinned.sort();
    let mut table = concrete;
    table.sort();
    assert_eq!(generated, pinned, "generated rules vs pinned fixture");
    assert_eq!(table, pinned, "committed table vs pinned fixture");
    assert_eq!(rules().len(), 653, "one rule per concrete Table E.1-1 attribute");
}

// covers 31.1-b — on an object carrying every attribute, the output matches the
// action of each row (default Basic Profile, no options).
#[test]
fn every_row_matches_its_action() {
    let original = full_object();
    let out = deid_default(&original);
    for (t, action, option) in fixture::FIXTURE {
        if !option.is_empty() {
            continue;
        }
        let t = tag(t);
        match effective(action) {
            "X" => assert!(!present(&out, t), "{t} should be removed (action {action})"),
            "Z" => {
                assert!(present(&out, t), "{t} should be present (action {action})");
                if t == tags::PATIENT_NAME {
                    assert_eq!(str_at(&out, t), "ANONYMOUS");
                } else if t == tags::PATIENT_ID {
                    assert!(!str_at(&out, t).is_empty(), "PatientID pseudonym");
                    assert_ne!(str_at(&out, t), str_at(&original, t));
                } else {
                    assert!(is_zero(&out, t), "{t} should be zero-length (action {action})");
                }
            }
            "D" => {
                assert!(present(&out, t), "{t} should be present (action D)");
                assert!(!is_zero(&out, t), "{t} should be a non-empty dummy (action D)");
            }
            "U" => {
                let new = str_at(&out, t);
                assert!(new.starts_with("2.25."), "{t} replacement UID {new}");
                assert_ne!(new, str_at(&original, t), "{t} UID should change");
            }
            other => panic!("unexpected base action {other}"),
        }
    }
}

// covers 31.1-b (group rows) — private, curve and overlay attributes are removed.
#[test]
fn private_curve_and_overlay_attributes_are_removed() {
    let out = deid_default(&full_object());
    for t in [Tag(0x0009, 0x0010), Tag(0x0009, 0x1001), Tag(0x5000, 0x0100), Tag(0x6000, 0x4000), Tag(0x6000, 0x3000)] {
        assert!(!present(&out, t), "{t} should be removed");
    }
}

// covers 31.1-b (recursion) — nested table attributes inside a kept sequence are
// protected too.
#[test]
fn nested_attributes_inside_sequences_are_protected() {
    let mut obj = InMemDicomObject::from_element_iter([DataElement::new(
        tags::ANATOMIC_REGION_SEQUENCE,
        VR::SQ,
        DataSetSequence::from(vec![nested_item()]),
    )]);
    deidentify(&mut obj, &Profile::default(), &mut UidMap::new(SALT));
    let items = obj.element(tags::ANATOMIC_REGION_SEQUENCE).unwrap().items().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(str_at(&items[0], tags::PATIENT_NAME), "ANONYMOUS");
    assert_ne!(str_at(&items[0], tags::PATIENT_ID), "NESTED-ID-999");
}

// covers 31.1-b / 31.5-a — a D-action sequence (e.g. the SR ContentSequence,
// (0040,A730)=D) keeps every item and de-identifies its contents recursively
// (PS3.15 Note 2.1); it is never collapsed to one empty item, which would
// destroy an exported SR ContentSequence.
#[test]
fn a_d_sequence_keeps_its_items_and_deidentifies_contents() {
    let mut obj = InMemDicomObject::from_element_iter([DataElement::new(
        tags::CONTENT_SEQUENCE,
        VR::SQ,
        DataSetSequence::from(vec![nested_item(), nested_item()]),
    )]);
    deidentify(&mut obj, &Profile::default(), &mut UidMap::new(SALT));
    let items = obj.element(tags::CONTENT_SEQUENCE).unwrap().items().unwrap();
    assert_eq!(items.len(), 2, "a D sequence keeps every item, never one empty item");
    for it in items {
        assert!(it.iter().next().is_some(), "the item keeps its de-identified contents");
        assert_eq!(str_at(it, tags::PATIENT_NAME), "ANONYMOUS");
        assert_ne!(str_at(it, tags::PATIENT_ID), "NESTED-ID-999");
    }
}

// covers 31.1-c — UIDs map stably within a run and reuse the export derivation.
#[test]
fn uids_are_consistent_within_a_run() {
    let study = "1.2.826.0.1.3680043.2.1";
    let series = "1.2.826.0.1.3680043.2.1125.1";
    let frame = "1.2.826.0.1.3680043.2.1125.9";
    let mut uids = UidMap::new(SALT);
    let profile = Profile { options: ProfileOptions { uid_salt: SALT.to_string(), ..Default::default() } };
    let mut replacements = Vec::new();
    for sop in ["1.2.826.0.1.3680043.2.1125.1.1", "1.2.826.0.1.3680043.2.1125.1.2"] {
        let mut obj = InMemDicomObject::from_element_iter([
            DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, PrimitiveValue::from(study)),
            DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, PrimitiveValue::from(series)),
            DataElement::new(tags::FRAME_OF_REFERENCE_UID, VR::UI, PrimitiveValue::from(frame)),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, PrimitiveValue::from(sop)),
        ]);
        deidentify(&mut obj, &profile, &mut uids);
        replacements.push((
            str_at(&obj, tags::STUDY_INSTANCE_UID),
            str_at(&obj, tags::SERIES_INSTANCE_UID),
            str_at(&obj, tags::FRAME_OF_REFERENCE_UID),
            str_at(&obj, tags::SOP_INSTANCE_UID),
        ));
    }
    // Study/series/frame stable across instances; SOP distinct.
    assert_eq!(replacements[0].0, replacements[1].0);
    assert_eq!(replacements[0].1, replacements[1].1);
    assert_eq!(replacements[0].2, replacements[1].2);
    assert_ne!(replacements[0].3, replacements[1].3);
    // Reuse of the export's salted derivation.
    assert_eq!(replacements[0].0, uid_from(&["ferrum-uid", SALT, study]));
    assert_eq!(replacements[0].3, uid_from(&["ferrum-uid", SALT, "1.2.826.0.1.3680043.2.1125.1.1"]));
    assert_ne!(replacements[0].0, study);
}

// covers 31.1-c — retain UIDs keeps them.
#[test]
fn retain_uids_keeps_the_original_uids() {
    let study = "1.2.826.0.1.3680043.2.1";
    let sop = "1.2.826.0.1.3680043.2.1125.1.1";
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, PrimitiveValue::from(study)),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, PrimitiveValue::from(sop)),
    ]);
    deidentify(&mut obj, &profile_with("retain_uids"), &mut UidMap::new(SALT));
    assert_eq!(str_at(&obj, tags::STUDY_INSTANCE_UID), study);
    assert_eq!(str_at(&obj, tags::SOP_INSTANCE_UID), sop);
}

// covers 31.1-d — kept attributes and pixel data survive, and the object reads
// back through FERRUM's own reader.
#[test]
fn kept_attributes_and_pixels_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deid.dcm");
    let pixels: Vec<u16> = (0..64).map(|i| i * 3).collect();
    let mut obj = ct_object(&pixels);
    deidentify(&mut obj, &Profile::default(), &mut UidMap::new(SALT));
    let file = obj.with_meta(FileMetaTableBuilder::new().transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)).unwrap();
    file.write_to_file(&path).unwrap();

    let repo = DicomRepository;
    let series = repo.scan(&[dir.path().to_path_buf()], &NoProgress).unwrap().remove(0);
    let loaded = repo.load(&series, &NoProgress).unwrap();
    assert_eq!(loaded.volume.dims().x, 8);
    assert_eq!(loaded.volume.dims().y, 8);
    // Pixel data kept (slope 1, intercept 0). Compare physical values (the
    // volume stores a fixed-point representation, so `voxel` is not the raw
    // value); the multiset is independent of the reader's LPS reorientation.
    let vol = &loaded.volume;
    let mut got: Vec<f32> = (0..8).flat_map(|j| (0..8).map(move |i| vol.physical(i, j, 0).unwrap())).collect();
    let mut want: Vec<f32> = pixels.iter().map(|&p| f32::from(p)).collect();
    got.sort_by(|a, b| a.total_cmp(b));
    want.sort_by(|a, b| a.total_cmp(b));
    assert_eq!(got, want, "pixel data preserved");
    // Kept (non-table) attributes survive; identifiers are gone.
    let back = dicom_object::open_file(&path).unwrap();
    assert_eq!(str_at(&back, tags::MODALITY), "CT");
    assert_eq!(str_at(&back, tags::PATIENT_NAME), "ANONYMOUS");
    assert_ne!(str_at(&back, tags::PATIENT_ID), "123456");
    assert!(is_zero(&back, tags::STUDY_DATE));
    assert!(is_zero(&back, tags::ACCESSION_NUMBER));
    assert!(!present(&back, tags::PATIENT_AGE), "age removed by the Basic Profile");
    assert_eq!(str_at(&back, tags::PATIENT_IDENTITY_REMOVED), "YES");
}

/// A small loadable CT object with identifiers, for the read-back test.
fn ct_object(pixels: &[u16]) -> InMemDicomObject {
    let text = |t, vr, v: &str| DataElement::new(t, vr, PrimitiveValue::from(v));
    InMemDicomObject::from_element_iter([
        text(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        text(tags::SOP_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.2.1125.1.1"),
        text(tags::STUDY_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.2.1"),
        text(tags::SERIES_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.2.1125.1"),
        text(tags::FRAME_OF_REFERENCE_UID, VR::UI, "1.2.826.0.1.3680043.2.1125.9"),
        text(tags::MODALITY, VR::CS, "CT"),
        text(tags::PATIENT_NAME, VR::PN, "DOE^JOHN"),
        text(tags::PATIENT_ID, VR::LO, "123456"),
        text(tags::PATIENT_BIRTH_DATE, VR::DA, "19700101"),
        text(tags::PATIENT_SEX, VR::CS, "M"),
        text(tags::PATIENT_AGE, VR::AS, "055Y"),
        text(tags::STUDY_DATE, VR::DA, "20240428"),
        text(tags::STUDY_TIME, VR::TM, "102417"),
        text(tags::ACCESSION_NUMBER, VR::SH, "ACC-777"),
        text(tags::STUDY_DESCRIPTION, VR::LO, "CT chest for DOE^JOHN"),
        text(tags::INSTANCE_NUMBER, VR::IS, "1"),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1u16)),
        text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(8u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(8u16)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15u16)),
        DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(0u16)),
        text(tags::PIXEL_SPACING, VR::DS, "0.8\\0.6"),
        text(tags::SLICE_THICKNESS, VR::DS, "2.5"),
        text(tags::RESCALE_SLOPE, VR::DS, "1"),
        text(tags::RESCALE_INTERCEPT, VR::DS, "0"),
        text(tags::IMAGE_POSITION_PATIENT, VR::DS, "0\\0\\100"),
        text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
        DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::U16(pixels.to_vec().into())),
    ])
}

/// Canonical per-tag signature of an object (method attributes excluded).
fn snapshot(obj: &InMemDicomObject) -> BTreeMap<Tag, String> {
    let mut m = BTreeMap::new();
    for e in obj.iter() {
        let t = e.tag();
        if METHOD_TAGS.contains(&t) {
            continue;
        }
        m.insert(t, elem_sig(e));
    }
    m
}

fn elem_sig(e: &Elem) -> String {
    if e.vr() == VR::SQ {
        let items = e.items().unwrap_or(&[]);
        let mut s = format!("SQ[{}]", items.len());
        for item in items {
            s.push_str(&obj_sig(item));
        }
        s
    } else {
        e.to_str().map(|s| s.into_owned()).unwrap_or_else(|_| format!("{:?}", e.value()))
    }
}

fn obj_sig(obj: &InMemDicomObject) -> String {
    let mut parts: Vec<String> = obj.iter().map(|e| format!("{:?}={}", e.tag(), elem_sig(e))).collect();
    parts.sort();
    parts.join(";")
}

fn override_tags(opt: &str) -> BTreeSet<Tag> {
    fixture::FIXTURE.iter().filter(|(_, _, o)| *o == opt).map(|(t, _, _)| tag(t)).collect()
}

// covers 31.2-a — each option changes exactly the rows the table marks with it.
#[test]
fn each_option_changes_exactly_its_rows() {
    let source = full_object();
    let baseline = deid_default(&source);
    let base_snap = snapshot(&baseline);
    for opt in OPT_NAMES {
        let mut variant = source.clone();
        deidentify(&mut variant, &profile_with(opt), &mut UidMap::new(SALT));
        let var_snap = snapshot(&variant);
        let mut diff: BTreeSet<Tag> = BTreeSet::new();
        for t in base_snap.keys().chain(var_snap.keys()) {
            if base_snap.get(t) != var_snap.get(t) {
                diff.insert(*t);
            }
        }
        assert_eq!(diff, override_tags(opt), "option {opt} changed exactly its rows");
    }
}

// covers 31.2-a — the dates option shifts dates per patient and keeps times.
#[test]
fn retain_dates_modified_shifts_dates_consistently_and_keeps_times() {
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::PATIENT_ID, VR::LO, PrimitiveValue::from("123456")),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20240428")),
        DataElement::new(tags::STUDY_TIME, VR::TM, PrimitiveValue::from("102417")),
        DataElement::new(tags::ACQUISITION_DATE, VR::DA, PrimitiveValue::from("20240430")),
        DataElement::new(tags::ACQUISITION_TIME, VR::TM, PrimitiveValue::from("112417")),
    ]);
    deidentify(&mut obj, &profile_with("retain_dates_modified"), &mut UidMap::new(SALT));
    let study = str_at(&obj, tags::STUDY_DATE);
    let acq = str_at(&obj, tags::ACQUISITION_DATE);
    assert_ne!(study, "20240428");
    assert_ne!(acq, "20240430");
    assert_eq!(str_at(&obj, tags::STUDY_TIME), "102417", "time kept");
    assert_eq!(str_at(&obj, tags::ACQUISITION_TIME), "112417", "time kept");
    // Same per-patient offset: the two dates keep their 2-day separation.
    let d = |s: &str| -> i64 { s[6..8].parse::<i64>().unwrap() };
    assert_eq!(d(&acq) - d(&study), 2);
    assert_eq!(str_at(&obj, tags::LONGITUDINAL_TEMPORAL_INFORMATION_MODIFIED), "MODIFIED");
}

// covers 31.2-a — clean descriptors scrubs identifiers from free text.
#[test]
fn clean_descriptors_scrubs_identifiers() {
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::PATIENT_NAME, VR::PN, PrimitiveValue::from("DOE^JOHN")),
        DataElement::new(tags::PATIENT_ID, VR::LO, PrimitiveValue::from("123456")),
        DataElement::new(
            tags::STUDY_DESCRIPTION,
            VR::LO,
            PrimitiveValue::from("CT chest for DOE^JOHN id 123456 on 20240428"),
        ),
    ]);
    deidentify(&mut obj, &profile_with("clean_descriptors"), &mut UidMap::new(SALT));
    let desc = str_at(&obj, tags::STUDY_DESCRIPTION);
    assert!(desc.contains("CT chest"), "useful description retained: {desc}");
    assert!(!desc.contains("DOE"), "{desc}");
    assert!(!desc.contains("JOHN"), "{desc}");
    assert!(!desc.contains("123456"), "{desc}");
    assert!(!desc.contains("20240428"), "{desc}");
}

// covers 31.2-a — retain patient characteristics keeps age/sex/size/weight.
#[test]
fn retain_patient_characteristics_keeps_them() {
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::PATIENT_AGE, VR::AS, PrimitiveValue::from("055Y")),
        DataElement::new(tags::PATIENT_SEX, VR::CS, PrimitiveValue::from("M")),
        DataElement::new(tags::PATIENT_SIZE, VR::DS, PrimitiveValue::from("1.75")),
        DataElement::new(tags::PATIENT_WEIGHT, VR::DS, PrimitiveValue::from("70")),
    ]);
    deidentify(&mut obj, &profile_with("retain_patient_characteristics"), &mut UidMap::new(SALT));
    assert_eq!(str_at(&obj, tags::PATIENT_AGE), "055Y");
    assert_eq!(str_at(&obj, tags::PATIENT_SEX), "M");
    assert_eq!(str_at(&obj, tags::PATIENT_SIZE), "1.75");
    assert_eq!(str_at(&obj, tags::PATIENT_WEIGHT), "70");
}

// covers 31.2-b — an unknown option is refused; the default is conservative.
#[test]
fn unknown_option_is_refused_and_default_is_conservative() {
    let mut options = ProfileOptions::default();
    assert_eq!(options.enable("retain_everything"), Err(DeidError::UnknownOption("retain_everything".into())));
    assert!(Opt::from_name("not_an_option").is_none());
    // Default: no options.
    let default = ProfileOptions::default();
    assert!(!default.retain_uids);
    assert!(!default.retain_device_identity);
    assert!(!default.retain_institution_identity);
    assert!(!default.retain_patient_characteristics);
    assert!(!default.retain_dates_modified);
    assert!(!default.clean_descriptors);
    // Conservative default: age removed, sex zeroed, study date zeroed, UID replaced.
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::PATIENT_AGE, VR::AS, PrimitiveValue::from("055Y")),
        DataElement::new(tags::PATIENT_SEX, VR::CS, PrimitiveValue::from("M")),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20240428")),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, PrimitiveValue::from("1.2.826.0.1.3680043.2.1125.1.1")),
    ]);
    deidentify(&mut obj, &Profile::default(), &mut UidMap::new(SALT));
    assert!(!present(&obj, tags::PATIENT_AGE));
    assert!(is_zero(&obj, tags::PATIENT_SEX));
    assert!(is_zero(&obj, tags::STUDY_DATE));
    assert_ne!(str_at(&obj, tags::SOP_INSTANCE_UID), "1.2.826.0.1.3680043.2.1125.1.1");
}

// covers E.1.1 item 6 — the method attributes record the profile and options.
#[test]
fn method_attributes_record_profile_and_options() {
    let out = deid_default(&full_object());
    assert_eq!(str_at(&out, tags::PATIENT_IDENTITY_REMOVED), "YES");
    assert!(str_at(&out, tags::DEIDENTIFICATION_METHOD).contains("Basic Application Level Confidentiality Profile"));
    let codes = out.element(tags::DEIDENTIFICATION_METHOD_CODE_SEQUENCE).unwrap().items().unwrap();
    assert_eq!(str_at(&codes[0], tags::CODE_VALUE), "113100");
    // With an option, its CID 7050 code is added.
    let mut obj = full_object();
    deidentify(&mut obj, &profile_with("retain_uids"), &mut UidMap::new(SALT));
    let codes = obj.element(tags::DEIDENTIFICATION_METHOD_CODE_SEQUENCE).unwrap().items().unwrap();
    let values: Vec<String> = codes.iter().map(|c| str_at(c, tags::CODE_VALUE)).collect();
    assert!(values.contains(&"113100".to_string()));
    assert!(values.contains(&"113110".to_string()), "retain UIDs code: {values:?}");
}
