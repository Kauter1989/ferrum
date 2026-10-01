//! Clipping of the rendered volume: an axis-aligned clip box, an arbitrary
//! oblique plane and a view-aligned cut ("virtual knife").
//!
//! All clipping primitives are reduced to a list of half-spaces
//! `dot(n, p) <= c` in model space ([`HalfSpace`]). Both the GPU shader and
//! the CPU reference renderer consume the same list, so behaviour is
//! identical by construction.

use glam::{Vec3, Vec4};

use crate::geometry::Ray;

/// Maximum number of half-spaces produced by [`ClipSettings::half_spaces`].
pub const MAX_HALF_SPACES: usize = 8;

/// A half-space `{ p : dot(normal, p) <= offset }` in model space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HalfSpace {
    /// Outward normal of the removed side (unit length).
    pub normal: Vec3,
    /// Plane offset.
    pub offset: f32,
}

impl HalfSpace {
    /// Packs as `(nx, ny, nz, offset)` for GPU upload.
    pub fn to_vec4(&self) -> Vec4 {
        self.normal.extend(self.offset)
    }

    /// Returns `true` if `p` is kept.
    pub fn contains(&self, p: Vec3) -> bool {
        self.normal.dot(p) <= self.offset
    }
}

/// Axis-aligned clip box in texture coordinates (`[0, 1]³`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipBox {
    /// Lower corner.
    pub min: Vec3,
    /// Upper corner.
    pub max: Vec3,
}

impl Default for ClipBox {
    fn default() -> Self {
        Self { min: Vec3::ZERO, max: Vec3::ONE }
    }
}

impl ClipBox {
    /// Returns a box with corners clamped to `[0, 1]` and `min <= max`.
    pub fn normalized(self) -> Self {
        let a = self.min.clamp(Vec3::ZERO, Vec3::ONE);
        let b = self.max.clamp(Vec3::ZERO, Vec3::ONE);
        Self { min: a.min(b), max: a.max(b) }
    }

    /// `true` if the box covers the whole volume.
    pub fn is_full(&self) -> bool {
        self.min == Vec3::ZERO && self.max == Vec3::ONE
    }
}

/// Oblique clip plane parameterised for user interaction by two angles and
/// a signed distance from the volume centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipPlane {
    /// Whether the plane is active.
    pub enabled: bool,
    /// Rotation around the vertical axis, radians.
    pub azimuth: f32,
    /// Elevation above the horizontal plane, radians.
    pub elevation: f32,
    /// Signed distance from the centre, in half-diagonals (`[-1, 1]`).
    pub distance: f32,
    /// Flip the kept side.
    pub flip: bool,
}

impl Default for ClipPlane {
    fn default() -> Self {
        Self { enabled: false, azimuth: 0.0, elevation: 0.0, distance: 0.0, flip: false }
    }
}

impl ClipPlane {
    /// Unit normal of the plane.
    pub fn normal(&self) -> Vec3 {
        let (se, ce) = self.elevation.sin_cos();
        let (sa, ca) = self.azimuth.sin_cos();
        let n = Vec3::new(ce * sa, se, ce * ca);
        if self.flip {
            -n
        } else {
            n
        }
    }

    /// Half-space removed by the plane for a volume of `extent`.
    pub fn half_space(&self, extent: Vec3) -> HalfSpace {
        let radius = extent.length() * 0.5;
        HalfSpace { normal: self.normal(), offset: self.distance.clamp(-1.0, 1.0) * radius }
    }
}

/// Aggregate clipping configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipSettings {
    /// Axis-aligned box.
    pub clip_box: ClipBox,
    /// Oblique plane.
    pub plane: ClipPlane,
    /// View-aligned cut ratio in `[0, 1]`; `1` keeps everything, `0` removes
    /// the whole volume (slider "Cut" of the original viewer).
    pub view_cut: f32,
}

impl Default for ClipSettings {
    fn default() -> Self {
        Self { clip_box: ClipBox::default(), plane: ClipPlane::default(), view_cut: 1.0 }
    }
}

/// Result of clipping a ray segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClippedSegment {
    /// Entry parameter.
    pub t_near: f32,
    /// Exit parameter.
    pub t_far: f32,
    /// `true` if the entry point lies on a clipping plane (a cut surface).
    pub entered_through_cut: bool,
}

