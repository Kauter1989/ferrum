//! GPU ↔ CPU parity tests.
//!
//! Every render mode and feature is rendered with the WGSL shader on a real
//! (or software, e.g. lavapipe) adapter and compared with the CPU reference
//! renderer. Tests are skipped when no adapter is available unless
//! `FERRUM_REQUIRE_GPU=1` is set (CI sets it after installing lavapipe).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, OnceLock};

use ferrum_domain::{
    ClipBox, ClipSettings, Dims3, EraserBrush, LabelMap, OrbitCamera, RenderMode, RenderSettings, Rgba8, SliceAxis,
    SliceImage, TransferFunction, Volume, VoxelMask, WindowLevel,
};
use ferrum_processing::{ambient_occlusion::AoParams, AmbientOcclusion, BrickGrid};
use ferrum_render::cpu::{CpuRaycaster, CpuScene, SegmentLayer};
use ferrum_render::frame::brick_classifier;
use ferrum_render::gpu::{GpuContext, VolumeRenderer};
use ferrum_render::{FrameParams, SliceParams};
use glam::{UVec2, Vec2, Vec3};

const SIZE: u32 = 64;

struct Gpu {
    ctx: GpuContext,
    renderer: VolumeRenderer,
}

fn gpu() -> Option<&'static Mutex<Gpu>> {
    static GPU: OnceLock<Option<Mutex<Gpu>>> = OnceLock::new();
    GPU.get_or_init(|| match GpuContext::headless() {
        Ok(ctx) => {
            eprintln!("GPU tests on {}", ctx.describe());
            let renderer = VolumeRenderer::new(&ctx.device, &ctx.queue, ctx.caps, ferrum_render::gpu::OFFSCREEN_FORMAT);
            Some(Mutex::new(Gpu { ctx, renderer }))
        }
        Err(e) => {
            assert!(std::env::var("FERRUM_REQUIRE_GPU").is_err(), "GPU required but unavailable: {e}");
            eprintln!("skipping GPU tests: {e}");
            None
        }
    })
    .as_ref()
}

/// Nested spheres with anisotropic spacing: outer shell 0.35, core 0.9.
fn phantom() -> Volume {
    let dims = Dims3::new(40, 36, 24);
    let mut vals = Vec::with_capacity(dims.voxel_count());
    let spacing = Vec3::new(1.0, 1.1, 1.6);
    let c = dims.as_vec3() * spacing * 0.5;
    for k in 0..dims.z {
        for j in 0..dims.y {
            for i in 0..dims.x {
                let p = (Vec3::new(i as f32, j as f32, k as f32) + 0.5) * spacing - c;
                let r = p.length() / c.min_element();
                let v = if r < 0.45 {
                    0.9
                } else if r < 0.8 {
                    0.35
                } else {
                    0.02
                };
                vals.push(v);
            }
        }
    }
    Volume::from_physical(dims, spacing, &vals).unwrap()
}

fn settings(mode: RenderMode) -> RenderSettings {
    let mut s = RenderSettings::default();
    s.mode = mode;
    s.iso_threshold = 0.5;
    s.quality = 0.5;
    s.tissue = ferrum_domain::TissueThresholds::new(0.2, 0.5, 0.7);
    s
}

fn camera() -> OrbitCamera {
    let mut c = OrbitCamera::default();
    c.rotate(0.6, 0.35);
    c.distance = 1.1;
    c
}

struct Scene {
    volume: Volume,
    tf: TransferFunction,
    mask: Option<VoxelMask>,
    ao: Option<AmbientOcclusion>,
    ess: bool,
    labels: Option<(LabelMap, [Rgba8; 256])>,
}

impl Scene {
    fn new() -> Self {
        Self { volume: phantom(), tf: TransferFunction::bone(0.3), mask: None, ao: None, ess: false, labels: None }
    }

    /// Phantom with two segments: a translucent red block on one side and
    /// an opaque green ball off-centre inside the core.
    fn segmented() -> Self {
        let mut scene = Self::new();
        scene.labels = Some(segment_fixture(scene.volume.dims()));
        scene
    }
}

fn segment_fixture(dims: Dims3) -> (LabelMap, [Rgba8; 256]) {
    let mut data = vec![0u8; dims.voxel_count()];
    for k in 0..dims.z {
        for j in 0..dims.y {
            for i in 0..dims.x {
                let ball = Vec3::new(i as f32 - 24.0, j as f32 - 16.0, k as f32 - 12.0).length() < 5.0;
                let block = (4..16).contains(&i) && (8..28).contains(&j) && (4..20).contains(&k);
                data[dims.index(i, j, k)] = if ball {
                    2
                } else if block {
                    1
                } else {
                    0
                };
            }
        }
    }
    let mut lut = [[0u8; 4]; 256];
    lut[1] = [230, 60, 50, 150];
    lut[2] = [60, 220, 90, 255];
    (LabelMap::from_data(dims, data).unwrap(), lut)
}

