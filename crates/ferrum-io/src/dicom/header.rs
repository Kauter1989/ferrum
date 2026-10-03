//! Per-file DICOM metadata needed to group and order slices.

use std::path::{Path, PathBuf};

use dicom_dictionary_std::tags;
use dicom_object::{mem::InMemDicomObject, OpenFileOptions, Tag};
use ferrum_domain::WindowLevel;
use glam::{DVec3, Vec2};

use crate::error::IoError;

/// Photometric interpretation relevant for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Photometric {
    /// Low values are dark.
    #[default]
    Monochrome2,
    /// Low values are bright (display must invert).
    Monochrome1,
    /// Colour image (converted to luminance).
    Color,
}

/// Header fields of one DICOM file, independent of pixel data.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceHeader {
    /// File containing the slice.
    pub path: PathBuf,
    /// Series Instance UID (empty if absent).
    pub series_uid: String,
    /// Series description.
    pub series_description: String,
    /// Modality.
    pub modality: String,
    /// Rows (image height).
    pub rows: u32,
    /// Columns (image width).
    pub columns: u32,
    /// Number of frames (1 for classic single-frame files).
    pub frames: u32,
    /// Pixel spacing `(column spacing, row spacing)` in mm.
    pub pixel_spacing: Option<Vec2>,
    /// Slice thickness in mm.
    pub slice_thickness: Option<f64>,
    /// Spacing between slices in mm.
    pub spacing_between_slices: Option<f64>,
    /// Image Position (Patient) of the first pixel.
    pub position: Option<DVec3>,
    /// Image Orientation (Patient): row and column direction cosines.
    pub orientation: Option<(DVec3, DVec3)>,
    /// Instance Number.
    pub instance_number: Option<i64>,
    /// Slice Location.
    pub slice_location: Option<f64>,
    /// First Window Center/Width pair, if present.
    pub window: Option<WindowLevel>,
    /// Photometric interpretation.
    pub photometric: Photometric,
}

pub(crate) fn str_of(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    obj.element_opt(tag)
        .ok()
        .flatten()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.trim().trim_end_matches('\0').to_string())
}

pub(crate) fn f64s_of(obj: &InMemDicomObject, tag: Tag) -> Option<Vec<f64>> {
    obj.element_opt(tag).ok().flatten().and_then(|e| e.to_multi_float64().ok())
}

pub(crate) fn f64_of(obj: &InMemDicomObject, tag: Tag) -> Option<f64> {
    f64s_of(obj, tag).and_then(|v| v.first().copied()).filter(|v| v.is_finite())
}

pub(crate) fn int_of(obj: &InMemDicomObject, tag: Tag) -> Option<i64> {
    obj.element_opt(tag).ok().flatten().and_then(|e| e.to_int::<i64>().ok())
}

impl SliceHeader {
    /// Extracts the header from an already parsed dataset.
    pub fn from_object(obj: &InMemDicomObject, path: &Path) -> Result<Self, IoError> {
        let rows = int_of(obj, tags::ROWS).ok_or_else(|| IoError::missing(path, "Rows"))?;
        let columns = int_of(obj, tags::COLUMNS).ok_or_else(|| IoError::missing(path, "Columns"))?;
        if rows <= 0 || columns <= 0 {
            return Err(IoError::invalid(path, "image has zero rows or columns"));
        }
        let pixel_spacing = f64s_of(obj, tags::PIXEL_SPACING)
            .or_else(|| f64s_of(obj, tags::IMAGER_PIXEL_SPACING))
            .filter(|v| v.len() >= 2 && v[0] > 0.0 && v[1] > 0.0)
            // DICOM order is (row spacing, column spacing)
            .map(|v| Vec2::new(v[1] as f32, v[0] as f32));
        let position =
            f64s_of(obj, tags::IMAGE_POSITION_PATIENT).filter(|v| v.len() >= 3).map(|v| DVec3::new(v[0], v[1], v[2]));
        let orientation = f64s_of(obj, tags::IMAGE_ORIENTATION_PATIENT)
            .filter(|v| v.len() >= 6)
            .map(|v| (DVec3::new(v[0], v[1], v[2]), DVec3::new(v[3], v[4], v[5])))
            .filter(|(r, c)| r.length() > 0.5 && c.length() > 0.5);
        let window = match (f64_of(obj, tags::WINDOW_CENTER), f64_of(obj, tags::WINDOW_WIDTH)) {
            (Some(c), Some(w)) if w > 0.0 => Some(WindowLevel::new(c as f32, w as f32)),
            _ => None,
        };
        let photometric = match str_of(obj, tags::PHOTOMETRIC_INTERPRETATION).as_deref() {
            Some("MONOCHROME1") => Photometric::Monochrome1,
            Some("MONOCHROME2") | None => Photometric::Monochrome2,
            Some(_) => Photometric::Color,
        };
        Ok(Self {
            path: path.to_path_buf(),
            series_uid: str_of(obj, tags::SERIES_INSTANCE_UID).unwrap_or_default(),
            series_description: str_of(obj, tags::SERIES_DESCRIPTION).unwrap_or_default(),
            modality: str_of(obj, tags::MODALITY).unwrap_or_default(),
            rows: rows as u32,
            columns: columns as u32,
            frames: int_of(obj, tags::NUMBER_OF_FRAMES).filter(|&n| n > 0).unwrap_or(1) as u32,
            pixel_spacing,
            slice_thickness: f64_of(obj, tags::SLICE_THICKNESS).filter(|v| *v > 0.0),
            spacing_between_slices: f64_of(obj, tags::SPACING_BETWEEN_SLICES).filter(|v| *v > 0.0),
            position,
            orientation,
            instance_number: int_of(obj, tags::INSTANCE_NUMBER),
            slice_location: f64_of(obj, tags::SLICE_LOCATION),
            window,
            photometric,
        })
    }

