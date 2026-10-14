//! Synthetic DICOM generator used by integration tests and benchmarks.
//! Files are created in temporary directories at test time; no image data
//! is committed to the repository.
#![allow(dead_code, missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};

#[derive(Debug, Clone)]
pub struct SyntheticSlice {
    pub rows: u16,
    pub cols: u16,
    pub position: [f64; 3],
    pub row_dir: [f64; 3],
    pub col_dir: [f64; 3],
    pub pixel_spacing: [f64; 2],
    pub instance: i32,
    pub series_uid: String,
    pub description: String,
    /// Stored pixel values (bit pattern for signed data).
    pub pixels: Vec<u16>,
    pub signed: bool,
    pub slope: f64,
    pub intercept: f64,
    pub transfer_syntax: &'static str,
    pub window: Option<(f64, f64)>,
    pub photometric: &'static str,
    pub frames: u32,
    pub include_position: bool,
    /// Echo Numbers, written when set.
    pub echo: Option<i32>,
}

impl SyntheticSlice {
    pub fn new(rows: u16, cols: u16, z: f64, instance: i32) -> Self {
        Self {
            rows,
            cols,
            position: [0.0, 0.0, z],
            row_dir: [1.0, 0.0, 0.0],
            col_dir: [0.0, 1.0, 0.0],
            pixel_spacing: [0.8, 0.6],
            instance,
            series_uid: "1.2.826.0.1.3680043.2.1125.1".into(),
            description: "SYNTH".into(),
            pixels: vec![0; rows as usize * cols as usize],
            signed: false,
            slope: 1.0,
            intercept: 0.0,
            transfer_syntax: uids::EXPLICIT_VR_LITTLE_ENDIAN,
            window: None,
            photometric: "MONOCHROME2",
            frames: 1,
            include_position: true,
            echo: None,
        }
    }

    pub fn write(&self, path: &Path) {
        let ds = |v: &[f64]| v.iter().map(|x| format!("{x}")).collect::<Vec<_>>().join("\\");
        let sop_uid = format!("{}.{}", self.series_uid, self.instance);
        let mut elements = vec![
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, PrimitiveValue::from(uids::CT_IMAGE_STORAGE)),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, PrimitiveValue::from(sop_uid.as_str())),
            DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, PrimitiveValue::from("1.2.826.0.1.3680043.2.1")),
            DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20240428")),
            DataElement::new(tags::STUDY_TIME, VR::TM, PrimitiveValue::from("102417")),
            DataElement::new(tags::STUDY_DESCRIPTION, VR::LO, PrimitiveValue::from("Chest CT")),
            DataElement::new(tags::SERIES_DESCRIPTION, VR::LO, PrimitiveValue::from(self.description.as_str())),
            DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, PrimitiveValue::from(self.series_uid.as_str())),
            DataElement::new(tags::INSTANCE_NUMBER, VR::IS, PrimitiveValue::from(self.instance.to_string())),
            DataElement::new(
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                PrimitiveValue::from(ds(&[self.row_dir, self.col_dir].concat())),
            ),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, PrimitiveValue::from(self.photometric)),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(self.rows)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(self.cols)),
            DataElement::new(tags::PIXEL_SPACING, VR::DS, PrimitiveValue::from(ds(&self.pixel_spacing))),
            DataElement::new(tags::SLICE_THICKNESS, VR::DS, PrimitiveValue::from("2.5")),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(u16::from(self.signed))),
            DataElement::new(tags::RESCALE_SLOPE, VR::DS, PrimitiveValue::from(ds(&[self.slope]))),
            DataElement::new(tags::RESCALE_INTERCEPT, VR::DS, PrimitiveValue::from(ds(&[self.intercept]))),
            DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::U16(self.pixels.clone().into())),
        ];
        if self.include_position {
            elements.push(DataElement::new(
                tags::IMAGE_POSITION_PATIENT,
                VR::DS,
                PrimitiveValue::from(ds(&self.position)),
            ));
        }
        if self.frames > 1 {
            elements.push(DataElement::new(
                tags::NUMBER_OF_FRAMES,
                VR::IS,
                PrimitiveValue::from(self.frames.to_string()),
            ));
            elements.push(DataElement::new(tags::SPACING_BETWEEN_SLICES, VR::DS, PrimitiveValue::from("1.25")));
        }
        if let Some(e) = self.echo {
            elements.push(DataElement::new(tags::ECHO_NUMBERS, VR::IS, PrimitiveValue::from(e.to_string())));
        }
        if let Some((c, w)) = self.window {
            elements.push(DataElement::new(tags::WINDOW_CENTER, VR::DS, PrimitiveValue::from(ds(&[c]))));
            elements.push(DataElement::new(tags::WINDOW_WIDTH, VR::DS, PrimitiveValue::from(ds(&[w]))));
        }
        let obj = InMemDicomObject::from_element_iter(elements);
        let file = obj.with_meta(FileMetaTableBuilder::new().transfer_syntax(self.transfer_syntax)).expect("meta");
        file.write_to_file(path).expect("write dicom");
    }
}

/// Value of the synthetic phantom at voxel (i, j, k): a gradient in x plus
/// the slice index, so ordering errors are detectable.
pub fn phantom_value(i: u32, j: u32, k: u32) -> u16 {
    (i * 10 + j + k * 1000) as u16
}

