//! Masked renders (Clean Pixel Data, Stage 31, criterion 31.3-c): a phantom
//! with burned-in text in a known region renders with the region replaced
//! by the solid fill and every other pixel unchanged, and the sidecar
//! reports what was applied — never silently passed.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{IntensityRange, Volume, WindowLevel};
use serde_json::{json, Value};

const COLS: u32 = 32;
const ROWS: u32 = 24;
const SLICES: u32 = 3;
/// The burned-in "text": a bright block at stored pixels x 5..17, y 3..9.
const TEXT: (u32, u32, u32, u32) = (5, 3, 12, 6);
const TEXT_VALUE: u16 = 30000;

/// Stored pixel `p` of any slice: a non-zero gradient, or the bright text.
fn stored(p: u32) -> u16 {
    let (x, y) = (p % COLS, p / COLS);
    if x >= TEXT.0 && x < TEXT.0 + TEXT.2 && y >= TEXT.1 && y < TEXT.1 + TEXT.3 {
        TEXT_VALUE
    } else {
        (p + 1) as u16
    }
}

/// Writes the phantom as a 3-slice axial DICOM series (1 mm, identity
/// orientation, slope 1 / intercept 0), with an optional BurnedInAnnotation
/// flag.
fn write_dicom(dir: &Path, flag: Option<&str>) {
    std::fs::create_dir_all(dir).unwrap();
    for k in 0..SLICES {
        let pixels: Vec<u16> = (0..COLS * ROWS).map(stored).collect();
        let text = |t, vr, v: &str| DataElement::new(t, vr, PrimitiveValue::from(v));
        let mut elements = vec![
            text(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
            text(tags::SOP_INSTANCE_UID, VR::UI, &format!("1.2.826.0.1.3680043.8.498.77.{}", k + 1)),
            text(tags::MODALITY, VR::CS, "CT"),
            text(tags::STUDY_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.8.498.77"),
            text(tags::SERIES_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.8.498.77.1"),
            text(tags::INSTANCE_NUMBER, VR::IS, &(k + 1).to_string()),
            text(tags::IMAGE_POSITION_PATIENT, VR::DS, &format!("0\\0\\{k}")),
            text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
            text(tags::PIXEL_SPACING, VR::DS, "1\\1"),
            text(tags::SLICE_THICKNESS, VR::DS, "1"),
            text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
            text(tags::RESCALE_SLOPE, VR::DS, "1"),
            text(tags::RESCALE_INTERCEPT, VR::DS, "0"),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(ROWS as u16)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(COLS as u16)),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(0u16)),
            DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::U16(pixels.into())),
        ];
        if let Some(f) = flag {
            elements.push(text(tags::BURNED_IN_ANNOTATION, VR::CS, f));
        }
        let obj = InMemDicomObject::from_element_iter(elements);
        obj.with_meta(FileMetaTableBuilder::new().transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN))
            .unwrap()
            .write_to_file(dir.join(format!("img_{k:03}.dcm")))
            .unwrap();
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    ws: String,
    agent: Agent,
}

/// A workspace over the DICOM phantom (with the flag), or over a NIfTI
/// phantom (no flag exists in NIfTI).
fn fixture(dicom: bool, flag: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    if dicom {
        write_dicom(&data, flag);
    } else {
        std::fs::create_dir_all(&data).unwrap();
        let dims = ferrum_domain::Dims3::new(COLS, ROWS, SLICES);
        let values: Vec<f32> =
            (0..dims.voxel_count()).map(|p| f32::from(stored((p % (COLS * ROWS) as usize) as u32))).collect();
        let v = Volume::from_physical(dims, glam::Vec3::ONE, &values).unwrap();
        ferrum_io::write_nifti(&v, &data.join("phantom.nii.gz")).unwrap();
    }
    let ws = dir.path().join("ws").to_string_lossy().into_owned();
    let agent = Agent::new(AgentConfig::default());
    let env = agent.run("study open", &json!({ "workspace": ws, "path": data }));
    assert_eq!(env["ok"], true, "study open: {env:#}");
    Fixture { _dir: dir, ws, agent }
}

impl Fixture {
    fn slice(&self, extra: Value) -> Value {
        let mut p = json!({
            "workspace": self.ws, "plane": "axial", "slice_number": 2,
            "window": "full_range", "size": COLS,
        });
        if let (Some(o), Value::Object(m)) = (p.as_object_mut(), extra) {
            o.extend(m);
        }
        let env = self.agent.run("view slice", &p);
        assert_eq!(env["ok"], true, "view slice: {env:#}");
        env["data"].clone()
    }
}

/// RGB pixels of a render PNG.
fn pixels(path: &str) -> (u32, u32, Vec<u8>) {
    let img = image::open(path).unwrap().to_rgb8();
    (img.width(), img.height(), img.into_raw())
}

fn in_text(x: u32, y: u32) -> bool {
    x >= TEXT.0 && x < TEXT.0 + TEXT.2 && y >= TEXT.1 && y < TEXT.1 + TEXT.3
}

