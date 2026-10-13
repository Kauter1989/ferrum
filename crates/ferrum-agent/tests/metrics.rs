//! `segment metrics` (Stage 18, `docs/radiomics.md`): the quantitative
//! profile of a segment through the public envelope, on phantoms with
//! known answers — a NIfTI study (no modality until declared) and a DICOM
//! CT series generated at run time.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use ferrum_agent::Agent;
use serde_json::{json, Value};

struct Fixture {
    _dir: tempfile::TempDir,
    nifti: PathBuf,
    ws: String,
    agent: Agent,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let nifti = ferrum_agent::evals::write_phantoms(&dir.path().join("in")).unwrap().remove(0);
    let ws = dir.path().join("ws/ct1").to_string_lossy().into_owned();
    Fixture { _dir: dir, nifti, ws, agent: Agent::default() }
}

impl Fixture {
    fn run(&self, command: &str, extra: Value) -> Value {
        let mut p = json!({ "workspace": self.ws });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.agent.run(command, &p)
    }

    fn ok(&self, command: &str, extra: Value) -> Value {
        let e = self.run(command, extra);
        assert_eq!(e["ok"], true, "{command}: {e:#}");
        e["data"].clone()
    }

    fn err(&self, command: &str, extra: Value) -> Value {
        let e = self.run(command, extra);
        assert_eq!(e["ok"], false, "{command} should fail: {e:#}");
        e["error"].clone()
    }

    /// Opens the NIfTI phantom, declared as CT unless `modality` is `None`,
    /// and cuts the cube (216 voxels of 1000 HU) and the sphere out of it.
    fn open(&self, modality: Option<&str>) {
        let mut p = json!({ "path": self.nifti });
        if let Some(m) = modality {
            p["modality"] = json!(m);
        }
        self.ok("study open", p);
        self.ok(
            "segment threshold",
            json!({ "seed": { "voxel": [30, 30, 14] }, "min": 500, "max": 1500, "name": "Cube" }),
        );
        self.ok(
            "segment threshold",
            json!({ "seed": { "voxel": [14, 20, 15] }, "min": 50, "max": 150, "name": "Sphere" }),
        );
    }
}

fn value(data: &Value, id: &str) -> f64 {
    data["features"][id]["value"].as_f64().unwrap_or_else(|| panic!("{id}: {:#}", data["features"][id]))
}

fn close(a: f64, b: f64, tol: f64, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} != {b}");
}

// covers 18.1-c, 18.2-a, 18.2-b, 18.3-c
#[test]
fn the_quick_profile_of_the_cube_has_the_known_answers() {
    let f = fixture();
    f.open(Some("ct"));
    let m = f.ok("segment metrics", json!({ "segment": 1 }));
    assert_eq!((m["profile"]["name"].as_str(), m["profile"]["version"].as_str()), (Some("quick"), Some("0.1")));
    assert_eq!((m["unit"].as_str(), m["params"]["preprocessing"].as_str()), (Some("HU"), Some("none")));
    assert_eq!(m["segment"]["label"], 1);
    assert_eq!(m["segment"]["provenance"]["status"], "proposed", "the segment's provenance travels with its numbers");
    // 6 x 6 x 6 voxels of 1 x 1 x 2 mm
    assert_eq!(value(&m, "size.voxels"), 216.0);
    assert_eq!(value(&m, "size.volume_mm3"), 432.0);
    assert_eq!(value(&m, "size.volume_ml"), 0.432);
    assert_eq!(
        [value(&m, "size.extent_i_mm"), value(&m, "size.extent_j_mm"), value(&m, "size.extent_k_mm")],
        [6.0, 6.0, 12.0]
    );
    // centroid voxel (30.5, 30.5, 14.5) with origin (-20, -20, 100) mm and 1 x 1 x 2 mm spacing
    assert_eq!(
        [value(&m, "size.centroid_x_mm"), value(&m, "size.centroid_y_mm"), value(&m, "size.centroid_z_mm")],
        [10.5, 10.5, 129.0]
    );
    // every voxel is 1000 HU
    for id in ["intensity.mean", "intensity.min", "intensity.max", "intensity.p5", "intensity.p50", "intensity.p95"] {
        close(value(&m, id), 1000.0, 0.1, id);
        assert_eq!(m["features"][id]["unit"], "HU");
    }
    for id in ["intensity.std", "intensity.range", "intensity.iqr"] {
        close(value(&m, id), 0.0, 0.1, id);
    }
    assert_eq!(m["features"]["size.volume_ml"]["unit"], "mL");
}

