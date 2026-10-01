//! Port (interface) through which the application obtains volumes.
//!
//! The data layer (`ferrum-io`) implements [`VolumeRepository`] for concrete
//! formats; the application layer depends only on this trait, keeping the
//! dependency rule `Presentation → Domain ← Data`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

use crate::geometry::Dims3;
use crate::volume::Volume;
use crate::window::WindowLevel;

/// Receives progress notifications from long-running operations and may
/// request cancellation.
pub trait ProgressSink: Sync {
    /// Reports completion `fraction ∈ [0, 1]` with a short status message.
    fn report(&self, fraction: f32, message: &str);

    /// Returns `true` if the operation should stop as soon as possible.
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// A [`ProgressSink`] that ignores everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn report(&self, _fraction: f32, _message: &str) {}
}

/// A cancellation flag that can be shared between threads.
#[derive(Debug, Default)]
pub struct CancelFlag(AtomicBool);

impl CancelFlag {
    /// Requests cancellation.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// `true` once [`CancelFlag::cancel`] was called.
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Errors reported by repositories.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum RepositoryError {
    /// No loadable data was found in the given paths.
    #[error("no supported medical images found")]
    NothingFound,
    /// The data format is recognised but a feature is not supported.
    #[error("unsupported data: {0}")]
    Unsupported(String),
    /// The data is malformed.
    #[error("corrupt or inconsistent data: {0}")]
    Corrupt(String),
    /// An operating-system level I/O error.
    #[error("I/O error: {0}")]
    Io(String),
    /// The user cancelled the operation.
    #[error("operation cancelled")]
    Cancelled,
}

/// Description of one loadable series found by [`VolumeRepository::scan`].
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesDescriptor {
    /// Stable identifier (e.g. DICOM Series Instance UID + geometry).
    pub id: String,
    /// Name of the format that produced the descriptor (e.g. `"DICOM"`).
    pub format: String,
    /// Human readable description.
    pub description: String,
    /// Imaging modality (`CT`, `MR`, …) if known.
    pub modality: String,
    /// Volume dimensions (`z` = number of slices).
    pub dims: Dims3,
    /// Files that make up the series.
    pub sources: Vec<PathBuf>,
}

/// Study and series identification of a loaded series, taken from the
/// DICOM header (empty strings when the format has no such field).
/// Dates and times keep the DICOM `YYYYMMDD` / `HHMMSS` form.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StudyInfo {
    /// Study Instance UID.
    pub study_instance_uid: String,
    /// Series Instance UID.
    pub series_instance_uid: String,
    /// Study date (`YYYYMMDD`).
    pub study_date: String,
    /// Study time (`HHMMSS[.ffffff]`).
    pub study_time: String,
    /// Study description.
    pub study_description: String,
    /// Series description.
    pub series_description: String,
    /// Series number.
    pub series_number: String,
    /// Accession number.
    pub accession_number: String,
    /// Imaging modality.
    pub modality: String,
}

/// Descriptive metadata of a loaded series.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeriesMetadata {
    /// Imaging modality.
    pub modality: String,
    /// Series description.
    pub description: String,
    /// Window suggested by the data (e.g. DICOM Window Center/Width).
    pub default_window: Option<WindowLevel>,
    /// Name/value pairs shown in the "info" dialog.
    pub attributes: Vec<(String, String)>,
    /// Study identification (DICOM only).
    pub study: StudyInfo,
    /// What was opened: the folder of a DICOM series or the NIfTI file.
    pub source: PathBuf,
}

/// A fully loaded series.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSeries {
    /// The volume.
    pub volume: Volume,
    /// Descriptive metadata.
    pub metadata: SeriesMetadata,
}

/// Source of volumes (DICOM directories, NIfTI files, …).
pub trait VolumeRepository: Send + Sync {
    /// Human-readable name of the repository.
    fn name(&self) -> &str;

    /// Finds all series contained in `paths` (files and/or directories).
    fn scan(&self, paths: &[PathBuf], progress: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError>;

    /// Loads the series described by `series`.
    fn load(&self, series: &SeriesDescriptor, progress: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_flag() {
        let f = CancelFlag::default();
        assert!(!f.is_set());
        f.cancel();
        assert!(f.is_set());
        assert!(!NoProgress.is_cancelled());
        NoProgress.report(0.5, "x");
    }

    #[test]
    fn errors_have_messages() {
        assert_eq!(RepositoryError::Cancelled.to_string(), "operation cancelled");
        assert!(RepositoryError::Corrupt("x".into()).to_string().contains('x'));
    }
}
