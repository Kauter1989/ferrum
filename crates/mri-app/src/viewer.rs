//! Application facade: the complete viewer state and its use cases.

use std::path::PathBuf;
use std::sync::Arc;

use glam::{UVec3, Vec2, Vec3};
use mri_domain::{
    Annotation, AnnotationId, AnnotationSet, ClipSettings, Dims3, EraserBrush, LoadedSeries, MaskHistory, OrbitCamera,
    RenderMode, RenderSettings, Rgba8, SeriesDescriptor, SliceAxis, SliceKey, SliceView, TransferFunction, Volume,
    VolumeRepository, VoxelMask, WindowLevel, WindowPreset,
};
use mri_processing::AmbientOcclusion;
use mri_render::cpu::{CpuRaycaster, CpuScene};
use mri_render::{FrameParams, SliceParams};

use crate::dataset::{Dataset, BRICK_SIZE};
use crate::jobs::{FilterKind, JobEvent, JobQueue};
use crate::tools::{InputKind, ProbeReading, SliceContext, ToolController, ToolInput, ToolKind, ToolOutcome};

/// Layout of the main area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// One 2D slice view.
    #[default]
    Slice2d,
    /// One 3D view.
    Volume3d,
    /// Axial, coronal, sagittal and 3D views in a 2×2 grid.
    Mpr,
}

impl ViewMode {
    /// All modes.
    pub const ALL: [ViewMode; 3] = [ViewMode::Slice2d, ViewMode::Volume3d, ViewMode::Mpr];

    /// Label.
    pub fn label(&self) -> &'static str {
        match self {
            ViewMode::Slice2d => "2D",
            ViewMode::Volume3d => "3D",
            ViewMode::Mpr => "MPR",
        }
    }
}

/// State of the 2D slice views.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceState {
    /// Orientation shown in [`ViewMode::Slice2d`].
    pub axis: SliceAxis,
    /// Current slice per orientation (indexed by [`SliceAxis::normal_axis`]).
    pub indices: [u32; 3],
    /// Pan/zoom per orientation.
    pub views: [SliceView; 3],
    /// Display window (physical units).
    pub window: WindowLevel,
    /// Nearest-neighbour instead of linear sampling.
    pub nearest: bool,
}

impl Default for SliceState {
    fn default() -> Self {
        Self {
            axis: SliceAxis::Axial,
            indices: [0; 3],
            views: [SliceView::default(); 3],
            window: WindowLevel::new(0.5, 1.0),
            nearest: false,
        }
    }
}

impl SliceState {
    /// Current slice index of `axis`.
    pub fn index(&self, axis: SliceAxis) -> u32 {
        self.indices[axis.normal_axis()]
    }
}

/// State of the 3D view.
#[derive(Debug, Clone)]
pub struct VolumeViewState {
    /// Rendering parameters (freely editable by the UI).
    pub settings: RenderSettings,
    /// Camera (freely editable by the UI).
    pub camera: OrbitCamera,
    /// Clipping (freely editable by the UI).
    pub clip: ClipSettings,
    /// Eraser brush.
    pub brush: EraserBrush,
    /// Eraser mode: clicks erase instead of rotating.
    pub eraser_enabled: bool,
    /// Render-resolution factor used while the user interacts.
    pub interactive_scale: f32,
    /// Set by the UI while the camera is being dragged.
    pub interacting: bool,
    tf: TransferFunction,
    tf_lut: Vec<Rgba8>,
    tf_revision: u64,
    mask: Option<VoxelMask>,
    mask_generation: u64,
    history: MaskHistory,
    ao: Option<(u64, f32, Arc<AmbientOcclusion>)>,
    ao_requested: Option<(u64, f32)>,
}

impl Default for VolumeViewState {
    fn default() -> Self {
        let tf = TransferFunction::legacy_default();
        Self {
            settings: RenderSettings::default(),
            camera: OrbitCamera::default(),
            clip: ClipSettings::default(),
            brush: EraserBrush::default(),
            eraser_enabled: false,
            interactive_scale: 0.5,
            interacting: false,
            tf_lut: tf.bake(TransferFunction::LUT_SIZE),
            tf,
            tf_revision: 0,
            mask: None,
            mask_generation: 0,
            history: MaskHistory::default(),
            ao: None,
            ao_requested: None,
        }
    }
}

