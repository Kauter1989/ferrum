//! Segmentation: a label map on the volume grid, the segments it refers to,
//! and undoable edits.
//!
//! A [`LabelMap`] stores one `u8` label per voxel: `0` is background and
//! `1..=255` are segments. A [`SegmentationSet`] owns the label map together
//! with the segment descriptions (name, colour, visibility, opacity), keeps
//! per-label voxel counts up to date and records edits for undo.

use glam::{UVec3, Vec3};
use thiserror::Error;

use crate::color::Rgba8;
use crate::geometry::Dims3;
use crate::provenance::{Provenance, ReviewStatus, Timestamp};

/// Errors raised by segmentation operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SegmentationError {
    /// A label map does not match the volume grid.
    #[error("label map is {actual:?}, the volume is {expected:?}")]
    DimsMismatch {
        /// Volume grid.
        expected: Dims3,
        /// Label map grid.
        actual: Dims3,
    },
    /// The label buffer length does not match the dimensions.
    #[error("label buffer has {actual} elements, expected {expected}")]
    DataLengthMismatch {
        /// Expected number of voxels.
        expected: usize,
        /// Actual buffer length.
        actual: usize,
    },
    /// A box is empty or exceeds the grid.
    #[error("box {min:?}..{max:?} is empty or outside the grid")]
    InvalidBox {
        /// Inclusive lower corner.
        min: UVec3,
        /// Exclusive upper corner.
        max: UVec3,
    },
    /// All 255 labels are in use.
    #[error("all 255 segment labels are in use")]
    NoFreeLabel,
    /// There is no volume to segment.
    #[error("no volume is loaded")]
    NoVolume,
    /// The label does not name a segment.
    #[error("no segment with label {0}")]
    UnknownSegment(u8),
}

/// Half-open voxel range `min ≤ (i, j, k) < max`, as in the engine protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoxelBox {
    /// Inclusive lower corner.
    pub min: UVec3,
    /// Exclusive upper corner.
    pub max: UVec3,
}

impl VoxelBox {
    /// Creates a box; it may be empty.
    pub const fn new(min: UVec3, max: UVec3) -> Self {
        Self { min, max }
    }

    /// The whole grid.
    pub fn full(dims: Dims3) -> Self {
        Self::new(UVec3::ZERO, dims.as_uvec3())
    }

    /// Box size per axis (zero when empty).
    pub fn size(&self) -> UVec3 {
        self.max.saturating_sub(self.min)
    }

    /// Number of voxels in the box.
    pub fn voxel_count(&self) -> usize {
        let s = self.size();
        s.x as usize * s.y as usize * s.z as usize
    }

    /// `true` if the box holds no voxel.
    pub fn is_empty(&self) -> bool {
        self.voxel_count() == 0
    }

    /// `true` if the box is non-empty and inside a grid of `dims`.
    pub fn fits(&self, dims: Dims3) -> bool {
        !self.is_empty() && self.max.cmple(dims.as_uvec3()).all()
    }

    /// Smallest box containing both (an empty box is ignored).
    pub fn union(self, other: VoxelBox) -> VoxelBox {
        match (self.is_empty(), other.is_empty()) {
            (true, _) => other,
            (_, true) => self,
            _ => VoxelBox::new(self.min.min(other.min), self.max.max(other.max)),
        }
    }
}

/// One `u8` label per voxel on the volume grid (`i` fastest).
#[derive(Debug, Clone, PartialEq)]
pub struct LabelMap {
    dims: Dims3,
    data: Vec<u8>,
}

impl LabelMap {
    /// An all-background label map.
    pub fn new(dims: Dims3) -> Self {
        Self { dims, data: vec![0; dims.voxel_count()] }
    }

    /// Wraps existing labels.
    pub fn from_data(dims: Dims3, data: Vec<u8>) -> Result<Self, SegmentationError> {
        if data.len() != dims.voxel_count() {
            return Err(SegmentationError::DataLengthMismatch { expected: dims.voxel_count(), actual: data.len() });
        }
        Ok(Self { dims, data })
    }

    /// Grid dimensions.
    pub fn dims(&self) -> Dims3 {
        self.dims
    }

    /// Raw labels (`i` fastest).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Label at `(i, j, k)`; `None` out of bounds.
    pub fn label(&self, i: u32, j: u32, k: u32) -> Option<u8> {
        self.dims.contains(i64::from(i), i64::from(j), i64::from(k)).then(|| self.data[self.dims.index(i, j, k)])
    }

