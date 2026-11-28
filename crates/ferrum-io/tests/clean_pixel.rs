//! Clean Pixel Data: region masking of burned-in annotation (Stage 31,
//! criterion 31.3-a). A phantom carries "burned-in text" — a bright block —
//! in a known region; the tests check pixel by pixel that the region is
//! replaced by the fill and the rest of the image is unchanged.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use dicom_core::value::Value;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;
use ferrum_io::dicom::clean_pixel::{
    burned_in_annotation, clean_pixel_data, sources_burned_in, CleanPixelError, CleanPixelSpec, PixelRegion,
};

const COLS: u32 = 32;
const ROWS: u32 = 24;
/// Where the phantom's "burned-in text" lies (stored pixel coordinates).
const TEXT: PixelRegion = PixelRegion::new(5, 3, 12, 6);
const FILL: u16 = 0;

/// Background gradient (every pixel non-zero, so a zero fill is visible)
/// with the bright text block in [`TEXT`].
fn pixels_with_text() -> Vec<u16> {
    let mut d: Vec<u16> = (0..COLS * ROWS).map(|p| (p + 1) as u16).collect();
    for row in TEXT.y..TEXT.y + TEXT.h {
        for col in TEXT.x..TEXT.x + TEXT.w {
            d[(row * COLS + col) as usize] = 30000;
        }
    }
    d
}

/// A monochrome 16-bit object with the phantom pixels and an optional flag.
fn phantom(flag: Option<&str>) -> InMemDicomObject {
    object(PrimitiveValue::U16(pixels_with_text().into()), 1, 16, flag, 1, None)
}

/// An object with the given pixel layout.
fn object(
    pixels: PrimitiveValue,
    spp: u16,
    bits: u16,
    flag: Option<&str>,
    frames: u32,
    planar: Option<u16>,
) -> InMemDicomObject {
    let mut elements = vec![
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(ROWS as u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(COLS as u16)),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(spp)),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from(if spp == 1 { "MONOCHROME2" } else { "RGB" }),
        ),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(bits)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(bits)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(bits - 1)),
        DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(0u16)),
        DataElement::new(tags::PIXEL_DATA, if bits == 16 { VR::OW } else { VR::OB }, pixels),
    ];
    if frames > 1 {
        elements.push(DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, PrimitiveValue::from(frames.to_string())));
    }
    if let Some(p) = planar {
        elements.push(DataElement::new(tags::PLANAR_CONFIGURATION, VR::US, PrimitiveValue::from(p)));
    }
    if let Some(f) = flag {
        elements.push(DataElement::new(tags::BURNED_IN_ANNOTATION, VR::CS, PrimitiveValue::from(f)));
    }
    InMemDicomObject::from_element_iter(elements)
}

/// The stored pixels of a 16-bit object (signed bit patterns included).
fn pixels_u16(obj: &InMemDicomObject) -> Vec<u16> {
    match obj.element(tags::PIXEL_DATA).unwrap().value() {
        Value::Primitive(PrimitiveValue::U16(d)) => d.to_vec(),
        Value::Primitive(PrimitiveValue::I16(d)) => d.iter().map(|v| *v as u16).collect(),
        other => panic!("expected 16-bit pixels, got {other:?}"),
    }
}

/// The stored pixels of an 8-bit object (signed 8-bit data is stored as
/// bytes too; the fill is a bit pattern).
fn pixels_u8(obj: &InMemDicomObject) -> Vec<u8> {
    match obj.element(tags::PIXEL_DATA).unwrap().value() {
        Value::Primitive(PrimitiveValue::U8(d)) => d.to_vec(),
        other => panic!("expected 8-bit pixels, got {other:?}"),
    }
}

fn inside(regions: &[PixelRegion], col: u32, row: u32) -> bool {
    regions.iter().any(|r| col >= r.x && col < r.x + r.w && row >= r.y && row < r.y + r.h)
}

/// Pixel by pixel: every pixel of `regions` holds the fill, every other
/// pixel holds its original value.
fn assert_masked(after: &[u16], original: &[u16], regions: &[PixelRegion], fill: u16) {
    assert_eq!(after.len(), original.len());
    for p in 0..after.len() {
        let (col, row) = (p as u32 % COLS, p as u32 / COLS);
        let want = if inside(regions, col, row) { fill } else { original[p] };
        assert_eq!(after[p], want, "pixel ({col}, {row})");
    }
}

