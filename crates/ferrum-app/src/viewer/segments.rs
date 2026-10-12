//! Segmentation use cases of the [`Viewer`]: the segment list, label-map
//! import, mask edits (used by segmentation engines) and undo.

use std::sync::Arc;

use ferrum_domain::{
    CancelFlag, LabelMap, Provenance, RegionError, ReviewStatus, Segment, SegmentStyle, SegmentationError,
    SegmentationSet, Timestamp, VoxelBox,
};

use super::Viewer;
use crate::jobs::RegionRequest;

/// Segmentation of the loaded dataset.
#[derive(Debug, Clone)]
pub struct SegmentationState {
    pub(super) set: Option<SegmentationSet>,
    pub(super) generation: u64,
    /// Draw segments in the 2D and 3D views.
    pub show: bool,
    /// How segments are drawn on slices.
    pub style: SegmentStyle,
    /// Fill opacity on slices, multiplied with each segment's opacity.
    pub fill_opacity: f32,
    /// Settings of the region tool.
    pub region: RegionSettings,
    /// Regions created so far (for their default names).
    regions: u32,
    /// Cancel flag of the region job that is running, if any.
    region_job: Option<Arc<CancelFlag>>,
    /// Outcome of the latest finished region job (for [`Viewer::grow_region_at`]).
    last_region: Option<Result<u8, String>>,
}

/// Settings of the region tool ([`ToolKind::Region`](crate::ToolKind::Region)):
/// a click grows the connected region of values within `tolerance` of the
/// clicked voxel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionSettings {
    /// Accepted distance from the seed value, in the volume's units (HU
    /// for CT).
    pub tolerance: f32,
    /// Largest region in millilitres; a larger one has leaked into
    /// neighbouring tissue and is not created.
    pub max_ml: f32,
    /// Half-width in millimetres of the smoothing applied before values
    /// are compared (suppresses noise; `0` compares raw voxels).
    pub smoothing_mm: f32,
    /// Radius in millimetres of the opening that cuts thin bridges to
    /// neighbouring structures (`0` keeps them).
    pub opening_mm: f32,
    /// Fill vessels and other cavities enclosed by the region.
    pub fill_holes: bool,
}

impl Default for RegionSettings {
    fn default() -> Self {
        Self { tolerance: 50.0, max_ml: 8000.0, smoothing_mm: 1.5, opening_mm: 5.0, fill_holes: true }
    }
}

impl RegionSettings {
    /// Defaults for a display window: a tolerance of a tenth of its width,
    /// so the region follows what looks alike on screen, but at most
    /// [`RegionSettings::MAX_DEFAULT_TOLERANCE`]: the automatic window of a
    /// whole CT is over 1 000 wide, and a tolerance that large leaks into
    /// every soft tissue on the first click.
    pub fn for_window(window: ferrum_domain::WindowLevel) -> Self {
        Self { tolerance: (window.width * 0.1).clamp(f32::EPSILON, Self::MAX_DEFAULT_TOLERANCE), ..Self::default() }
    }

    /// Largest tolerance a display window sets by default, in the volume's
    /// units (HU for CT; soft tissues differ by 10–40 HU).
    pub const MAX_DEFAULT_TOLERANCE: f32 = 40.0;
}

impl Default for SegmentationState {
    fn default() -> Self {
        Self {
            set: None,
            generation: 0,
            show: false,
            style: SegmentStyle::default(),
            fill_opacity: 1.0,
            region: RegionSettings::default(),
            region_job: None,
            last_region: None,
            regions: 0,
        }
    }
}

impl SegmentationState {
    /// The segmentation, if any segment was created or imported.
    pub fn set(&self) -> Option<&SegmentationSet> {
        self.set.as_ref()
    }

    /// `true` if the overlay has something to draw.
    pub fn overlay_active(&self) -> bool {
        self.show && self.set.as_ref().is_some_and(|s| s.segments().iter().any(|g| g.visible && g.opacity > 0.0))
    }
}