    /// Labels over `bx` (`i` fastest); `None` if the box does not fit.
    pub fn extract(&self, bx: VoxelBox) -> Option<Vec<u8>> {
        if !bx.fits(self.dims) {
            return None;
        }
        let mut out = Vec::with_capacity(bx.voxel_count());
        for k in bx.min.z..bx.max.z {
            for j in bx.min.y..bx.max.y {
                let row = self.dims.index(bx.min.x, j, k);
                out.extend_from_slice(&self.data[row..row + bx.size().x as usize]);
            }
        }
        Some(out)
    }

    /// Number of voxels per label value.
    pub fn histogram(&self) -> [u64; 256] {
        let mut h = [0u64; 256];
        for &v in &self.data {
            h[usize::from(v)] += 1;
        }
        h
    }
}

/// A named, coloured segment, i.e. one label value of the label map.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Label value in the label map (`1..=255`).
    pub label: u8,
    /// Display name.
    pub name: String,
    /// Display colour (sRGB).
    pub color: [u8; 3],
    /// Shown in the views.
    pub visible: bool,
    /// Overlay opacity in `[0, 1]`.
    pub opacity: f32,
    /// Who created the segment and whether it was confirmed.
    pub provenance: Provenance,
}

impl Segment {
    /// A visible segment in the palette colour of `label` with the default
    /// opacity, drawn by a person.
    pub fn new(label: u8, name: impl Into<String>) -> Self {
        Self {
            label,
            name: name.into(),
            color: palette_color(label),
            visible: true,
            opacity: DEFAULT_OPACITY,
            provenance: Provenance::default(),
        }
    }
}

/// Default segment colours, cycled by label value.
pub const PALETTE: [[u8; 3]; 12] = [
    [230, 85, 75],
    [80, 175, 95],
    [70, 140, 225],
    [240, 190, 60],
    [175, 100, 210],
    [60, 195, 200],
    [240, 135, 50],
    [220, 110, 170],
    [150, 200, 80],
    [120, 120, 235],
    [200, 160, 120],
    [110, 210, 160],
];

/// Default colour of `label`.
pub fn palette_color(label: u8) -> [u8; 3] {
    PALETTE[usize::from(label.saturating_sub(1)) % PALETTE.len()]
}

/// Default overlay opacity of new segments.
pub const DEFAULT_OPACITY: f32 = 0.5;

/// Voxels changed by one edit, sufficient to undo it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LabelEdit {
    indices: Vec<u32>,
    previous: Vec<u8>,
    bounds: Option<VoxelBox>,
}

impl LabelEdit {
    /// `true` if nothing changed.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Number of changed voxels.
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Box bounding every changed voxel.
    pub fn bounds(&self) -> Option<VoxelBox> {
        self.bounds
    }
}

/// Label map + segments + undo history.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentationSet {
    labels: LabelMap,
    segments: Vec<Segment>,
    counts: [u64; 256],
    history: Vec<LabelEdit>,
    revision: u64,
    dirty: Option<VoxelBox>,
}

impl SegmentationSet {
    /// Maximum number of edits kept for undo.
    pub const HISTORY: usize = 32;

    /// An empty segmentation on a grid of `dims`.
    pub fn new(dims: Dims3) -> Self {
        Self::from_labels(LabelMap::new(dims), Vec::new())
    }

    /// Builds a segmentation from an existing label map. Every label present
    /// in the map gets a segment: the one from `segments` with that label if
    /// given, otherwise `"Segment {label}"` in the palette colour. Segments
    /// whose label is absent from the map are kept too.
    pub fn from_labels(labels: LabelMap, segments: Vec<Segment>) -> Self {
        let counts = labels.histogram();
        let mut all: Vec<Segment> = segments.into_iter().filter(|s| s.label != 0).collect();
        for label in 1..=255u8 {
            if counts[usize::from(label)] > 0 && !all.iter().any(|s| s.label == label) {
                all.push(Segment::new(label, format!("Segment {label}")));
            }
        }
        all.sort_by_key(|s| s.label);
        all.dedup_by_key(|s| s.label);
        let full = VoxelBox::full(labels.dims());
        Self { labels, segments: all, counts, history: Vec::new(), revision: 1, dirty: Some(full) }
    }