impl ClipSettings {
    /// Half-spaces in model space for a volume with model `extent`, given
    /// the viewing direction `view_dir` (model space, from the eye into the
    /// scene).
    pub fn half_spaces(&self, extent: Vec3, view_dir: Vec3) -> Vec<HalfSpace> {
        let mut out = Vec::with_capacity(MAX_HALF_SPACES);
        let b = self.clip_box.normalized();
        let lo = (b.min - Vec3::splat(0.5)) * extent;
        let hi = (b.max - Vec3::splat(0.5)) * extent;
        for (axis, unit) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
            if b.max[axis] < 1.0 {
                out.push(HalfSpace { normal: unit, offset: hi[axis] });
            }
            if b.min[axis] > 0.0 {
                out.push(HalfSpace { normal: -unit, offset: -lo[axis] });
            }
        }
        if self.plane.enabled {
            out.push(self.plane.half_space(extent));
        }
        let view_dir = view_dir.normalize_or_zero();
        if self.view_cut < 1.0 && view_dir != Vec3::ZERO {
            let radius = extent.length() * 0.5;
            // keep dot(d, p) >= -R + (1 - ratio) * 2R  <=>  dot(-d, p) <= R - (1-ratio)*2R
            let c = radius - (1.0 - self.view_cut.clamp(0.0, 1.0)) * 2.0 * radius;
            out.push(HalfSpace { normal: -view_dir, offset: c });
        }
        out
    }

    /// Clips the ray segment `[t_near, t_far]` against `half_spaces`.
    /// Returns `None` if nothing remains.
    pub fn clip_segment(half_spaces: &[HalfSpace], ray: &Ray, t_near: f32, t_far: f32) -> Option<ClippedSegment> {
        let mut seg = ClippedSegment { t_near, t_far, entered_through_cut: false };
        for h in half_spaces {
            let denom = h.normal.dot(ray.direction);
            let dist = h.offset - h.normal.dot(ray.origin);
            if denom.abs() < 1e-8 {
                if dist < 0.0 {
                    return None;
                }
                continue;
            }
            let t = dist / denom;
            if denom > 0.0 {
                seg.t_far = seg.t_far.min(t);
            } else if t > seg.t_near {
                seg.t_near = t;
                seg.entered_through_cut = true;
            }
        }
        (seg.t_near < seg.t_far).then_some(seg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Aabb;

    fn ray_down_z() -> Ray {
        Ray::new(Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 0.0, -1.0))
    }

    #[test]
    fn default_settings_produce_no_planes() {
        let s = ClipSettings::default();
        assert!(s.half_spaces(Vec3::ONE, Vec3::NEG_Z).is_empty());
    }

    #[test]
    fn clip_box_cuts_front() {
        let mut s = ClipSettings::default();
        s.clip_box.max.z = 0.75; // remove z > 0.25 in model space
        let hs = s.half_spaces(Vec3::ONE, Vec3::NEG_Z);
        assert_eq!(hs.len(), 1);
        let r = ray_down_z();
        let (n, f) = Aabb::centered(Vec3::ONE).intersect(&r).unwrap();
        let seg = ClipSettings::clip_segment(&hs, &r, n, f).unwrap();
        assert!((seg.t_near - 1.75).abs() < 1e-5);
        assert!((seg.t_far - 2.5).abs() < 1e-5);
        assert!(seg.entered_through_cut);
    }

    #[test]
    fn clip_box_cuts_back_without_marking_cut() {
        let mut s = ClipSettings::default();
        s.clip_box.min.z = 0.5; // remove z < 0
        let hs = s.half_spaces(Vec3::ONE, Vec3::NEG_Z);
        let r = ray_down_z();
        let seg = ClipSettings::clip_segment(&hs, &r, 1.5, 2.5).unwrap();
        assert!((seg.t_far - 2.0).abs() < 1e-5);
        assert!(!seg.entered_through_cut);
    }

    #[test]
    fn view_cut_extremes() {
        let r = ray_down_z();
        let mut s = ClipSettings::default();
        s.view_cut = 0.0;
        let hs = s.half_spaces(Vec3::ONE, r.direction);
        assert!(ClipSettings::clip_segment(&hs, &r, 1.5, 2.5).is_none());
        s.view_cut = 0.5; // plane through the centre
        let hs = s.half_spaces(Vec3::ONE, r.direction);
        let seg = ClipSettings::clip_segment(&hs, &r, 1.5, 2.5).unwrap();
        assert!((seg.t_near - 2.0).abs() < 1e-5);
    }

    #[test]
    fn oblique_plane_normal_is_unit() {
        let p = ClipPlane { enabled: true, azimuth: 0.7, elevation: -0.3, distance: 0.2, flip: false };
        assert!((p.normal().length() - 1.0).abs() < 1e-6);
        let flipped = ClipPlane { flip: true, ..p };
        assert!((p.normal() + flipped.normal()).length() < 1e-6);
    }

    #[test]
    fn parallel_ray_outside_plane_is_rejected() {
        let hs = [HalfSpace { normal: Vec3::X, offset: 0.0 }];
        let r = Ray::new(Vec3::new(0.5, 0.0, 2.0), Vec3::NEG_Z);
        assert!(ClipSettings::clip_segment(&hs, &r, 0.0, 10.0).is_none());
        let r2 = Ray::new(Vec3::new(-0.5, 0.0, 2.0), Vec3::NEG_Z);
        assert!(ClipSettings::clip_segment(&hs, &r2, 0.0, 10.0).is_some());
    }
}