// covers 18.2-d, 18.3-a
#[test]
fn size_equals_segment_shape_and_intensity_equals_stats() {
    let f = fixture();
    f.open(Some("ct"));
    for label in [1, 2] {
        let m = f.ok("segment metrics", json!({ "segment": label }));
        let shape = &f.ok("segment shape", json!({ "segment": label }))["shape"];
        for (id, key) in [("size.extent_i_mm", 0), ("size.extent_j_mm", 1), ("size.extent_k_mm", 2)] {
            close(value(&m, id), shape["extent_mm"][key].as_f64().unwrap(), 0.01, id);
        }
        close(value(&m, "size.long_axis_mm"), shape["long_axis"]["mm"].as_f64().unwrap(), 0.01, "long axis");
        close(value(&m, "size.short_axis_mm"), shape["short_axis"]["mm"].as_f64().unwrap(), 0.01, "short axis");
        assert_eq!(value(&m, "size.largest_slice_axial"), shape["slices"]["axial"]["largest"].as_f64().unwrap());
        assert_eq!(value(&m, "size.largest_slice_coronal"), shape["slices"]["coronal"]["largest"].as_f64().unwrap());
        assert_eq!(value(&m, "size.largest_slice_sagittal"), shape["slices"]["sagittal"]["largest"].as_f64().unwrap());
        close(value(&m, "size.volume_ml"), shape["volume_ml"].as_f64().unwrap(), 0.0005, "volume");

        let stats = f.ok("stats", json!({ "segment": label }));
        close(value(&m, "intensity.mean"), stats["mean"].as_f64().unwrap(), 0.005, "mean");
        close(value(&m, "intensity.std"), stats["std"].as_f64().unwrap(), 0.005, "std");
        // `stats` leaves the percentiles unrounded, the envelope of the metrics keeps four digits
        for (id, p) in
            [("intensity.p5", "p5"), ("intensity.p25", "p25"), ("intensity.p50", "p50"), ("intensity.p95", "p95")]
        {
            close(value(&m, id), stats["percentiles"][p].as_f64().unwrap(), 0.0001, id);
        }
        assert_eq!(value(&m, "size.voxels"), stats["voxels"].as_f64().unwrap());
    }
}

// covers 18.3-e
#[test]
fn a_study_without_a_modality_is_refused_with_a_hint_and_the_shape_still_works() {
    let f = fixture();
    f.open(None);
    let e = f.err("segment metrics", json!({ "segment": 1 }));
    assert_eq!(e["code"], "bad_request");
    assert!(e["message"].as_str().unwrap().contains("unit of the voxel values is unknown"), "{e:#}");
    assert!(e["hint"].as_str().unwrap().contains("--modality CT"), "{e:#}");
    f.ok("segment shape", json!({ "segment": 1 }));

    // declaring MR does not make HU of it
    f.ok("study open", json!({ "path": f.nifti, "modality": "mr" }));
    let mr = f.err("segment metrics", json!({ "segment": 1 }));
    assert!(mr["message"].as_str().unwrap().contains("MR"), "{mr:#}");

    // declared CT: the same call now works, in HU
    f.ok("study open", json!({ "path": f.nifti, "modality": "ct" }));
    let ct = f.ok("segment metrics", json!({ "segment": 1 }));
    assert_eq!(ct["unit"], "HU");
    assert_eq!(value(&ct, "size.voxels"), 216.0);
}

