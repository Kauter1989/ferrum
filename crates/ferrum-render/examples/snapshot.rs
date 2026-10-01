//! Renders every 3D mode of a DICOM/NIfTI dataset headlessly into PNGs.
//!
//! `cargo run --release -p ferrum-render --example snapshot -- <path> <out_dir>`
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Instant;

use ferrum_domain::{
    ClipSettings, NoProgress, OrbitCamera, RenderMode, RenderSettings, TransferFunction, VolumeRepository,
};
use ferrum_processing::BrickGrid;
use ferrum_render::frame::brick_classifier;
use ferrum_render::gpu::{GpuContext, VolumeRenderer, OFFSCREEN_FORMAT};
use ferrum_render::FrameParams;
use glam::{UVec2, Vec2};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = PathBuf::from(args.get(1).expect("usage: snapshot <input> <out_dir>"));
    let out = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("."));
    std::fs::create_dir_all(&out).unwrap();
    let repo = ferrum_io::CompositeRepository::default();
    let t = Instant::now();
    let series = repo.scan(&[input], &NoProgress).unwrap();
    let s = series.iter().max_by_key(|s| s.dims.z).unwrap();
    let loaded = repo.load(s, &NoProgress).unwrap();
    let v = loaded.volume;
    println!("loaded {:?} spacing {:?} range {:?} in {:?}", v.dims(), v.spacing(), v.range(), t.elapsed());

    let w: u32 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(512);
    let h: u32 = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(512);
    let t = Instant::now();
    let _hist = ferrum_processing::Histogram::compute(&v, 256);
    let bricks = BrickGrid::compute(&v, 8);
    println!("histogram + brick grid: {:?}", t.elapsed());

    let ctx = GpuContext::headless().unwrap();
    println!("adapter: {}", ctx.describe());
    let mut r = VolumeRenderer::new(&ctx.device, &ctx.queue, ctx.caps, OFFSCREEN_FORMAT);
    let t = Instant::now();
    r.set_volume(&ctx.device, &ctx.queue, &v);
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    println!("GPU upload ({:?}): {:?}", ctx.caps.volume_format, t.elapsed());
    let tf = TransferFunction::legacy_default();
    r.set_transfer_function(&ctx.queue, &tf.bake(256));
    let mut cam = OrbitCamera::default();
    cam.rotate(0.5, -0.4);
    let size = UVec2::new(w, h);
    const FRAMES: u32 = 8;
    for mode in RenderMode::ALL {
        for ess in [false, true] {
            let mut s = RenderSettings::default();
            s.mode = mode;
            s.empty_space_skipping = ess;
            r.set_occupancy(&ctx.device, &ctx.queue, bricks.grid_dims(), &bricks.occupancy(brick_classifier(&s, &tf)));
            let p = FrameParams::new(&v, &cam, &s, &ClipSettings::default(), size.as_vec2(), 8, None);
            let t = Instant::now();
            let img = r.render_volume_image(&ctx.device, &ctx.queue, &p, size).unwrap();
            let first = t.elapsed();
            let t = Instant::now();
            for _ in 0..FRAMES {
                r.render_volume_image(&ctx.device, &ctx.queue, &p, size).unwrap();
            }
            let avg = t.elapsed() / FRAMES;
            println!(
                "{mode:?} {w}x{h} ess={ess}: first frame {first:?}, avg {avg:?} ({:.1} fps, incl. readback)",
                1.0 / avg.as_secs_f64()
            );
            if ess {
                let bytes: Vec<u8> = img.into_iter().flatten().collect();
                let name = out.join(format!("{}.png", mode.label().replace(' ', "_").to_lowercase()));
                image::save_buffer(&name, &bytes, size.x, size.y, image::ExtendedColorType::Rgba8).unwrap();
            }
        }
    }
    let axis = ferrum_domain::SliceAxis::Axial;
    let sp = ferrum_render::SliceParams {
        rect: (Vec2::ZERO, size.as_vec2()),
        window: (0.0, 1.0),
        position: 0.5,
        axis: axis.id(),
        nearest: false,
        background: [0.0, 0.0, 0.0, 1.0],
        segments: false,
    };
    r.render_slice_image(&ctx.device, &ctx.queue, &sp, size).unwrap();
    let t = Instant::now();
    for i in 0..FRAMES {
        let sp = ferrum_render::SliceParams { position: (i as f32 + 0.5) / FRAMES as f32, ..sp };
        r.render_slice_image(&ctx.device, &ctx.queue, &sp, size).unwrap();
    }
    println!("2D slice change {w}x{h}: avg {:?} (incl. readback)", t.elapsed() / FRAMES);
    let _ = Vec2::ZERO;
}