    /// The label map.
    pub fn labels(&self) -> &LabelMap {
        &self.labels
    }

    /// Grid dimensions.
    pub fn dims(&self) -> Dims3 {
        self.labels.dims()
    }

    /// Segments ordered by label.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Segment with `label`.
    pub fn segment(&self, label: u8) -> Option<&Segment> {
        self.segments.iter().find(|s| s.label == label)
    }

    fn segment_mut(&mut self, label: u8) -> Result<&mut Segment, SegmentationError> {
        self.segments.iter_mut().find(|s| s.label == label).ok_or(SegmentationError::UnknownSegment(label))
    }

    /// `true` if there are no segments.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Monotonic revision, incremented by every change that affects display.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Takes and clears the box of labels changed since the last call.
    pub fn take_dirty(&mut self) -> Option<VoxelBox> {
        self.dirty.take()
    }

    fn touch(&mut self, bx: Option<VoxelBox>) {
        self.revision += 1;
        if let Some(bx) = bx {
            self.dirty = Some(self.dirty.map_or(bx, |d| d.union(bx)));
        }
    }

    /// Adds a segment with the lowest free label and returns the label.
    pub fn add_segment(&mut self, name: &str) -> Result<u8, SegmentationError> {
        let label = (1..=255u8).find(|l| self.segment(*l).is_none()).ok_or(SegmentationError::NoFreeLabel)?;
        let name = name.trim();
        let name = if name.is_empty() { format!("Segment {label}") } else { name.to_owned() };
        let seg = Segment::new(label, name);
        let at = self.segments.partition_point(|s| s.label < label);
        self.segments.insert(at, seg);
        self.touch(None);
        Ok(label)
    }

    /// Removes a segment and clears its voxels. History is dropped because
    /// earlier edits may refer to the removed label.
    pub fn remove_segment(&mut self, label: u8) -> Result<(), SegmentationError> {
        let at = self.segments.iter().position(|s| s.label == label).ok_or(SegmentationError::UnknownSegment(label))?;
        self.segments.remove(at);
        let mut bounds: Option<VoxelBox> = None;
        if self.counts[usize::from(label)] > 0 {
            let dims = self.labels.dims();
            for (idx, v) in self.labels.data.iter_mut().enumerate() {
                if *v == label {
                    *v = 0;
                    let p = index_to_voxel(dims, idx);
                    bounds = Some(
                        bounds.map_or(VoxelBox::new(p, p + UVec3::ONE), |b| b.union(VoxelBox::new(p, p + UVec3::ONE))),
                    );
                }
            }
            self.counts[0] += self.counts[usize::from(label)];
            self.counts[usize::from(label)] = 0;
        }
        self.history.clear();
        self.touch(bounds);
        Ok(())
    }

    /// Renames a segment; blank names are rejected (returns `false`).
    pub fn rename(&mut self, label: u8, name: &str) -> bool {
        let name = name.trim();
        match self.segment_mut(label) {
            Ok(s) if !name.is_empty() => {
                s.name = name.to_owned();
                true
            }
            _ => false,
        }
    }

    /// Sets a segment's colour.
    pub fn set_color(&mut self, label: u8, color: [u8; 3]) -> Result<(), SegmentationError> {
        self.segment_mut(label)?.color = color;
        self.touch(None);
        Ok(())
    }

    /// Shows or hides a segment.
    pub fn set_visible(&mut self, label: u8, visible: bool) -> Result<(), SegmentationError> {
        self.segment_mut(label)?.visible = visible;
        self.touch(None);
        Ok(())
    }

