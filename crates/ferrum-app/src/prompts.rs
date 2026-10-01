//! Conversion of 2D drawing on a slice into engine prompts.
//!
//! Positions arrive in in-plane millimetres (as for annotations) and are
//! mapped to image pixels of the slice and then to voxels of the volume.
//! Box, scribble and lasso prompts are planar: one voxel thick along the
//! slice normal.

use ferrum_domain::{Prompt, SliceAxis, Volume, VoxelBox};
use glam::{UVec3, Vec2};

/// Brush radius of scribbles in screen-independent image pixels.
pub const SCRIBBLE_RADIUS_PX: f32 = 1.5;

/// A slice of a volume on which prompts are drawn.
#[derive(Clone, Copy)]
pub struct PromptPlane<'a> {
    /// The volume.
    pub volume: &'a Volume,
    /// Orientation.
    pub axis: SliceAxis,
    /// Slice index.
    pub index: u32,
}

impl PromptPlane<'_> {
    fn dims(&self) -> (u32, u32) {
        self.axis.plane_dims(self.volume)
    }

    /// Continuous image-pixel position of in-plane `mm`.
    fn px_of(&self, mm: Vec2) -> Vec2 {
        let (w, h) = self.dims();
        mm / self.axis.plane_size_mm(self.volume) * Vec2::new(w as f32, h as f32)
    }

    fn pixel(&self, px: Vec2) -> Option<(u32, u32)> {
        let (w, h) = self.dims();
        (px.x >= 0.0 && px.y >= 0.0 && px.x < w as f32 && px.y < h as f32).then_some((px.x as u32, px.y as u32))
    }

    fn clamped(&self, px: Vec2) -> (u32, u32) {
        let (w, h) = self.dims();
        (px.x.clamp(0.0, w as f32 - 1.0) as u32, px.y.clamp(0.0, h as f32 - 1.0) as u32)
    }

    fn voxel(&self, (px, py): (u32, u32)) -> Option<UVec3> {
        self.axis.voxel_at(self.volume, self.index, px, py)
    }

    /// Point prompt at `mm`; `None` outside the image.
    pub fn point(&self, mm: Vec2, positive: bool) -> Option<Prompt> {
        let voxel = self.voxel(self.pixel(self.px_of(mm))?)?;
        Some(Prompt::Point { positive, voxel })
    }

    /// Planar box prompt spanning `a`–`b` (clamped to the image); `None`
    /// if the box misses the image entirely.
    pub fn rect(&self, a: Vec2, b: Vec2, positive: bool) -> Option<Prompt> {
        let (pa, pb) = (self.px_of(a), self.px_of(b));
        let (w, h) = self.dims();
        let lo = pa.min(pb);
        let hi = pa.max(pb);
        if hi.x < 0.0 || hi.y < 0.0 || lo.x >= w as f32 || lo.y >= h as f32 {
            return None;
        }
        let corners = [self.clamped(lo), self.clamped(hi)];
        let bx = bounds(corners.iter().filter_map(|&p| self.voxel(p)))?;
        Some(Prompt::Box { positive, bx })
    }

    /// Scribble prompt: pixels within [`SCRIBBLE_RADIUS_PX`] of the stroke.
    pub fn scribble(&self, stroke: &[Vec2], positive: bool) -> Option<Prompt> {
        let pts: Vec<Vec2> = stroke.iter().map(|&mm| self.px_of(mm)).collect();
        let (bx, mask) =
            self.mask(|c| distance_to_polyline(c, &pts) <= SCRIBBLE_RADIUS_PX, &pts, SCRIBBLE_RADIUS_PX)?;
        Some(Prompt::Scribble { positive, bx, mask })
    }

    /// Lasso prompt: pixels whose centres lie inside the closed outline.
    pub fn lasso(&self, outline: &[Vec2], positive: bool) -> Option<Prompt> {
        if outline.len() < 3 {
            return None;
        }
        let pts: Vec<Vec2> = outline.iter().map(|&mm| self.px_of(mm)).collect();
        let (bx, mask) = self.mask(|c| inside_polygon(c, &pts), &pts, 0.0)?;
        Some(Prompt::Lasso { positive, bx, mask })
    }

    /// Rasterises `inside` over the pixels around `pts` (grown by `margin`)
    /// into a voxel box and a mask over it.
    fn mask(&self, inside: impl Fn(Vec2) -> bool, pts: &[Vec2], margin: f32) -> Option<(VoxelBox, Vec<u8>)> {
        let lo = pts.iter().copied().reduce(Vec2::min)? - Vec2::splat(margin + 1.0);
        let hi = pts.iter().copied().reduce(Vec2::max)? + Vec2::splat(margin + 1.0);
        let (lo, hi) = (self.clamped(lo), self.clamped(hi));
        let mut voxels = Vec::new();
        for py in lo.1..=hi.1 {
            for px in lo.0..=hi.0 {
                let centre = Vec2::new(px as f32 + 0.5, py as f32 + 0.5);
                if inside(centre) {
                    voxels.extend(self.voxel((px, py)));
                }
            }
        }
        let bx = bounds(voxels.iter().copied())?;
        let s = bx.size();
        let mut mask = vec![0u8; bx.voxel_count()];
        for v in voxels {
            let q = v - bx.min;
            mask[(q.x + s.x * (q.y + s.y * q.z)) as usize] = 1;
        }
        Some((bx, mask))
    }
}