/// The grey the full-range window gives a physical value of the phantom.
fn grey(value: f32) -> u8 {
    // the reader's range: the data are integers on a 1-step grid from 1
    let range = IntensityRange::for_data(1.0, f32::from(TEXT_VALUE), |min, step| {
        (1..=COLS * ROWS).map(|p| f32::from(stored(p - 1))).all(|v| IntensityRange::is_on_grid(v, min, step))
    })
    .unwrap();
    let window = WindowLevel::full_range(range);
    ferrum_domain::color::unit_to_u8(window.apply(value))
}

#[test]
fn a_masked_render_replaces_the_region_and_keeps_every_other_pixel() {
    // covers 31.3-c
    let f = fixture(true, Some("YES"));
    let base = f.slice(json!({}));
    let masked = f.slice(json!({
        "masked": true,
        "masks": [{ "min": { "voxel": [5, 3, 1] }, "max": { "voxel": [16, 8, 1] } }],
    }));
    let (bw, bh, bpx) = pixels(base["image"].as_str().unwrap());
    let (mw, mh, mpx) = pixels(masked["image"].as_str().unwrap());
    assert_eq!((bw, bh), (COLS, ROWS), "one pixel per voxel at size 32");
    assert_eq!((mw, mh), (bw, bh));
    let fill_grey = grey(1.0); // the fill is stored 0 = the range minimum
    let text_grey = grey(f32::from(TEXT_VALUE));
    assert!(text_grey > fill_grey + 50, "the text is bright against the fill");
    for y in 0..ROWS {
        for x in 0..COLS {
            let i = ((y * COLS + x) * 3) as usize;
            if in_text(x, y) {
                assert_eq!(bpx[i], text_grey, "the unmasked render shows the text at ({x}, {y})");
                assert_eq!(mpx[i], fill_grey, "the masked render fills ({x}, {y})");
            } else {
                assert_eq!(mpx[i..i + 3], bpx[i..i + 3], "pixel ({x}, {y}) outside the region is unchanged");
            }
        }
    }
    // the sidecar reports the mask: requested, applied, where it came from
    let mask = &masked["mask"];
    assert_eq!(mask["requested"], true);
    assert_eq!(mask["applied"], true);
    assert_eq!(mask["source"], "regions");
    assert_eq!(mask["fill"], 0);
    assert_eq!(mask["boxes"], json!([{ "min": [5, 3, 1], "max": [17, 9, 2] }]));
    assert!(base.get("mask").is_none(), "an unmasked render carries no mask block");
}

#[test]
fn the_flag_masks_the_whole_volume_when_no_region_is_given() {
    // covers 31.3-c: the conservative default, reported as burned_in.
    let f = fixture(true, Some("YES"));
    let masked = f.slice(json!({ "masked": true }));
    let (w, h, px) = pixels(masked["image"].as_str().unwrap());
    assert_eq!((w, h), (COLS, ROWS));
    let fill_grey = grey(1.0);
    assert!(px.chunks(3).all(|p| p[0] == fill_grey), "every pixel holds the fill");
    let mask = &masked["mask"];
    assert_eq!(mask["applied"], true);
    assert_eq!(mask["source"], "burned_in");
    assert_eq!(mask["boxes"], json!([{ "min": [0, 0, 0], "max": [COLS, ROWS, SLICES] }]));
}

#[test]
fn an_unflagged_study_reports_nothing_to_mask() {
    // covers 31.3-c: skipped is reported, never silently passed.
    let f = fixture(false, None); // NIfTI: no flag exists
    let base = f.slice(json!({}));
    let masked = f.slice(json!({ "masked": true }));
    let (_, _, bpx) = pixels(base["image"].as_str().unwrap());
    let (_, _, mpx) = pixels(masked["image"].as_str().unwrap());
    assert_eq!(mpx, bpx, "nothing to mask: the image is unchanged");
    let mask = &masked["mask"];
    assert_eq!(mask["requested"], true);
    assert_eq!(mask["applied"], false);
    assert_eq!(mask["source"], "none");
    assert_eq!(mask["boxes"], json!([]));
}

#[test]
fn a_mask_region_at_the_edge_masks_to_the_border() {
    // covers 31.3-c
    let f = fixture(true, Some("NO"));
    let masked = f.slice(json!({
        "masked": true,
        "masks": [{ "min": { "voxel": [28, 20, 1] }, "max": { "voxel": [31, 23, 1] } }],
    }));
    let mask = &masked["mask"];
    assert_eq!(mask["applied"], true);
    assert_eq!(mask["source"], "regions");
    // inclusive corners become the half-open box up to the grid border
    assert_eq!(mask["boxes"], json!([{ "min": [28, 20, 1], "max": [32, 24, 2] }]));
    let (_, _, px) = pixels(masked["image"].as_str().unwrap());
    let fill_grey = grey(1.0);
    for y in 20..ROWS {
        for x in 28..COLS {
            let i = ((y * COLS + x) * 3) as usize;
            assert_eq!(px[i], fill_grey, "the edge region is filled at ({x}, {y})");
        }
    }
    // a corner outside the grid is refused, as for every box parameter
    let env = f.agent.run(
        "view slice",
        &json!({
            "workspace": f.ws, "plane": "axial", "slice_number": 2, "window": "full_range", "size": COLS,
            "masked": true,
            "masks": [{ "min": { "voxel": [28, 20, 1] }, "max": { "voxel": [40, 30, 1] } }],
        }),
    );
    assert_eq!(env["ok"], false);
    assert_eq!(env["error"]["code"], "out_of_volume");
}
