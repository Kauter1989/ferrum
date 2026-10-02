//! DICOM SEG and SR export of a synthetic CT series: structure, geometry,
//! review status and privacy. With `FERRUM_DICOM_EXPORT_DIR` set, the
//! exported files are also copied there for validation with highdicom
//! (`scripts/validate_dicom_export.py`).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use dicom_dictionary_std::tags;
use dicom_object::{open_file, InMemDicomObject};
use ferrum_domain::{
    Annotation, AnnotationReport, AnnotationSet, LabelMap, NoProgress, Provenance, ReviewStatus, Segment,
    SegmentationSet, SliceAxis, SliceKey, Timestamp, Volume, VolumeRepository,
};
use ferrum_io::dicom::export::{read_source, uid_from, DicomExportOptions, DicomExporter, PRIVATE_SCHEME};
use ferrum_io::DicomRepository;
use glam::Vec2;

const SERIES_UID: &str = "1.2.826.0.1.3680043.2.1125.1";
const STUDY_UID: &str = "1.2.826.0.1.3680043.2.1";

/// 24 × 20 × 10 CT, slices 2.5 mm apart.
fn ct(dir: &Path) -> (Volume, Vec<PathBuf>) {
    common::write_series(dir, 20, 24, 10, 2.5, |_| {});
    let repo = DicomRepository;
    let series = repo.scan(&[dir.to_path_buf()], &NoProgress).unwrap().remove(0);
    let loaded = repo.load(&series, &NoProgress).unwrap();
    (loaded.volume, series.sources)
}

/// Label 1 (human) in slices 2..=4, label 2 (engine, proposed) in 4..=6,
/// label 3 rejected, label 4 empty.
fn segments(volume: &Volume) -> SegmentationSet {
    let d = volume.dims();
    let mut data = vec![0u8; d.voxel_count()];
    for k in 0..d.z {
        for j in 0..d.y {
            for i in 0..d.x {
                let v = match (i, j, k) {
                    (3..=8, 4..=9, 2..=4) => 1,
                    (12..=15, 10..=12, 4..=6) => 2,
                    (20.., 0..=1, 0) => 3,
                    _ => 0,
                };
                data[d.index(i, j, k)] = v;
            }
        }
    }
    let t = Timestamp(1_790_000_000);
    let mut liver = Segment::new(1, "liver");
    liver.provenance = Provenance::human(t);
    let mut lesion = Segment::new(2, "lesion");
    lesion.color = [255, 0, 0];
    lesion.provenance = Provenance::engine("nnInteractive", "2.6", true, t);
    let mut noise = Segment::new(3, "noise");
    noise.provenance = Provenance::agent(None, t);
    noise.provenance.review(ReviewStatus::Rejected, Some("dr.k"), t);
    let empty = Segment::new(4, "empty");
    SegmentationSet::from_labels(LabelMap::from_data(d, data).unwrap(), vec![liver, lesion, noise, empty])
}

fn annotations(volume: &Volume) -> AnnotationReport {
    let mut set = AnnotationSet::default();
    let axial = SliceKey::new(SliceAxis::Axial, 3);
    let t = Timestamp(1_790_000_000);
    set.add(axial, Annotation::Distance { a: Vec2::new(1.2, 1.6), b: Vec2::new(6.0, 1.6) });
    let id = set.add_with(
        axial,
        Annotation::Polygon { points: vec![Vec2::new(1.0, 1.0), Vec2::new(5.0, 1.0), Vec2::new(5.0, 4.0)] },
        Provenance::agent(Some("run-1".into()), t),
    );
    set.rename(id, "Nodule area");
    set.add(SliceKey::new(SliceAxis::Coronal, 5), Annotation::Rect { a: Vec2::new(1.0, 2.0), b: Vec2::new(4.0, 7.0) });
    set.add(axial, Annotation::Angle { a: Vec2::new(1.0, 1.0), vertex: Vec2::new(2.0, 2.0), b: Vec2::new(3.0, 1.0) });
    set.add(axial, Annotation::Text { pos: Vec2::new(1.0, 1.0), text: "note".into() });
    AnnotationReport::build(PathBuf::new(), Default::default(), volume, &set)
}

