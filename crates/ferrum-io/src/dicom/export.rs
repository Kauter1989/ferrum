//! DICOM export of results: a binary Segmentation (SEG) of the segments and
//! a Comprehensive 3D Structured Report (TID 1500) of the measurements and
//! segment volumes.
//!
//! Both objects lie in the study of the source series and on its frame of
//! reference, so a PACS or viewer shows them with the images. What leaves
//! FERRUM is decided by [`DicomExportOptions`]: patient identifiers, dates
//! and original UIDs are copied only when the operator allows it.
//!
//! Unconfirmed items (status `proposed`) are exported and marked as such;
//! rejected items are left out. The format is described in
//! `docs/workspace-format.md` §5.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use dicom_core::value::DataSetSequence;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject, OpenFileOptions};
use ferrum_domain::{
    AnnotationRecord, AnnotationReport, Author, Provenance, ReviewStatus, SegmentationSet, SliceAxis, Timestamp, Volume,
};
use glam::{DVec3, Vec3};
use sha2::{Digest, Sha256};

use super::header::{f64s_of, str_of};
use crate::error::IoError;

type Elem = DataElement<InMemDicomObject>;

/// Coding scheme of FERRUM's private codes (review status, author).
pub const PRIVATE_SCHEME: &str = "99FERRUM";

/// What may be copied from the source into exported DICOM objects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DicomExportOptions {
    /// Copy patient name, ID and sex (and the accession number). Otherwise
    /// they are left empty and the objects are marked as de-identified.
    pub copy_identifiers: bool,
    /// Copy study date and time (and, with identifiers, the birth date).
    pub copy_dates: bool,
    /// Keep the source UIDs (study, series, instances, frame of
    /// reference) and reference the source images. Otherwise UIDs are
    /// replaced by salted hashes and source images are not referenced.
    pub keep_uids: bool,
    /// Salt of the replacement UIDs.
    pub uid_salt: String,
    /// Software version written into the equipment module.
    pub software_version: String,
    /// Identifier of the source series; seeds UIDs when the source is not
    /// DICOM.
    pub series_id: String,
}

/// One image of the source series.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceInstance {
    /// SOP Class UID.
    pub sop_class_uid: String,
    /// SOP Instance UID.
    pub sop_instance_uid: String,
    /// Image Position (Patient), if present.
    pub position: Option<DVec3>,
}

/// What the export takes from the source DICOM series.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DicomSource {
    /// Patient's Name.
    pub patient_name: String,
    /// Patient ID.
    pub patient_id: String,
    /// Patient's Birth Date.
    pub patient_birth_date: String,
    /// Patient's Sex.
    pub patient_sex: String,
    /// Study Instance UID.
    pub study_uid: String,
    /// Series Instance UID.
    pub series_uid: String,
    /// Frame of Reference UID (empty if absent).
    pub frame_of_reference_uid: String,
    /// Study ID.
    pub study_id: String,
    /// Study Date.
    pub study_date: String,
    /// Study Time.
    pub study_time: String,
    /// Study Description.
    pub study_description: String,
    /// Accession Number.
    pub accession_number: String,
    /// Images of the series, in file order.
    pub instances: Vec<SourceInstance>,
}

/// Reads the study, patient and image references of a DICOM series from
/// `files`. Files that are not DICOM are skipped; `None` when none is.
pub fn read_source(files: &[PathBuf]) -> Option<DicomSource> {
    let mut src: Option<DicomSource> = None;
    for f in files {
        let Ok(obj) = OpenFileOptions::new().read_until(tags::PIXEL_DATA).open_file(f) else {
            continue;
        };
        let s = |tag| str_of(&obj, tag).unwrap_or_default();
        let src = src.get_or_insert_with(|| DicomSource {
            patient_name: s(tags::PATIENT_NAME),
            patient_id: s(tags::PATIENT_ID),
            patient_birth_date: s(tags::PATIENT_BIRTH_DATE),
            patient_sex: s(tags::PATIENT_SEX),
            study_uid: s(tags::STUDY_INSTANCE_UID),
            series_uid: s(tags::SERIES_INSTANCE_UID),
            frame_of_reference_uid: s(tags::FRAME_OF_REFERENCE_UID),
            study_id: s(tags::STUDY_ID),
            study_date: s(tags::STUDY_DATE),
            study_time: s(tags::STUDY_TIME),
            study_description: s(tags::STUDY_DESCRIPTION),
            accession_number: s(tags::ACCESSION_NUMBER),
            instances: Vec::new(),
        });
        let (class, instance) = (s(tags::SOP_CLASS_UID), s(tags::SOP_INSTANCE_UID));
        if class.is_empty() || instance.is_empty() {
            continue;
        }
        let position =
            f64s_of(&obj, tags::IMAGE_POSITION_PATIENT).filter(|v| v.len() >= 3).map(|v| DVec3::new(v[0], v[1], v[2]));
        src.instances.push(SourceInstance { sop_class_uid: class, sop_instance_uid: instance, position });
    }
    src
}

/// A UID under the `2.25` root (ISO/IEC 9834-8) derived from `parts`.
pub fn uid_from(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    let digest = h.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    format!("2.25.{}", u128::from_be_bytes(bytes))
}

/// A new, unique UID.
fn new_uid(opts: &DicomExportOptions, kind: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!("{nanos}:{n}:{}", std::process::id());
    uid_from(&["ferrum-new", &opts.series_id, kind, &seed])
}