/// Mean absolute difference and fraction of pixels differing by > 24/255.
fn compare(a: &[[u8; 4]], b: &[[u8; 4]]) -> (f64, f64) {
    assert_eq!(a.len(), b.len());
    let mut sum = 0u64;
    let mut bad = 0usize;
    for (x, y) in a.iter().zip(b) {
        let d = (0..3).map(|c| (i32::from(x[c]) - i32::from(y[c])).unsigned_abs()).max().unwrap_or(0);
        sum += u64::from(d);
        if d > 24 {
            bad += 1;
        }
    }
    (sum as f64 / a.len() as f64, bad as f64 / a.len() as f64)
}

type ImagePair = (Vec<[u8; 4]>, Vec<[u8; 4]>);

fn render_both(scene: &Scene, s: &RenderSettings, clip: &ClipSettings) -> Option<ImagePair> {
    let gpu = gpu()?;
    let mut g = gpu.lock().unwrap();
    let Gpu { ctx, renderer } = &mut *g;
    let (device, queue) = (&ctx.device, &ctx.queue);
    renderer.set_volume(device, queue, &scene.volume);
    let lut = scene.tf.bake(256);
    renderer.set_transfer_function(queue, &lut);
    if let Some(m) = &scene.mask {
        renderer.update_mask(device, queue, m, None);
    }
    renderer.set_ambient_occlusion(device, queue, scene.ao.as_ref());
    renderer.update_labels(device, queue, scene.labels.as_ref().map(|l| &l.0), None);
    if let Some((_, seg_lut)) = &scene.labels {
        renderer.set_segment_colors(queue, seg_lut);
    }
    let bricks = BrickGrid::compute(&scene.volume, 8);
    let occupancy = bricks.occupancy(brick_classifier(s, &scene.tf));
    if scene.ess {
        renderer.set_occupancy(device, queue, bricks.grid_dims(), &occupancy);
    }
    let mut s = *s;
    s.empty_space_skipping = scene.ess;
    let mut p = FrameParams::new(
        &scene.volume,
        &camera(),
        &s,
        clip,
        Vec2::splat(SIZE as f32),
        8,
        scene.ao.as_ref().map(|a| a.tex_scale),
    )
    .with_segments(scene.labels.is_some());
    p.jitter = false;
    let gpu_img = renderer.render_volume_image(device, queue, &p, UVec2::splat(SIZE)).unwrap();
    let cpu_scene = CpuScene {
        volume: &scene.volume,
        mask: scene.mask.as_ref(),
        ao: scene.ao.as_ref(),
        lut: &lut,
        occupancy: scene.ess.then_some(occupancy.as_slice()),
        segments: scene.labels.as_ref().map(|(labels, lut)| SegmentLayer { labels, lut }),
    };
    let cpu_img = CpuRaycaster::new(cpu_scene, &p).render(SIZE, SIZE);
    Some((gpu_img, cpu_img))
}

fn assert_parity(name: &str, scene: &Scene, s: &RenderSettings, clip: &ClipSettings) {
    let Some((g, c)) = render_both(scene, s, clip) else {
        return;
    };
    let (mean, bad) = compare(&g, &c);
    let lit = g.iter().filter(|p| p[0] > 10 || p[1] > 10 || p[2] > 10).count();
    eprintln!("{name}: mean diff {mean:.3}, bad {:.2}%, lit px {lit}", bad * 100.0);
    assert!(lit > (SIZE * SIZE / 16) as usize, "{name}: image is (almost) empty");
    assert!(mean < 2.5, "{name}: mean difference {mean}");
    assert!(bad < 0.02, "{name}: {:.2}% pixels differ", bad * 100.0);
}

#[test]
fn isosurface_matches_cpu() {
    assert_parity("iso", &Scene::new(), &settings(RenderMode::Isosurface), &ClipSettings::default());
}

#[test]
fn tissue_matches_cpu() {
    assert_parity("tissue", &Scene::new(), &settings(RenderMode::Tissue), &ClipSettings::default());
}

#[test]
fn mip_matches_cpu() {
    assert_parity("mip", &Scene::new(), &settings(RenderMode::Mip), &ClipSettings::default());
}

#[test]
fn transfer_function_matches_cpu() {
    assert_parity("tf", &Scene::new(), &settings(RenderMode::TransferFunction), &ClipSettings::default());
}