fn str_at(obj: &InMemDicomObject, tag: dicom_object::Tag) -> String {
    obj.element(tag).unwrap().to_str().unwrap().trim_end_matches(['\0', ' ']).to_string()
}

fn items(obj: &InMemDicomObject, tag: dicom_object::Tag) -> Vec<InMemDicomObject> {
    obj.element(tag).map(|e| e.items().unwrap().to_vec()).unwrap_or_default()
}

fn first(obj: &InMemDicomObject, tags: &[dicom_object::Tag]) -> InMemDicomObject {
    tags.iter().fold(obj.clone(), |o, t| items(&o, *t).remove(0))
}

fn concept(obj: &InMemDicomObject) -> String {
    str_at(&first(obj, &[tags::CONCEPT_NAME_CODE_SEQUENCE]), tags::CODE_VALUE)
}

fn keep_samples(files: &[&Path], name: &str) {
    if let Ok(dir) = std::env::var("FERRUM_DICOM_EXPORT_DIR") {
        let dir = Path::new(&dir).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::copy(f, dir.join(f.file_name().unwrap())).unwrap();
        }
    }
}

/// The segmentation of [`segments`] on the CT, with source references.
fn check_seg(seg_path: &Path, volume: &Volume, set: &SegmentationSet, source: &ferrum_io::dicom::export::DicomSource) {
    let obj = open_file(seg_path).unwrap();
    assert_eq!(str_at(&obj, tags::STUDY_INSTANCE_UID), STUDY_UID);
    assert_eq!(str_at(&obj, tags::NUMBER_OF_FRAMES), "6");
    assert_eq!(str_at(&obj, tags::STUDY_DATE), "20240428");
    let segs = items(&obj, tags::SEGMENT_SEQUENCE);
    assert_eq!(str_at(&segs[0], tags::SEGMENT_ALGORITHM_TYPE), "MANUAL");
    assert_eq!(str_at(&segs[1], tags::SEGMENT_ALGORITHM_TYPE), "AUTOMATIC");
    assert_eq!(str_at(&segs[1], tags::SEGMENT_ALGORITHM_NAME), "nnInteractive 2.6");
    assert!(str_at(&segs[1], tags::SEGMENT_DESCRIPTION).contains("unconfirmed"));
    let frames = items(&obj, tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE);
    assert_eq!(frames.len(), 6);
    // Frame 0: segment 1 at slice k = 2.
    let pos = str_at(&first(&frames[0], &[tags::PLANE_POSITION_SEQUENCE]), tags::IMAGE_POSITION_PATIENT);
    let expected = volume.voxel_to_patient(glam::Vec3::new(0.0, 0.0, 2.0));
    let parsed: Vec<f32> = pos.split('\\').map(|v| v.parse().unwrap()).collect();
    assert!((glam::Vec3::from_slice(&parsed) - expected).length() < 1e-3, "{pos} vs {expected}");
    let source_image = first(&frames[0], &[tags::DERIVATION_IMAGE_SEQUENCE, tags::SOURCE_IMAGE_SEQUENCE]);
    let referenced = str_at(&source_image, tags::REFERENCED_SOP_INSTANCE_UID);
    let k2 = source.instances.iter().find(|i| i.sop_instance_uid == referenced).unwrap();
    assert!((k2.position.unwrap().as_vec3() - expected).length() < 1e-3, "derived from the slice at the same position");
    assert_eq!(items(&first(&obj, &[tags::REFERENCED_SERIES_SEQUENCE]), tags::REFERENCED_INSTANCE_SEQUENCE).len(), 5);
    // Pixel bits: frame 0 is label 1 on slice 2 (i 3..=8, j 4..=9).
    let pixels = obj.element(tags::PIXEL_DATA).unwrap().to_bytes().unwrap();
    let bit = |frame: usize, i: usize, j: usize| {
        let b = frame * 24 * 20 + j * 24 + i;
        pixels[b / 8] >> (b % 8) & 1
    };
    assert_eq!((bit(0, 3, 4), bit(0, 8, 9), bit(0, 2, 4), bit(0, 9, 9)), (1, 1, 0, 0));
    assert_eq!((bit(3, 12, 10), bit(3, 3, 4)), (1, 0), "frame 3 is the lesion on slice 4");
    let ones: u32 = pixels.iter().map(|b| b.count_ones()).sum();
    assert_eq!(ones as u64, set.voxel_count(1) + set.voxel_count(2));
}

