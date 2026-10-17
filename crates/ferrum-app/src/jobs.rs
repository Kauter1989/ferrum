//! Background jobs (scanning, loading, ambient occlusion, filters) running
//! on worker threads and reporting back through a channel, so the UI thread
//! never blocks.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ferrum_domain::{
    grow_region_robust, CancelFlag, LabelMap, LoadedSeries, MaskRegion, ProgressSink, RegionError, RegionParams,
    RepositoryError, SeriesDescriptor, Volume, VolumeRepository, VoxelBox,
};
use ferrum_processing::ambient_occlusion::AoParams;
use ferrum_processing::{filters, AmbientOcclusion};
use ferrum_radiomics::{compute, Input, MetricsError, MetricsResult, Params, Profile, ValueUnit};
use glam::UVec3;

/// Volume filters offered to the user.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterKind {
    /// Gaussian smoothing with the given sigma (voxels).
    Gaussian(f32),
    /// Sobel gradient magnitude.
    Sobel,
}

impl FilterKind {
    /// Human-readable label.
    pub fn label(&self) -> String {
        match self {
            FilterKind::Gaussian(s) => format!("Gaussian smoothing (σ = {s:.1})"),
            FilterKind::Sobel => "Sobel edges".to_string(),
        }
    }
}

/// What a region job was asked to do, handed back with its result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionRequest {
    /// Dataset revision the job ran on; a result for another revision is
    /// dropped.
    pub revision: u64,
    /// Clicked voxel.
    pub seed: UVec3,
    /// Physical value at the seed.
    pub seed_value: f32,
    /// Tolerance used, in the volume's units.
    pub tolerance: f32,
    /// Size limit used, in millilitres.
    pub max_ml: f32,
}

/// What a metrics job was asked to do, handed back with its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricsRequest {
    /// Dataset revision the job ran on.
    pub dataset_revision: u64,
    /// Revision of the segmentation the segment was taken from.
    pub segments_revision: u64,
    /// The segment.
    pub label: u8,
    /// Name of the profile.
    pub profile: &'static str,
    /// Number of the request: two requests for the same segment (say with
    /// other parameters) are never mistaken for each other.
    pub serial: u64,
}

/// A loaded series and its volumes.
#[derive(Debug)]
pub struct LoadedStudy {
    /// The series as scanned.
    pub series: SeriesDescriptor,
    /// Its volumes, when it holds several (empty otherwise).
    pub volumes: Vec<SeriesDescriptor>,
    /// Which of `volumes` was loaded.
    pub index: usize,
    /// The voxels and metadata.
    pub loaded: LoadedSeries,
}

/// Lists the volumes of `series` and loads the wanted one, else the first.
fn load_study(
    repo: &dyn VolumeRepository,
    series: &SeriesDescriptor,
    wanted: Option<&str>,
    p: &dyn ProgressSink,
) -> Result<Box<LoadedStudy>, RepositoryError> {
    let mut volumes = repo.volumes(series, p)?;
    if volumes.len() <= 1 {
        volumes.clear();
    }
    let index = wanted.and_then(|id| volumes.iter().position(|v| v.id == id)).unwrap_or(0);
    let loaded = repo.load(volumes.get(index).unwrap_or(series), p)?;
    Ok(Box::new(LoadedStudy { series: series.clone(), volumes, index, loaded }))
}

