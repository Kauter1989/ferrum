//! Per-frame render parameters derived purely from domain state.
//!
//! [`FrameParams`] is the single source of truth consumed by both the GPU
//! shader (packed into [`VolumeUniforms`]) and the CPU reference renderer,
//! which guarantees that both implement the same image formation model.

use bytemuck::{Pod, Zeroable};
use ferrum_domain::clip::{HalfSpace, MAX_HALF_SPACES};
use ferrum_domain::{ClipSettings, OrbitCamera, RenderMode, RenderSettings, Rgb, TransferFunction, Volume};
use glam::{Mat4, Vec2, Vec3, Vec4};

/// Density scale of the tissue band (matches the original viewer's
/// `OPACITY_SCALE = 175`).
pub const TISSUE_DENSITY_SCALE: f32 = 175.0;
/// Maximum number of ray-marching iterations per pixel.
pub const MAX_STEPS: u32 = 4096;
/// Opacity at which front-to-back compositing terminates early.
pub const EARLY_EXIT_ALPHA: f32 = 0.97;
/// Number of bisection steps refining an isosurface hit.
pub const REFINE_STEPS: u32 = 6;

/// Everything needed to render one frame of the 3D view.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameParams {
    /// Inverse of `projection × view` (model space = world space).
    pub inv_view_proj: Mat4,
    /// Eye position in model space.
    pub eye: Vec3,
    /// Model-space box size.
    pub extent: Vec3,
    /// Volume dimensions in voxels.
    pub dims: Vec3,
    /// Direction the light travels.
    pub light_dir: Vec3,
    /// User settings.
    pub settings: RenderSettings,
    /// Ray-marching step in model units.
    pub step: f32,
    /// Clipping half-spaces.
    pub planes: Vec<HalfSpace>,
    /// Brick edge length in voxels (0 disables empty-space skipping).
    pub brick_size: u32,
    /// Brick grid dimensions.
    pub brick_grid: Vec3,
    /// AO texture-coordinate scale.
    pub ao_scale: Vec3,
    /// Whether an AO volume is available and enabled.
    pub ao_enabled: bool,
    /// Per-pixel start offset jitter (reduces wood-grain artefacts).
    pub jitter: bool,
}

impl FrameParams {
    /// Builds parameters for a view of `size` pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        volume: &Volume,
        camera: &OrbitCamera,
        settings: &RenderSettings,
        clip: &ClipSettings,
        size: Vec2,
        brick_size: u32,
        ao_scale: Option<Vec3>,
    ) -> Self {
        let aspect = size.x / size.y.max(1.0);
        let extent = volume.model_extent();
        let dims = volume.dims().as_vec3();
        let brick_grid = if brick_size > 0 { (dims / brick_size as f32).ceil() } else { Vec3::ONE };
        Self {
            inv_view_proj: camera.view_projection(aspect).inverse(),
            eye: camera.eye(),
            extent,
            dims,
            light_dir: camera.light_dir(),
            settings: *settings,
            step: settings.step_size(),
            planes: clip.half_spaces(extent, camera.view_dir()),
            brick_size: if settings.empty_space_skipping { brick_size } else { 0 },
            brick_grid,
            ao_scale: ao_scale.unwrap_or(Vec3::ONE),
            ao_enabled: settings.ambient_occlusion && ao_scale.is_some(),
            jitter: true,
        }
    }

    /// Packs the parameters for the GPU.
    pub fn uniforms(&self, viewport: Vec4) -> VolumeUniforms {
        let s = &self.settings;
        let v4 = |c: Rgb, w: f32| [c.r, c.g, c.b, w];
        let mut planes = [[0.0f32; 4]; MAX_HALF_SPACES];
        for (dst, p) in planes.iter_mut().zip(self.planes.iter()) {
            *dst = p.to_vec4().to_array();
        }
        VolumeUniforms {
            inv_view_proj: self.inv_view_proj.to_cols_array_2d(),
            eye_step: self.eye.extend(self.step).to_array(),
            extent_opacity: self.extent.extend(s.opacity).to_array(),
            dims_brightness: self.dims.extend(s.brightness).to_array(),
            light_iso: self.light_dir.extend(s.iso_threshold).to_array(),
            tissue: [s.tissue.low, s.tissue.high, s.tissue.surface, s.cut_surface_opacity],
            color_low: v4(s.tissue_color_low, f32::from(u8::from(self.ao_enabled))),
            color_high: v4(s.tissue_color_high, f32::from(u8::from(self.brick_size > 0))),
            surf_lit: v4(s.surface_color_lit, self.planes.len().min(MAX_HALF_SPACES) as f32),
            surf_shadow: v4(s.surface_color_shadow, self.brick_size as f32),
            bricks_jitter: self.brick_grid.extend(f32::from(u8::from(self.jitter))).to_array(),
            ao_scale: self.ao_scale.extend(RenderSettings::REFERENCE_STEP).to_array(),
            viewport: viewport.to_array(),
            planes,
        }
    }

    /// Shader pipeline variant for the render mode.
    pub fn mode_id(&self) -> u32 {
        mode_id(self.settings.mode)
    }
}

