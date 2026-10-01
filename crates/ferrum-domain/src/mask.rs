//! Visibility mask edited by the 3D volume eraser, with undo support.

use glam::{UVec3, Vec3};

use crate::geometry::Dims3;

/// Per-voxel visibility (`255` visible, `0` erased).
#[derive(Debug, Clone, PartialEq)]
pub struct VoxelMask {
    dims: Dims3,
    data: Vec<u8>,
    revision: u64,
    dirty: Option<(UVec3, UVec3)>,
}

/// Brush of the volume eraser: a cylinder aligned with the view direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EraserBrush {
    /// Cylinder radius in millimetres.
    pub radius_mm: f32,
    /// Cylinder depth along the view direction in millimetres.
    pub depth_mm: f32,
}

impl Default for EraserBrush {
    fn default() -> Self {
        Self { radius_mm: 10.0, depth_mm: 20.0 }
    }
}

/// Voxels changed by one erase operation, sufficient to undo it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EraseStroke {
    /// Linear voxel indices that were changed.
    pub indices: Vec<u32>,
    /// Their previous values.
    pub previous: Vec<u8>,
    /// Inclusive voxel bounding box of the change.
    pub bounds: Option<(UVec3, UVec3)>,
}

impl EraseStroke {
    /// `true` if nothing changed.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

fn union(a: Option<(UVec3, UVec3)>, b: Option<(UVec3, UVec3)>) -> Option<(UVec3, UVec3)> {
    match (a, b) {
        (Some((a0, a1)), Some((b0, b1))) => Some((a0.min(b0), a1.max(b1))),
        (x, None) | (None, x) => x,
    }
}

impl VoxelMask {
    /// Fully visible mask.
    pub fn new(dims: Dims3) -> Self {
        Self { dims, data: vec![255; dims.voxel_count()], revision: 0, dirty: None }
    }

    /// Mask dimensions.
    pub fn dims(&self) -> Dims3 {
        self.dims
    }

    /// Raw data (`x` fastest).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Monotonic revision, incremented on every change.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// `true` if voxel `(i, j, k)` is visible (out-of-range = invisible).
    pub fn is_visible(&self, i: u32, j: u32, k: u32) -> bool {
        self.dims.contains(i64::from(i), i64::from(j), i64::from(k)) && self.data[self.dims.index(i, j, k)] != 0
    }

    /// Number of erased voxels.
    pub fn erased_count(&self) -> usize {
        self.data.iter().filter(|&&v| v == 0).count()
    }

    /// Takes and clears the accumulated dirty region since the last call.
    pub fn take_dirty(&mut self) -> Option<(UVec3, UVec3)> {
        self.dirty.take()
    }

    /// Erases a cylinder starting at `hit_tex` (texture coordinates) and
    /// extending `brush.depth_mm` along `direction`, with `spacing` the voxel
    /// size in millimetres.
    pub fn erase(&mut self, hit_tex: Vec3, direction: Vec3, brush: EraserBrush, spacing: Vec3) -> EraseStroke {
        let dir = direction.normalize_or_zero();
        let size_mm = self.dims.as_vec3() * spacing;
        let start = hit_tex * size_mm;
        let end = start + dir * brush.depth_mm;
        let r = brush.radius_mm.max(0.0);
        let lo_mm = start.min(end) - Vec3::splat(r);
        let hi_mm = start.max(end) + Vec3::splat(r);
        let max_idx = self.dims.as_vec3() - Vec3::ONE;
        let lo = (lo_mm / spacing - Vec3::splat(0.5)).floor().clamp(Vec3::ZERO, max_idx).as_uvec3();
        let hi = (hi_mm / spacing - Vec3::splat(0.5)).ceil().clamp(Vec3::ZERO, max_idx).as_uvec3();
        let back = spacing.max_element();
        let mut stroke = EraseStroke::default();
        if lo_mm.cmpgt(size_mm).any() || hi_mm.cmplt(Vec3::ZERO).any() {
            return stroke;
        }
        for k in lo.z..=hi.z {
            for j in lo.y..=hi.y {
                for i in lo.x..=hi.x {
                    let p = (UVec3::new(i, j, k).as_vec3() + Vec3::splat(0.5)) * spacing;
                    let rel = p - start;
                    let t = rel.dot(dir);
                    if t < -back || t > brush.depth_mm {
                        continue;
                    }
                    if (rel - dir * t).length_squared() > r * r {
                        continue;
                    }
                    let idx = self.dims.index(i, j, k);
                    if self.data[idx] != 0 {
                        stroke.indices.push(idx as u32);
                        stroke.previous.push(self.data[idx]);
                        self.data[idx] = 0;
                        let v = UVec3::new(i, j, k);
                        stroke.bounds = union(stroke.bounds, Some((v, v)));
                    }
                }
            }
        }
        if !stroke.is_empty() {
            self.revision += 1;
            self.dirty = union(self.dirty, stroke.bounds);
        }
        stroke
    }