/// Half-open box around `voxels`; `None` if empty.
fn bounds(voxels: impl Iterator<Item = UVec3>) -> Option<VoxelBox> {
    voxels.fold(None, |acc: Option<VoxelBox>, v| {
        let one = VoxelBox::new(v, v + UVec3::ONE);
        Some(acc.map_or(one, |b| b.union(one)))
    })
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

fn distance_to_polyline(p: Vec2, pts: &[Vec2]) -> f32 {
    match pts {
        [] => f32::INFINITY,
        [one] => p.distance(*one),
        _ => pts.windows(2).map(|w| distance_to_segment(p, w[0], w[1])).fold(f32::INFINITY, f32::min),
    }
}

/// Even-odd point-in-polygon test.
fn inside_polygon(p: Vec2, poly: &[Vec2]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::Dims3;
    use glam::Vec3;

    fn volume() -> Volume {
        let d = Dims3::new(10, 8, 6);
        Volume::from_physical(d, Vec3::new(1.0, 2.0, 3.0), &vec![0.0; d.voxel_count()]).unwrap()
    }

    #[test]
    fn points_map_to_voxels_in_every_plane() {
        let v = volume();
        let axial = PromptPlane { volume: &v, axis: SliceAxis::Axial, index: 4 };
        // pixel (3, 2) of an axial slice: 1 mm × 2 mm pixels
        assert_eq!(
            axial.point(Vec2::new(3.5, 5.0), true),
            Some(Prompt::Point { positive: true, voxel: UVec3::new(3, 2, 4) })
        );
        assert_eq!(axial.point(Vec2::new(-1.0, 5.0), true), None);
        // coronal images are flipped vertically: the top row is the last slice
        let coronal = PromptPlane { volume: &v, axis: SliceAxis::Coronal, index: 7 };
        assert_eq!(
            coronal.point(Vec2::new(0.5, 1.0), false),
            Some(Prompt::Point { positive: false, voxel: UVec3::new(0, 7, 5) })
        );
    }

    #[test]
    fn rects_are_planar_and_clamped() {
        let v = volume();
        let axial = PromptPlane { volume: &v, axis: SliceAxis::Axial, index: 2 };
        let Some(Prompt::Box { bx, .. }) = axial.rect(Vec2::new(8.5, 15.0), Vec2::new(-5.0, 3.0), true) else {
            panic!("box expected");
        };
        assert_eq!(bx, VoxelBox::new(UVec3::new(0, 1, 2), UVec3::new(9, 8, 3)));
        assert_eq!(axial.rect(Vec2::new(-9.0, -9.0), Vec2::new(-1.0, -1.0), true), None);
        let sag = PromptPlane { volume: &v, axis: SliceAxis::Sagittal, index: 9 };
        let Some(Prompt::Box { bx, .. }) = sag.rect(Vec2::ZERO, Vec2::new(4.0, 6.0), false) else {
            panic!("box expected");
        };
        assert_eq!(bx.size().x, 1, "planar along the sagittal normal");
        assert_eq!(bx.min.x, 9);
    }

    #[test]
    fn scribbles_and_lassos_rasterise_on_the_slice() {
        let v = volume();
        let axial = PromptPlane { volume: &v, axis: SliceAxis::Axial, index: 0 };
        let Some(Prompt::Scribble { bx, mask, positive }) =
            axial.scribble(&[Vec2::new(2.5, 5.0), Vec2::new(6.5, 5.0)], true)
        else {
            panic!("scribble expected");
        };
        assert!(positive);
        assert_eq!(bx.size().z, 1);
        assert_eq!(mask.len(), bx.voxel_count());
        let row: Vec<u8> =
            (bx.min.x..bx.max.x).map(|i| mask[((i - bx.min.x) + bx.size().x * (2 - bx.min.y)) as usize]).collect();
        assert!(row.iter().all(|&m| m == 1), "the stroke row is filled: {row:?}");
        let square = [Vec2::new(2.0, 2.0), Vec2::new(8.0, 2.0), Vec2::new(8.0, 12.0), Vec2::new(2.0, 12.0)];
        let Some(Prompt::Lasso { bx, mask, .. }) = axial.lasso(&square, false) else {
            panic!("lasso expected");
        };
        // pixel centres 2.5..7.5 in x and 1.5..5.5 in y lie inside
        assert_eq!(bx, VoxelBox::new(UVec3::new(2, 1, 0), UVec3::new(8, 6, 1)));
        assert!(mask.iter().all(|&m| m == 1));
        assert_eq!(axial.lasso(&square[..2], true), None);
        assert_eq!(axial.lasso(&[Vec2::splat(-9.0), Vec2::new(-8.0, -9.0), Vec2::splat(-8.0)], true), None);
    }

    #[test]
    fn geometry_helpers() {
        assert_eq!(distance_to_polyline(Vec2::ZERO, &[]), f32::INFINITY);
        assert_eq!(distance_to_polyline(Vec2::ZERO, &[Vec2::new(3.0, 4.0)]), 5.0);
        assert_eq!(distance_to_segment(Vec2::new(1.0, 1.0), Vec2::ZERO, Vec2::ZERO), 2f32.sqrt());
        let tri = [Vec2::ZERO, Vec2::new(4.0, 0.0), Vec2::new(0.0, 4.0)];
        assert!(inside_polygon(Vec2::new(1.0, 1.0), &tri));
        assert!(!inside_polygon(Vec2::new(3.0, 3.0), &tri));
    }
}