/// Study-level identity shared by every object of one export.
#[derive(Debug, Clone)]
struct Context {
    opts: DicomExportOptions,
    source: Option<DicomSource>,
    study_uid: String,
    frame_of_reference_uid: String,
    /// Source series UID as written (pseudonymised unless kept).
    source_series_uid: String,
    date: String,
    time: String,
}

impl Context {
    fn new(source: Option<&DicomSource>, opts: &DicomExportOptions) -> Self {
        let salt = opts.uid_salt.as_str();
        let map = |uid: &str, kind: &str| {
            if uid.is_empty() {
                uid_from(&["ferrum", kind, salt, &opts.series_id])
            } else if opts.keep_uids {
                uid.to_owned()
            } else {
                uid_from(&["ferrum-uid", salt, uid])
            }
        };
        let (study, series, frame) = match source {
            Some(s) => {
                let frame = if s.frame_of_reference_uid.is_empty() {
                    // No frame of reference in the source: one per series.
                    uid_from(&["ferrum-for", salt, &s.series_uid, &opts.series_id])
                } else {
                    map(&s.frame_of_reference_uid, "for")
                };
                (map(&s.study_uid, "study"), map(&s.series_uid, "series"), frame)
            }
            None => (map("", "study"), map("", "series"), map("", "for")),
        };
        let now = Timestamp::now().to_rfc3339();
        let digits = |r: std::ops::Range<usize>| now[r].replace(['-', ':'], "");
        Self {
            opts: opts.clone(),
            source: source.cloned(),
            study_uid: study,
            frame_of_reference_uid: frame,
            source_series_uid: series,
            date: digits(0..10),
            time: digits(11..19),
        }
    }

    /// Source images may be referenced.
    fn references(&self) -> bool {
        self.opts.keep_uids && self.source.as_ref().is_some_and(|s| !s.instances.is_empty())
    }

    /// Patient, study, equipment and SOP common attributes.
    fn common(&self, sop_class: &str, sop_instance: &str, series_uid: &str, modality: &str) -> Vec<Elem> {
        let o = &self.opts;
        let src = self.source.clone().unwrap_or_default();
        let keep = |allowed: bool, v: &str| if allowed { v.to_owned() } else { String::new() };
        let mut e = vec![
            text(tags::SPECIFIC_CHARACTER_SET, VR::CS, "ISO_IR 192"),
            text(tags::SOP_CLASS_UID, VR::UI, sop_class),
            text(tags::SOP_INSTANCE_UID, VR::UI, sop_instance),
            text(tags::PATIENT_NAME, VR::PN, &keep(o.copy_identifiers, &src.patient_name)),
            text(tags::PATIENT_ID, VR::LO, &keep(o.copy_identifiers, &src.patient_id)),
            text(tags::PATIENT_BIRTH_DATE, VR::DA, &keep(o.copy_identifiers && o.copy_dates, &src.patient_birth_date)),
            text(tags::PATIENT_SEX, VR::CS, &keep(o.copy_identifiers, &src.patient_sex)),
            text(tags::STUDY_INSTANCE_UID, VR::UI, &self.study_uid),
            text(tags::STUDY_DATE, VR::DA, &keep(o.copy_dates, &src.study_date)),
            text(tags::STUDY_TIME, VR::TM, &keep(o.copy_dates, &src.study_time)),
            text(tags::REFERRING_PHYSICIAN_NAME, VR::PN, ""),
            text(tags::STUDY_ID, VR::SH, &keep(o.copy_identifiers, &src.study_id)),
            text(tags::ACCESSION_NUMBER, VR::SH, &keep(o.copy_identifiers, &src.accession_number)),
            text(tags::STUDY_DESCRIPTION, VR::LO, &src.study_description),
            text(tags::MODALITY, VR::CS, modality),
            text(tags::SERIES_INSTANCE_UID, VR::UI, series_uid),
            text(tags::SERIES_NUMBER, VR::IS, if modality == "SEG" { "9001" } else { "9002" }),
            text(tags::INSTANCE_NUMBER, VR::IS, "1"),
            text(tags::CONTENT_DATE, VR::DA, &self.date),
            text(tags::CONTENT_TIME, VR::TM, &self.time),
            text(tags::INSTANCE_CREATION_DATE, VR::DA, &self.date),
            text(tags::INSTANCE_CREATION_TIME, VR::TM, &self.time),
            text(tags::MANUFACTURER, VR::LO, "FERRUM project"),
            text(tags::MANUFACTURER_MODEL_NAME, VR::LO, "FERRUM"),
            text(tags::DEVICE_SERIAL_NUMBER, VR::LO, "0"),
            text(tags::SOFTWARE_VERSIONS, VR::LO, &o.software_version),
        ];
        if !o.copy_identifiers {
            e.push(text(tags::PATIENT_IDENTITY_REMOVED, VR::CS, "YES"));
            e.push(text(tags::DEIDENTIFICATION_METHOD, VR::LO, "FERRUM export: identifiers not copied"));
        }
        e
    }
}

fn text(tag: Tag, vr: VR, value: &str) -> Elem {
    DataElement::new(tag, vr, PrimitiveValue::from(value))
}

fn u16s(tag: Tag, values: &[u16]) -> Elem {
    DataElement::new(tag, VR::US, PrimitiveValue::U16(values.iter().copied().collect()))
}

fn seq(tag: Tag, items: Vec<InMemDicomObject>) -> Elem {
    DataElement::new(tag, VR::SQ, DataSetSequence::from(items))
}

fn item(elements: Vec<Elem>) -> InMemDicomObject {
    InMemDicomObject::from_element_iter(elements)
}