    /// Reverts a stroke.
    pub fn undo(&mut self, stroke: &EraseStroke) {
        for (&idx, &prev) in stroke.indices.iter().zip(&stroke.previous) {
            if let Some(v) = self.data.get_mut(idx as usize) {
                *v = prev;
            }
        }
        if !stroke.is_empty() {
            self.revision += 1;
            self.dirty = union(self.dirty, stroke.bounds);
        }
    }

    /// Makes every voxel visible again.
    pub fn reset(&mut self) {
        self.data.fill(255);
        self.revision += 1;
        let max = self.dims.as_uvec3().saturating_sub(UVec3::ONE);
        self.dirty = Some((UVec3::ZERO, max));
    }
}

/// Undo stack of eraser strokes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaskHistory {
    strokes: Vec<EraseStroke>,
}

impl MaskHistory {
    /// Maximum number of strokes kept.
    pub const CAPACITY: usize = 64;

    /// Records a non-empty stroke, dropping the oldest beyond capacity.
    pub fn push(&mut self, stroke: EraseStroke) {
        if stroke.is_empty() {
            return;
        }
        if self.strokes.len() == Self::CAPACITY {
            self.strokes.remove(0);
        }
        self.strokes.push(stroke);
    }

    /// Undoes the latest stroke on `mask`; returns `false` if nothing to undo.
    pub fn undo(&mut self, mask: &mut VoxelMask) -> bool {
        match self.strokes.pop() {
            Some(s) => {
                mask.undo(&s);
                true
            }
            None => false,
        }
    }

    /// Number of recorded strokes.
    pub fn len(&self) -> usize {
        self.strokes.len()
    }

    /// `true` if there is nothing to undo.
    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }

    /// Forgets all strokes.
    pub fn clear(&mut self) {
        self.strokes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erase_removes_cylinder_and_undo_restores() {
        let dims = Dims3::new(20, 20, 20);
        let mut m = VoxelMask::new(dims);
        let brush = EraserBrush { radius_mm: 3.0, depth_mm: 10.0 };
        let s = m.erase(Vec3::new(0.5, 0.5, 0.0), Vec3::Z, brush, Vec3::ONE);
        assert!(!s.is_empty());
        assert!(!m.is_visible(10, 10, 5));
        assert!(m.is_visible(10, 10, 15));
        assert!(m.is_visible(0, 0, 5));
        let erased = m.erased_count();
        assert_eq!(erased, s.indices.len());
        // roughly pi * r^2 * depth voxels
        assert!(erased > 200 && erased < 400, "{erased}");
        let rev = m.revision();
        let dirty = m.take_dirty().unwrap();
        assert!(dirty.0.z == 0 && dirty.1.z <= 10);
        assert!(m.take_dirty().is_none());
        m.undo(&s);
        assert_eq!(m.erased_count(), 0);
        assert!(m.revision() > rev);
    }

    #[test]
    fn erase_outside_volume_is_noop() {
        let mut m = VoxelMask::new(Dims3::new(8, 8, 8));
        let s = m.erase(Vec3::splat(5.0), Vec3::Z, EraserBrush::default(), Vec3::ONE);
        assert!(s.is_empty());
        assert_eq!(m.revision(), 0);
    }

    #[test]
    fn repeated_erase_records_only_new_voxels() {
        let mut m = VoxelMask::new(Dims3::new(10, 10, 10));
        let b = EraserBrush { radius_mm: 2.0, depth_mm: 4.0 };
        let s1 = m.erase(Vec3::splat(0.5), Vec3::Z, b, Vec3::ONE);
        let s2 = m.erase(Vec3::splat(0.5), Vec3::Z, b, Vec3::ONE);
        assert!(!s1.is_empty());
        assert!(s2.is_empty());
    }

    #[test]
    fn history_undo_order_and_capacity() {
        let mut m = VoxelMask::new(Dims3::new(10, 10, 10));
        let mut h = MaskHistory::default();
        let b = EraserBrush { radius_mm: 1.0, depth_mm: 1.0 };
        h.push(m.erase(Vec3::new(0.2, 0.5, 0.5), Vec3::Z, b, Vec3::ONE));
        h.push(m.erase(Vec3::new(0.8, 0.5, 0.5), Vec3::Z, b, Vec3::ONE));
        h.push(EraseStroke::default());
        assert_eq!(h.len(), 2);
        assert!(h.undo(&mut m));
        assert!(m.is_visible(8, 5, 5));
        assert!(!m.is_visible(2, 5, 5));
        assert!(h.undo(&mut m));
        assert!(!h.undo(&mut m));
        assert_eq!(m.erased_count(), 0);

        for _ in 0..MaskHistory::CAPACITY + 5 {
            h.push(EraseStroke { indices: vec![0], previous: vec![255], bounds: None });
        }
        assert_eq!(h.len(), MaskHistory::CAPACITY);
        h.clear();
        assert!(h.is_empty());
    }

    #[test]
    fn reset_marks_everything_dirty() {
        let mut m = VoxelMask::new(Dims3::new(3, 4, 5));
        m.reset();
        assert_eq!(m.take_dirty(), Some((UVec3::ZERO, UVec3::new(2, 3, 4))));
    }
}
