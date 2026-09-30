//! CPU reference implementation of `shaders/volume.wgsl`.
//!
//! Serves three purposes:
//! 1. an oracle for GPU tests (images must agree within a small tolerance),
//! 2. ray picking for interactive tools (eraser, 3D probe) using exactly the
//!    visibility model the user sees,
//! 3. a software fallback when no GPU adapter is available.

use glam::{Vec2, Vec3, Vec4};
use mri_domain::clip::ClipSettings;
use mri_domain::color::unit_to_u8;
use mri_domain::{Dims3, Ray, RenderMode, Rgba8, Volume, VoxelMask};
use mri_processing::AmbientOcclusion;
use rayon::prelude::*;

use crate::frame::{pixel_jitter, FrameParams, EARLY_EXIT_ALPHA, MAX_STEPS, REFINE_STEPS, TISSUE_DENSITY_SCALE};

/// Data sampled by the CPU renderer (mirrors the GPU bindings).
#[derive(Clone, Copy)]
pub struct CpuScene<'a> {
    /// Scalar volume.
    pub volume: &'a Volume,
    /// Optional eraser mask.
    pub mask: Option<&'a VoxelMask>,
    /// Optional ambient occlusion volume.
    pub ao: Option<&'a AmbientOcclusion>,
    /// Baked transfer function (256 entries).
    pub lut: &'a [Rgba8],
    /// Optional brick occupancy (for empty-space skipping parity tests).
    pub occupancy: Option<&'a [u8]>,
}

/// Trilinear sampling of a grid with clamp-to-edge, identical to GPU linear
/// filtering of a 3D texture.
pub fn trilinear(dims: Dims3, t: Vec3, fetch: impl Fn(u32, u32, u32) -> f32) -> f32 {
    let g = t * dims.as_vec3() - Vec3::splat(0.5);
    let b = g.floor();
    let f = g - b;
    let cl = |v: f32, n: u32| (v as i64).clamp(0, i64::from(n) - 1) as u32;
    let (x0, y0, z0) = (cl(b.x, dims.x), cl(b.y, dims.y), cl(b.z, dims.z));
    let (x1, y1, z1) = (cl(b.x + 1.0, dims.x), cl(b.y + 1.0, dims.y), cl(b.z + 1.0, dims.z));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c00 = lerp(fetch(x0, y0, z0), fetch(x1, y0, z0), f.x);
    let c10 = lerp(fetch(x0, y1, z0), fetch(x1, y1, z0), f.x);
    let c01 = lerp(fetch(x0, y0, z1), fetch(x1, y0, z1), f.x);
    let c11 = lerp(fetch(x0, y1, z1), fetch(x1, y1, z1), f.x);
    lerp(lerp(c00, c10, f.y), lerp(c01, c11, f.y), f.z)
}

struct Segment {
    t0: f32,
    t1: f32,
    cut: bool,
}

/// Reference renderer bound to a scene and frame parameters.
pub struct CpuRaycaster<'a> {
    scene: CpuScene<'a>,
    p: &'a FrameParams,
}

impl<'a> CpuRaycaster<'a> {
    /// Creates a renderer.
    pub fn new(scene: CpuScene<'a>, params: &'a FrameParams) -> Self {
        Self { scene, p: params }
    }

    fn to_tex(&self, p: Vec3) -> Vec3 {
        p / self.p.extent + Vec3::splat(0.5)
    }

    /// Masked normalised density at model-space point `p`.
    pub fn density(&self, p: Vec3) -> f32 {
        let t = self.to_tex(p);
        let v = self.scene.volume.sample(t);
        match self.scene.mask {
            Some(m) => {
                let d = m.dims();
                let data = m.data();
                v * trilinear(d, t, |i, j, k| f32::from(data[d.index(i, j, k)]) / 255.0)
            }
            None => v,
        }
    }

    fn gradient(&self, p: Vec3) -> Vec3 {
        let d = self.p.extent / self.p.dims;
        let gx = self.density(p + Vec3::new(d.x, 0.0, 0.0)) - self.density(p - Vec3::new(d.x, 0.0, 0.0));
        let gy = self.density(p + Vec3::new(0.0, d.y, 0.0)) - self.density(p - Vec3::new(0.0, d.y, 0.0));
        let gz = self.density(p + Vec3::new(0.0, 0.0, d.z)) - self.density(p - Vec3::new(0.0, 0.0, d.z));
        Vec3::new(gx / d.x, gy / d.y, gz / d.z)
    }