/// A code sequence item.
fn code(value: &str, scheme: &str, meaning: &str) -> InMemDicomObject {
    item(vec![
        text(tags::CODE_VALUE, VR::SH, value),
        text(tags::CODING_SCHEME_DESIGNATOR, VR::SH, scheme),
        text(tags::CODE_MEANING, VR::LO, meaning),
    ])
}

/// Decimal string of at most 16 characters.
fn ds(v: f64) -> String {
    for decimals in (0..=6).rev() {
        let s = format!("{v:.decimals$}");
        let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s };
        let s = if s == "-0" { "0".to_owned() } else { s };
        if s.len() <= 16 {
            return s;
        }
    }
    format!("{v:.3e}")
}

fn ds_list(values: &[f64]) -> String {
    values.iter().map(|v| ds(*v)).collect::<Vec<_>>().join("\\")
}

fn write_file(path: &Path, elements: Vec<Elem>) -> Result<(), IoError> {
    let file = item(elements)
        .with_meta(FileMetaTableBuilder::new().transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN))
        .map_err(|e| IoError::invalid(path, e.to_string()))?;
    file.write_to_file(path).map_err(|e| IoError::invalid(path, e.to_string()))
}

/// DICOM `RecommendedDisplayCIELabValue` of an sRGB colour (D65).
pub fn cielab(rgb: [u8; 3]) -> [u16; 3] {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.950_47;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.088_83;
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    let (l, a, bb) = (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz));
    let scale = |v: f64, lo: f64, span: f64| ((v - lo) / span * 65535.0).round().clamp(0.0, 65535.0) as u16;
    [scale(l, 0.0, 100.0), scale(a, -128.0, 255.0), scale(bb, -128.0, 255.0)]
}

/// Review state and author in words, e.g. `proposed by engine X (unconfirmed)`.
fn provenance_text(p: &Provenance) -> String {
    let who = p.author.describe();
    match p.status {
        ReviewStatus::Proposed => format!("proposed by {who}; unconfirmed"),
        ReviewStatus::Confirmed => match &p.reviewed_by {
            Some(by) if p.author != Author::Human => format!("by {who}; confirmed by {by}"),
            _ => format!("by {who}; confirmed"),
        },
        ReviewStatus::Rejected => format!("by {who}; rejected"),
    }
}

/// What [`write_seg`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegExport {
    /// SOP Instance UID of the segmentation.
    pub sop_instance_uid: String,
    /// Series Instance UID of the segmentation.
    pub series_instance_uid: String,
    /// Study Instance UID (shared with the report).
    pub study_instance_uid: String,
    /// Number of frames.
    pub frames: usize,
    /// `(label, segment number)` of every exported segment.
    pub segments: Vec<(u8, u16)>,
    /// Labels left out, with the reason.
    pub skipped: Vec<(u8, String)>,
}

/// Writes the results of one series as DICOM: `segmentation.dcm` (when
/// `segments` has exportable segments) and `measurements.dcm` (when there
/// are measurements or segments) in `dir`.
#[derive(Debug)]
pub struct DicomExporter {
    ctx: Context,
}

/// What [`DicomExporter`] wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DicomExport {
    /// The segmentation, if written.
    pub seg: Option<SegExport>,
    /// Number of measurements in the report, if written.
    pub report_measurements: Option<usize>,
    /// Items left out, with the reason.
    pub skipped: Vec<String>,
}

impl DicomExporter {
    /// An exporter for a series read from `source` (`None` when the source
    /// is not DICOM).
    pub fn new(source: Option<&DicomSource>, opts: &DicomExportOptions) -> Self {
        Self { ctx: Context::new(source, opts) }
    }

    /// Study Instance UID of the exported objects.
    pub fn study_uid(&self) -> &str {
        &self.ctx.study_uid
    }

    /// Writes `seg_path` (if there are segments) and `sr_path` (if there is
    /// anything to report).
    pub fn export(
        &self,
        volume: &Volume,
        segments: &SegmentationSet,
        report: &AnnotationReport,
        seg_path: &Path,
        sr_path: &Path,
    ) -> Result<DicomExport, IoError> {
        let mut out = DicomExport::default();
        let seg = self.write_seg(seg_path, volume, segments)?;
        if let Some(seg) = &seg {
            out.skipped.extend(seg.skipped.iter().map(|(l, why)| format!("segment {l}: {why}")));
        }
        let (n, skipped) = self.write_sr(sr_path, volume, report, segments, seg.as_ref())?;
        out.report_measurements = n;
        out.skipped.extend(skipped);
        out.seg = seg;
        Ok(out)
    }