#[test]
fn a_configured_region_is_replaced_and_the_rest_is_unchanged() {
    // covers 31.3-a
    let mut obj = phantom(None);
    let original = pixels_with_text();
    let spec = CleanPixelSpec { regions: vec![TEXT], fill: FILL };
    let report = clean_pixel_data(&mut obj, &spec).unwrap();
    assert!(report.masked);
    assert!(!report.burned_in_flag);
    assert!(!report.whole_image);
    assert_eq!(report.regions, vec![TEXT]);
    assert_masked(&pixels_u16(&obj), &original, &[TEXT], FILL);
    // the text is gone: no pixel holds the bright value any more
    assert!(!pixels_u16(&obj).contains(&30000));
    // after masking the flag says NO
    assert_eq!(obj.element(tags::BURNED_IN_ANNOTATION).unwrap().to_str().unwrap(), "NO");
}

#[test]
fn the_flag_without_regions_masks_the_whole_image() {
    // covers 31.3-a: the conservative default — the text's location is
    // unknown, so nothing of the image may survive.
    let mut obj = phantom(Some("YES"));
    let report = clean_pixel_data(&mut obj, &CleanPixelSpec::default()).unwrap();
    assert!(report.masked);
    assert!(report.burned_in_flag);
    assert!(report.whole_image);
    assert_eq!(report.regions, vec![PixelRegion::new(0, 0, COLS, ROWS)]);
    assert!(pixels_u16(&obj).iter().all(|p| *p == FILL));
    assert!(!burned_in_annotation(&obj));
}

#[test]
fn no_flag_and_no_regions_is_a_reported_no_op() {
    // covers 31.3-a: nothing to mask is reported, never silently passed.
    let mut obj = phantom(Some("NO"));
    let original = pixels_with_text();
    let report = clean_pixel_data(&mut obj, &CleanPixelSpec::default()).unwrap();
    assert!(!report.masked);
    assert!(!report.burned_in_flag);
    assert!(!report.whole_image);
    assert!(report.regions.is_empty());
    assert_eq!(pixels_u16(&obj), original);
    // the flag is untouched
    assert_eq!(obj.element(tags::BURNED_IN_ANNOTATION).unwrap().to_str().unwrap(), "NO");
}

#[test]
fn regions_mask_even_when_the_flag_is_no() {
    // covers 31.3-a: a configured region is masked regardless of the flag.
    let mut obj = phantom(Some("NO"));
    let original = pixels_with_text();
    let spec = CleanPixelSpec { regions: vec![TEXT], fill: 7 };
    let report = clean_pixel_data(&mut obj, &spec).unwrap();
    assert!(report.masked);
    assert_masked(&pixels_u16(&obj), &original, &[TEXT], 7);
}

#[test]
fn a_region_over_the_edge_is_clipped() {
    // covers 31.3-a
    let mut obj = phantom(None);
    let original = pixels_with_text();
    let over = PixelRegion::new(COLS - 4, ROWS - 2, 10, 10);
    let spec = CleanPixelSpec { regions: vec![over], fill: FILL };
    let report = clean_pixel_data(&mut obj, &spec).unwrap();
    let clipped = PixelRegion::new(COLS - 4, ROWS - 2, 4, 2);
    assert_eq!(report.regions, vec![clipped]);
    assert_masked(&pixels_u16(&obj), &original, &[clipped], FILL);
    // a region entirely outside masks nothing and is dropped
    let mut obj = phantom(None);
    let spec = CleanPixelSpec { regions: vec![PixelRegion::new(COLS, 0, 4, 4)], fill: FILL };
    let report = clean_pixel_data(&mut obj, &spec).unwrap();
    assert!(!report.masked);
    assert_eq!(pixels_u16(&obj), pixels_with_text());
}

#[test]
fn eight_bit_monochrome_is_masked() {
    // covers 31.3-a
    let pixels: Vec<u8> = pixels_with_text().iter().map(|p| (*p % 251) as u8).collect();
    let mut obj = object(PrimitiveValue::U8(pixels.clone().into()), 1, 8, None, 1, None);
    let spec = CleanPixelSpec { regions: vec![TEXT], fill: 0 };
    clean_pixel_data(&mut obj, &spec).unwrap();
    let after = pixels_u8(&obj);
    for p in 0..after.len() {
        let (col, row) = (p as u32 % COLS, p as u32 / COLS);
        let want = if inside(&[TEXT], col, row) { 0 } else { pixels[p] };
        assert_eq!(after[p], want, "pixel ({col}, {row})");
    }
}

