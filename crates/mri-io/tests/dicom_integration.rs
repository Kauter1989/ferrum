//! Integration tests of the DICOM repository against synthetic files and
//! an optional real series given by `MRI_SAMPLE_DICOM`.
#![allow(missing_docs, deprecated, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;
use std::sync::Mutex;

use common::{phantom_value, write_series, SyntheticSlice};
use dicom_dictionary_std::uids;
use glam::Vec3;
use mri_domain::{Dims3, NoProgress, ProgressSink, RepositoryError, VolumeRepository};
use mri_io::{CompositeRepository, DicomRepository};

fn load_single(dir: &std::path::Path) -> mri_domain::LoadedSeries {
    let repo = DicomRepository;
    let series = repo.scan(&[dir.to_path_buf()], &NoProgress).expect("scan");
    assert_eq!(series.len(), 1, "{series:?}");
    repo.load(&series[0], &NoProgress).expect("load")
}

fn assert_phantom(v: &mri_domain::Volume, rows: u32, cols: u32, n: u32) {
    assert_eq!(v.dims(), Dims3::new(cols, rows, n));
    for &(i, j, k) in &[(0, 0, 0), (cols - 1, 0, 0), (0, rows - 1, 1), (2, 3, n - 1)] {
        let expected = f32::from(phantom_value(i, j, k));
        let got = v.physical(i, j, k).unwrap();
        assert!((got - expected).abs() < 0.1, "voxel ({i},{j},{k}): {got} != {expected}");
    }
}

#[test]
fn loads_explicit_little_endian_series_in_spatial_order() {
    let dir = tempfile::tempdir().unwrap();
    write_series(dir.path(), 6, 8, 5, 2.0, |_| {});
    let l = load_single(dir.path());
    assert_phantom(&l.volume, 6, 8, 5);
    assert!((l.volume.spacing() - Vec3::new(0.6, 0.8, 2.0)).length() < 1e-5);
    assert_eq!(l.metadata.modality, "CT");
    assert!(l.metadata.attributes.iter().any(|(k, v)| k == "Ordering" && v == "Position"));
}

#[test]
fn loads_implicit_vr_and_big_endian() {
    for ts in [uids::IMPLICIT_VR_LITTLE_ENDIAN, uids::EXPLICIT_VR_BIG_ENDIAN] {
        let dir = tempfile::tempdir().unwrap();
        write_series(dir.path(), 4, 4, 3, 1.0, |s| s.transfer_syntax = ts);
        let l = load_single(dir.path());
        assert_phantom(&l.volume, 4, 4, 3);
    }
}

#[test]
fn applies_rescale_and_signed_pixels() {
    let dir = tempfile::tempdir().unwrap();
    write_series(dir.path(), 4, 4, 2, 1.0, |s| {
        s.signed = true;
        s.slope = 2.0;
        s.intercept = -1024.0;
        s.pixels[0] = (-5i16) as u16;
        s.window = Some((40.0, 400.0));
    });
    let l = load_single(dir.path());
    let v = &l.volume;
    assert!((v.physical(0, 0, 0).unwrap() - (-1034.0)).abs() < 0.1);
    assert!((v.physical(1, 0, 0).unwrap() - (10.0 * 2.0 - 1024.0)).abs() < 0.1);
    assert!((v.range().min - (-1034.0)).abs() < 0.1);
    let w = l.metadata.default_window.unwrap();
    assert_eq!((w.center, w.width), (40.0, 400.0));
}

#[test]
fn monochrome1_is_inverted() {
    let dir = tempfile::tempdir().unwrap();
    write_series(dir.path(), 4, 4, 2, 1.0, |s| s.photometric = "MONOCHROME1");
    let l = load_single(dir.path());
    let v = &l.volume;
    // brightest stored value becomes darkest
    assert!(v.voxel(0, 0, 0).unwrap() > v.voxel(3, 3, 1).unwrap());
}

#[test]
fn separates_multiple_series_and_loads_the_requested_one() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    write_series(&a, 4, 4, 3, 1.0, |s| s.series_uid = "1.2.3.1".into());
    write_series(&b, 8, 8, 2, 1.0, |s| s.series_uid = "1.2.3.2".into());
    let repo = DicomRepository;
    let series = repo.scan(&[dir.path().to_path_buf()], &NoProgress).unwrap();
    assert_eq!(series.len(), 2);
    let big = series.iter().find(|s| s.dims.x == 8).unwrap();
    assert_eq!(big.dims, Dims3::new(8, 8, 2));
    let l = repo.load(big, &NoProgress).unwrap();
    assert_eq!(l.volume.dims(), Dims3::new(8, 8, 2));
}