    fn normal_at(&self, p: Vec3, dir: Vec3) -> Vec3 {
        let g = self.gradient(p);
        let len = g.length();
        if len < 1e-6 {
            -dir
        } else {
            -g / len
        }
    }

    fn ambient_occlusion(&self, p: Vec3) -> f32 {
        match (self.p.ao_enabled, self.scene.ao) {
            (true, Some(ao)) => {
                let t = self.to_tex(p) * self.p.ao_scale;
                trilinear(ao.dims, t, |i, j, k| f32::from(ao.data[ao.dims.index(i, j, k)]) / 255.0)
            }
            _ => 1.0,
        }
    }

    fn shade_surface(&self, p: Vec3, dir: Vec3) -> Vec3 {
        let s = &self.p.settings;
        let n = self.normal_at(p, dir);
        let l = -self.p.light_dir;
        let diff = n.dot(l).max(0.0);
        let r = reflect(-l, n);
        let spec = r.dot(-dir).max(0.0).powf(40.0);
        let ao = self.ambient_occlusion(p);
        let base = Vec3::from(s.surface_color_shadow.to_array()).lerp(Vec3::from(s.surface_color_lit.to_array()), diff);
        let b = 0.5 * (s.brightness + 1.5);
        base * b * (0.4 * ao + 0.6 * diff) + Vec3::splat(0.25 * spec * ao)
    }