// covers 18.1-b, 18.2-f, 18.4-g
#[test]
fn errors_are_specific_and_describe_adds_the_definitions() {
    let f = fixture();
    f.open(Some("ct"));
    let bad = f.err("segment metrics", json!({ "segment": 1, "profile": "everything" }));
    assert_eq!(bad["code"], "bad_request");
    assert!(bad["message"].as_str().unwrap().contains("quick"), "the known profiles are listed: {bad:#}");
    assert_eq!(f.err("segment metrics", json!({ "segment": 99 }))["code"], "not_found");
    let plain = f.ok("segment metrics", json!({ "segment": 2 }));
    assert!(plain["features"].as_object().unwrap().values().all(|v| v.get("definition").is_none()));
    let described = f.ok("segment metrics", json!({ "segment": 2, "describe": true }));
    let features = described["features"].as_object().unwrap();
    assert_eq!(features.len(), plain["features"].as_object().unwrap().len());
    for (id, v) in features {
        assert!(v["definition"].as_str().unwrap().len() > 20, "{id} has no definition");
    }
    assert!(features["intensity.std"]["definition"].as_str().unwrap().contains("Population"));

    // a segment that exists but holds no voxels: cut the cube away with a box elsewhere
    let cut = json!({ "segment": 1, "op": "restrict_to_box", "box": { "min": { "voxel": [0, 0, 0] }, "max": { "voxel": [2, 2, 2] } } });
    f.ok("segment edit", cut);
    let empty = f.err("segment metrics", json!({ "segment": 1 }));
    assert_eq!(empty["code"], "not_found", "{empty:#}");
    assert!(empty["message"].as_str().unwrap().contains("no voxels"), "{empty:#}");
}

// covers 18.1-c
#[test]
fn the_same_call_gives_the_same_numbers_bit_for_bit() {
    let f = fixture();
    f.open(Some("ct"));
    let (a, b) = (f.ok("segment metrics", json!({ "segment": 2 })), f.ok("segment metrics", json!({ "segment": 2 })));
    assert_eq!(a, b);
}

