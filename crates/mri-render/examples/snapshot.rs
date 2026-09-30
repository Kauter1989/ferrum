//! Renders every 3D mode of a DICOM/NIfTI dataset headlessly into PNGs.
//!
//! `cargo run --release -p mri-render --example snapshot -- <path> <out_dir>`
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Instant;

use glam::{UVec2, Vec2};
use mri_domain::{
    ClipSettings, NoProgress, OrbitCamera, RenderMode, RenderSettings, TransferFunction, VolumeRepository,
};
use mri_processing::BrickGrid;
use mri_render::frame::brick_classifier;
use mri_render::gpu::{GpuContext, VolumeRenderer, OFFSCREEN_FORMAT};
use mri_render::FrameParams;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = PathBuf::from(args.get(1).expect("usage: snapshot <input> <out_dir>"));
    let out = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("."));
    std::fs::create_dir_all(&out).unwrap();
    let repo = mri_io::CompositeRepository::default();
    let t = Instant::now();
    let series = repo.scan(&[input], &NoProgress).unwrap();
    let s = series.iter().max_by_key(|s| s.dims.z).unwrap();
    let loaded = repo.load(s, &NoProgress).unwrap();
    let v = loaded.volume;
    println!("loaded {:?} spacing {:?} range {:?} in {:?}", v.dims(), v.spacing(), v.range(), t.elapsed());

    let ctx = GpuContext::headless().unwrap();
    println!("adapter: {}", ctx.describe());
    let mut r = VolumeRenderer::new(&ctx.device, &ctx.queue, ctx.caps, OFFSCREEN_FORMAT);
    r.set_volume(&ctx.device, &ctx.queue, &v);
    let tf = TransferFunction::legacy_default();
    r.set_transfer_function(&ctx.queue, &tf.bake(256));
    let bricks = BrickGrid::compute(&v, 8);
    let mut cam = OrbitCamera::default();
    cam.rotate(0.5, -0.4);
    let size = UVec2::new(512, 512);
    for mode in RenderMode::ALL {
        let mut s = RenderSettings::default();
        s.mode = mode;
        r.set_occupancy(&ctx.device, &ctx.queue, bricks.grid_dims(), &bricks.occupancy(brick_classifier(&s, &tf)));
        let p = FrameParams::new(&v, &cam, &s, &ClipSettings::default(), size.as_vec2(), 8, None);
        let t = Instant::now();
        let img = r.render_volume_image(&ctx.device, &ctx.queue, &p, size).unwrap();
        println!("{mode:?}: {:?}", t.elapsed());
        let bytes: Vec<u8> = img.into_iter().flatten().collect();
        let name = out.join(format!("{}.png", mode.label().replace(' ', "_").to_lowercase()));
        image::save_buffer(&name, &bytes, size.x, size.y, image::ExtendedColorType::Rgba8).unwrap();
    }
    let _ = Vec2::ZERO;
}