/// The report of [`annotations`] and the segment volumes.
fn check_sr(sr_path: &Path, report: &AnnotationReport, volume: &Volume, set: &SegmentationSet, seg_uid: &str) {
    let sr = open_file(sr_path).unwrap();
    assert_eq!(str_at(&sr, tags::COMPLETION_FLAG), "PARTIAL", "unconfirmed items");
    assert_eq!(str_at(&sr, tags::VERIFICATION_FLAG), "UNVERIFIED");
    assert_eq!(concept(&sr), "126000");
    let root = items(&sr, tags::CONTENT_SEQUENCE);
    let measurements = root.iter().find(|i| concept(i) == "126010").unwrap();
    let groups = items(measurements, tags::CONTENT_SEQUENCE);
    assert_eq!(groups.len(), 5, "3 measurements and 2 segment volumes");
    let find =
        |g: &InMemDicomObject, code: &str| items(g, tags::CONTENT_SEQUENCE).into_iter().find(|i| concept(i) == code);
    let length = find(&groups[0], "410668003").unwrap();
    let value: f64 = str_at(&first(&length, &[tags::MEASURED_VALUE_SEQUENCE]), tags::NUMERIC_VALUE).parse().unwrap();
    assert!((value - f64::from(report.annotations[0].value.unwrap())).abs() < 1e-3);
    let line = first(&length, &[tags::CONTENT_SEQUENCE]);
    assert_eq!(str_at(&line, tags::GRAPHIC_TYPE), "POLYLINE");
    let data = line.element(tags::GRAPHIC_DATA).unwrap().to_multi_float32().unwrap();
    let a = glam::Vec3::from_slice(&data[..3]);
    let b = glam::Vec3::from_slice(&data[3..]);
    assert!(((a - b).length() - value as f32).abs() < 1e-3, "the line is as long as the measurement");
    let rect = find(&groups[2], "42798000").unwrap();
    let polygon = first(&rect, &[tags::CONTENT_SEQUENCE]);
    assert_eq!(polygon.element(tags::GRAPHIC_DATA).unwrap().to_multi_float32().unwrap().len(), 15, "closed rectangle");
    let status = |g: &InMemDicomObject| {
        let c = find(g, "review-status").unwrap();
        str_at(&first(&c, &[tags::CONCEPT_CODE_SEQUENCE]), tags::CODE_VALUE)
    };
    assert_eq!(
        (status(&groups[0]), status(&groups[1]), status(&groups[4])),
        ("confirmed".into(), "proposed".into(), "proposed".into())
    );
    let volume_ml: f64 =
        str_at(&first(&find(&groups[4], "118565006").unwrap(), &[tags::MEASURED_VALUE_SEQUENCE]), tags::NUMERIC_VALUE)
            .parse()
            .unwrap();
    assert!((volume_ml - set.volume_ml(2, volume.spacing())).abs() < 1e-6);
    let segment_ref = first(&find(&groups[4], "121191").unwrap(), &[tags::REFERENCED_SOP_SEQUENCE]);
    assert_eq!(str_at(&segment_ref, tags::REFERENCED_SOP_INSTANCE_UID), seg_uid);
    let scheme = first(&sr, &[tags::CODING_SCHEME_IDENTIFICATION_SEQUENCE]);
    assert_eq!(str_at(&scheme, tags::CODING_SCHEME_DESIGNATOR), PRIVATE_SCHEME);
}