/// 16 x 16 x 8 CT in HU: air (-1000) and a block of 400 HU (voxels 5..11 in i
/// and j, slices 2..6), 1 x 1 x 2 mm, with a `CT` modality in the header.
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
        let uid = "1.2.826.0.1.3680043.8.498.777";
        let elements = vec![
            text(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
            text(tags::SOP_INSTANCE_UID, VR::UI, &format!("{uid}.{}", k + 1)),
            text(tags::MODALITY, VR::CS, "CT"),
            text(tags::STUDY_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.8.498.776"),
            text(tags::SERIES_INSTANCE_UID, VR::UI, uid),
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

// covers 18.3-c
#[test]
fn a_dicom_ct_series_is_in_hu_without_being_told() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("series");
    write_ct(&src);
    let ws = dir.path().join("ws/ct").to_string_lossy().into_owned();
    let agent = Agent::default();
    let run = |c: &str, extra: Value| {
        let mut p = json!({ "workspace": ws });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let e = agent.run(c, &p);
        assert_eq!(e["ok"], true, "{c}: {e:#}");
        e["data"].clone()
    };
    run("study open", json!({ "path": src }));
    run("segment threshold", json!({ "seed": { "voxel": [8, 8, 3] }, "min": 300, "max": 500 }));
    let m = run("segment metrics", json!({ "segment": 1 }));
    assert_eq!(m["unit"], "HU");
    // 6 x 6 x 4 voxels of 1 x 1 x 2 mm, all 400 HU; origin (-8, -8, 0)
    assert_eq!(value(&m, "size.voxels"), 144.0);
    assert_eq!(value(&m, "size.volume_mm3"), 288.0);
    assert_eq!([value(&m, "size.centroid_x_mm"), value(&m, "size.centroid_y_mm")], [-8.0 + 7.5, -8.0 + 7.5]);
    assert_eq!(value(&m, "size.centroid_z_mm"), 7.0);
    close(value(&m, "intensity.mean"), 400.0, 1e-6, "mean");
    close(value(&m, "intensity.std"), 0.0, 1e-6, "std");
}

// covers 18.4-a
#[test]
fn the_schema_lists_exactly_the_registered_profiles() {
    let (description, schema) =
        ferrum_agent::schema::command_schema("segment metrics").expect("the command has a schema");
    assert!(description.contains("Quantitative profile"));
    let listed: Vec<&str> =
        schema["properties"]["profile"]["enum"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let registered: Vec<&str> = ferrum_radiomics::PROFILES.iter().map(|p| p.name).collect();
    assert_eq!(listed, registered, "the schema and the registry must not drift apart");
    assert_eq!(schema["required"], json!(["workspace", "segment"]));
}

// covers 18.5-b, 18.6-b, 18.6-c, 18.6-d
#[test]
fn the_clinical_profile_adds_the_shape_with_known_axes_and_says_why_a_direction_is_missing() {
    let f = fixture();
    f.open(Some("ct"));
    let quick = f.ok("segment metrics", json!({ "segment": 1 }));
    let c = f.ok("segment metrics", json!({ "segment": 1, "profile": "clinical" }));
    assert_eq!((c["profile"]["name"].as_str(), c["profile"]["version"].as_str()), (Some("clinical"), Some("0.3")));
    for (id, v) in quick["features"].as_object().unwrap() {
        assert_eq!(&c["features"][id], v, "{id} is the same in both profiles");
    }
    // the cube: 6 x 6 x 6 voxels of 1 x 1 x 2 mm. Index variance 35 / 12, four times that along k
    close(value(&c, "shape.major_axis_mm"), 4.0 * (35.0_f64 / 3.0).sqrt(), 1e-3, "major axis");
    close(value(&c, "shape.minor_axis_mm"), 4.0 * (35.0_f64 / 12.0).sqrt(), 1e-3, "minor axis");
    close(value(&c, "shape.least_axis_mm"), 4.0 * (35.0_f64 / 12.0).sqrt(), 1e-3, "least axis");
    close(value(&c, "shape.elongation"), 0.5, 1e-6, "elongation");
    close(value(&c, "shape.flatness"), 0.5, 1e-6, "flatness");
    assert_eq!(
        [value(&c, "shape.major_axis_dir_x"), value(&c, "shape.major_axis_dir_y"), value(&c, "shape.major_axis_dir_z")],
        [0.0, 0.0, 1.0]
    );
    // the other two axes are equally long: no direction, and the reason is in the result
    let tie = &c["features"]["shape.minor_axis_dir_x"];
    assert!(tie["value"].is_null() && tie["reason"].as_str().unwrap().contains("not unique"), "{tie:#}");
    // a cube is less spherical than a ball, and the formulas agree with each other
    let sphericity = value(&c, "shape.sphericity");
    assert!((0.78..0.86).contains(&sphericity), "{sphericity}");
    close(sphericity.powi(3), value(&c, "shape.compactness_2"), 1e-5, "compactness 2 is the cube of the sphericity");
    close(sphericity * value(&c, "shape.spherical_disproportion"), 1.0, 1e-5, "disproportion is the inverse");
    assert_eq!(c["features"]["shape.surface_area_mm2"]["unit"], "mm2");

    // the ball of radius 8 mm: round, so rounder than the cube; its mesh area is a little above 4 pi r^2
    let ball = f.ok("segment metrics", json!({ "segment": 2, "profile": "clinical" }));
    let s = value(&ball, "shape.sphericity");
    assert!((0.89..0.95).contains(&s) && s > sphericity, "{s}");
    let area = value(&ball, "shape.surface_area_mm2");
    let analytic = 4.0 * std::f64::consts::PI * 64.0;
    assert!((1.03..1.09).contains(&(area / analytic)), "area {area} against {analytic}");
    // 4 a / sqrt(5) = 14.31 mm; measured 14.23 on 2 mm slices: within 1 %
    close(value(&ball, "shape.major_axis_mm"), 4.0 * 8.0 / 5.0_f64.sqrt(), 0.15, "ball axis 4 a / sqrt(5)");
}

// covers 18.2-c, 18.7-a, 18.8-a, 18.8-b, 18.8-c, 18.8-d
#[test]
fn the_clinical_profile_adds_diameter_hull_and_connectivity() {
    let f = fixture();
    f.open(Some("ct"));
    let cube = f.run("segment metrics", json!({ "segment": 1, "profile": "clinical" }));
    assert_eq!(cube["ok"], true, "{cube:#}");
    let c = &cube["data"];
    // 6 x 6 x 6 voxels of 1 x 1 x 2 mm: the centres span 5 steps on each axis
    close(value(c, "size.max_diameter_mm"), 150f64.sqrt(), 1e-3, "space diagonal");
    // a box is its own hull
    close(value(c, "shape.solidity"), 1.0, 1e-5, "solidity");
    close(value(c, "shape.convexity"), 1.0, 1e-5, "convexity");
    close(value(c, "shape.hull_volume_ml"), value(c, "shape.mesh_volume_ml"), 1e-5, "hull volume");
    assert_eq!(c["features"]["shape.hull_area_mm2"]["unit"], "mm2");
    for (id, expected) in [
        ("topology.components", 1.0),
        ("topology.largest_component_share", 1.0),
        ("topology.holes", 0.0),
        ("topology.hole_volume_mm3", 0.0),
        ("topology.euler_number", 1.0),
    ] {
        assert_eq!(value(c, id), expected, "{id}");
    }
    let warnings = cube["warnings"].to_string();
    assert!(!warnings.contains("fragmented") && !warnings.contains("has_holes"), "{warnings}");
    // the ball: a connected body without a cavity; its hull is only a little larger than the mesh
    let ball = f.ok("segment metrics", json!({ "segment": 2, "profile": "clinical" }));
    assert_eq!((value(&ball, "topology.components"), value(&ball, "topology.holes")), (1.0, 0.0));
    assert_eq!(value(&ball, "topology.euler_number"), 1.0);
    let solidity = value(&ball, "shape.solidity");
    assert!((0.9..=1.0).contains(&solidity), "{solidity}");
    close(value(&ball, "size.max_diameter_mm"), 2.0 * 8.0, 1.5, "ball diameter");
    // `quick` has no hull or connectivity, and no warnings of its own
    let quick = f.run("segment metrics", json!({ "segment": 1 }));
    assert!(quick["data"]["features"].get("shape.solidity").is_none());
    assert!(quick["data"]["features"].get("size.max_diameter_mm").is_none());
}

// covers 18.9-a, 18.9-c, 18.9-d, 18.10-a, 18.11-a, 18.11-c, 18.11-d, 18.12-b
#[test]
fn the_clinical_profile_adds_the_distribution_the_curve_and_the_hu_ranges() {
    let f = fixture();
    f.open(Some("ct"));
    let c = f.ok("segment metrics", json!({ "segment": 1, "profile": "clinical" }));
    // the cube is one value (1000 HU): no skewness or kurtosis, and a reason
    for id in ["intensity.skewness", "intensity.kurtosis"] {
        let v = &c["features"][id];
        assert!(v["value"].is_null() && v["reason"].as_str().unwrap().contains("all voxel values are equal"), "{v:#}");
    }
    close(value(&c, "intensity.energy"), 216.0 * 1e6, 1e3, "energy");
    assert_eq!(c["features"]["intensity.energy"]["unit"], "HU2");
    close(value(&c, "intensity.rms"), 1000.0, 1e-3, "rms");
    for (id, expected) in [
        ("intensity.mad", 0.0),
        ("intensity.rmad", 0.0),
        ("intensity.entropy", 0.0),
        ("intensity.uniformity", 1.0),
        ("intensity.ivh_v10", 1.0),
        ("intensity.ivh_v90", 1.0),
    ] {
        close(value(&c, id), expected, 1e-6, id);
    }
    close(value(&c, "intensity.ivh_i10"), 1000.0, 1e-3, "I10");
    // the parameters the numbers depend on are in the result
    assert_eq!((c["params"]["bin_width"].as_f64(), c["params"]["bin_width_unit"].as_str()), (Some(25.0), Some("HU")));
    // the curve: 101 samples, starting at 1 and never rising
    let fraction: Vec<f64> =
        c["ivh"]["volume_fraction"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    assert_eq!((fraction.len(), c["ivh"]["intensity"].as_array().unwrap().len()), (101, 101));
    assert!(fraction[0] == 1.0 && fraction.windows(2).all(|w| w[1] <= w[0]));
    // the ranges: defaults are marked as such and, with the outside, add up to one
    let r = &c["hu_ranges"];
    assert_eq!(r["default"], true);
    assert!(r["note"].as_str().unwrap().contains("not clinical thresholds"), "{r:#}");
    let sum: f64 = r["ranges"].as_array().unwrap().iter().map(|x| x["fraction"].as_f64().unwrap()).sum::<f64>()
        + r["outside"].as_f64().unwrap();
    close(sum, 1.0, 1e-9, "shares and outside");
    // ranges given by the caller come back as given
    let own = f.ok(
        "segment metrics",
        json!({ "segment": 1, "profile": "clinical", "hu_ranges": [[900, 1100], [-50, 50]], "bin_width": 10 }),
    );
    let ranges = own["hu_ranges"]["ranges"].as_array().unwrap();
    assert_eq!((own["hu_ranges"]["default"].clone(), ranges.len()), (json!(false), 2));
    assert_eq!(
        (ranges[0]["min"].as_f64(), ranges[0]["max"].as_f64(), ranges[0]["fraction"].as_f64()),
        (Some(900.0), Some(1100.0), Some(1.0))
    );
    assert_eq!((ranges[1]["voxels"].as_u64(), own["hu_ranges"]["outside"].as_f64()), (Some(0), Some(0.0)));
    assert_eq!(own["params"]["bin_width"].as_f64(), Some(10.0));
    // quick has no histogram, curve or ranges
    let quick = f.ok("segment metrics", json!({ "segment": 1 }));
    assert!(quick.get("ivh").is_none() && quick.get("hu_ranges").is_none());
    assert!(quick["params"].get("bin_width").is_none());
}

// covers 18.11-b
#[test]
fn nonsense_parameters_are_bad_requests_with_a_hint() {
    let f = fixture();
    f.open(Some("ct"));
    let metrics = |extra: Value| {
        let mut p = json!({ "segment": 1, "profile": "clinical" });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        f.err("segment metrics", p)
    };
    let overlap = metrics(json!({ "hu_ranges": [[0, 100], [50, 150]] }));
    assert_eq!(overlap["code"], "bad_request");
    assert!(overlap["message"].as_str().unwrap().contains("overlap"), "{overlap:#}");
    assert!(overlap["hint"].as_str().unwrap().contains("half-open"), "{overlap:#}");
    assert_eq!(metrics(json!({ "bin_width": 0 }))["code"], "bad_request");
    assert_eq!(metrics(json!({ "bin_width": -5 }))["code"], "bad_request");
    assert_eq!(metrics(json!({ "bin_width": "wide" }))["code"], "bad_request");
    for bad in [json!([[1]]), json!([[1, 2, 3]]), json!(["a"]), json!([[0, "x"]]), json!(5), json!([[5, 5]])] {
        assert_eq!(metrics(json!({ "hu_ranges": bad.clone() }))["code"], "bad_request", "{bad}");
    }
    // touching ranges are fine
    f.ok("segment metrics", json!({ "segment": 1, "profile": "clinical", "hu_ranges": [[0, 100], [100, 200]] }));
}
