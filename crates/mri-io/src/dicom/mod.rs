//! DICOM data source.

pub mod assemble;
pub mod header;
pub mod series;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use dicom_dictionary_std::tags;
use dicom_object::OpenFileOptions;
use mri_domain::{
    Dims3, LoadedSeries, ProgressSink, RepositoryError, SeriesDescriptor, SeriesMetadata, VolumeRepository,
};
use rayon::prelude::*;

use crate::error::IoError;
use crate::files::{collect_files, detect, FileKind};
use header::SliceHeader;

/// Reads DICOM Part 10 files and directories.
#[derive(Debug, Default, Clone, Copy)]
pub struct DicomRepository;

impl DicomRepository {
    /// Parses headers of all DICOM files in `files` in parallel. Files that
    /// fail to parse are skipped (logged).
    fn read_headers(files: &[PathBuf], progress: &dyn ProgressSink) -> Result<Vec<SliceHeader>, IoError> {
        let total = files.len().max(1);
        let done = AtomicUsize::new(0);
        let headers: Vec<Option<SliceHeader>> = files
            .par_iter()
            .map(|f| {
                if progress.is_cancelled() {
                    return Err(IoError::Cancelled);
                }
                let h = match SliceHeader::read(f) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        log::warn!("skipping {}: {e}", f.display());
                        None
                    }
                };
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                progress.report(d as f32 / total as f32, "Reading DICOM headers");
                Ok(h)
            })
            .collect::<Result<_, _>>()?;
        Ok(headers.into_iter().flatten().collect())
    }

    fn describe(key: &series::SeriesKey, headers: &[SliceHeader], planes: usize) -> SeriesDescriptor {
        let first = &headers[0];
        let label = if first.series_description.is_empty() { "Unnamed series" } else { &first.series_description };
        SeriesDescriptor {
            id: key.id(),
            format: "DICOM".into(),
            description: format!("{label} — {planes} slices, {}×{}", first.columns, first.rows),
            modality: first.modality.clone(),
            dims: Dims3::new(first.columns, first.rows, planes as u32),
            sources: headers.iter().map(|h| h.path.clone()).collect(),
        }
    }
}

impl VolumeRepository for DicomRepository {
    fn name(&self) -> &str {
        "DICOM"
    }

    fn scan(&self, paths: &[PathBuf], progress: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        let files: Vec<PathBuf> =
            collect_files(paths).into_par_iter().filter(|f| detect(f) == FileKind::Dicom).collect();
        if files.is_empty() {
            return Err(RepositoryError::NothingFound);
        }
        let headers = Self::read_headers(&files, progress)?;
        let series: Vec<SeriesDescriptor> = series::group(headers)
            .into_iter()
            .map(|(key, hs)| {
                let planes = series::order(&hs).planes.len();
                Self::describe(&key, &hs, planes)
            })
            .collect();
        if series.is_empty() {
            return Err(RepositoryError::NothingFound);
        }
        Ok(series)
    }

    fn load(&self, series: &SeriesDescriptor, progress: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        let headers = Self::read_headers(&series.sources, &ScaledProgress::new(progress, 0.0, 0.1))?;
        // Keep only slices matching the requested series (sources may have
        // been assembled from a directory with several series).
        let headers: Vec<SliceHeader> = series::group(headers)
            .into_iter()
            .find(|(k, _)| k.id() == series.id)
            .map(|(_, hs)| hs)
            .ok_or(RepositoryError::NothingFound)?;
        let ordered = series::order(&headers);
        let volume = assemble::assemble(&headers, &ordered, &ScaledProgress::new(progress, 0.1, 1.0))?;
        let first = &headers[0];
        let attributes = OpenFileOptions::new()
            .read_until(tags::PIXEL_DATA)
            .open_file(&first.path)
            .map(|obj| header::describe(&obj))
            .unwrap_or_default();
        let mut attributes = attributes;
        attributes.push(("Slices".into(), ordered.planes.len().to_string()));
        attributes.push(("Slice spacing".into(), format!("{:.3} mm", ordered.slice_spacing)));
        attributes.push(("Ordering".into(), format!("{:?}", ordered.method)));
        Ok(LoadedSeries {
            volume,
            metadata: SeriesMetadata {
                modality: first.modality.clone(),
                description: first.series_description.clone(),
                default_window: first.window,
                attributes,
            },
        })
    }
}

/// Maps progress of a sub-task into `[from, to]` of the parent.
pub(crate) struct ScaledProgress<'a> {
    inner: &'a dyn ProgressSink,
    from: f32,
    to: f32,
}

impl<'a> ScaledProgress<'a> {
    pub(crate) fn new(inner: &'a dyn ProgressSink, from: f32, to: f32) -> Self {
        Self { inner, from, to }
    }
}

impl ProgressSink for ScaledProgress<'_> {
    fn report(&self, fraction: f32, message: &str) {
        self.inner.report(self.from + (self.to - self.from) * fraction.clamp(0.0, 1.0), message);
    }

    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}