#[test]
fn clipping_and_cut_surface_match_cpu() {
    let mut clip = ClipSettings::default();
    clip.clip_box = ClipBox { min: Vec3::new(0.0, 0.0, 0.0), max: Vec3::new(1.0, 0.6, 1.0) };
    clip.view_cut = 0.6;
    let mut s = settings(RenderMode::Isosurface);
    s.cut_surface_opacity = 0.5;
    assert_parity("clip", &Scene::new(), &s, &clip);
}

#[test]
fn oblique_plane_matches_cpu() {
    let mut clip = ClipSettings::default();
    clip.plane = ferrum_domain::ClipPlane { enabled: true, azimuth: 0.8, elevation: 0.3, distance: 0.1, flip: false };
    assert_parity("plane", &Scene::new(), &settings(RenderMode::Tissue), &clip);
}

#[test]
fn eraser_mask_matches_cpu() {
    let mut scene = Scene::new();
    let mut mask = VoxelMask::new(scene.volume.dims());
    mask.erase(
        Vec3::new(0.5, 0.5, 0.0),
        Vec3::Z,
        EraserBrush { radius_mm: 8.0, depth_mm: 20.0 },
        scene.volume.spacing(),
    );
    scene.mask = Some(mask);
    assert_parity("mask", &scene, &settings(RenderMode::Isosurface), &ClipSettings::default());
}

#[test]
fn ambient_occlusion_matches_cpu() {
    let mut scene = Scene::new();
    scene.ao = Some(AmbientOcclusion::compute(
        &scene.volume,
        AoParams { threshold: 0.5, radius: 2, strength: 1.0, max_dim: 32 },
    ));
    let mut s = settings(RenderMode::Isosurface);
    s.ambient_occlusion = true;
    assert_parity("ao", &scene, &s, &ClipSettings::default());
}

#[test]
fn empty_space_skipping_does_not_change_the_image() {
    for mode in RenderMode::ALL {
        let s = settings(mode);
        let mut scene = Scene::new();
        let Some((plain, _)) = render_both(&scene, &s, &ClipSettings::default()) else {
            return;
        };
        scene.ess = true;
        let (skipped, cpu_skipped) = render_both(&scene, &s, &ClipSettings::default()).unwrap();
        let (mean, bad) = compare(&plain, &skipped);
        eprintln!("ess {mode:?}: mean {mean:.3} bad {bad:.4}");
        assert!(mean < 1.0 && bad < 0.01, "{mode:?}: ESS changed the GPU image");
        let (mean, bad) = compare(&skipped, &cpu_skipped);
        assert!(mean < 2.5 && bad < 0.02, "{mode:?}: ESS parity with CPU");
    }
}

#[test]
fn segment_overlay_matches_cpu_in_every_mode() {
    for mode in RenderMode::ALL {
        let name = format!("segments {mode:?}");
        assert_parity(&name, &Scene::segmented(), &settings(mode), &ClipSettings::default());
    }
}

#[test]
fn segments_change_the_image_and_respect_the_eraser() {
    let s = settings(RenderMode::Isosurface);
    let Some((plain, _)) = render_both(&Scene::new(), &s, &ClipSettings::default()) else {
        return;
    };
    let (seg, _) = render_both(&Scene::segmented(), &s, &ClipSettings::default()).unwrap();
    let (mean, _) = compare(&plain, &seg);
    assert!(mean > 2.0, "segments are invisible (mean diff {mean})");
    let mut erased = Scene::segmented();
    let mut mask = VoxelMask::new(erased.volume.dims());
    let brush = EraserBrush { radius_mm: 12.0, depth_mm: 60.0 };
    mask.erase(Vec3::new(0.5, 0.5, 0.0), Vec3::Z, brush, erased.volume.spacing());
    erased.mask = Some(mask);
    let (g, c) = render_both(&erased, &s, &ClipSettings::default()).unwrap();
    let (mean, bad) = compare(&g, &c);
    assert!(mean < 2.5 && bad < 0.02, "eraser + segments parity: {mean} {bad}");
    assert!(compare(&seg, &g).0 > 1.0, "the eraser must hide segments too");
}