    /// Writes a binary Segmentation of the non-rejected, non-empty
    /// segments of `set` to `path`; `None` (nothing written) when there are
    /// none.
    pub fn write_seg(&self, path: &Path, volume: &Volume, set: &SegmentationSet) -> Result<Option<SegExport>, IoError> {
        let ctx = &self.ctx;
        let dims = set.dims();
        if dims != volume.dims() {
            return Err(IoError::invalid(path, "segmentation and volume differ in size"));
        }
        let (nx, ny, nz) = (dims.x as usize, dims.y as usize, dims.z as usize);
        let data = set.labels().data();
        let present = slices_with_labels(data, nx * ny, nz);
        let mut skipped = Vec::new();
        let mut exported = Vec::new();
        for s in set.segments() {
            if s.provenance.status == ReviewStatus::Rejected {
                skipped.push((s.label, "rejected".to_owned()));
            } else if !present.contains_key(&s.label) {
                skipped.push((s.label, "empty".to_owned()));
            } else {
                exported.push(s);
            }
        }
        if exported.is_empty() {
            return Ok(None);
        }
        let spacing = volume.spacing();
        let normal = volume.geometry().direction.z_axis.as_dvec3();
        let position = |k: usize| volume.voxel_to_patient(Vec3::new(0.0, 0.0, k as f32)).as_dvec3();
        // Source image of each slice (same position along the normal).
        let source_of = |k: usize| -> Option<&SourceInstance> {
            let src = ctx.source.as_ref().filter(|_| ctx.references())?;
            let p = position(k);
            src.instances
                .iter()
                .filter_map(|inst| inst.position.map(|q| ((q - p).dot(normal).abs(), inst)))
                .filter(|(d, _)| *d < f64::from(spacing.z) * 0.5)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, inst)| inst)
        };
        let used: BTreeSet<usize> = exported.iter().flat_map(|s| present[&s.label].iter().copied()).collect();
        let rank: BTreeMap<usize, usize> = used.iter().enumerate().map(|(r, k)| (*k, r + 1)).collect();

        let frame_bits = nx * ny;
        let total: usize = exported.iter().map(|s| present[&s.label].len()).sum();
        let mut pixels = vec![0u8; (total * frame_bits).div_ceil(8).next_multiple_of(2)];
        let mut per_frame = Vec::with_capacity(total);
        let mut referenced: BTreeMap<String, SourceInstance> = BTreeMap::new();
        let mut segment_items = Vec::new();
        let mut numbers = Vec::new();
        let mut frame = 0usize;
        for (n, s) in exported.iter().enumerate() {
            let number = u16::try_from(n + 1).unwrap_or(u16::MAX);
            numbers.push((s.label, number));
            segment_items.push(segment_item(s, number));
            for &k in &present[&s.label] {
                let slice = &data[k * frame_bits..(k + 1) * frame_bits];
                let base = frame * frame_bits;
                for (p, _) in slice.iter().enumerate().filter(|(_, v)| **v == s.label) {
                    let bit = base + p;
                    pixels[bit / 8] |= 1 << (bit % 8);
                }
                let mut groups = frame_groups(number, rank[&k], position(k));
                if let Some(inst) = source_of(k) {
                    referenced.insert(inst.sop_instance_uid.clone(), inst.clone());
                    groups.push(derivation(inst));
                }
                per_frame.push(item(groups));
                frame += 1;
            }
        }

        let sop_instance = new_uid(&ctx.opts, "seg-instance");
        let series = new_uid(&ctx.opts, "seg-series");
        let organisation = new_uid(&ctx.opts, "seg-dimensions");
        let mut e = ctx.common(uids::SEGMENTATION_STORAGE, &sop_instance, &series, "SEG");
        e.extend([
            text(tags::SERIES_DESCRIPTION, VR::LO, "FERRUM segmentation"),
            text(tags::FRAME_OF_REFERENCE_UID, VR::UI, &ctx.frame_of_reference_uid),
            text(tags::POSITION_REFERENCE_INDICATOR, VR::LO, ""),
            text(tags::IMAGE_TYPE, VR::CS, "DERIVED\\PRIMARY"),
            text(tags::CONTENT_LABEL, VR::CS, "FERRUM_SEGMENTS"),
            text(tags::CONTENT_DESCRIPTION, VR::LO, "Segments from FERRUM; see each segment's review status"),
            text(tags::CONTENT_CREATOR_NAME, VR::PN, ""),
            text(tags::SEGMENTATION_TYPE, VR::CS, "BINARY"),
            text(tags::LOSSY_IMAGE_COMPRESSION, VR::CS, "00"),
            u16s(tags::SAMPLES_PER_PIXEL, &[1]),
            text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
            u16s(tags::ROWS, &[dims.y as u16]),
            u16s(tags::COLUMNS, &[dims.x as u16]),
            u16s(tags::BITS_ALLOCATED, &[1]),
            u16s(tags::BITS_STORED, &[1]),
            u16s(tags::HIGH_BIT, &[0]),
            u16s(tags::PIXEL_REPRESENTATION, &[0]),
            text(tags::NUMBER_OF_FRAMES, VR::IS, &total.to_string()),
            seq(tags::SEGMENT_SEQUENCE, segment_items),
            seq(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE, per_frame),
            DataElement::new(tags::PIXEL_DATA, VR::OB, PrimitiveValue::from(pixels)),
        ]);
        e.extend(multiframe_layout(volume, &organisation));
        if !referenced.is_empty() {
            e.push(referenced_series(ctx, referenced.values()));
        }
        write_file(path, e)?;
        Ok(Some(SegExport {
            sop_instance_uid: sop_instance,
            series_instance_uid: series,
            study_instance_uid: ctx.study_uid.clone(),
            frames: total,
            segments: numbers,
            skipped,
        }))
    }

    /// Writes the measurement report; returns the number of measurements
    /// (`None` when nothing was written) and the items left out.
    fn write_sr(
        &self,
        path: &Path,
        volume: &Volume,
        report: &AnnotationReport,
        set: &SegmentationSet,
        seg: Option<&SegExport>,
    ) -> Result<(Option<usize>, Vec<String>), IoError> {
        let ctx = &self.ctx;
        let mut skipped = Vec::new();
        let mut groups = Vec::new();
        let mut unconfirmed = false;
        for a in &report.annotations {
            if a.provenance.status == ReviewStatus::Rejected {
                skipped.push(format!("annotation {} ({}): rejected", a.id, a.name));
                continue;
            }
            match measurement_group(ctx, volume, a) {
                Some(g) => {
                    unconfirmed |= a.provenance.is_pending();
                    groups.push(g);
                }
                None => skipped.push(format!("annotation {} ({}): {} is not exported to DICOM", a.id, a.name, a.kind)),
            }
        }
        let measurements = groups.len();
        if let Some(seg) = seg {
            for &(label, number) in &seg.segments {
                let Some(s) = set.segment(label) else { continue };
                unconfirmed |= s.provenance.is_pending();
                let ml = set.volume_ml(label, volume.spacing());
                groups.push(volume_group(ctx, seg, number, &s.name, ml, &s.provenance));
            }
        }
        if groups.is_empty() {
            return Ok((None, skipped));
        }
        let sop_instance = new_uid(&ctx.opts, "sr-instance");
        let series = new_uid(&ctx.opts, "sr-series");
        let content = vec![
            content_code(
                "HAS CONCEPT MOD",
                ("121049", "DCM", "Language of Content Item and Descendants"),
                ("en-US", "RFC5646", "English (United States)"),
            ),
            content_code("HAS OBS CONTEXT", ("121005", "DCM", "Observer Type"), ("121007", "DCM", "Device")),
            content_item(
                "HAS OBS CONTEXT",
                "UIDREF",
                ("121012", "DCM", "Device Observer UID"),
                vec![text(tags::UID, VR::UI, &uid_from(&["ferrum-device"]))],
            ),
            content_item(
                "HAS OBS CONTEXT",
                "TEXT",
                ("121013", "DCM", "Device Observer Name"),
                vec![text(tags::TEXT_VALUE, VR::UT, "FERRUM")],
            ),
            content_code(
                "HAS CONCEPT MOD",
                ("121058", "DCM", "Procedure reported"),
                ("363679005", "SCT", "Imaging procedure"),
            ),
            container("CONTAINS", ("111028", "DCM", "Image Library"), None, Vec::new()),
            container("CONTAINS", ("126010", "DCM", "Imaging Measurements"), None, groups),
        ];
        let mut e = ctx.common(uids::COMPREHENSIVE3_DSR_STORAGE, &sop_instance, &series, "SR");
        e.extend([
            text(tags::SERIES_DESCRIPTION, VR::LO, "FERRUM measurements"),
            seq(tags::REFERENCED_PERFORMED_PROCEDURE_STEP_SEQUENCE, Vec::new()),
            text(tags::COMPLETION_FLAG, VR::CS, if unconfirmed { "PARTIAL" } else { "COMPLETE" }),
            text(tags::VERIFICATION_FLAG, VR::CS, "UNVERIFIED"),
            seq(tags::PERFORMED_PROCEDURE_CODE_SEQUENCE, Vec::new()),
            seq(
                tags::CODING_SCHEME_IDENTIFICATION_SEQUENCE,
                vec![item(vec![
                    text(tags::CODING_SCHEME_DESIGNATOR, VR::SH, PRIVATE_SCHEME),
                    text(tags::CODING_SCHEME_NAME, VR::ST, "FERRUM private codes"),
                    text(tags::CODING_SCHEME_RESPONSIBLE_ORGANIZATION, VR::ST, "FERRUM project"),
                ])],
            ),
            text(tags::VALUE_TYPE, VR::CS, "CONTAINER"),
            seq(tags::CONCEPT_NAME_CODE_SEQUENCE, vec![code("126000", "DCM", "Imaging Measurement Report")]),
            text(tags::CONTINUITY_OF_CONTENT, VR::CS, "SEPARATE"),
            seq(
                tags::CONTENT_TEMPLATE_SEQUENCE,
                vec![item(vec![
                    text(tags::MAPPING_RESOURCE, VR::CS, "DCMR"),
                    text(tags::TEMPLATE_IDENTIFIER, VR::CS, "1500"),
                ])],
            ),
            seq(tags::CONTENT_SEQUENCE, content),
        ]);
        if let Some(seg) = seg {
            let seg_ref = item(vec![
                text(tags::REFERENCED_SOP_CLASS_UID, VR::UI, uids::SEGMENTATION_STORAGE),
                text(tags::REFERENCED_SOP_INSTANCE_UID, VR::UI, &seg.sop_instance_uid),
            ]);
            let series_item = item(vec![
                text(tags::SERIES_INSTANCE_UID, VR::UI, &seg.series_instance_uid),
                seq(tags::REFERENCED_SOP_SEQUENCE, vec![seg_ref]),
            ]);
            let study_item = item(vec![
                text(tags::STUDY_INSTANCE_UID, VR::UI, &ctx.study_uid),
                seq(tags::REFERENCED_SERIES_SEQUENCE, vec![series_item]),
            ]);
            e.push(seq(tags::CURRENT_REQUESTED_PROCEDURE_EVIDENCE_SEQUENCE, vec![study_item]));
        }
        write_file(path, e)?;
        Ok((Some(measurements), skipped))
    }
}