    /// Reads the header of `path`, stopping before the pixel data.
    pub fn read(path: &Path) -> Result<Self, IoError> {
        let obj =
            OpenFileOptions::new().read_until(tags::PIXEL_DATA).open_file(path).map_err(|e| IoError::parse(path, e))?;
        Self::from_object(&obj, path)
    }

    /// Unit normal of the image plane (`row × column`), if orientation known.
    pub fn normal(&self) -> Option<DVec3> {
        self.orientation.map(|(r, c)| r.cross(c).normalize_or_zero()).filter(|n| n.length() > 0.5)
    }
}

/// Study and series identification of a DICOM object.
pub fn study_info(obj: &InMemDicomObject) -> ferrum_domain::StudyInfo {
    let s = |tag| str_of(obj, tag).unwrap_or_default();
    ferrum_domain::StudyInfo {
        study_instance_uid: s(tags::STUDY_INSTANCE_UID),
        series_instance_uid: s(tags::SERIES_INSTANCE_UID),
        study_date: s(tags::STUDY_DATE),
        study_time: s(tags::STUDY_TIME),
        study_description: s(tags::STUDY_DESCRIPTION),
        series_description: s(tags::SERIES_DESCRIPTION),
        series_number: s(tags::SERIES_NUMBER),
        accession_number: s(tags::ACCESSION_NUMBER),
        modality: s(tags::MODALITY),
    }
}

/// Selected, human-readable attributes shown in the info dialog.
pub fn describe(obj: &InMemDicomObject) -> Vec<(String, String)> {
    const SHOWN: [(Tag, &str); 20] = [
        (tags::MODALITY, "Modality"),
        (tags::MANUFACTURER, "Manufacturer"),
        (tags::MANUFACTURER_MODEL_NAME, "Model"),
        (tags::INSTITUTION_NAME, "Institution"),
        (tags::STUDY_DATE, "Study date"),
        (tags::STUDY_DESCRIPTION, "Study description"),
        (tags::SERIES_DESCRIPTION, "Series description"),
        (tags::SERIES_NUMBER, "Series number"),
        (tags::BODY_PART_EXAMINED, "Body part"),
        (tags::PATIENT_POSITION, "Patient position"),
        (tags::ROWS, "Rows"),
        (tags::COLUMNS, "Columns"),
        (tags::PIXEL_SPACING, "Pixel spacing"),
        (tags::SLICE_THICKNESS, "Slice thickness"),
        (tags::BITS_STORED, "Bits stored"),
        (tags::PHOTOMETRIC_INTERPRETATION, "Photometric"),
        (tags::RESCALE_SLOPE, "Rescale slope"),
        (tags::RESCALE_INTERCEPT, "Rescale intercept"),
        (tags::WINDOW_CENTER, "Window center"),
        (tags::WINDOW_WIDTH, "Window width"),
    ];
    SHOWN
        .iter()
        .filter_map(|(tag, name)| str_of(obj, *tag).filter(|s| !s.is_empty()).map(|v| (name.to_string(), v)))
        .collect()
}