impl VolumeViewState {
    /// Current transfer function.
    pub fn transfer_function(&self) -> &TransferFunction {
        &self.tf
    }

    /// Resolution factor for the next frame.
    pub fn render_scale(&self) -> f32 {
        if self.interacting {
            self.interactive_scale.clamp(0.1, 1.0)
        } else {
            1.0
        }
    }

    /// Threshold for which AO must be computed in the current mode, if any.
    fn ao_threshold(&self) -> Option<f32> {
        if !self.settings.ambient_occlusion {
            return None;
        }
        match self.settings.mode {
            RenderMode::Isosurface => Some(self.settings.iso_threshold),
            RenderMode::Tissue => Some(self.settings.tissue.surface),
            _ => None,
        }
    }
}

/// Status bar model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// Last status message.
    pub message: String,
    /// Progress of the running I/O job.
    pub progress: Option<f32>,
    /// Errors not yet acknowledged by the user.
    pub errors: Vec<String>,
}

/// Port through which the viewer pushes data to the GPU (implemented by the
/// presentation layer on top of `mri_render::gpu::VolumeRenderer`).
pub trait GpuSink {
    /// Replaces the volume texture (resets mask, AO and occupancy).
    fn upload_volume(&mut self, volume: &Volume);
    /// Replaces the transfer function lookup table.
    fn upload_transfer_function(&mut self, lut: &[Rgba8]);
    /// Replaces the brick occupancy map.
    fn upload_occupancy(&mut self, grid: Dims3, occupancy: &[u8]);
    /// Uploads (part of) the eraser mask.
    fn upload_mask(&mut self, mask: &VoxelMask, dirty: Option<(UVec3, UVec3)>);
    /// Removes the eraser mask.
    fn clear_mask(&mut self);
    /// Sets or clears the ambient occlusion volume.
    fn upload_ambient_occlusion(&mut self, ao: Option<&AmbientOcclusion>);
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OccupancyKey {
    revision: u64,
    mode: RenderMode,
    iso: u32,
    low: u32,
    tf: u64,
}

/// What the GPU currently holds; owned by the presentation layer.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GpuSyncState {
    volume: Option<u64>,
    tf: Option<u64>,
    occupancy: Option<OccupancyKey>,
    mask: Option<(u64, Option<u64>)>,
    ao: Option<Option<(u64, u32)>>,
}

/// The viewer application service.
pub struct Viewer {
    repo: Arc<dyn VolumeRepository>,
    jobs: JobQueue,
    dataset: Option<Dataset>,
    next_revision: u64,
    /// Active layout.
    pub view_mode: ViewMode,
    /// 2D state.
    pub slices: SliceState,
    /// 3D state.
    pub volume: VolumeViewState,
    /// Active 2D tool.
    pub tool: ToolKind,
    tool_ctl: ToolController,
    annotations: AnnotationSet,
    /// Latest probe reading.
    pub probe: Option<ProbeReading>,
    /// Status bar.
    pub status: Status,
    /// Series found by the last scan when more than one is available; the
    /// UI lets the user pick one and calls [`Viewer::load_series`].
    pub series_choice: Option<Vec<SeriesDescriptor>>,
    /// Pending text annotation position (UI shows an input box).
    pub pending_text: Option<(SliceKey, Vec2)>,
}

impl Viewer {
    /// Creates a viewer backed by `repo`.
    pub fn new(repo: Arc<dyn VolumeRepository>) -> Self {
        Self {
            repo,
            jobs: JobQueue::default(),
            dataset: None,
            next_revision: 1,
            view_mode: ViewMode::default(),
            slices: SliceState::default(),
            volume: VolumeViewState::default(),
            tool: ToolKind::default(),
            tool_ctl: ToolController::default(),
            annotations: AnnotationSet::default(),
            probe: None,
            status: Status { message: "Open a DICOM folder or NIfTI file to start".into(), ..Status::default() },
            series_choice: None,
            pending_text: None,
        }
    }

    // ----------------------------------------------------------------- data

    /// The loaded dataset.
    pub fn dataset(&self) -> Option<&Dataset> {
        self.dataset.as_ref()
    }