/// Numeric id of a render mode as used by the shader's `MODE` constant.
pub fn mode_id(mode: RenderMode) -> u32 {
    match mode {
        RenderMode::Tissue => 0,
        RenderMode::Isosurface => 1,
        RenderMode::Mip => 2,
        RenderMode::TransferFunction => 3,
    }
}

/// GPU uniform block of the volume shader (layout mirrors `volume.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct VolumeUniforms {
    /// Inverse view-projection matrix.
    pub inv_view_proj: [[f32; 4]; 4],
    /// Eye xyz, step w.
    pub eye_step: [f32; 4],
    /// Extent xyz, opacity w.
    pub extent_opacity: [f32; 4],
    /// Dims xyz, brightness w.
    pub dims_brightness: [f32; 4],
    /// Light direction xyz, iso threshold w.
    pub light_iso: [f32; 4],
    /// Tissue low/high/surface, cut-surface opacity.
    pub tissue: [f32; 4],
    /// Tissue low colour, AO flag.
    pub color_low: [f32; 4],
    /// Tissue high colour, empty-space-skipping flag.
    pub color_high: [f32; 4],
    /// Lit surface colour, plane count.
    pub surf_lit: [f32; 4],
    /// Shadowed surface colour, brick size.
    pub surf_shadow: [f32; 4],
    /// Brick grid dims, jitter flag.
    pub bricks_jitter: [f32; 4],
    /// AO scale xyz, reference step w.
    pub ao_scale: [f32; 4],
    /// Viewport x, y, width, height in pixels.
    pub viewport: [f32; 4],
    /// Clip half-spaces `(n, offset)`.
    pub planes: [[f32; 4]; MAX_HALF_SPACES],
}

/// Returns the empty-space classifier for the given mode: a brick whose
/// value range is `[min, max]` must be sampled iff the closure returns true.
/// Delegates to the domain rule [`RenderSettings::range_visible`].
pub fn brick_classifier<'a>(
    settings: &'a RenderSettings,
    tf: &'a TransferFunction,
) -> impl Fn(f32, f32) -> bool + Sync + 'a {
    move |min: f32, max: f32| settings.range_visible(min, max, tf)
}

/// Deterministic per-pixel hash shared by the shader and the CPU renderer
/// (PCG output permutation).
pub fn pcg_hash(x: u32) -> u32 {
    let state = x.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

/// Start-offset jitter in `[0, 1]` for pixel `(x, y)`.
pub fn pixel_jitter(x: u32, y: u32) -> f32 {
    pcg_hash(x.wrapping_add(y.wrapping_mul(7919))) as f32 / u32::MAX as f32
}

/// 2D slice rendering parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceParams {
    /// Image rectangle `(min, max)` in target pixels.
    pub rect: (Vec2, Vec2),
    /// Window bounds in normalised intensity.
    pub window: (f32, f32),
    /// Normalised slice position.
    pub position: f32,
    /// Slice orientation id.
    pub axis: u32,
    /// Use nearest-neighbour sampling.
    pub nearest: bool,
    /// Background colour.
    pub background: [f32; 4],
}