/// Events emitted by jobs.
#[derive(Debug)]
pub enum JobEvent {
    /// Progress of the current I/O job.
    Progress {
        /// Completion in `[0, 1]`.
        fraction: f32,
        /// Status text.
        message: String,
    },
    /// Result of scanning paths.
    Scanned(Result<Vec<SeriesDescriptor>, RepositoryError>),
    /// Result of loading a series (its first or chosen volume).
    Loaded(Result<Box<LoadedStudy>, RepositoryError>),
    /// Result of loading another volume of the open series.
    VolumeLoaded {
        /// Id of the volume's descriptor.
        id: String,
        /// The volume, or why it could not be read.
        result: Result<Box<LoadedSeries>, RepositoryError>,
    },
    /// Ambient occlusion computed for `threshold` on dataset `revision`.
    AmbientOcclusion {
        /// Dataset revision the AO belongs to.
        revision: u64,
        /// Threshold used.
        threshold: f32,
        /// Result.
        ao: Arc<AmbientOcclusion>,
    },
    /// A region grown from a click (see [`JobQueue::region`]).
    Region {
        /// What was asked.
        request: RegionRequest,
        /// Bounding box and mask of the region, or why there is none.
        result: Result<(VoxelBox, Vec<u8>), RegionError>,
    },
    /// Quantitative profile of a segment (see [`JobQueue::metrics`]).
    Metrics {
        /// What was asked.
        request: MetricsRequest,
        /// The features, or why there are none.
        result: Result<Box<MetricsResult>, MetricsError>,
    },
    /// Filter applied to dataset `revision`.
    Filtered {
        /// Dataset revision the filter was applied to.
        revision: u64,
        /// Filter.
        kind: FilterKind,
        /// Result.
        result: Result<Box<Volume>, String>,
    },
}

struct ChannelProgress {
    tx: Mutex<Sender<JobEvent>>,
    last: Mutex<f32>,
    cancel: Arc<CancelFlag>,
}

impl ProgressSink for ChannelProgress {
    fn report(&self, fraction: f32, message: &str) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        // Throttle to ~1 % steps to keep the channel light.
        if fraction >= 1.0 || fraction - *last >= 0.01 || fraction < *last {
            *last = fraction;
            if let Ok(tx) = self.tx.lock() {
                let _ = tx.send(JobEvent::Progress { fraction, message: message.to_string() });
            }
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.is_set()
    }
}

/// Owner of all running jobs and of the event channel.
pub struct JobQueue {
    tx: Sender<JobEvent>,
    rx: Receiver<JobEvent>,
    io_cancel: Option<Arc<CancelFlag>>,
    handles: Vec<JoinHandle<()>>,
    running_io: usize,
    running_compute: usize,
}

impl Default for JobQueue {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx, io_cancel: None, handles: Vec::new(), running_io: 0, running_compute: 0 }
    }
}

impl JobQueue {
    fn spawn_io(&mut self, f: impl FnOnce(&ChannelProgress) -> JobEvent + Send + 'static) {
        if let Some(c) = self.io_cancel.take() {
            c.cancel();
        }
        let cancel = Arc::new(CancelFlag::default());
        self.io_cancel = Some(cancel.clone());
        let tx = self.tx.clone();
        self.running_io += 1;
        self.handles.push(std::thread::spawn(move || {
            let progress = ChannelProgress { tx: Mutex::new(tx.clone()), last: Mutex::new(-1.0), cancel };
            let _ = tx.send(f(&progress));
        }));
    }

    fn spawn_compute(&mut self, f: impl FnOnce() -> JobEvent + Send + 'static) {
        let tx = self.tx.clone();
        self.running_compute += 1;
        self.handles.push(std::thread::spawn(move || {
            let _ = tx.send(f());
        }));
    }

    /// Scans `paths` for series.
    pub fn scan(&mut self, repo: Arc<dyn VolumeRepository>, paths: Vec<PathBuf>) {
        self.spawn_io(move |p| JobEvent::Scanned(repo.scan(&paths, p)));
    }

    /// Loads a series: the volume `wanted` (an id) when the series holds
    /// several and one is named, else its first volume.
    pub fn load(&mut self, repo: Arc<dyn VolumeRepository>, series: SeriesDescriptor, wanted: Option<String>) {
        self.spawn_io(move |p| JobEvent::Loaded(load_study(repo.as_ref(), &series, wanted.as_deref(), p)));
    }