    /// Sets a segment's overlay opacity (clamped to `[0, 1]`).
    pub fn set_opacity(&mut self, label: u8, opacity: f32) -> Result<(), SegmentationError> {
        self.segment_mut(label)?.opacity = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 0.0 };
        self.touch(None);
        Ok(())
    }

    /// Replaces a segment's provenance.
    pub fn set_provenance(&mut self, label: u8, provenance: Provenance) -> Result<(), SegmentationError> {
        self.segment_mut(label)?.provenance = provenance;
        self.touch(None);
        Ok(())
    }

    /// Records a review decision on a segment (see [`Provenance::review`]).
    pub fn review(
        &mut self,
        label: u8,
        status: ReviewStatus,
        by: Option<&str>,
        at: Timestamp,
    ) -> Result<(), SegmentationError> {
        self.segment_mut(label)?.provenance.review(status, by, at);
        self.touch(None);
        Ok(())
    }

    /// Number of segments waiting for review.
    pub fn pending(&self) -> usize {
        self.segments.iter().filter(|s| s.provenance.is_pending()).count()
    }

    /// Number of voxels labelled `label`.
    pub fn voxel_count(&self, label: u8) -> u64 {
        self.counts[usize::from(label)]
    }

    /// Volume of a segment in millilitres for voxel `spacing` in mm.
    pub fn volume_ml(&self, label: u8, spacing: Vec3) -> f64 {
        self.voxel_count(label) as f64 * f64::from(spacing.x) * f64::from(spacing.y) * f64::from(spacing.z) / 1000.0
    }

    /// Writes a binary `mask` (over `bx`, `i` fastest, non-zero = inside)
    /// into segment `label`. Inside voxels become `label`; voxels of `label`
    /// outside the mask but inside the box become background. Other
    /// segments are left alone unless `overwrite` is set. The edit is
    /// recorded for undo; the returned edit is empty if nothing changed.
    pub fn apply_mask(
        &mut self,
        label: u8,
        bx: VoxelBox,
        mask: &[u8],
        overwrite: bool,
    ) -> Result<LabelEdit, SegmentationError> {
        self.segment(label).ok_or(SegmentationError::UnknownSegment(label))?;
        let dims = self.labels.dims();
        if !bx.fits(dims) {
            return Err(SegmentationError::InvalidBox { min: bx.min, max: bx.max });
        }
        if mask.len() != bx.voxel_count() {
            return Err(SegmentationError::DataLengthMismatch { expected: bx.voxel_count(), actual: mask.len() });
        }
        let mut edit = LabelEdit::default();
        let mut m = mask.iter();
        for k in bx.min.z..bx.max.z {
            for j in bx.min.y..bx.max.y {
                for i in bx.min.x..bx.max.x {
                    let inside = m.next().is_some_and(|&v| v != 0);
                    let idx = dims.index(i, j, k);
                    let old = self.labels.data[idx];
                    let new = match (inside, old) {
                        (true, 0) => label,
                        (true, o) if o != label && overwrite => label,
                        (false, o) if o == label => 0,
                        _ => old,
                    };
                    if new != old {
                        self.set_voxel(idx, new, &mut edit, UVec3::new(i, j, k));
                    }
                }
            }
        }
        self.record(edit.clone());
        Ok(edit)
    }

    /// Writes an engine label map into segments in one undoable edit.
    /// `values` holds one engine label per voxel (`i` fastest); `mapping`
    /// pairs engine values with segment labels. Mapped voxels take the
    /// segment label if they are background (or always with `overwrite`);
    /// unmapped values are ignored.
    pub fn apply_label_values(
        &mut self,
        values: &[u16],
        mapping: &[(u16, u8)],
        overwrite: bool,
    ) -> Result<LabelEdit, SegmentationError> {
        let dims = self.labels.dims();
        if values.len() != dims.voxel_count() {
            return Err(SegmentationError::DataLengthMismatch { expected: dims.voxel_count(), actual: values.len() });
        }
        let mut lut = vec![0u8; mapping.iter().map(|(v, _)| usize::from(*v) + 1).max().unwrap_or(0)];
        for &(value, label) in mapping {
            self.segment(label).ok_or(SegmentationError::UnknownSegment(label))?;
            lut[usize::from(value)] = label;
        }
        let mut edit = LabelEdit::default();
        for (idx, &v) in values.iter().enumerate() {
            let new = lut.get(usize::from(v)).copied().unwrap_or(0);
            let old = self.labels.data[idx];
            if new != 0 && new != old && (old == 0 || overwrite) {
                self.set_voxel(idx, new, &mut edit, index_to_voxel(dims, idx));
            }
        }
        self.record(edit.clone());
        Ok(edit)
    }

    fn set_voxel(&mut self, idx: usize, value: u8, edit: &mut LabelEdit, p: UVec3) {
        let old = self.labels.data[idx];
        edit.indices.push(idx as u32);
        edit.previous.push(old);
        self.labels.data[idx] = value;
        self.counts[usize::from(old)] -= 1;
        self.counts[usize::from(value)] += 1;
        let one = VoxelBox::new(p, p + UVec3::ONE);
        edit.bounds = Some(edit.bounds.map_or(one, |b| b.union(one)));
    }

    fn record(&mut self, edit: LabelEdit) {
        if edit.is_empty() {
            return;
        }
        self.touch(edit.bounds);
        if self.history.len() == Self::HISTORY {
            self.history.remove(0);
        }
        self.history.push(edit);
    }

    /// `true` if there is an edit to undo.
    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// Reverts the latest edit; returns `false` if there is none.
    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.history.pop() else { return false };
        for (&idx, &prev) in edit.indices.iter().zip(&edit.previous).rev() {
            let idx = idx as usize;
            let cur = self.labels.data[idx];
            self.counts[usize::from(cur)] -= 1;
            self.counts[usize::from(prev)] += 1;
            self.labels.data[idx] = prev;
        }
        self.touch(edit.bounds);
        true
    }

    /// Colour lookup table for rendering: entry `l` is the RGBA of label
    /// `l`, with alpha = opacity for visible segments and `0` otherwise.
    /// Entry `0` (background) is transparent.
    pub fn lut(&self) -> [Rgba8; 256] {
        let mut lut = [[0u8; 4]; 256];
        for s in self.segments.iter().filter(|s| s.visible) {
            let [r, g, b] = s.color;
            lut[usize::from(s.label)] = [r, g, b, crate::color::unit_to_u8(s.opacity)];
        }
        lut
    }
}