/// Writes an `n`-slice series into `dir` in shuffled file order.
pub fn write_series(dir: &Path, rows: u16, cols: u16, n: u32, spacing: f64, configure: impl Fn(&mut SyntheticSlice)) {
    for k in 0..n {
        let mut s = SyntheticSlice::new(rows, cols, 100.0 + k as f64 * spacing, (k + 1) as i32);
        s.pixels = (0..rows as u32 * cols as u32).map(|p| phantom_value(p % cols as u32, p / cols as u32, k)).collect();
        configure(&mut s);
        // reverse-ish file naming so directory order != spatial order
        let name = format!("img_{:03}.dcm", (k * 7) % n + n * ((k * 7) / n));
        s.write(&dir.join(name));
    }
}

/// One frame of a synthetic Enhanced multi-frame file.
#[derive(Debug, Clone)]
pub struct EnhancedFrame {
    pub position: [f64; 3],
    pub temporal: Option<i32>,
    pub intercept: f64,
    /// Stored pixel values.
    pub pixels: Vec<u16>,
}

fn sequence(tag: dicom_core::Tag, items: Vec<InMemDicomObject>) -> DataElement<InMemDicomObject> {
    DataElement::new(tag, VR::SQ, dicom_core::value::DataSetSequence::from(items))
}

fn item(elements: Vec<DataElement<InMemDicomObject>>) -> InMemDicomObject {
    InMemDicomObject::from_element_iter(elements)
}

/// Writes an Enhanced MR file: geometry only in the functional groups
/// (Plane Position per frame; Plane Orientation and, unless
/// `per_frame_measures`, Pixel Measures shared), per-frame rescale and
/// Temporal Position Index. Nothing about position, orientation or pixel
/// spacing at the top level.
pub fn write_enhanced(
    path: &Path,
    rows: u16,
    cols: u16,
    series_uid: &str,
    frames: &[EnhancedFrame],
    per_frame_measures: bool,
) {
    let ds = |v: &[f64]| v.iter().map(|x| format!("{x}")).collect::<Vec<_>>().join("\\");
    let text = |t, vr, v: &str| DataElement::new(t, vr, PrimitiveValue::from(v));
    let measures = || {
        sequence(
            tags::PIXEL_MEASURES_SEQUENCE,
            vec![item(vec![text(tags::PIXEL_SPACING, VR::DS, "0.7\\0.9"), text(tags::SLICE_THICKNESS, VR::DS, "2")])],
        )
    };
    let mut shared = vec![sequence(
        tags::PLANE_ORIENTATION_SEQUENCE,
        vec![item(vec![text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0")])],
    )];
    if !per_frame_measures {
        shared.push(measures());
    }
    let per_frame: Vec<InMemDicomObject> = frames
        .iter()
        .map(|f| {
            let mut groups = vec![
                sequence(
                    tags::PLANE_POSITION_SEQUENCE,
                    vec![item(vec![text(tags::IMAGE_POSITION_PATIENT, VR::DS, &ds(&f.position))])],
                ),
                sequence(
                    tags::PIXEL_VALUE_TRANSFORMATION_SEQUENCE,
                    vec![item(vec![
                        text(tags::RESCALE_INTERCEPT, VR::DS, &ds(&[f.intercept])),
                        text(tags::RESCALE_SLOPE, VR::DS, "1"),
                        text(tags::RESCALE_TYPE, VR::LO, "US"),
                    ])],
                ),
            ];
            if let Some(t) = f.temporal {
                groups.push(sequence(
                    tags::FRAME_CONTENT_SEQUENCE,
                    vec![item(vec![DataElement::new(
                        tags::TEMPORAL_POSITION_INDEX,
                        VR::UL,
                        PrimitiveValue::from(t as u32),
                    )])],
                ));
            }
            if per_frame_measures {
                groups.push(measures());
            }
            item(groups)
        })
        .collect();
    let pixels: Vec<u16> = frames.iter().flat_map(|f| f.pixels.iter().copied()).collect();
    let elements = vec![
        text(tags::SOP_CLASS_UID, VR::UI, uids::ENHANCED_MR_IMAGE_STORAGE),
        text(tags::SOP_INSTANCE_UID, VR::UI, &format!("{series_uid}.{}", path.display().to_string().len())),
        text(tags::MODALITY, VR::CS, "MR"),
        text(tags::STUDY_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.2.1"),
        text(tags::SERIES_INSTANCE_UID, VR::UI, series_uid),
        text(tags::SERIES_DESCRIPTION, VR::LO, "ENHANCED"),
        text(tags::INSTANCE_NUMBER, VR::IS, "1"),
        text(tags::NUMBER_OF_FRAMES, VR::IS, &frames.len().to_string()),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1u16)),
        text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(rows)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(cols)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15u16)),
        DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(0u16)),
        sequence(tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE, vec![item(shared)]),
        sequence(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE, per_frame),
        DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::U16(pixels.into())),
    ];
    let obj = InMemDicomObject::from_element_iter(elements);
    let file =
        obj.with_meta(FileMetaTableBuilder::new().transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)).expect("meta");
    file.write_to_file(path).expect("write enhanced dicom");
}