/// Slices (`k`) holding each label of a label map with `slice_len` voxels
/// per slice.
fn slices_with_labels(data: &[u8], slice_len: usize, nz: usize) -> BTreeMap<u8, BTreeSet<usize>> {
    let mut present: BTreeMap<u8, BTreeSet<usize>> = BTreeMap::new();
    for (k, slice) in data.chunks(slice_len).enumerate().take(nz) {
        let mut seen = [false; 256];
        for &v in slice {
            seen[usize::from(v)] = true;
        }
        for (label, _) in seen.iter().enumerate().skip(1).filter(|(_, s)| **s) {
            present.entry(label as u8).or_default().insert(k);
        }
    }
    present
}

/// Per-frame functional groups of one frame: dimension indices, position
/// and segment.
fn frame_groups(number: u16, position_index: usize, position: DVec3) -> Vec<Elem> {
    let indices = [u32::from(number), u32::try_from(position_index).unwrap_or(u32::MAX)];
    vec![
        seq(
            tags::FRAME_CONTENT_SEQUENCE,
            vec![item(vec![DataElement::new(
                tags::DIMENSION_INDEX_VALUES,
                VR::UL,
                PrimitiveValue::U32(indices.into_iter().collect()),
            )])],
        ),
        seq(
            tags::PLANE_POSITION_SEQUENCE,
            vec![item(vec![text(tags::IMAGE_POSITION_PATIENT, VR::DS, &ds_list(&position.to_array()))])],
        ),
        seq(tags::SEGMENT_IDENTIFICATION_SEQUENCE, vec![item(vec![u16s(tags::REFERENCED_SEGMENT_NUMBER, &[number])])]),
    ]
}