/// A row of the segment list.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentSummary {
    /// The segment.
    pub segment: Segment,
    /// Number of labelled voxels.
    pub voxels: u64,
    /// Volume in millilitres.
    pub volume_ml: f64,
}

impl Viewer {
    /// Segmentation state.
    pub fn segmentation(&self) -> &SegmentationState {
        &self.segments
    }

    /// Shows or hides the segment overlay.
    pub fn set_segments_shown(&mut self, show: bool) {
        self.segments.show = show;
    }

    /// Chooses how segments are drawn on slices.
    pub fn set_segment_style(&mut self, style: SegmentStyle) {
        self.segments.style = style;
    }

    /// Sets the fill opacity on slices (`[0, 1]`, multiplied with each
    /// segment's opacity).
    pub fn set_segment_fill_opacity(&mut self, opacity: f32) {
        self.segments.fill_opacity = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 1.0 };
    }

    /// Segment list with voxel counts and volumes.
    pub fn segment_summaries(&self) -> Vec<SegmentSummary> {
        let (Some(set), Some(d)) = (self.segments.set.as_ref(), self.dataset.as_ref()) else {
            return Vec::new();
        };
        let spacing = d.volume.spacing();
        set.segments()
            .iter()
            .map(|s| SegmentSummary {
                segment: s.clone(),
                voxels: set.voxel_count(s.label),
                volume_ml: set.volume_ml(s.label, spacing),
            })
            .collect()
    }

    fn segmentation_mut(&mut self) -> Option<&mut SegmentationSet> {
        let dims = self.dataset.as_ref()?.volume.dims();
        if self.segments.set.is_none() {
            self.segments.set = Some(SegmentationSet::new(dims));
            self.segments.generation = self.bump_revision();
        }
        self.segments.set.as_mut()
    }

    /// Adds an empty segment and shows the overlay. Returns its label.
    pub fn add_segment(&mut self, name: &str) -> Result<u8, SegmentationError> {
        let set = self.segmentation_mut().ok_or(SegmentationError::NoVolume)?;
        let label = set.add_segment(name)?;
        self.segments.show = true;
        Ok(label)
    }

    fn with_set<T>(
        &mut self,
        f: impl FnOnce(&mut SegmentationSet) -> Result<T, SegmentationError>,
    ) -> Result<T, SegmentationError> {
        let set = self.segments.set.as_mut().ok_or(SegmentationError::NoVolume)?;
        f(set)
    }

    /// Renames a segment; blank names are ignored (returns `false`).
    pub fn rename_segment(&mut self, label: u8, name: &str) -> bool {
        self.segments.set.as_mut().is_some_and(|s| s.rename(label, name))
    }

    /// Removes a segment and its voxels.
    pub fn remove_segment(&mut self, label: u8) -> Result<(), SegmentationError> {
        self.with_set(|s| s.remove_segment(label))
    }

    /// Shows or hides one segment.
    pub fn set_segment_visible(&mut self, label: u8, visible: bool) -> Result<(), SegmentationError> {
        self.with_set(|s| s.set_visible(label, visible))
    }

    /// Sets the overlay opacity of one segment.
    pub fn set_segment_opacity(&mut self, label: u8, opacity: f32) -> Result<(), SegmentationError> {
        self.with_set(|s| s.set_opacity(label, opacity))
    }

    /// Replaces the provenance of one segment.
    pub fn set_segment_provenance(&mut self, label: u8, provenance: Provenance) -> Result<(), SegmentationError> {
        self.with_set(|s| s.set_provenance(label, provenance))
    }

    /// Confirms, rejects or reopens one segment; `by` names the reviewer.
    pub fn review_segment(
        &mut self,
        label: u8,
        status: ReviewStatus,
        by: Option<&str>,
    ) -> Result<(), SegmentationError> {
        self.with_set(|s| s.review(label, status, by, Timestamp::now()))
    }

    /// Sets the colour of one segment.
    pub fn set_segment_color(&mut self, label: u8, color: [u8; 3]) -> Result<(), SegmentationError> {
        self.with_set(|s| s.set_color(label, color))
    }

    /// Writes a binary mask over `bx` into segment `label` (see
    /// [`SegmentationSet::apply_mask`]). Returns the number of changed voxels.
    pub fn apply_segment_mask(
        &mut self,
        label: u8,
        bx: VoxelBox,
        mask: &[u8],
        overwrite: bool,
    ) -> Result<usize, SegmentationError> {
        self.with_set(|s| s.apply_mask(label, bx, mask, overwrite).map(|e| e.len()))
    }

    /// `true` if a segmentation edit can be undone.
    pub fn can_undo_segmentation(&self) -> bool {
        self.segments.set.as_ref().is_some_and(SegmentationSet::can_undo)
    }

    /// Undoes the latest segmentation edit.
    pub fn undo_segmentation(&mut self) -> bool {
        self.segments.set.as_mut().is_some_and(SegmentationSet::undo)
    }

    /// Replaces the segmentation with an imported label map (one segment per
    /// label value present). Returns the number of segments.
    pub fn import_label_map(&mut self, labels: LabelMap) -> Result<usize, SegmentationError> {
        let d = self.dataset.as_ref().ok_or(SegmentationError::NoVolume)?;
        if labels.dims() != d.volume.dims() {
            return Err(SegmentationError::DimsMismatch { expected: d.volume.dims(), actual: labels.dims() });
        }
        let set = SegmentationSet::from_labels(labels, Vec::new());
        let n = set.segments().len();
        self.segments.set = Some(set);
        self.segments.generation = self.bump_revision();
        self.segments.show = true;
        self.status.message = format!("Imported {n} segment(s)");
        Ok(n)
    }

    /// Settings of the region tool, for the UI to edit.
    pub fn region_settings_mut(&mut self) -> &mut RegionSettings {
        &mut self.segments.region
    }

    /// `true` while a region is growing in the background.
    pub fn region_pending(&self) -> bool {
        self.segments.region_job.is_some()
    }

    /// Stops the region that is growing in the background; no segment is
    /// created from it, even if it had just finished.
    pub fn cancel_region(&mut self) {
        if let Some(cancel) = &self.segments.region_job {
            cancel.cancel();
            self.status.message = "Cancelling the region…".into();
        }
    }

    /// Starts growing a region from `seed` with the region settings on a
    /// worker thread; the segment appears when the job has finished (see
    /// [`Viewer::poll`]). Fails at once, with a message, when nothing can
    /// grow: segmentation not possible in this view, a region is already
    /// growing, or the seed is outside the volume or inside a segment.
    pub fn start_region_at(&mut self, seed: glam::UVec3) -> Result<(), String> {
        self.can_segment()?;
        if self.region_pending() {
            return Err("A region is still growing: wait for it or cancel it".into());
        }
        let d = self.dataset.as_ref().ok_or("Open a study first")?;
        let (volume, revision) = (d.volume.clone(), d.revision);
        let seed_value = volume.physical(seed.x, seed.y, seed.z).ok_or("The click is outside the volume")?;
        if let Some(name) = self.segments.set.as_ref().and_then(|s| {
            let label = s.labels().label(seed.x, seed.y, seed.z).filter(|l| *l != 0)?;
            s.segment(label).map(|g| g.name.clone())
        }) {
            return Err(format!("This voxel already belongs to {name}"));
        }
        let RegionSettings { tolerance, max_ml, smoothing_mm, opening_mm, fill_holes } = self.segments.region;
        let sp = volume.spacing();
        let voxel_ml = f64::from(sp.x) * f64::from(sp.y) * f64::from(sp.z) / 1000.0;
        let max_voxels = (f64::from(max_ml) / voxel_ml).floor().max(1.0) as u64;
        let labels = self.segmentation_mut().ok_or("Open a study first")?.labels().clone();
        let params = ferrum_domain::RegionParams { tolerance, max_voxels, smoothing_mm, opening_mm, fill_holes };
        let cancel = Arc::new(CancelFlag::default());
        let request = RegionRequest { revision, seed, seed_value, tolerance, max_ml };
        self.jobs.region(volume, labels, request, params, cancel.clone());
        self.segments.region_job = Some(cancel);
        self.status.message = "Growing the region…".into();
        Ok(())
    }

    /// Grows a region from `seed` like [`Viewer::start_region_at`] and
    /// waits for it (tests and embedders without an event loop). Returns
    /// its label, or a message saying why nothing was created.
    pub fn grow_region_at(&mut self, seed: glam::UVec3) -> Result<u8, String> {
        self.start_region_at(seed)?;
        self.segments.last_region = None;
        while self.region_pending() {
            let Some(event) = self.jobs.wait() else {
                break;
            };
            self.handle_event(event);
        }
        self.segments.last_region.take().unwrap_or_else(|| Err("The region was not created".into()))
    }

    /// Applies a finished region job: a new segment drawn by the user. A
    /// result is dropped when it was cancelled or the study changed
    /// meanwhile; voxels labelled since the click are not overwritten.
    pub(super) fn finish_region(&mut self, request: RegionRequest, result: Result<(VoxelBox, Vec<u8>), RegionError>) {
        let cancelled = self.segments.region_job.take().is_none_or(|c| c.is_set());
        let outcome = if cancelled || result == Err(RegionError::Cancelled) {
            Err("Region growing cancelled".to_string())
        } else if self.dataset.as_ref().is_none_or(|d| d.revision != request.revision) {
            Err("The study changed: the region was dropped".to_string())
        } else {
            result
                .map_err(|e| region_error_message(e, &request))
                .and_then(|(bx, mask)| self.add_region(&request, bx, &mask))
        };
        if let Err(message) = &outcome {
            self.status.message = message.clone();
        }
        self.segments.last_region = Some(outcome);
    }

    fn add_region(&mut self, request: &RegionRequest, bx: VoxelBox, mask: &[u8]) -> Result<u8, String> {
        self.segments.regions += 1;
        let label = self.add_segment(&format!("Region {}", self.segments.regions)).map_err(|e| e.to_string())?;
        let provenance = Provenance::human(Timestamp::now());
        self.with_set(|s| {
            s.set_provenance(label, provenance)?;
            s.apply_mask(label, bx, mask, false)
        })
        .map_err(|e| e.to_string())?;
        let sp = self.dataset.as_ref().map_or(glam::Vec3::ONE, |d| d.volume.spacing());
        let ml = self.segments.set.as_ref().map_or(0.0, |s| s.volume_ml(label, sp));
        self.status.message = format!(
            "Region {}: {ml:.1} ml, values {:.0} ± {:.0}",
            self.segments.regions, request.seed_value, request.tolerance
        );
        Ok(label)
    }

    /// Removes every segment.
    pub fn clear_segmentation(&mut self) {
        if let Some(cancel) = &self.segments.region_job {
            cancel.cancel();
        }
        self.segments.regions = 0;
        self.segments.set = None;
        self.segments.generation = self.bump_revision();
    }
}

/// Why a region job produced nothing, in words for the status line.
fn region_error_message(e: RegionError, request: &RegionRequest) -> String {
    match e {
        RegionError::TooLarge => format!(
            "The region grows beyond {:.0} ml: lower the tolerance (now ±{:.0}), raise the opening or click further \
             from the edge",
            request.max_ml, request.tolerance
        ),
        RegionError::Labelled => "This voxel already belongs to a segment".into(),
        RegionError::OutOfGrid => "The click is outside the volume".into(),
        RegionError::Cancelled => "Region growing cancelled".into(),
    }
}