#[test]
fn slice_overlay_fills_and_outlines_segments() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut g = gpu.lock().unwrap();
    let Gpu { ctx, renderer } = &mut *g;
    let volume = phantom();
    renderer.set_volume(&ctx.device, &ctx.queue, &volume);
    let (labels, lut) = segment_fixture(volume.dims());
    renderer.update_labels(&ctx.device, &ctx.queue, Some(&labels), None);
    renderer.set_segment_colors(&ctx.queue, &lut);
    let (w, h) = (volume.dims().x * 4, volume.dims().y * 4);
    let params = SliceParams {
        rect: (Vec2::ZERO, Vec2::new(w as f32, h as f32)),
        window: (0.0, 1.0),
        position: (10.0 + 0.5) / volume.dims().z as f32,
        axis: SliceAxis::Axial.id(),
        nearest: true,
        background: [0.0, 0.0, 0.0, 1.0],
        segments: true,
    };
    let on = renderer.render_slice_image(&ctx.device, &ctx.queue, &params, UVec2::new(w, h)).unwrap();
    let off = renderer
        .render_slice_image(&ctx.device, &ctx.queue, &SliceParams { segments: false, ..params }, UVec2::new(w, h))
        .unwrap();
    let px = |img: &[[u8; 4]], i: u32, j: u32| img[(j * w + i) as usize];
    // voxel (10, 18) is inside the block: fill = grey blended with red at alpha 150/255
    let (fi, fj) = (10 * 4 + 2, 18 * 4 + 2);
    let grey = f32::from(px(&off, fi, fj)[0]);
    let a = 150.0 / 255.0;
    let expect = |c: u8| (grey + (f32::from(c) - grey) * a).round();
    let got = px(&on, fi, fj);
    for (ch, c) in [(0usize, 230u8), (1, 60), (2, 50)] {
        assert!((f32::from(got[ch]) - expect(c)).abs() <= 2.0, "fill {got:?}");
    }
    // first pixel column of the block (voxel i = 4) is the outline: full colour
    assert_eq!(px(&on, 4 * 4, fj)[..3], [230, 60, 50]);
    // outside every segment nothing changes
    assert_eq!(px(&on, 1, 1), px(&off, 1, 1));
    renderer.update_labels(&ctx.device, &ctx.queue, None, None);
}

#[test]
fn slice_rendering_matches_domain_extraction() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut g = gpu.lock().unwrap();
    let Gpu { ctx, renderer } = &mut *g;
    let volume = phantom();
    renderer.set_volume(&ctx.device, &ctx.queue, &volume);
    let window = WindowLevel::new(0.5, 1.0);
    for axis in SliceAxis::ALL {
        let index = axis.slice_count(&volume) / 2;
        let img = SliceImage::extract(&volume, axis, index).unwrap();
        let (w, h) = (img.width, img.height);
        let (lo, hi) = window.normalized_bounds(volume.range());
        let params = SliceParams {
            rect: (Vec2::ZERO, Vec2::new(w as f32, h as f32)),
            window: (lo, hi),
            position: axis.slice_position(&volume, index),
            axis: axis.id(),
            nearest: true,
            background: [0.0, 0.0, 0.0, 1.0],
            segments: false,
        };
        let out = renderer.render_slice_image(&ctx.device, &ctx.queue, &params, UVec2::new(w, h)).unwrap();
        let mut worst = 0i32;
        for (px, &stored) in out.iter().zip(&img.pixels) {
            let expected = ferrum_domain::color::unit_to_u8(window.apply(volume.range().from_storage(stored)));
            worst = worst.max((i32::from(px[0]) - i32::from(expected)).abs());
        }
        assert!(worst <= 2, "{axis:?}: max deviation {worst}");
    }
}

#[test]
fn oversized_volumes_are_downsampled_to_fit_the_device() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut g = gpu.lock().unwrap();
    let Gpu { ctx, renderer } = &mut *g;
    // Emulate a device with a tiny 3D texture limit.
    let caps = ferrum_render::gpu::GpuCaps { max_texture_3d: 16, ..ctx.caps };
    let mut small = VolumeRenderer::new(&ctx.device, &ctx.queue, caps, ferrum_render::gpu::OFFSCREEN_FORMAT);
    let volume = phantom(); // 40×36×24 > 16
    small.set_volume(&ctx.device, &ctx.queue, &volume);
    renderer.set_volume(&ctx.device, &ctx.queue, &volume);
    let mut s = settings(RenderMode::Mip);
    s.empty_space_skipping = false;
    let mut p = FrameParams::new(&volume, &camera(), &s, &ClipSettings::default(), Vec2::splat(32.0), 8, None);
    p.jitter = false;
    let a = small.render_volume_image(&ctx.device, &ctx.queue, &p, UVec2::splat(32)).unwrap();
    let b = renderer.render_volume_image(&ctx.device, &ctx.queue, &p, UVec2::splat(32)).unwrap();
    let (mean, _) = compare(&a, &b);
    assert!(mean < 25.0, "downsampled render diverges too much: {mean}");
    assert!(a.iter().any(|p| p[0] > 40), "downsampled render is empty");
}