/// Shared functional groups (spacing, orientation) and the dimensions
/// (segment, then position) of a segmentation on the volume grid.
fn multiframe_layout(volume: &Volume, organisation: &str) -> Vec<Elem> {
    let spacing = volume.spacing();
    let geometry = volume.geometry();
    let (dir_i, dir_j) = (geometry.direction.x_axis.as_dvec3(), geometry.direction.y_axis.as_dvec3());
    let dimension = |index: Tag, group: Tag, label: &str| {
        item(vec![
            DataElement::new(
                tags::DIMENSION_INDEX_POINTER,
                VR::AT,
                PrimitiveValue::Tags([index].into_iter().collect()),
            ),
            DataElement::new(
                tags::FUNCTIONAL_GROUP_POINTER,
                VR::AT,
                PrimitiveValue::Tags([group].into_iter().collect()),
            ),
            text(tags::DIMENSION_ORGANIZATION_UID, VR::UI, organisation),
            text(tags::DIMENSION_DESCRIPTION_LABEL, VR::LO, label),
        ])
    };
    let shared = item(vec![
        seq(
            tags::PIXEL_MEASURES_SEQUENCE,
            vec![item(vec![
                // Row spacing (along j) first.
                text(tags::PIXEL_SPACING, VR::DS, &ds_list(&[f64::from(spacing.y), f64::from(spacing.x)])),
                text(tags::SLICE_THICKNESS, VR::DS, &ds(f64::from(spacing.z))),
                text(tags::SPACING_BETWEEN_SLICES, VR::DS, &ds(f64::from(spacing.z))),
            ])],
        ),
        seq(
            tags::PLANE_ORIENTATION_SEQUENCE,
            vec![item(vec![text(
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                &ds_list(&[dir_i.x, dir_i.y, dir_i.z, dir_j.x, dir_j.y, dir_j.z]),
            )])],
        ),
    ]);
    vec![
        seq(tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE, vec![shared]),
        seq(
            tags::DIMENSION_ORGANIZATION_SEQUENCE,
            vec![item(vec![text(tags::DIMENSION_ORGANIZATION_UID, VR::UI, organisation)])],
        ),
        seq(
            tags::DIMENSION_INDEX_SEQUENCE,
            vec![
                dimension(tags::REFERENCED_SEGMENT_NUMBER, tags::SEGMENT_IDENTIFICATION_SEQUENCE, "Segment"),
                dimension(tags::IMAGE_POSITION_PATIENT, tags::PLANE_POSITION_SEQUENCE, "Position"),
            ],
        ),
    ]
}

/// An item of the Segment Sequence.
fn segment_item(s: &ferrum_domain::Segment, number: u16) -> InMemDicomObject {
    let (algorithm_type, algorithm) = match &s.provenance.author {
        Author::Human => ("MANUAL", None),
        Author::Agent { .. } => ("SEMIAUTOMATIC", Some("FERRUM agent".to_owned())),
        Author::Engine { name, version, .. } => {
            ("AUTOMATIC", Some(if version.is_empty() { name.clone() } else { format!("{name} {version}") }))
        }
    };
    let mut e = vec![
        u16s(tags::SEGMENT_NUMBER, &[number]),
        text(tags::SEGMENT_LABEL, VR::LO, &s.name),
        text(
            tags::SEGMENT_DESCRIPTION,
            VR::ST,
            &format!("FERRUM label {}; {}", s.label, provenance_text(&s.provenance)),
        ),
        text(tags::SEGMENT_ALGORITHM_TYPE, VR::CS, algorithm_type),
        seq(tags::SEGMENTED_PROPERTY_CATEGORY_CODE_SEQUENCE, vec![code("91723000", "SCT", "Anatomical Structure")]),
        seq(tags::SEGMENTED_PROPERTY_TYPE_CODE_SEQUENCE, vec![code("85756007", "SCT", "Tissue")]),
        u16s(tags::RECOMMENDED_DISPLAY_CIE_LAB_VALUE, &cielab(s.color)),
    ];
    if let Some(name) = algorithm {
        e.push(text(tags::SEGMENT_ALGORITHM_NAME, VR::LO, &name));
    }
    item(e)
}