    /// Loads one volume of the open series.
    pub fn load_volume(&mut self, repo: Arc<dyn VolumeRepository>, volume: SeriesDescriptor) {
        self.spawn_io(move |p| JobEvent::VolumeLoaded {
            id: volume.id.clone(),
            result: repo.load(&volume, p).map(Box::new),
        });
    }

    /// Computes ambient occlusion.
    pub fn ambient_occlusion(&mut self, volume: Arc<Volume>, revision: u64, threshold: f32) {
        self.spawn_compute(move || {
            let ao = AmbientOcclusion::compute(&volume, AoParams { threshold, ..AoParams::default() });
            JobEvent::AmbientOcclusion { revision, threshold, ao: Arc::new(ao) }
        });
    }

    /// Applies a filter.
    pub fn filter(&mut self, volume: Arc<Volume>, revision: u64, kind: FilterKind) {
        self.spawn_compute(move || {
            let result = match kind {
                FilterKind::Gaussian(s) => filters::gaussian_smooth(&volume, s),
                FilterKind::Sobel => filters::sobel_magnitude(&volume),
            };
            JobEvent::Filtered { revision, kind, result: result.map(Box::new).map_err(|e| e.to_string()) }
        });
    }

    /// Grows a region on a worker thread (10–25 s on the finest studies).
    /// `labels` is a snapshot taken at the click; `cancel` is checked while
    /// growing.
    pub fn region(
        &mut self,
        volume: Arc<Volume>,
        labels: LabelMap,
        request: RegionRequest,
        params: RegionParams,
        cancel: Arc<CancelFlag>,
    ) {
        self.spawn_compute(move || {
            let result = grow_region_robust(&volume, &labels, request.seed, &params, &cancel);
            JobEvent::Region { request, result }
        });
    }

    /// Measures a segment on a worker thread. `region` is a snapshot of the
    /// segment's voxels.
    pub fn metrics(
        &mut self,
        volume: Arc<Volume>,
        region: MaskRegion,
        unit: ValueUnit,
        profile: &'static Profile,
        params: Params,
        request: MetricsRequest,
    ) {
        self.spawn_compute(move || {
            let input = Input { region: &region, volume: &volume, unit };
            let result = compute(&input, profile, &params).map(Box::new);
            JobEvent::Metrics { request, result }
        });
    }

    /// Requests cancellation of the running I/O job.
    pub fn cancel_io(&mut self) {
        if let Some(c) = &self.io_cancel {
            c.cancel();
        }
    }

    /// `true` while an I/O job runs.
    pub fn io_busy(&self) -> bool {
        self.running_io > 0
    }

    /// `true` while a compute job runs.
    pub fn compute_busy(&self) -> bool {
        self.running_compute > 0
    }

    fn account(&mut self, e: &JobEvent) {
        match e {
            JobEvent::Scanned(_) | JobEvent::Loaded(_) | JobEvent::VolumeLoaded { .. } => {
                self.running_io = self.running_io.saturating_sub(1)
            }
            JobEvent::AmbientOcclusion { .. }
            | JobEvent::Filtered { .. }
            | JobEvent::Region { .. }
            | JobEvent::Metrics { .. } => self.running_compute = self.running_compute.saturating_sub(1),
            JobEvent::Progress { .. } => {}
        }
        self.handles.retain(|h| !h.is_finished());
    }

    /// Returns all events that are ready, without blocking.
    pub fn poll(&mut self) -> Vec<JobEvent> {
        let events: Vec<JobEvent> = self.rx.try_iter().collect();
        events.iter().for_each(|e| self.account(e));
        events
    }

    /// Blocks until the next event (tests and headless tools).
    pub fn wait(&mut self) -> Option<JobEvent> {
        if self.running_io + self.running_compute == 0 {
            return None;
        }
        let e = self.rx.recv().ok()?;
        self.account(&e);
        Some(e)
    }
}