#[test]
fn segmentation_and_report_with_source_references() {
    let src_dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let (volume, files) = ct(src_dir.path());
    let set = segments(&volume);
    let report = annotations(&volume);
    let source = read_source(&files).unwrap();
    assert_eq!((source.instances.len(), source.series_uid.as_str()), (10, SERIES_UID));
    let opts = DicomExportOptions {
        copy_identifiers: true,
        copy_dates: true,
        keep_uids: true,
        uid_salt: "s".into(),
        software_version: "0.2.0".into(),
        series_id: SERIES_UID.into(),
    };
    let exporter = DicomExporter::new(Some(&source), &opts);
    assert_eq!(exporter.study_uid(), STUDY_UID);
    let (seg_path, sr_path) = (out.path().join("segmentation.dcm"), out.path().join("measurements.dcm"));
    let result = exporter.export(&volume, &set, &report, &seg_path, &sr_path).unwrap();
    keep_samples(&[&seg_path, &sr_path], "references");

    let seg = result.seg.clone().unwrap();
    assert_eq!(seg.segments, vec![(1, 1), (2, 2)], "rejected and empty segments are left out");
    assert_eq!(seg.frames, 6, "one frame per segment and slice with voxels");
    assert_eq!(result.report_measurements, Some(3), "distance, area, rectangle");
    assert_eq!(result.skipped.len(), 4, "{:?}", result.skipped);

    check_seg(&seg_path, &volume, &set, &source);
    check_sr(&sr_path, &report, &volume, &set, &seg.sop_instance_uid);
}

#[test]
fn nothing_identifying_leaves_without_permission() {
    let src_dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let (volume, files) = ct(src_dir.path());
    let source = read_source(&files).unwrap();
    let opts = DicomExportOptions { uid_salt: "salt".into(), series_id: "anon".into(), ..Default::default() };
    let exporter = DicomExporter::new(Some(&source), &opts);
    assert_eq!(exporter.study_uid(), uid_from(&["ferrum-uid", "salt", STUDY_UID]));
    let (seg_path, sr_path) = (out.path().join("segmentation.dcm"), out.path().join("measurements.dcm"));
    exporter.export(&volume, &segments(&volume), &annotations(&volume), &seg_path, &sr_path).unwrap();
    keep_samples(&[&seg_path, &sr_path], "pseudonymised");
    for path in [&seg_path, &sr_path] {
        let bytes = std::fs::read(path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for secret in [STUDY_UID, SERIES_UID, "20240428", "102417"] {
            assert!(!text.contains(secret), "{secret} in {}", path.display());
        }
        let obj = open_file(path).unwrap();
        assert_eq!(str_at(&obj, tags::PATIENT_IDENTITY_REMOVED), "YES");
        assert_eq!(str_at(&obj, tags::PATIENT_ID), "");
    }
    let seg = open_file(seg_path).unwrap();
    assert!(seg.element(tags::REFERENCED_SERIES_SEQUENCE).is_err(), "no references to source images");
}

#[test]
fn non_dicom_sources_and_nothing_to_export() {
    let src_dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let (volume, _) = ct(src_dir.path());
    assert!(read_source(&[out.path().join("missing.dcm")]).is_none());
    let opts = DicomExportOptions { series_id: "volume.nii.gz".into(), ..Default::default() };
    let exporter = DicomExporter::new(None, &opts);
    let (seg_path, sr_path) = (out.path().join("segmentation.dcm"), out.path().join("measurements.dcm"));
    let empty = AnnotationReport::build(PathBuf::new(), Default::default(), &volume, &AnnotationSet::default());
    let r = exporter.export(&volume, &SegmentationSet::new(volume.dims()), &empty, &seg_path, &sr_path).unwrap();
    assert_eq!((r.seg, r.report_measurements), (None, None));
    assert!(!seg_path.exists() && !sr_path.exists());
    let r = exporter.export(&volume, &segments(&volume), &empty, &seg_path, &sr_path).unwrap();
    assert_eq!(r.report_measurements, Some(0), "segment volumes only");
    keep_samples(&[&seg_path, &sr_path], "no-dicom-source");
    let seg = open_file(&seg_path).unwrap();
    assert!(str_at(&seg, tags::STUDY_INSTANCE_UID).starts_with("2.25."));
    let wrong = SegmentationSet::new(ferrum_domain::Dims3::new(2, 2, 2));
    assert!(exporter.write_seg(&seg_path, &volume, &wrong).is_err());
}