    fn refine(&self, eye: Vec3, dir: Vec3, a: f32, b: f32, thr: f32) -> f32 {
        let (mut lo, mut hi) = (a, b);
        for _ in 0..REFINE_STEPS {
            let mid = 0.5 * (lo + hi);
            if self.density(eye + dir * mid) > thr {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        0.5 * (lo + hi)
    }

    fn brick_of(&self, p: Vec3) -> Vec3 {
        let bs = self.p.brick_size as f32;
        (self.to_tex(p) * self.p.dims / bs).floor().clamp(Vec3::ZERO, self.p.brick_grid - Vec3::ONE)
    }

    fn skip_empty(&self, eye: Vec3, dir: Vec3, t: f32, base: f32, step: f32) -> f32 {
        let Some(occ) = self.scene.occupancy.filter(|_| self.p.brick_size > 0) else {
            return t;
        };
        let p = eye + dir * t;
        let b = self.brick_of(p);
        let g = self.p.brick_grid;
        let idx = b.x as usize + g.x as usize * (b.y as usize + g.y as usize * b.z as usize);
        if occ.get(idx).copied().unwrap_or(255) >= 128 {
            return t;
        }
        let bs = self.p.brick_size as f32;
        let lo = (b * bs / self.p.dims - Vec3::splat(0.5)) * self.p.extent;
        let hi = ((b + Vec3::ONE) * bs / self.p.dims - Vec3::splat(0.5)) * self.p.extent;
        let safe = Vec3::select(dir.abs().cmplt(Vec3::splat(1e-7)), Vec3::splat(1e-7), dir);
        let tmax = ((lo - p) / safe).max((hi - p) / safe);
        let exit = t + tmax.min_element().max(0.0);
        let k = ((exit - base) / step).ceil();
        (base + k * step).max(t + step)
    }

    fn clip_ray(&self, eye: Vec3, dir: Vec3) -> Option<Segment> {
        let ray = Ray { origin: eye, direction: dir };
        let (t0, t1) = mri_domain::Aabb::centered(self.p.extent).intersect(&ray)?;
        let seg = ClipSettings::clip_segment(&self.p.planes, &ray, t0, t1)?;
        Some(Segment { t0: seg.t_near, t1: seg.t_far, cut: seg.entered_through_cut })
    }

    fn march_tissue(&self, eye: Vec3, dir: Vec3, seg: &Segment, base: f32) -> Vec3 {
        let s = &self.p.settings;
        let step = self.p.step;
        let (low, high, surf) = (s.tissue.low, s.tissue.high, s.tissue.surface);
        let l = -self.p.light_dir;
        let (mut acc, mut alpha, mut surface) = (Vec3::ZERO, 0.0f32, Vec3::ZERO);
        let mut t = base;
        for _ in 0..MAX_STEPS {
            if t > seg.t1 {
                break;
            }
            let nt = self.skip_empty(eye, dir, t, base, step);
            if nt != t {
                t = nt;
                continue;
            }
            let p = eye + dir * t;
            let v = self.density(p);
            if v > surf {
                let th = self.refine(eye, dir, (t - step).max(seg.t0), t, surf);
                surface = self.shade_surface(eye + dir * th, dir);
                break;
            }
            let w = high - low;
            let band = if w > 0.0 { ((v - low).min(high - v) / w).max(0.0) * 2.0 } else { 0.0 };
            if band > 0.0 {
                let a = 1.0 - (-TISSUE_DENSITY_SCALE * s.opacity * band * step).exp();
                let n = self.normal_at(p, dir);
                let lit = 0.5 * n.dot(l).max(0.0) + 0.5;
                let c = Vec3::from(s.tissue_color_low.to_array())
                    .lerp(Vec3::from(s.tissue_color_high.to_array()), ((v - low) / w.max(1e-6)).clamp(0.0, 1.0))
                    * lit;
                acc += (1.0 - alpha) * a * c;
                alpha += (1.0 - alpha) * a;
                if alpha > EARLY_EXIT_ALPHA {
                    break;
                }
            }
            t += step;
        }
        acc * (2.0 * s.brightness) + (1.0 - alpha) * surface
    }

    fn first_iso_hit(&self, eye: Vec3, dir: Vec3, seg: &Segment, base: f32, thr: f32) -> Option<f32> {
        let step = self.p.step;
        let mut t = base;
        for _ in 0..MAX_STEPS {
            if t > seg.t1 {
                break;
            }
            let nt = self.skip_empty(eye, dir, t, base, step);
            if nt != t {
                t = nt;
                continue;
            }
            if self.density(eye + dir * t) > thr {
                return Some(self.refine(eye, dir, (t - step).max(seg.t0), t, thr));
            }
            t += step;
        }
        None
    }

    fn march_mip(&self, eye: Vec3, dir: Vec3, seg: &Segment, base: f32) -> (f32, f32) {
        let step = self.p.step;
        let (mut mx, mut t_max) = (0.0f32, seg.t0);
        let mut t = base;
        for _ in 0..MAX_STEPS {
            if t > seg.t1 {
                break;
            }
            let nt = self.skip_empty(eye, dir, t, base, step);
            if nt != t {
                t = nt;
                continue;
            }
            let v = self.density(eye + dir * t);
            if v > mx {
                mx = v;
                t_max = t;
            }
            t += step;
        }
        (mx, t_max)
    }

    /// Linear lookup in the baked transfer function (matches GPU sampling).
    pub fn tf_lookup(&self, v: f32) -> Vec4 {
        let lut = self.scene.lut;
        if lut.is_empty() {
            return Vec4::ZERO;
        }
        let n = lut.len();
        let u = (v * 255.0 + 0.5) / 256.0;
        let x = u * n as f32 - 0.5;
        let i0 = (x.floor() as i64).clamp(0, n as i64 - 1) as usize;
        let i1 = (x.floor() as i64 + 1).clamp(0, n as i64 - 1) as usize;
        let f = x - x.floor();
        let c = |i: usize| Vec4::from_array(lut[i].map(|b| f32::from(b) / 255.0));
        c(i0).lerp(c(i1), f)
    }

    fn march_tf(&self, eye: Vec3, dir: Vec3, seg: &Segment, base: f32) -> (Vec3, Option<f32>) {
        let s = &self.p.settings;
        let step = self.p.step;
        let l = -self.p.light_dir;
        let exponent = step / mri_domain::RenderSettings::REFERENCE_STEP;
        let (mut acc, mut alpha, mut first) = (Vec3::ZERO, 0.0f32, None);
        let mut t = base;
        for _ in 0..MAX_STEPS {
            if t > seg.t1 {
                break;
            }
            let nt = self.skip_empty(eye, dir, t, base, step);
            if nt != t {
                t = nt;
                continue;
            }
            let p = eye + dir * t;
            let c = self.tf_lookup(self.density(p));
            let a0 = (c.w * s.opacity * 2.0).min(0.999);
            if a0 > 0.0 {
                let a = 1.0 - (1.0 - a0).powf(exponent);
                let n = self.normal_at(p, dir);
                let lit = 0.5 * n.dot(l).max(0.0) + 0.5;
                acc += (1.0 - alpha) * a * c.truncate() * lit;
                alpha += (1.0 - alpha) * a;
                if first.is_none() && alpha > 0.05 {
                    first = Some(t);
                }
                if alpha > EARLY_EXIT_ALPHA {
                    break;
                }
            }
            t += step;
        }
        (acc * (2.0 * s.brightness), first)
    }

    fn primary_ray(&self, ndc: Vec2) -> (Vec3, Vec3) {
        let far = self.p.inv_view_proj.project_point3(ndc.extend(1.0));
        (self.p.eye, (far - self.p.eye).normalize())
    }

    /// Shades one pixel; `(x, y)` are pixel indices of a `w × h` target.
    pub fn shade_pixel(&self, x: u32, y: u32, w: u32, h: u32) -> Vec3 {
        let ndc = Vec2::new((x as f32 + 0.5) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + 0.5) / h as f32 * 2.0);
        let (eye, dir) = self.primary_ray(ndc);
        let Some(seg) = self.clip_ray(eye, dir) else {
            return Vec3::ZERO;
        };
        let jitter = if self.p.jitter { pixel_jitter(x, y) } else { 0.0 };
        let base = seg.t0 + jitter * self.p.step;
        let s = &self.p.settings;
        let mut rgb = match s.mode {
            RenderMode::Tissue => self.march_tissue(eye, dir, &seg, base),
            RenderMode::Isosurface => self
                .first_iso_hit(eye, dir, &seg, base, s.iso_threshold)
                .map(|t| self.shade_surface(eye + dir * t, dir))
                .unwrap_or(Vec3::ZERO),
            RenderMode::Mip => {
                let (mx, _) = self.march_mip(eye, dir, &seg, base);
                mx * Vec3::from(s.surface_color_lit.to_array()) * (2.0 * s.brightness)
            }
            RenderMode::TransferFunction => self.march_tf(eye, dir, &seg, base).0,
        };
        if seg.cut && s.cut_surface_opacity > 0.0 {
            let v = self.density(eye + dir * seg.t0);
            rgb = rgb.lerp(Vec3::splat(v), s.cut_surface_opacity);
        }
        rgb.clamp(Vec3::ZERO, Vec3::ONE)
    }

    /// Renders a full RGBA8 image (row-major, top row first) in parallel.
    pub fn render(&self, width: u32, height: u32) -> Vec<Rgba8> {
        (0..width * height)
            .into_par_iter()
            .map(|i| {
                let c = self.shade_pixel(i % width, i / width, width, height);
                [unit_to_u8(c.x), unit_to_u8(c.y), unit_to_u8(c.z), 255]
            })
            .collect()
    }

    /// Finds the first visible point along the ray through `ndc` according
    /// to the current mode, returning its texture coordinate.
    pub fn pick(&self, ndc: Vec2) -> Option<Vec3> {
        let (eye, dir) = self.primary_ray(ndc);
        let seg = self.clip_ray(eye, dir)?;
        let base = seg.t0;
        let s = &self.p.settings;
        let t = match s.mode {
            RenderMode::Isosurface => self.first_iso_hit(eye, dir, &seg, base, s.iso_threshold),
            RenderMode::Tissue => self.first_iso_hit(eye, dir, &seg, base, s.tissue.low.max(1e-3)),
            RenderMode::Mip => Some(self.march_mip(eye, dir, &seg, base)).filter(|m| m.0 > 0.0).map(|m| m.1),
            RenderMode::TransferFunction => self.march_tf(eye, dir, &seg, base).1,
        }?;
        Some(self.to_tex(eye + dir * t))
    }
}

fn reflect(i: Vec3, n: Vec3) -> Vec3 {
    i - 2.0 * n.dot(i) * n
}

#[cfg(test)]
mod tests {
    use super::*;
    use mri_domain::{ClipSettings, IntensityRange, OrbitCamera, RenderSettings, TransferFunction};