    /// Starts scanning files/directories.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.status.message = format!("Scanning {} item(s)…", paths.len());
        self.status.progress = Some(0.0);
        self.series_choice = None;
        self.jobs.scan(self.repo.clone(), paths);
    }

    /// Starts loading a series.
    pub fn load_series(&mut self, series: SeriesDescriptor) {
        self.status.message = format!("Loading {}…", series.description);
        self.status.progress = Some(0.0);
        self.series_choice = None;
        self.jobs.load(self.repo.clone(), series);
    }

    /// Cancels a running scan/load.
    pub fn cancel_loading(&mut self) {
        self.jobs.cancel_io();
    }

    /// `true` while scanning or loading.
    pub fn is_loading(&self) -> bool {
        self.jobs.io_busy()
    }

    /// `true` while a compute job (AO, filter) is running.
    pub fn is_computing(&self) -> bool {
        self.jobs.compute_busy()
    }

    /// Installs a loaded series as the current dataset.
    pub fn install(&mut self, loaded: LoadedSeries) {
        let revision = self.bump_revision();
        let LoadedSeries { volume, metadata } = loaded;
        let window = metadata.default_window;
        let dataset = Dataset::new(volume, metadata, revision);
        let dims = dataset.volume.dims().as_uvec3();
        self.slices = SliceState {
            indices: [dims.x / 2, dims.y / 2, dims.z / 2],
            window: window.unwrap_or_else(|| Self::auto_window_for(&dataset)),
            ..SliceState::default()
        };
        let settings = self.volume.settings;
        self.volume =
            VolumeViewState { settings, tf_revision: self.volume.tf_revision + 1, ..VolumeViewState::default() };
        self.volume.mask_generation = self.bump_revision();
        self.annotations.clear();
        self.tool_ctl.cancel();
        self.probe = None;
        self.status.message = format!(
            "Loaded {} ({}×{}×{}, {:.2}×{:.2}×{:.2} mm)",
            if dataset.metadata.description.is_empty() { "volume" } else { &dataset.metadata.description },
            dims.x,
            dims.y,
            dims.z,
            dataset.volume.spacing().x,
            dataset.volume.spacing().y,
            dataset.volume.spacing().z,
        );
        self.dataset = Some(dataset);
    }

    fn bump_revision(&mut self) -> u64 {
        self.next_revision += 1;
        self.next_revision
    }

    fn auto_window_for(d: &Dataset) -> WindowLevel {
        let lo = d.histogram.percentile(0.005);
        let hi = d.histogram.percentile(0.995).max(lo + 1e-3);
        let r = d.volume.range();
        let (a, b) = (r.denormalize(lo), r.denormalize(hi));
        WindowLevel::new((a + b) * 0.5, b - a)
    }

    /// Processes finished background work. Call once per frame.
    pub fn poll(&mut self) {
        for event in self.jobs.poll() {
            self.handle_event(event);
        }
        self.request_ambient_occlusion();
    }

    /// Blocks until all background jobs finished (tests, batch tools).
    pub fn wait_idle(&mut self) {
        while let Some(e) = self.jobs.wait() {
            self.handle_event(e);
            self.request_ambient_occlusion();
        }
    }

    fn handle_event(&mut self, event: JobEvent) {
        match event {
            JobEvent::Progress { fraction, message } => {
                self.status.progress = Some(fraction);
                self.status.message = message;
            }
            JobEvent::Scanned(Ok(mut series)) => {
                self.status.progress = None;
                if series.len() == 1 {
                    let s = series.remove(0);
                    self.load_series(s);
                } else {
                    self.status.message = format!("Found {} series — choose one", series.len());
                    self.series_choice = Some(series);
                }
            }
            JobEvent::Loaded(Ok(loaded)) => {
                self.status.progress = None;
                self.install(*loaded);
            }
            JobEvent::Scanned(Err(e)) | JobEvent::Loaded(Err(e)) => {
                self.status.progress = None;
                self.status.message = format!("Loading failed: {e}");
                self.status.errors.push(e.to_string());
            }
            JobEvent::AmbientOcclusion { revision, threshold, ao } => {
                if self.dataset.as_ref().is_some_and(|d| d.revision == revision) {
                    self.volume.ao = Some((revision, threshold, ao));
                }
            }
            JobEvent::Filtered { revision, kind, result } => match result {
                Ok(v) if self.dataset.as_ref().is_some_and(|d| d.revision == revision) => {
                    let rev = self.bump_revision();
                    if let Some(d) = self.dataset.take() {
                        self.dataset = Some(Dataset::new(*v, d.metadata, rev));
                    }
                    self.volume.ao = None;
                    self.status.message = format!("Applied {}", kind.label());
                }
                Ok(_) => {}
                Err(e) => self.status.errors.push(format!("{}: {e}", kind.label())),
            },
        }
    }

    fn request_ambient_occlusion(&mut self) {
        let (Some(d), Some(thr)) = (&self.dataset, self.volume.ao_threshold()) else {
            return;
        };
        let fresh = self.volume.ao.as_ref().is_some_and(|(r, t, _)| *r == d.revision && (t - thr).abs() < 0.01);
        let requested = self.volume.ao_requested.is_some_and(|(r, t)| r == d.revision && (t - thr).abs() < 0.01);
        if !fresh && !requested && !self.jobs.compute_busy() {
            self.volume.ao_requested = Some((d.revision, thr));
            self.jobs.ambient_occlusion(d.volume.clone(), d.revision, thr);
        }
    }

    /// Applies a filter in the background; the result replaces the dataset.
    pub fn apply_filter(&mut self, kind: FilterKind) {
        if let Some(d) = &self.dataset {
            self.status.message = format!("Applying {}…", kind.label());
            self.jobs.filter(d.volume.clone(), d.revision, kind);
        }
    }

    // ------------------------------------------------------------------ 2D

    /// Sets the slice index of `axis` (clamped).
    pub fn set_slice_index(&mut self, axis: SliceAxis, index: u32) {
        if let Some(d) = &self.dataset {
            let n = axis.slice_count(&d.volume);
            self.slices.indices[axis.normal_axis()] = index.min(n.saturating_sub(1));
            self.tool_ctl.cancel();
        }
    }

    /// Moves the slice of `axis` by `delta`.
    pub fn step_slice(&mut self, axis: SliceAxis, delta: i32) {
        let cur = i64::from(self.slices.index(axis));
        self.set_slice_index(axis, (cur + i64::from(delta)).max(0) as u32);
    }

    /// Applies a window preset.
    pub fn apply_window_preset(&mut self, preset: WindowPreset) {
        if let Some(d) = &self.dataset {
            self.slices.window = preset.window(d.volume.range());
        }
    }

    /// Sets the window from the 0.5–99.5 % histogram percentiles.
    pub fn auto_window(&mut self) {
        if let Some(d) = &self.dataset {
            self.slices.window = Self::auto_window_for(d);
        }
    }

    /// Annotations of all slices.
    pub fn annotations(&self) -> &AnnotationSet {
        &self.annotations
    }

    /// Annotation currently being drawn.
    pub fn annotation_preview(&self) -> Option<Annotation> {
        self.tool_ctl.preview(self.tool)
    }

    /// Selects a 2D tool (cancelling any drawing in progress).
    pub fn select_tool(&mut self, tool: ToolKind) {
        self.tool = tool;
        self.tool_ctl.cancel();
    }

    /// Removes all annotations.
    pub fn clear_annotations(&mut self) {
        self.annotations.clear();
        self.tool_ctl.cancel();
    }

    /// Forwards pointer input on the slice view of `axis` to the active
    /// tool. `viewport` is the view size in points.
    pub fn slice_input(&mut self, axis: SliceAxis, kind: InputKind, screen: Vec2, viewport: Vec2) -> ToolOutcome {
        let Some(d) = &self.dataset else {
            return ToolOutcome::None;
        };
        let volume = d.volume.clone();
        let index = self.slices.index(axis);
        let key = SliceKey::new(axis, index);
        let image_mm = axis.plane_size_mm(&volume);
        let view = &mut self.slices.views[axis.normal_axis()];
        let mm = view.screen_to_mm(screen, viewport, image_mm);
        let tolerance_mm = 6.0 / view.scale(viewport, image_mm).max(1e-6);
        let mut ctx = SliceContext {
            key,
            axis,
            index,
            volume: &volume,
            view,
            viewport,
            window: &mut self.slices.window,
            annotations: &mut self.annotations,
            tolerance_mm,
        };
        let outcome = self.tool_ctl.handle(self.tool, ToolInput { kind, mm, screen }, &mut ctx);
        match &outcome {
            ToolOutcome::Probe(p) => self.probe = *p,
            ToolOutcome::RequestText(pos) => self.pending_text = Some((key, *pos)),
            _ => {}
        }
        outcome
    }

    /// Completes a pending text annotation.
    pub fn commit_text(&mut self, text: &str) -> Option<AnnotationId> {
        let (key, pos) = self.pending_text.take()?;
        self.tool_ctl.commit_text(&mut self.annotations, key, pos, text)
    }

    /// MPR navigation: moves the other two slices through the voxel under
    /// `screen` in the view of `axis`.
    pub fn navigate_to(&mut self, axis: SliceAxis, screen: Vec2, viewport: Vec2) {
        let Some(d) = &self.dataset else {
            return;
        };
        let volume = d.volume.clone();
        let image_mm = axis.plane_size_mm(&volume);
        let mm = self.slices.views[axis.normal_axis()].screen_to_mm(screen, viewport, image_mm);
        let (w, h) = axis.plane_dims(&volume);
        let uv = mm / image_mm;
        if !(0.0..1.0).contains(&uv.x) || !(0.0..1.0).contains(&uv.y) {
            return;
        }
        let px = (uv.x * w as f32) as u32;
        let py = (uv.y * h as f32) as u32;
        if let Some(v) = axis.voxel_at(&volume, self.slices.index(axis), px, py) {
            for other in SliceAxis::ALL.into_iter().filter(|a| *a != axis) {
                self.slices.indices[other.normal_axis()] = v[other.normal_axis()];
            }
        }
    }

    /// GPU parameters of the slice view of `axis` occupying `rect_px`
    /// (min, max in target pixels) with `pixels_per_point` scaling.
    pub fn slice_params(&self, axis: SliceAxis, rect_px: (Vec2, Vec2), pixels_per_point: f32) -> Option<SliceParams> {
        let d = self.dataset.as_ref()?;
        let viewport_pt = (rect_px.1 - rect_px.0) / pixels_per_point;
        let image_mm = axis.plane_size_mm(&d.volume);
        let (lo, hi) = self.slices.views[axis.normal_axis()].image_rect(viewport_pt, image_mm);
        let index = self.slices.index(axis);
        Some(SliceParams {
            rect: (rect_px.0 + lo * pixels_per_point, rect_px.0 + hi * pixels_per_point),
            window: self.slices.window.normalized_bounds(d.volume.range()),
            position: axis.slice_position(&d.volume, index),
            axis: axis.id(),
            nearest: self.slices.nearest,
            background: [0.05, 0.05, 0.06, 1.0],
        })
    }

    // ------------------------------------------------------------------ 3D

    /// Replaces the transfer function.
    pub fn set_transfer_function(&mut self, tf: TransferFunction) {
        self.volume.tf_lut = tf.bake(TransferFunction::LUT_SIZE);
        self.volume.tf = tf;
        self.volume.tf_revision += 1;
    }

    /// Edits the transfer function in place.
    pub fn edit_transfer_function(&mut self, f: impl FnOnce(&mut TransferFunction)) {
        let mut tf = self.volume.tf.clone();
        f(&mut tf);
        if tf != self.volume.tf {
            self.set_transfer_function(tf);
        }
    }

    /// Resets camera and clipping.
    pub fn reset_view_3d(&mut self) {
        self.volume.camera = OrbitCamera::default();
        self.volume.clip = ClipSettings::default();
    }

    /// Frame parameters of the 3D view with `size` pixels.
    pub fn frame_params(&self, size: Vec2) -> Option<FrameParams> {
        let d = self.dataset.as_ref()?;
        let ao = self.volume.ao.as_ref().filter(|(r, _, _)| *r == d.revision).map(|(_, _, a)| a.tex_scale);
        Some(FrameParams::new(
            &d.volume,
            &self.volume.camera,
            &self.volume.settings,
            &self.volume.clip,
            size,
            BRICK_SIZE,
            ao,
        ))
    }

    /// Eraser mask, if the eraser has been used.
    pub fn mask(&self) -> Option<&VoxelMask> {
        self.volume.mask.as_ref()
    }

    /// Number of undoable eraser strokes.
    pub fn erase_history_len(&self) -> usize {
        self.volume.history.len()
    }

    /// Picks the first visible point under normalised device coordinates
    /// `ndc` of a view with `aspect`, returning its texture coordinate.
    pub fn pick(&self, ndc: Vec2, aspect: f32) -> Option<Vec3> {
        let d = self.dataset.as_ref()?;
        let mut params = self.frame_params(Vec2::new(aspect * 100.0, 100.0))?;
        params.jitter = false;
        let scene = CpuScene {
            volume: &d.volume,
            mask: self.volume.mask.as_ref(),
            ao: None,
            lut: &self.volume.tf_lut,
            occupancy: None,
        };
        CpuRaycaster::new(scene, &params).pick(ndc)
    }

    /// Erases material under `ndc` with the current brush. Returns `true`
    /// if anything was removed.
    pub fn erase_at(&mut self, ndc: Vec2, aspect: f32) -> bool {
        let Some(hit) = self.pick(ndc, aspect) else {
            return false;
        };
        let Some(d) = &self.dataset else {
            return false;
        };
        let (dims, spacing) = (d.volume.dims(), d.volume.spacing());
        let dir = self.volume.camera.view_dir();
        let mask = self.volume.mask.get_or_insert_with(|| VoxelMask::new(dims));
        let stroke = mask.erase(hit, dir, self.volume.brush, spacing);
        let changed = !stroke.is_empty();
        self.volume.history.push(stroke);
        changed
    }

    /// Undoes the latest eraser stroke.
    pub fn undo_erase(&mut self) -> bool {
        match self.volume.mask.as_mut() {
            Some(m) => self.volume.history.undo(m),
            None => false,
        }
    }

    /// Restores every erased voxel.
    pub fn reset_mask(&mut self) {
        self.volume.mask = None;
        self.volume.history.clear();
        self.volume.mask_generation = self.bump_revision();
    }

    /// Pushes every change since the last call to the GPU.
    pub fn sync_gpu(&mut self, sync: &mut GpuSyncState, sink: &mut dyn GpuSink) {
        let Some(d) = &self.dataset else {
            return;
        };
        if sync.volume != Some(d.revision) {
            sink.upload_volume(&d.volume);
            sync.volume = Some(d.revision);
            // a new volume texture resets everything that depends on it
            sync.occupancy = None;
            sync.mask = None;
            sync.ao = None;
        }
        if sync.tf != Some(self.volume.tf_revision) {
            sink.upload_transfer_function(&self.volume.tf_lut);
            sync.tf = Some(self.volume.tf_revision);
        }
        let s = &self.volume.settings;
        let key = OccupancyKey {
            revision: d.revision,
            mode: s.mode,
            iso: s.iso_threshold.to_bits(),
            low: s.tissue.low.to_bits(),
            tf: if s.mode == RenderMode::TransferFunction { self.volume.tf_revision } else { 0 },
        };
        if sync.occupancy != Some(key) {
            let tf = &self.volume.tf;
            let occ = d.bricks.occupancy(|mn, mx| s.range_visible(mn, mx, tf));
            sink.upload_occupancy(d.bricks.grid_dims(), &occ);
            sync.occupancy = Some(key);
        }
        let generation = self.volume.mask_generation;
        match self.volume.mask.as_mut() {
            Some(m) => {
                let dirty = m.take_dirty();
                match sync.mask {
                    // Same mask already on the GPU: send only what changed.
                    Some((g, Some(rev))) if g == generation => {
                        if rev != m.revision() {
                            sink.upload_mask(m, dirty);
                        }
                    }
                    _ => sink.upload_mask(m, None),
                }
                sync.mask = Some((generation, Some(m.revision())));
            }
            None => {
                if matches!(sync.mask, Some((_, Some(_)))) {
                    sink.clear_mask();
                }
                sync.mask = Some((generation, None));
            }
        }
        let ao = self.volume.ao.as_ref().filter(|(r, _, _)| *r == d.revision && self.volume.settings.ambient_occlusion);
        let ao_key = ao.map(|(r, t, _)| (*r, t.to_bits()));
        if sync.ao != Some(ao_key) {
            sink.upload_ambient_occlusion(ao.map(|(_, _, a)| a.as_ref()));
            sync.ao = Some(ao_key);
        }
    }
}