#[test]
fn rgb_eight_bit_is_masked_in_every_channel() {
    // covers 31.3-a: both planar configurations, fill in all three channels.
    for planar in [0u16, 1] {
        let frame_len = (COLS * ROWS) as usize;
        let mut pixels = vec![0u8; frame_len * 3];
        for p in 0..frame_len {
            let v = (p % 200 + 1) as u8;
            if planar == 0 {
                pixels[p * 3..p * 3 + 3].copy_from_slice(&[v, v.wrapping_add(1), v.wrapping_add(2)]);
            } else {
                for c in 0..3 {
                    pixels[c * frame_len + p] = v.wrapping_add(c as u8);
                }
            }
        }
        let mut obj = object(PrimitiveValue::U8(pixels.clone().into()), 3, 8, Some("YES"), 1, Some(planar));
        let spec = CleanPixelSpec { regions: vec![TEXT], fill: 0 };
        let report = clean_pixel_data(&mut obj, &spec).unwrap();
        assert!(report.masked);
        let after = pixels_u8(&obj);
        for p in 0..frame_len {
            let (col, row) = (p as u32 % COLS, p as u32 / COLS);
            for c in 0..3 {
                let idx = if planar == 0 { p * 3 + c } else { c * frame_len + p };
                let want = if inside(&[TEXT], col, row) { 0 } else { pixels[idx] };
                assert_eq!(after[idx], want, "planar {planar}, pixel ({col}, {row}), channel {c}");
            }
        }
    }
}

#[test]
fn multi_frame_masks_every_frame() {
    // covers 31.3-a
    let frame = pixels_with_text();
    let pixels: Vec<u16> = frame.iter().copied().chain(frame.iter().copied()).collect();
    let mut obj = object(PrimitiveValue::U16(pixels.into()), 1, 16, Some("YES"), 2, None);
    let spec = CleanPixelSpec { regions: vec![TEXT], fill: FILL };
    clean_pixel_data(&mut obj, &spec).unwrap();
    let after = pixels_u16(&obj);
    let frame_len = (COLS * ROWS) as usize;
    for f in 0..2 {
        assert_masked(&after[f * frame_len..(f + 1) * frame_len], &frame, &[TEXT], FILL);
    }
}

#[test]
fn multi_frame_rgb_masks_every_frame_in_both_planar_modes() {
    // covers 31.3-a: the frame offset must not be double-counted in the
    // separate-plane (planar 1) layout.
    let frame_len = (COLS * ROWS) as usize;
    for planar in [0u16, 1] {
        let mut one = vec![0u8; frame_len * 3];
        for p in 0..frame_len {
            let v = (p % 200 + 1) as u8;
            if planar == 0 {
                one[p * 3..p * 3 + 3].copy_from_slice(&[v, v.wrapping_add(1), v.wrapping_add(2)]);
            } else {
                for c in 0..3 {
                    one[c * frame_len + p] = v.wrapping_add(c as u8);
                }
            }
        }
        let pixels: Vec<u8> = one.iter().copied().chain(one.iter().copied()).collect();
        let mut obj = object(PrimitiveValue::U8(pixels.clone().into()), 3, 8, Some("YES"), 2, Some(planar));
        clean_pixel_data(&mut obj, &CleanPixelSpec { regions: vec![TEXT], fill: 0 }).unwrap();
        let after = pixels_u8(&obj);
        for f in 0..2 {
            for p in 0..frame_len {
                let (col, row) = (p as u32 % COLS, p as u32 / COLS);
                for c in 0..3 {
                    let idx =
                        if planar == 0 { (f * frame_len + p) * 3 + c } else { f * frame_len * 3 + c * frame_len + p };
                    let want = if inside(&[TEXT], col, row) { 0 } else { pixels[idx] };
                    assert_eq!(after[idx], want, "planar {planar}, frame {f}, pixel ({col}, {row}), channel {c}");
                }
            }
        }
    }
}

#[test]
fn signed_sixteen_bit_fill_is_a_bit_pattern() {
    // covers 31.3-a
    let pixels: Vec<i16> = pixels_with_text().iter().map(|p| *p as i16 - 1000).collect();
    let mut obj = object(PrimitiveValue::I16(pixels.clone().into()), 1, 16, None, 1, None);
    let spec = CleanPixelSpec { regions: vec![TEXT], fill: 0 };
    clean_pixel_data(&mut obj, &spec).unwrap();
    let after = pixels_u16(&obj);
    let original: Vec<u16> = pixels.iter().map(|p| *p as u16).collect();
    assert_masked(&after, &original, &[TEXT], 0);
}

