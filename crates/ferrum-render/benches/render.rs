//! Frame-time benchmarks: CPU reference renderer and (if an adapter is
//! available) the GPU ray caster with and without empty-space skipping.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use criterion::{criterion_group, criterion_main, Criterion};
use ferrum_domain::{ClipSettings, Dims3, OrbitCamera, RenderMode, RenderSettings, TransferFunction, Volume};
use ferrum_processing::BrickGrid;
use ferrum_render::cpu::{CpuRaycaster, CpuScene};
use ferrum_render::frame::brick_classifier;
use ferrum_render::gpu::{GpuContext, VolumeRenderer, OFFSCREEN_FORMAT};
use ferrum_render::FrameParams;
use glam::{UVec2, Vec2, Vec3};

fn phantom(n: u32) -> Volume {
    let dims = Dims3::new(n, n, n);
    let c = n as f32 * 0.5;
    let mut vals = Vec::with_capacity(dims.voxel_count());
    for k in 0..n {
        for j in 0..n {
            for i in 0..n {
                let r = Vec3::new(i as f32 - c, j as f32 - c, k as f32 - c).length() / c;
                vals.push(if r < 0.4 {
                    0.9
                } else if r < 0.7 {
                    0.35
                } else {
                    0.0
                });
            }
        }
    }
    Volume::from_physical(dims, Vec3::ONE, &vals).unwrap()
}

fn bench(c: &mut Criterion) {
    let v = phantom(128);
    let tf = TransferFunction::legacy_default();
    let lut = tf.bake(256);
    let cam = OrbitCamera::default();
    let mut g = c.benchmark_group("render");
    g.sample_size(10);
    for mode in RenderMode::ALL {
        let mut s = RenderSettings::default();
        s.mode = mode;
        let p = FrameParams::new(&v, &cam, &s, &ClipSettings::default(), Vec2::splat(128.0), 0, None);
        let scene = CpuScene { volume: &v, mask: None, ao: None, lut: &lut, occupancy: None, segments: None };
        g.bench_function(format!("cpu_128px_{mode:?}"), |b| b.iter(|| CpuRaycaster::new(scene, &p).render(128, 128)));
    }
    if let Ok(ctx) = GpuContext::headless() {
        let mut r = VolumeRenderer::new(&ctx.device, &ctx.queue, ctx.caps, OFFSCREEN_FORMAT);
        r.set_volume(&ctx.device, &ctx.queue, &v);
        r.set_transfer_function(&ctx.queue, &lut);
        let bricks = BrickGrid::compute(&v, 8);
        for mode in RenderMode::ALL {
            for ess in [false, true] {
                let mut s = RenderSettings::default();
                s.mode = mode;
                s.empty_space_skipping = ess;
                r.set_occupancy(
                    &ctx.device,
                    &ctx.queue,
                    bricks.grid_dims(),
                    &bricks.occupancy(brick_classifier(&s, &tf)),
                );
                let p = FrameParams::new(&v, &cam, &s, &ClipSettings::default(), Vec2::splat(256.0), 8, None);
                g.bench_function(format!("gpu_256px_{mode:?}_ess_{ess}"), |b| {
                    b.iter(|| r.render_volume_image(&ctx.device, &ctx.queue, &p, UVec2::splat(256)).unwrap())
                });
            }
        }
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