/// Derivation Image functional group pointing at a source image.
fn derivation(inst: &SourceInstance) -> Elem {
    let source = item(vec![
        text(tags::REFERENCED_SOP_CLASS_UID, VR::UI, &inst.sop_class_uid),
        text(tags::REFERENCED_SOP_INSTANCE_UID, VR::UI, &inst.sop_instance_uid),
        seq(
            tags::PURPOSE_OF_REFERENCE_CODE_SEQUENCE,
            vec![code("121322", "DCM", "Source image for image processing operation")],
        ),
    ]);
    seq(
        tags::DERIVATION_IMAGE_SEQUENCE,
        vec![item(vec![
            seq(tags::DERIVATION_CODE_SEQUENCE, vec![code("113076", "DCM", "Segmentation")]),
            seq(tags::SOURCE_IMAGE_SEQUENCE, vec![source]),
        ])],
    )
}

/// Common Instance Reference module: the source images used.
fn referenced_series<'a>(ctx: &Context, instances: impl Iterator<Item = &'a SourceInstance>) -> Elem {
    let items = instances
        .map(|i| {
            item(vec![
                text(tags::REFERENCED_SOP_CLASS_UID, VR::UI, &i.sop_class_uid),
                text(tags::REFERENCED_SOP_INSTANCE_UID, VR::UI, &i.sop_instance_uid),
            ])
        })
        .collect();
    seq(
        tags::REFERENCED_SERIES_SEQUENCE,
        vec![item(vec![
            text(tags::SERIES_INSTANCE_UID, VR::UI, &ctx.source_series_uid),
            seq(tags::REFERENCED_INSTANCE_SEQUENCE, items),
        ])],
    )
}

type Code<'a> = (&'a str, &'a str, &'a str);

/// An SR content item with a concept name and value attributes.
fn content_item(relationship: &str, value_type: &str, name: Code<'_>, value: Vec<Elem>) -> InMemDicomObject {
    let mut e = vec![
        text(tags::RELATIONSHIP_TYPE, VR::CS, relationship),
        text(tags::VALUE_TYPE, VR::CS, value_type),
        seq(tags::CONCEPT_NAME_CODE_SEQUENCE, vec![code(name.0, name.1, name.2)]),
    ];
    e.extend(value);
    item(e)
}

fn content_code(relationship: &str, name: Code<'_>, value: Code<'_>) -> InMemDicomObject {
    content_item(
        relationship,
        "CODE",
        name,
        vec![seq(tags::CONCEPT_CODE_SEQUENCE, vec![code(value.0, value.1, value.2)])],
    )
}

fn content_text(relationship: &str, name: Code<'_>, value: &str) -> InMemDicomObject {
    content_item(relationship, "TEXT", name, vec![text(tags::TEXT_VALUE, VR::UT, value)])
}

fn container(
    relationship: &str,
    name: Code<'_>,
    template: Option<&str>,
    children: Vec<InMemDicomObject>,
) -> InMemDicomObject {
    let mut value = vec![text(tags::CONTINUITY_OF_CONTENT, VR::CS, "SEPARATE")];
    if let Some(t) = template {
        value.push(seq(
            tags::CONTENT_TEMPLATE_SEQUENCE,
            vec![item(vec![text(tags::MAPPING_RESOURCE, VR::CS, "DCMR"), text(tags::TEMPLATE_IDENTIFIER, VR::CS, t)])],
        ));
    }
    if !children.is_empty() {
        value.push(seq(tags::CONTENT_SEQUENCE, children));
    }
    content_item(relationship, "CONTAINER", name, value)
}

fn num(name: Code<'_>, value: f64, unit: Code<'_>, children: Vec<InMemDicomObject>) -> InMemDicomObject {
    let measured = item(vec![
        text(tags::NUMERIC_VALUE, VR::DS, &ds(value)),
        seq(tags::MEASUREMENT_UNITS_CODE_SEQUENCE, vec![code(unit.0, unit.1, unit.2)]),
    ]);
    let mut v = vec![seq(tags::MEASURED_VALUE_SEQUENCE, vec![measured])];
    if !children.is_empty() {
        v.push(seq(tags::CONTENT_SEQUENCE, children));
    }
    content_item("CONTAINS", "NUM", name, v)
}

/// Tracking identifiers, review status and author of a measurement group.
fn group_context(name: &str, tracking_seed: &str) -> Vec<InMemDicomObject> {
    vec![
        content_text("HAS OBS CONTEXT", ("112039", "DCM", "Tracking Identifier"), name),
        content_item(
            "HAS OBS CONTEXT",
            "UIDREF",
            ("112040", "DCM", "Tracking Unique Identifier"),
            vec![text(tags::UID, VR::UI, &uid_from(&["ferrum-tracking", tracking_seed]))],
        ),
    ]
}

/// Qualitative evaluations: review status (private code) and author.
fn review_items(p: &Provenance) -> Vec<InMemDicomObject> {
    let (value, meaning) = match p.status {
        ReviewStatus::Proposed => ("proposed", "Proposed, unconfirmed"),
        ReviewStatus::Confirmed => ("confirmed", "Confirmed by a person"),
        ReviewStatus::Rejected => ("rejected", "Rejected by a person"),
    };
    vec![
        content_code("CONTAINS", ("review-status", PRIVATE_SCHEME, "Review status"), (value, PRIVATE_SCHEME, meaning)),
        content_text("CONTAINS", ("provenance", PRIVATE_SCHEME, "Provenance"), &provenance_text(p)),
    ]
}