#[test]
fn orders_by_instance_number_without_positions() {
    let dir = tempfile::tempdir().unwrap();
    write_series(dir.path(), 4, 4, 4, 1.0, |s| s.include_position = false);
    let l = load_single(dir.path());
    assert_phantom(&l.volume, 4, 4, 4);
    assert!((l.volume.spacing().z - 2.5).abs() < 1e-6, "falls back to slice thickness");
}

#[test]
fn loads_multiframe_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = SyntheticSlice::new(4, 5, 0.0, 1);
    s.frames = 3;
    s.pixels = (0..3u32).flat_map(|k| (0..20u32).map(move |p| phantom_value(p % 5, p / 5, k))).collect();
    s.write(&dir.path().join("multi.dcm"));
    let l = load_single(dir.path());
    assert_phantom(&l.volume, 4, 5, 3);
    assert!((l.volume.spacing().z - 1.25).abs() < 1e-6);
}

#[test]
fn scan_of_directory_without_dicom_reports_nothing_found() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.txt"), "hello").unwrap();
    assert_eq!(DicomRepository.scan(&[dir.path().to_path_buf()], &NoProgress), Err(RepositoryError::NothingFound));
}

#[test]
fn corrupt_file_is_skipped_during_scan() {
    let dir = tempfile::tempdir().unwrap();
    write_series(dir.path(), 4, 4, 2, 1.0, |_| {});
    let mut junk = vec![0u8; 128];
    junk.extend_from_slice(b"DICMgarbage");
    std::fs::write(dir.path().join("zzz.dcm"), junk).unwrap();
    let series = DicomRepository.scan(&[dir.path().to_path_buf()], &NoProgress).unwrap();
    assert_eq!(series.len(), 1);
}

struct Recorder {
    events: Mutex<Vec<f32>>,
    cancel_after: Option<usize>,
}

impl ProgressSink for Recorder {
    fn report(&self, fraction: f32, _message: &str) {
        self.events.lock().unwrap().push(fraction);
    }
    fn is_cancelled(&self) -> bool {
        self.cancel_after.is_some_and(|n| self.events.lock().unwrap().len() >= n)
    }
}

#[test]
fn reports_progress_and_supports_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    write_series(dir.path(), 8, 8, 16, 1.0, |_| {});
    let repo = DicomRepository;
    let series = repo.scan(&[dir.path().to_path_buf()], &NoProgress).unwrap();

    let rec = Recorder { events: Mutex::new(Vec::new()), cancel_after: None };
    repo.load(&series[0], &rec).unwrap();
    let ev = rec.events.lock().unwrap().clone();
    assert!(!ev.is_empty());
    assert!(ev.iter().all(|f| (0.0..=1.0).contains(f)));
    assert_eq!(*ev.last().unwrap(), 1.0);

    let cancel = Recorder { events: Mutex::new(Vec::new()), cancel_after: Some(1) };
    assert_eq!(repo.load(&series[0], &cancel), Err(RepositoryError::Cancelled));
}

#[test]
fn composite_repository_finds_dicom_and_nifti() {
    let dir = tempfile::tempdir().unwrap();
    write_series(dir.path(), 4, 4, 2, 1.0, |_| {});
    let v = mri_domain::Volume::from_physical(Dims3::new(2, 2, 2), Vec3::ONE, &[0.0; 8]).unwrap();
    mri_io::write_nifti(&v, &dir.path().join("x.nii")).unwrap();
    let repo = CompositeRepository::default();
    let series = repo.scan(&[dir.path().to_path_buf()], &NoProgress).unwrap();
    assert_eq!(series.len(), 2);
    for s in &series {
        repo.load(s, &NoProgress).unwrap();
    }
}

/// Optional real DICOM series, supplied via `MRI_SAMPLE_DICOM=<dir>` (no
/// image data is committed to the repository).
fn sample_dir() -> Option<PathBuf> {
    std::env::var_os("MRI_SAMPLE_DICOM").map(PathBuf::from)
}

#[test]
fn loads_repository_sample_series() {
    let Some(dir) = sample_dir().filter(|d| d.exists()) else {
        eprintln!("MRI_SAMPLE_DICOM not set, skipping real-data test");
        return;
    };
    let repo = DicomRepository;
    let series = repo.scan(&[dir], &NoProgress).unwrap();
    assert!(!series.is_empty());
    let s = series.iter().max_by_key(|s| s.dims.z).unwrap();
    let l = repo.load(s, &NoProgress).unwrap();
    let v = &l.volume;
    assert_eq!(v.dims().z, s.dims.z);
    assert!(v.dims().x > 16 && v.dims().y > 16);
    assert!(v.spacing().cmpgt(Vec3::ZERO).all());
    assert!(v.range().span() > 0.0);
    // non-trivial content
    let nonzero = v.data().iter().filter(|&&x| x > 0).count();
    assert!(nonzero > v.data().len() / 10);
}
