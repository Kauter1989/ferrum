//! Segmentation use cases of the [`Viewer`]: the segment list, label-map
//! import, mask edits (used by segmentation engines) and undo.

use ferrum_domain::{
    LabelMap, Provenance, ReviewStatus, Segment, SegmentationError, SegmentationSet, Timestamp, VoxelBox,
};

use super::Viewer;

/// Segmentation of the loaded dataset.
#[derive(Debug, Clone, Default)]
pub struct SegmentationState {
    pub(super) set: Option<SegmentationSet>,
    pub(super) generation: u64,
    /// Draw segments in the 2D and 3D views.
    pub show: bool,
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

    /// Removes every segment.
    pub fn clear_segmentation(&mut self) {
        self.segments.set = None;
        self.segments.generation = self.bump_revision();
    }
}