/// Patient coordinates of an annotation's outline: a line for a distance,
/// a closed polygon for an area or rectangle.
fn outline(volume: &Volume, a: &AnnotationRecord) -> Option<(&'static str, Vec<Vec3>)> {
    let pts = &a.points_voxel;
    let (kind, voxels) = match a.kind {
        "Distance" if pts.len() == 2 => ("POLYLINE", pts.clone()),
        "Area" if pts.len() >= 3 => ("POLYGON", pts.clone()),
        "Rectangle" if pts.len() == 2 => {
            // The two in-plane axes of the slice; the normal stays fixed.
            let n = a.plane.normal_axis();
            let (u, v) = match a.plane {
                SliceAxis::Sagittal => (1, 2),
                SliceAxis::Coronal => (0, 2),
                SliceAxis::Axial => (0, 1),
            };
            debug_assert!(u != n && v != n);
            let corner = |cu: f32, cv: f32| {
                let mut p = pts[0];
                p[u] = cu;
                p[v] = cv;
                p
            };
            let (a0, b0, a1, b1) = (pts[0][u], pts[0][v], pts[1][u], pts[1][v]);
            ("POLYGON", vec![corner(a0, b0), corner(a1, b0), corner(a1, b1), corner(a0, b1)])
        }
        _ => return None,
    };
    let mut patient: Vec<Vec3> = voxels.iter().map(|p| volume.voxel_to_patient(*p)).collect();
    if kind == "POLYGON" {
        patient.push(patient[0]);
    }
    Some((kind, patient))
}

/// Measurement group (TID 1501) of a distance or area annotation.
fn measurement_group(ctx: &Context, volume: &Volume, a: &AnnotationRecord) -> Option<InMemDicomObject> {
    let value = f64::from(a.value?);
    let (name, unit): (Code<'_>, Code<'_>) = match a.unit {
        "mm" => (("410668003", "SCT", "Length"), ("mm", "UCUM", "millimeter")),
        "mm2" => (("42798000", "SCT", "Area"), ("mm2", "UCUM", "square millimeter")),
        _ => return None,
    };
    let (graphic, points) = outline(volume, a)?;
    let data: Vec<f32> = points.iter().flat_map(|p| p.to_array()).collect();
    let region_name = if graphic == "POLYLINE" { ("121055", "DCM", "Path") } else { ("111030", "DCM", "Image Region") };
    let region = content_item(
        "INFERRED FROM",
        "SCOORD3D",
        region_name,
        vec![
            text(tags::GRAPHIC_TYPE, VR::CS, graphic),
            DataElement::new(tags::GRAPHIC_DATA, VR::FL, PrimitiveValue::F32(data.into_iter().collect())),
            text(tags::REFERENCED_FRAME_OF_REFERENCE_UID, VR::UI, &ctx.frame_of_reference_uid),
        ],
    );
    let seed = format!("{}:{}:{}", ctx.source_series_uid, a.id, a.name);
    let mut children = group_context(&a.name, &seed);
    children.push(num(name, value, unit, vec![region]));
    children.extend(review_items(&a.provenance));
    Some(container("CONTAINS", ("125007", "DCM", "Measurement Group"), Some("1501"), children))
}

/// Volumetric ROI measurement group (TID 1411) of a segment.
fn volume_group(ctx: &Context, seg: &SegExport, number: u16, name: &str, ml: f64, p: &Provenance) -> InMemDicomObject {
    let segment = content_item(
        "CONTAINS",
        "IMAGE",
        ("121191", "DCM", "Referenced Segment"),
        vec![seq(
            tags::REFERENCED_SOP_SEQUENCE,
            vec![item(vec![
                text(tags::REFERENCED_SOP_CLASS_UID, VR::UI, uids::SEGMENTATION_STORAGE),
                text(tags::REFERENCED_SOP_INSTANCE_UID, VR::UI, &seg.sop_instance_uid),
                u16s(tags::REFERENCED_SEGMENT_NUMBER, &[number]),
            ])],
        )],
    );
    let series = content_item(
        "CONTAINS",
        "UIDREF",
        ("121232", "DCM", "Source series for segmentation"),
        vec![text(tags::UID, VR::UI, &ctx.source_series_uid)],
    );
    let seed = format!("{}:segment:{number}:{name}", ctx.source_series_uid);
    let mut children = group_context(name, &seed);
    children.extend([
        segment,
        series,
        num(("118565006", "SCT", "Volume"), ml, ("ml", "UCUM", "milliliter"), Vec::new()),
    ]);
    children.extend(review_items(p));
    container("CONTAINS", ("125007", "DCM", "Measurement Group"), Some("1411"), children)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_strings_fit() {
        assert_eq!(ds(1.5), "1.5");
        assert_eq!(ds(-0.0000001), "0");
        assert_eq!(ds(2.0), "2");
        assert_eq!(ds(-123456.123456789), "-123456.123457");
        assert!(ds(1e30).len() <= 16);
        assert_eq!(ds_list(&[1.0, 0.25]), "1\\0.25");
    }

    #[test]
    fn uids_are_valid() {
        let u = uid_from(&["a", "b"]);
        assert!(u.starts_with("2.25.") && u.len() <= 64 && u[5..].bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(u, uid_from(&["a", "b"]));
        assert_ne!(u, uid_from(&["ab"]));
        let o = DicomExportOptions::default();
        assert_ne!(new_uid(&o, "x"), new_uid(&o, "x"));
    }

    #[test]
    fn colours_in_cielab() {
        let near = |a: [u16; 3], b: [u16; 3]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 8);
        assert!(near(cielab([255, 255, 255]), [65535, 32896, 32896]), "white");
        assert!(near(cielab([0, 0, 0]), [0, 32896, 32896]), "black");
        let red = cielab([255, 0, 0]);
        assert!(red[1] > 50000 && red[0] > 30000, "{red:?}");
    }
}
