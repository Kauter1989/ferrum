//! DICOM data source.

pub mod assemble;
pub mod export;
pub mod frames;
pub mod header;
pub mod series;
pub mod volumes;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use dicom_dictionary_std::tags;
use dicom_object::OpenFileOptions;
use ferrum_domain::{
    Dims3, LoadedSeries, ProgressSink, RepositoryError, SeriesDescriptor, SeriesMetadata, VolumeInfo, VolumeRepository,
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
            volume: None,
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

    fn volumes(
        &self,
        series: &SeriesDescriptor,
        progress: &dyn ProgressSink,
    ) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        if series.volume.is_some() {
            return Ok(vec![series.clone()]);
        }
        let headers = Self::read_headers(&series.sources, progress)?;
        let Some(headers) = series::group(headers).into_iter().find(|(k, _)| k.id() == series.id).map(|(_, hs)| hs)
        else {
            return Err(RepositoryError::NothingFound);
        };
        let specs = volumes::partition(&headers);
        if specs.is_empty() {
            return Ok(vec![series.clone()]);
        }
        let count = specs.len() as u32;
        Ok(specs
            .into_iter()
            .enumerate()
            .map(|(index, v)| volume_descriptor(series, &headers, &v, index as u32, count))
            .collect())
    }

    fn load(&self, series: &SeriesDescriptor, progress: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        let headers = Self::read_headers(&series.sources, &ScaledProgress::new(progress, 0.0, 0.1))?;
        // Keep only slices matching the requested series (sources may have
        // been assembled from a directory with several series).
        let (group_id, volume_key) = split_volume_id(&series.id);
        let headers: Vec<SliceHeader> = series::group(headers)
            .into_iter()
            .find(|(k, _)| k.id() == group_id)
            .map(|(_, hs)| hs)
            .ok_or(RepositoryError::NothingFound)?;
        // One volume: only its images. Its files alone may already hold one
        // image per position (classic files), then the whole group is it.
        let spec = volume_key.and_then(|key| volumes::partition(&headers).into_iter().find(|v| v.key == key));
        let ordered = match &spec {
            Some(v) => series::order_selected(&headers, &v.members),
            None => series::order(&headers),
        };
        let volume = assemble::assemble(&headers, &ordered, &ScaledProgress::new(progress, 0.1, 1.0))?;
        let first = &headers[0];
        let (mut attributes, study) = OpenFileOptions::new()
            .read_until(tags::PIXEL_DATA)
            .open_file(&first.path)
            .map(|obj| (header::describe(&obj), header::study_info(&obj)))
            .unwrap_or_default();
        attributes.push(("Slices".into(), ordered.planes.len().to_string()));
        attributes.push(("Slice spacing".into(), format!("{:.3} mm", ordered.slice_spacing)));
        attributes.push(("Ordering".into(), format!("{:?}", ordered.method)));
        if let Some(info) = &series.volume {
            attributes.push(("Volume".into(), format!("{} ({} of {})", info.label, info.index + 1, info.count)));
        }
        let mut warnings = ordered.warnings;
        if first.pixel_spacing.is_none() {
            warnings.push("the series carries no pixel spacing: 1 mm per pixel is assumed".into());
        }
        Ok(LoadedSeries {
            volume,
            metadata: SeriesMetadata {
                modality: first.modality.clone(),
                description: first.series_description.clone(),
                default_window: first.window,
                attributes,
                study,
                source: series_folder(&series.sources),
                warnings,
            },
        })
    }
}

/// Separator between a series id and the key of one of its volumes.
pub const VOLUME_SEPARATOR: &str = "#v=";

/// `(series id, volume key)` of a descriptor id.
fn split_volume_id(id: &str) -> (&str, Option<&str>) {
    match id.rsplit_once(VOLUME_SEPARATOR) {
        Some((series, key)) => (series, Some(key)),
        None => (id, None),
    }
}

/// The descriptor of volume `v` (number `index` of `count`) of `series`.
fn volume_descriptor(
    series: &SeriesDescriptor,
    headers: &[SliceHeader],
    v: &volumes::VolumeSpec,
    index: u32,
    count: u32,
) -> SeriesDescriptor {
    let mut sources: Vec<PathBuf> = Vec::new();
    for p in &v.members {
        let path = &headers[p.header].path;
        if !sources.contains(path) {
            sources.push(path.clone());
        }
    }
    let planes = v.members.len() as u32;
    let k = &v.stack;
    SeriesDescriptor {
        id: format!("{}{VOLUME_SEPARATOR}{}", series.id, v.key),
        description: format!("{} — {}", v.label, series.description),
        dims: Dims3::new(series.dims.x, series.dims.y, planes),
        sources,
        volume: Some(VolumeInfo {
            group: series.id.clone(),
            index,
            count,
            label: v.label.clone(),
            b_value: k.b_value,
            echo_time_ms: k.echo_time,
            echo_number: k.echo_number,
            temporal_position: k.temporal_position,
            frame_time_s: k.frame_reference_time.map(|t| t / 1000.0),
            complete: v.complete,
        }),
        ..series.clone()
    }
}

/// The folder holding a series: the deepest directory shared by all its
/// files (the file itself for a single-file series).
fn series_folder(sources: &[PathBuf]) -> PathBuf {
    let Some(first) = sources.first() else {
        return PathBuf::new();
    };
    if sources.len() == 1 {
        return first.clone();
    }
    let mut common = first.parent().map(PathBuf::from).unwrap_or_default();
    while !sources.iter().all(|p| p.starts_with(&common)) && common.pop() {}
    common
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