    fn sphere(n: u32) -> Volume {
        let dims = Dims3::new(n, n, n);
        let c = (n as f32 - 1.0) * 0.5;
        let mut vals = Vec::with_capacity(dims.voxel_count());
        for k in 0..n {
            for j in 0..n {
                for i in 0..n {
                    let r = Vec3::new(i as f32 - c, j as f32 - c, k as f32 - c).length() / c;
                    vals.push(if r < 0.6 { 1.0 } else { 0.0 });
                }
            }
        }
        Volume::from_physical(dims, Vec3::ONE, &vals).unwrap()
    }

    fn params(v: &Volume, mode: RenderMode) -> FrameParams {
        let mut s = RenderSettings::default();
        s.mode = mode;
        s.iso_threshold = 0.5;
        let mut p =
            FrameParams::new(v, &OrbitCamera::default(), &s, &ClipSettings::default(), Vec2::splat(32.0), 0, None);
        p.jitter = false;
        p
    }

    #[test]
    fn trilinear_matches_volume_sampler() {
        let v = sphere(9);
        for t in [Vec3::splat(0.5), Vec3::new(0.1, 0.7, 0.33), Vec3::new(-0.2, 1.3, 0.5)] {
            let a = v.sample(t);
            let b = trilinear(v.dims(), t, |i, j, k| v.normalized_clamped(i.into(), j.into(), k.into()));
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn iso_sphere_is_lit_in_centre_and_black_at_corners() {
        let v = sphere(24);
        let lut = TransferFunction::legacy_default().bake(256);
        let p = params(&v, RenderMode::Isosurface);
        let r = CpuRaycaster::new(CpuScene { volume: &v, mask: None, ao: None, lut: &lut, occupancy: None }, &p);
        let img = r.render(32, 32);
        assert!(img[16 * 32 + 16][0] > 60, "{:?}", img[16 * 32 + 16]);
        assert_eq!(img[0], [0, 0, 0, 255]);
    }

    #[test]
    fn pick_hits_sphere_front() {
        let v = sphere(24);
        let lut = TransferFunction::legacy_default().bake(256);
        let p = params(&v, RenderMode::Isosurface);
        let r = CpuRaycaster::new(CpuScene { volume: &v, mask: None, ao: None, lut: &lut, occupancy: None }, &p);
        let hit = r.pick(Vec2::ZERO).expect("hit");
        // default camera looks along +z, so the front of the sphere is at low z
        assert!((hit.x - 0.5).abs() < 0.05 && (hit.y - 0.5).abs() < 0.05);
        assert!(hit.z > 0.15 && hit.z < 0.35, "{hit:?}");
        assert!(r.pick(Vec2::new(0.95, 0.95)).is_none());
    }

    #[test]
    fn mask_removes_material() {
        let v = sphere(24);
        let lut = TransferFunction::legacy_default().bake(256);
        let p = params(&v, RenderMode::Mip);
        let mut mask = VoxelMask::new(v.dims());
        let scene = CpuScene { volume: &v, mask: None, ao: None, lut: &lut, occupancy: None };
        let before = CpuRaycaster::new(scene, &p).shade_pixel(16, 16, 32, 32);
        mask.erase(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::Z,
            mri_domain::EraserBrush { radius_mm: 4.0, depth_mm: 30.0 },
            Vec3::ONE,
        );
        let after = CpuRaycaster::new(CpuScene { mask: Some(&mask), ..scene }, &p).shade_pixel(16, 16, 32, 32);
        assert!(before.x > 0.5);
        assert!(after.x < 0.05, "{after:?}");
    }

    #[test]
    fn tf_lookup_endpoints() {
        let v = sphere(4);
        let lut = TransferFunction::linear_ramp().bake(256);
        let p = params(&v, RenderMode::TransferFunction);
        let r = CpuRaycaster::new(CpuScene { volume: &v, mask: None, ao: None, lut: &lut, occupancy: None }, &p);
        assert!(r.tf_lookup(0.0).w < 1e-6);
        assert!((r.tf_lookup(1.0).w - 1.0).abs() < 1e-6);
        assert!((r.tf_lookup(0.5).w - 0.5).abs() < 0.01);
    }

    #[test]
    fn range_is_used() {
        let _ = IntensityRange::new(0.0, 1.0).unwrap();
    }
}