/// GPU uniform block of the slice shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SliceUniforms {
    /// Image rect min.xy, max.xy (pixels).
    pub rect: [f32; 4],
    /// Window lo, hi, slice position, axis id.
    pub window: [f32; 4],
    /// Nearest flag and padding.
    pub options: [f32; 4],
    /// Background colour.
    pub background: [f32; 4],
}

impl SliceParams {
    /// Packs the parameters for the GPU.
    pub fn uniforms(&self) -> SliceUniforms {
        SliceUniforms {
            rect: [self.rect.0.x, self.rect.0.y, self.rect.1.x, self.rect.1.y],
            window: [self.window.0, self.window.1, self.position, self.axis as f32],
            options: [f32::from(u8::from(self.nearest)), 0.0, 0.0, 0.0],
            background: self.background,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::{Dims3, IntensityRange};

    fn vol() -> Volume {
        let d = Dims3::new(8, 8, 4);
        Volume::new(d, Vec3::new(1.0, 1.0, 2.0), IntensityRange::new(0.0, 1.0).unwrap(), vec![0; d.voxel_count()])
            .unwrap()
    }

    #[test]
    fn uniform_layout_is_stable() {
        // 4x4 matrix + 12 vec4 + 8 planes = 16 + 48 + 32 floats
        assert_eq!(std::mem::size_of::<VolumeUniforms>(), (16 + 48 + 32) * 4);
        assert_eq!(std::mem::size_of::<SliceUniforms>(), 64);
    }

    #[test]
    fn params_follow_settings() {
        let mut s = RenderSettings::default();
        s.empty_space_skipping = false;
        s.ambient_occlusion = true;
        let p = FrameParams::new(
            &vol(),
            &OrbitCamera::default(),
            &s,
            &ClipSettings::default(),
            Vec2::new(100.0, 50.0),
            8,
            None,
        );
        assert_eq!(p.brick_size, 0);
        assert!(!p.ao_enabled, "AO requires an AO volume");
        assert_eq!(p.extent, Vec3::ONE);
        let u = p.uniforms(Vec4::new(0.0, 0.0, 100.0, 50.0));
        assert_eq!(u.color_high[3], 0.0);
        assert_eq!(u.surf_lit[3], 0.0);
        assert_eq!(u.eye_step[3], s.step_size());
    }

    #[test]
    fn inverse_view_projection_unprojects_centre_to_view_axis() {
        let cam = OrbitCamera::default();
        let p = FrameParams::new(
            &vol(),
            &cam,
            &RenderSettings::default(),
            &ClipSettings::default(),
            Vec2::splat(64.0),
            8,
            None,
        );
        let far = p.inv_view_proj.project_point3(Vec3::new(0.0, 0.0, 1.0));
        let dir = (far - p.eye).normalize();
        assert!((dir - cam.view_dir()).length() < 1e-4);
    }

    #[test]
    fn classifier_per_mode() {
        let mut s = RenderSettings::default();
        let tf = TransferFunction::legacy_default();
        s.mode = RenderMode::Isosurface;
        assert!(!brick_classifier(&s, &tf)(0.0, s.iso_threshold - 0.01));
        assert!(brick_classifier(&s, &tf)(0.0, s.iso_threshold + 0.01));
        s.mode = RenderMode::TransferFunction;
        assert!(!brick_classifier(&s, &tf)(62.0 / 255.0, 110.0 / 255.0));
        assert!(brick_classifier(&s, &tf)(0.3, 0.9));
        s.mode = RenderMode::Mip;
        assert!(!brick_classifier(&s, &tf)(0.0, 0.0));
    }

    #[test]
    fn jitter_is_deterministic_and_in_range() {
        assert_eq!(pixel_jitter(3, 4), pixel_jitter(3, 4));
        assert_ne!(pixel_jitter(3, 4), pixel_jitter(4, 3));
        for i in 0..1000 {
            let j = pixel_jitter(i, i * 3);
            assert!((0.0..=1.0).contains(&j));
        }
    }
}