fn index_to_voxel(dims: Dims3, idx: usize) -> UVec3 {
    let (nx, ny) = (dims.x as usize, dims.y as usize);
    UVec3::new((idx % nx) as u32, ((idx / nx) % ny) as u32, (idx / (nx * ny)) as u32)
}

/// Region growing: the 6-connected region of unlabelled voxels of
/// `labels` whose physical value lies in `[lo, hi]`, grown from `seed`.
///
/// Returns the region's bounding box and its mask over that box (`i`
/// fastest, `1` inside), ready for [`SegmentationSet::apply_mask`].
/// Returns `None` when the seed is outside the grid, labelled or outside
/// the range, or once the region exceeds `max_voxels` (it leaked into
/// neighbouring tissue).
pub fn grow_region(
    volume: &crate::volume::Volume,
    labels: &SegmentationSet,
    seed: UVec3,
    lo: f32,
    hi: f32,
    max_voxels: u64,
) -> Option<(VoxelBox, Vec<u8>)> {
    let d = volume.dims();
    if labels.dims() != d || !d.contains(i64::from(seed.x), i64::from(seed.y), i64::from(seed.z)) {
        return None;
    }
    let accept = |i: u32, j: u32, k: u32| {
        labels.labels().label(i, j, k) == Some(0) && volume.physical(i, j, k).is_some_and(|x| (lo..=hi).contains(&x))
    };
    if !accept(seed.x, seed.y, seed.z) {
        return None;
    }
    let mut inside = vec![false; d.voxel_count()];
    let mut queue = std::collections::VecDeque::from([seed]);
    inside[d.index(seed.x, seed.y, seed.z)] = true;
    let (mut lo_c, mut hi_c, mut count) = (seed, seed, 0u64);
    while let Some(v) = queue.pop_front() {
        count += 1;
        if count > max_voxels {
            return None;
        }
        lo_c = lo_c.min(v);
        hi_c = hi_c.max(v);
        let neighbours = [
            v.x.checked_sub(1).map(|x| UVec3::new(x, v.y, v.z)),
            (v.x + 1 < d.x).then(|| UVec3::new(v.x + 1, v.y, v.z)),
            v.y.checked_sub(1).map(|y| UVec3::new(v.x, y, v.z)),
            (v.y + 1 < d.y).then(|| UVec3::new(v.x, v.y + 1, v.z)),
            v.z.checked_sub(1).map(|z| UVec3::new(v.x, v.y, z)),
            (v.z + 1 < d.z).then(|| UVec3::new(v.x, v.y, v.z + 1)),
        ];
        for n in neighbours.into_iter().flatten() {
            let idx = d.index(n.x, n.y, n.z);
            if !inside[idx] && accept(n.x, n.y, n.z) {
                inside[idx] = true;
                queue.push_back(n);
            }
        }
    }
    let bx = VoxelBox::new(lo_c, hi_c + UVec3::ONE);
    let mut mask = Vec::with_capacity(bx.voxel_count());
    for k in bx.min.z..bx.max.z {
        for j in bx.min.y..bx.max.y {
            for i in bx.min.x..bx.max.x {
                mask.push(u8::from(inside[d.index(i, j, k)]));
            }
        }
    }
    Some((bx, mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims() -> Dims3 {
        Dims3::new(4, 3, 2)
    }

    #[test]
    fn region_growing_stays_in_range_and_out_of_segments() {
        // 6 × 1 × 1 row: 10 10 50 10 10 10, voxel 4 already labelled
        let d = Dims3::new(6, 1, 1);
        let v = crate::volume::Volume::from_physical(d, Vec3::ONE, &[10.0, 10.0, 50.0, 10.0, 10.0, 10.0]).unwrap();
        let mut set = SegmentationSet::new(d);
        let taken = set.add_segment("taken").unwrap();
        set.apply_mask(taken, VoxelBox::new(UVec3::new(4, 0, 0), UVec3::new(5, 1, 1)), &[1], false).unwrap();
        let (bx, mask) = grow_region(&v, &set, UVec3::ZERO, 0.0, 20.0, u64::MAX).unwrap();
        assert_eq!(
            (bx, mask),
            (VoxelBox::new(UVec3::ZERO, UVec3::new(2, 1, 1)), vec![1, 1]),
            "stops at the bright voxel"
        );
        let (bx, mask) = grow_region(&v, &set, UVec3::ZERO, 0.0, 60.0, u64::MAX).unwrap();
        assert_eq!((bx.max.x, mask.len()), (4, 4), "stops at the labelled voxel");
        assert!(grow_region(&v, &set, UVec3::ZERO, 0.0, 60.0, 3).is_none(), "too large");
        assert!(grow_region(&v, &set, UVec3::new(2, 0, 0), 0.0, 20.0, 10).is_none(), "seed out of range");
        assert!(grow_region(&v, &set, UVec3::new(4, 0, 0), 0.0, 20.0, 10).is_none(), "seed labelled");
        assert!(grow_region(&v, &set, UVec3::new(9, 0, 0), 0.0, 20.0, 10).is_none(), "seed outside");
        assert!(grow_region(&v, &SegmentationSet::new(dims()), UVec3::ZERO, 0.0, 20.0, 10).is_none(), "other grid");
    }

    #[test]
    fn voxel_box_basics() {
        let b = VoxelBox::new(UVec3::new(1, 0, 0), UVec3::new(3, 2, 1));
        assert_eq!(b.size(), UVec3::new(2, 2, 1));
        assert_eq!(b.voxel_count(), 4);
        assert!(b.fits(dims()));
        assert!(!VoxelBox::new(UVec3::ZERO, UVec3::new(5, 1, 1)).fits(dims()));
        let e = VoxelBox::new(UVec3::ONE, UVec3::ONE);
        assert!(e.is_empty() && !e.fits(dims()));
        assert_eq!(e.union(b), b);
        assert_eq!(b.union(e), b);
        let c = VoxelBox::new(UVec3::new(0, 1, 1), UVec3::new(1, 3, 2));
        assert_eq!(b.union(c), VoxelBox::new(UVec3::ZERO, UVec3::new(3, 3, 2)));
        assert_eq!(VoxelBox::full(dims()).voxel_count(), 24);
    }

    #[test]
    fn label_map_access_and_extract() {
        let mut data = vec![0u8; 24];
        data[dims().index(2, 1, 1)] = 7;
        assert!(LabelMap::from_data(dims(), vec![0; 3]).is_err());
        let m = LabelMap::from_data(dims(), data).unwrap();
        assert_eq!(m.label(2, 1, 1), Some(7));
        assert_eq!(m.label(4, 0, 0), None);
        let bx = VoxelBox::new(UVec3::new(1, 1, 1), UVec3::new(3, 2, 2));
        assert_eq!(m.extract(bx), Some(vec![0, 7]));
        assert_eq!(m.extract(VoxelBox::new(UVec3::ZERO, UVec3::new(9, 1, 1))), None);
        assert_eq!(m.histogram()[7], 1);
        assert_eq!(m.histogram()[0], 23);
    }

    #[test]
    fn segments_are_added_renamed_styled_and_removed() {
        let mut s = SegmentationSet::new(dims());
        assert!(s.is_empty());
        let a = s.add_segment("Liver").unwrap();
        let b = s.add_segment("  ").unwrap();
        assert_eq!((a, b), (1, 2));
        assert_eq!(s.segment(b).unwrap().name, "Segment 2");
        assert_eq!(s.segment(a).unwrap().color, PALETTE[0]);
        assert!(s.rename(a, " Liver L "));
        assert!(!s.rename(a, ""));
        assert!(!s.rename(9, "x"));
        assert_eq!(s.segment(a).unwrap().name, "Liver L");
        s.set_color(a, [1, 2, 3]).unwrap();
        s.set_opacity(a, 2.0).unwrap();
        s.set_opacity(b, f32::NAN).unwrap();
        s.set_visible(b, false).unwrap();
        assert_eq!(s.set_visible(9, true), Err(SegmentationError::UnknownSegment(9)));
        let lut = s.lut();
        assert_eq!(lut[1], [1, 2, 3, 255]);
        assert_eq!(lut[2], [0; 4]);
        assert_eq!(lut[0], [0; 4]);
        s.remove_segment(a).unwrap();
        assert_eq!(s.segments().len(), 1);
        assert_eq!(s.add_segment("again").unwrap(), 1);
        assert!(s.remove_segment(42).is_err());
    }

    #[test]
    fn labels_run_out_at_255() {
        let mut s = SegmentationSet::new(dims());
        for _ in 0..255 {
            s.add_segment("x").unwrap();
        }
        assert_eq!(s.add_segment("x"), Err(SegmentationError::NoFreeLabel));
    }

    #[test]
    fn masks_paint_replace_and_undo() {
        let mut s = SegmentationSet::new(dims());
        let a = s.add_segment("A").unwrap();
        let b = s.add_segment("B").unwrap();
        s.take_dirty();
        let full = VoxelBox::full(dims());
        let mut mask = vec![0u8; 24];
        mask[dims().index(1, 1, 0)] = 1;
        mask[dims().index(2, 1, 0)] = 1;
        let e = s.apply_mask(a, full, &mask, false).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e.bounds(), Some(VoxelBox::new(UVec3::new(1, 1, 0), UVec3::new(3, 2, 1))));
        assert_eq!(s.voxel_count(a), 2);
        assert_eq!(s.take_dirty(), e.bounds());
        // B over the same voxels without overwrite: nothing changes
        let e = s.apply_mask(b, full, &mask, false).unwrap();
        assert!(e.is_empty());
        assert_eq!(s.voxel_count(b), 0);
        // with overwrite B takes them
        s.apply_mask(b, full, &mask, true).unwrap();
        assert_eq!((s.voxel_count(a), s.voxel_count(b)), (0, 2));
        // a planar box with an empty mask clears B inside the box only
        let plane = VoxelBox::new(UVec3::new(2, 0, 0), UVec3::new(4, 3, 1));
        s.apply_mask(b, plane, &[0; 6], false).unwrap();
        assert_eq!(s.voxel_count(b), 1);
        assert_eq!(s.labels().label(1, 1, 0), Some(b));
        let spacing = Vec3::new(2.0, 5.0, 10.0);
        assert!((s.volume_ml(b, spacing) - 0.1).abs() < 1e-12);
        assert!(s.can_undo());
        assert!(s.undo());
        assert_eq!(s.voxel_count(b), 2);
        assert!(s.undo());
        assert_eq!((s.voxel_count(a), s.voxel_count(b)), (2, 0));
        assert!(s.undo());
        assert_eq!(s.voxel_count(a), 0);
        assert_eq!(s.voxel_count(0), 24);
        assert!(!s.undo());
    }

    #[test]
    fn engine_label_maps_fill_several_segments_at_once() {
        let mut s = SegmentationSet::new(dims());
        let liver = s.add_segment("liver").unwrap();
        let spleen = s.add_segment("spleen").unwrap();
        let full = VoxelBox::full(dims());
        let mut pre = vec![0u8; 24];
        pre[1] = 1;
        s.apply_mask(spleen, full, &pre, false).unwrap(); // voxel 1 already spleen
        let mut values = vec![0u16; 24];
        values[0] = 5; // liver
        values[1] = 5; // liver, but spleen is there: kept without overwrite
        values[2] = 9; // spleen
        values[3] = 7; // unmapped
        let e = s.apply_label_values(&values, &[(5, liver), (9, spleen)], false).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!((s.voxel_count(liver), s.voxel_count(spleen)), (1, 2));
        let e = s.apply_label_values(&values, &[(5, liver)], true).unwrap();
        assert_eq!(e.len(), 1, "overwrite takes voxel 1 from the spleen");
        assert!(s.undo());
        assert!(s.undo());
        assert_eq!((s.voxel_count(liver), s.voxel_count(spleen)), (0, 1));
        assert!(matches!(s.apply_label_values(&[0; 3], &[], false), Err(SegmentationError::DataLengthMismatch { .. })));
        assert_eq!(s.apply_label_values(&values, &[(5, 77)], false), Err(SegmentationError::UnknownSegment(77)));
        assert!(s.apply_label_values(&values, &[], false).unwrap().is_empty());
    }

    #[test]
    fn apply_mask_validates_input() {
        let mut s = SegmentationSet::new(dims());
        let a = s.add_segment("A").unwrap();
        let full = VoxelBox::full(dims());
        assert!(matches!(s.apply_mask(9, full, &[0; 24], false), Err(SegmentationError::UnknownSegment(9))));
        assert!(matches!(
            s.apply_mask(a, VoxelBox::new(UVec3::ZERO, UVec3::new(5, 1, 1)), &[0; 5], false),
            Err(SegmentationError::InvalidBox { .. })
        ));
        assert!(matches!(s.apply_mask(a, full, &[0; 3], false), Err(SegmentationError::DataLengthMismatch { .. })));
    }

    #[test]
    fn history_is_bounded_and_cleared_by_removal() {
        let mut s = SegmentationSet::new(dims());
        let a = s.add_segment("A").unwrap();
        let full = VoxelBox::full(dims());
        for n in 0..(SegmentationSet::HISTORY + 3) {
            let mut m = vec![0u8; 24];
            m[n % 24] = 1;
            s.apply_mask(a, full, &m, false).unwrap();
        }
        let mut undone = 0;
        while s.undo() {
            undone += 1;
        }
        assert_eq!(undone, SegmentationSet::HISTORY);
        s.apply_mask(a, full, &[1; 24], false).unwrap();
        s.take_dirty();
        s.remove_segment(a).unwrap();
        assert!(!s.can_undo());
        assert_eq!(s.voxel_count(0), 24);
        assert_eq!(s.take_dirty(), Some(full));
    }

    #[test]
    fn imported_labels_get_segments() {
        let mut data = vec![0u8; 24];
        data[0] = 3;
        data[5] = 3;
        data[7] = 1;
        let labels = LabelMap::from_data(dims(), data).unwrap();
        let named = Segment { color: [9, 9, 9], opacity: 1.0, ..Segment::new(3, "Tumour") };
        let empty = Segment { color: [1, 1, 1], visible: false, opacity: 0.2, ..Segment::new(8, "Unused") };
        let zero = Segment::new(0, "bg");
        let s = SegmentationSet::from_labels(labels, vec![named.clone(), empty, zero]);
        let names: Vec<_> = s.segments().iter().map(|s| (s.label, s.name.as_str())).collect();
        assert_eq!(names, vec![(1, "Segment 1"), (3, "Tumour"), (8, "Unused")]);
        assert_eq!(s.voxel_count(3), 2);
        assert_eq!(s.segment(3), Some(&named));
        let mut s = s;
        assert_eq!(s.take_dirty(), Some(VoxelBox::full(dims())));
        assert_eq!(s.dims(), dims());
    }

    #[test]
    fn segment_provenance_and_review() {
        let mut s = SegmentationSet::new(dims());
        let a = s.add_segment("Liver").unwrap();
        assert_eq!(s.pending(), 0, "drawn segments are confirmed");
        s.set_provenance(a, Provenance::engine("TotalSegmentator", "2.18", false, Timestamp(1))).unwrap();
        assert_eq!(s.pending(), 1);
        let revision = s.revision();
        s.review(a, ReviewStatus::Confirmed, Some("dr.k"), Timestamp(2)).unwrap();
        assert!(s.revision() > revision, "reviews count as changes (saved workspaces)");
        assert_eq!(s.pending(), 0);
        assert_eq!(s.segment(a).unwrap().provenance.reviewed_by.as_deref(), Some("dr.k"));
        assert_eq!(s.review(9, ReviewStatus::Rejected, None, Timestamp(3)), Err(SegmentationError::UnknownSegment(9)));
        assert!(s.set_provenance(9, Provenance::default()).is_err());
    }
}
