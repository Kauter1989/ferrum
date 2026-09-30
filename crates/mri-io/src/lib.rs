//! # mri-io
//!
//! Data layer of the viewer. Implements the domain port
//! [`mri_domain::VolumeRepository`] for DICOM (via `dicom-rs`, all common
//! transfer syntaxes including JPEG, JPEG 2000 and RLE) and NIfTI-1.
//!
//! Loading is parallel end to end: headers are parsed concurrently (stopping
//! before pixel data), slices are grouped into series and ordered by their
//! position along the plane normal, then pixel data of all slices is decoded
//! concurrently and normalised into the volume's `u16` storage.

pub mod dicom;
pub mod error;
pub mod files;
pub mod nifti;

use std::path::PathBuf;

use mri_domain::{LoadedSeries, ProgressSink, RepositoryError, SeriesDescriptor, VolumeRepository};

pub use dicom::DicomRepository;
pub use error::IoError;
pub use nifti::{read_nifti, write_nifti, NiftiRepository};

/// Repository that dispatches to every supported format.
pub struct CompositeRepository {
    repositories: Vec<Box<dyn VolumeRepository>>,
}

impl Default for CompositeRepository {
    fn default() -> Self {
        Self { repositories: vec![Box::new(DicomRepository), Box::new(NiftiRepository)] }
    }
}

impl CompositeRepository {
    /// Creates a composite of the given repositories (Open/Closed: new
    /// formats are added without modifying existing code).
    pub fn new(repositories: Vec<Box<dyn VolumeRepository>>) -> Self {
        Self { repositories }
    }
}

impl VolumeRepository for CompositeRepository {
    fn name(&self) -> &str {
        "All formats"
    }

    fn scan(&self, paths: &[PathBuf], progress: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        let mut out = Vec::new();
        for r in &self.repositories {
            match r.scan(paths, progress) {
                Ok(mut s) => out.append(&mut s),
                Err(RepositoryError::NothingFound) => {}
                Err(e) => return Err(e),
            }
        }
        if out.is_empty() {
            Err(RepositoryError::NothingFound)
        } else {
            Ok(out)
        }
    }

    fn load(&self, series: &SeriesDescriptor, progress: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        self.repositories
            .iter()
            .find(|r| r.name() == series.format)
            .ok_or_else(|| RepositoryError::Unsupported(format!("format {}", series.format)))?
            .load(series, progress)
    }
}