#[test]
fn encapsulated_pixel_data_is_refused_not_passed() {
    // covers 31.3-a: compressed pixels cannot be masked here — an error,
    // never a silent pass.
    let mut obj = phantom(Some("YES"));
    let fragments = dicom_core::value::fragments::Fragments::new(vec![1u8, 2, 3], 0);
    let seq: dicom_core::value::PixelFragmentSequence<_> = vec![fragments].into();
    obj.put(DataElement::new(tags::PIXEL_DATA, VR::OB, Value::PixelSequence(seq)));
    let err = clean_pixel_data(&mut obj, &CleanPixelSpec::default()).unwrap_err();
    assert!(matches!(err, CleanPixelError::Unsupported(_)), "{err}");
    // the object keeps its pixel data
    assert!(obj.element(tags::PIXEL_DATA).is_ok());
}

#[test]
fn an_unsupported_layout_is_refused() {
    // covers 31.3-a
    let mut obj = object(PrimitiveValue::U16(pixels_with_text().into()), 3, 16, Some("YES"), 1, None);
    let err = clean_pixel_data(&mut obj, &CleanPixelSpec::default()).unwrap_err();
    assert!(matches!(err, CleanPixelError::Unsupported(_)), "{err}");
}

#[test]
fn a_flagged_object_without_pixel_data_is_refused() {
    // covers 31.3-a
    let mut obj = phantom(Some("YES"));
    obj.remove_element(tags::PIXEL_DATA);
    let err = clean_pixel_data(&mut obj, &CleanPixelSpec::default()).unwrap_err();
    assert!(matches!(err, CleanPixelError::NoPixelData), "{err}");
}

#[test]
fn a_wrong_pixel_count_is_refused_and_the_object_stays() {
    // covers 31.3-a
    let short: Vec<u16> = pixels_with_text()[..100].to_vec();
    let mut obj = object(PrimitiveValue::U16(short.clone().into()), 1, 16, Some("YES"), 1, None);
    let err = clean_pixel_data(&mut obj, &CleanPixelSpec::default()).unwrap_err();
    assert!(matches!(err, CleanPixelError::Unsupported(_)), "{err}");
    assert_eq!(pixels_u16(&obj), short);
}

#[test]
fn the_flag_is_read_case_insensitively() {
    // covers 31.3-a
    assert!(burned_in_annotation(&phantom(Some("YES"))));
    assert!(burned_in_annotation(&phantom(Some("yes "))));
    assert!(!burned_in_annotation(&phantom(Some("NO"))));
    assert!(!burned_in_annotation(&phantom(None)));
}

#[test]
fn sources_burned_in_reads_the_flag_from_files() {
    // covers 31.3-a: the render-side flag check over a series' files.
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, flag: Option<&str>| {
        let path = dir.path().join(name);
        phantom(flag)
            .with_meta(dicom_object::FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
            .unwrap()
            .write_to_file(&path)
            .unwrap();
        path
    };
    let no = write("no.dcm", Some("NO"));
    let yes = write("yes.dcm", Some("YES"));
    let none = write("none.dcm", None);
    assert!(sources_burned_in(&[no.clone(), yes.clone()]));
    assert!(!sources_burned_in(&[no.clone(), none.clone()]));
    assert!(!sources_burned_in(&[]));
    // a non-DICOM file contributes false, not an error
    let txt = dir.path().join("not-dicom.txt");
    std::fs::write(&txt, "hello").unwrap();
    assert!(!sources_burned_in(&[txt]));
}

#[test]
fn a_masked_file_reads_back_through_ferrums_reader() {
    // covers 31.3-a: the masked data set is still a readable DICOM series,
    // and the region holds the fill's physical value.
    use ferrum_domain::{NoProgress, VolumeRepository};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("img.dcm");
    let mut obj = phantom(Some("YES"));
    clean_pixel_data(&mut obj, &CleanPixelSpec { regions: vec![TEXT], fill: FILL }).unwrap();
    obj.with_meta(dicom_object::FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
        .unwrap()
        .write_to_file(&path)
        .unwrap();
    let repo = ferrum_io::DicomRepository;
    let series = repo.scan(&[dir.path().to_path_buf()], &NoProgress).unwrap();
    let loaded = repo.load(&series[0], &NoProgress).unwrap();
    let v = &loaded.volume;
    assert_eq!(v.dims().x, COLS);
    assert_eq!(v.dims().y, ROWS);
    // axial identity orientation: voxel (i, j) is stored (col, row)
    for row in 0..ROWS {
        for col in 0..COLS {
            let value = v.physical(col, row, 0).unwrap();
            if inside(&[TEXT], col, row) {
                assert_eq!(value, 0.0, "pixel ({col}, {row})");
            } else {
                assert_eq!(value, (row * COLS + col + 1) as f32, "pixel ({col}, {row})");
            }
        }
    }
}
