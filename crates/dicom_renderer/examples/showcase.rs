//! Renders the README screenshots from a real dataset, headlessly.
//!
//! ```text
//! cargo run --release --example showcase -- <volume> <out_dir>
//! ```
//!
//! Every scene is configured through the application layer and the whole
//! window is rendered through wgpu, so the images show exactly what the
//! desktop app displays.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use dicom_renderer::ViewerApp;
use egui_kittest::Harness;
use glam::{Vec2, Vec3};
use mri_app::{ViewMode, Viewer};
use mri_domain::{
    Annotation, ClipBox, CtPreset, RenderMode, SliceAxis, SliceKey, TransferFunction, ViewPreset, WindowPreset,
};

struct Scene {
    name: &'static str,
    setup: fn(&mut Viewer),
}

fn reset(v: &mut Viewer) {
    v.volume.settings = Default::default();
    v.volume.clip = Default::default();
    v.volume.camera = Default::default();
    v.volume.camera.look_from(ViewPreset::Anterior);
    v.volume.camera.distance = 1.35;
    v.clear_annotations();
}

fn ct_tf(v: &mut Viewer, preset: CtPreset) {
    let range = v.dataset().unwrap().volume.range();
    v.set_transfer_function(TransferFunction::ct_preset(preset, range));
}

fn scenes() -> Vec<Scene> {
    vec![
        Scene {
            name: "mpr",
            setup: |v| {
                reset(v);
                v.view_mode = ViewMode::Mpr;
                v.apply_window_preset(WindowPreset::Lung);
                v.volume.settings.mode = RenderMode::TransferFunction;
                ct_tf(v, CtPreset::SoftTissueBone);
            },
        },
        Scene {
            name: "volume_soft_tissue",
            setup: |v| {
                reset(v);
                v.view_mode = ViewMode::Volume3d;
                v.volume.settings.mode = RenderMode::TransferFunction;
                v.volume.settings.quality = 0.8;
                ct_tf(v, CtPreset::SoftTissueBone);
            },
        },
        Scene {
            name: "volume_lung_vessels",
            setup: |v| {
                reset(v);
                v.view_mode = ViewMode::Volume3d;
                v.volume.settings.mode = RenderMode::TransferFunction;
                v.volume.settings.quality = 0.8;
                ct_tf(v, CtPreset::LungVessels);
                // remove the anterior chest wall to expose the lungs
                v.volume.clip.clip_box = ClipBox { min: Vec3::new(0.0, 0.42, 0.0), max: Vec3::ONE };
            },
        },
        Scene {
            name: "volume_bone",
            setup: |v| {
                reset(v);
                v.view_mode = ViewMode::Volume3d;
                v.volume.settings.mode = RenderMode::Isosurface;
                let r = v.dataset().unwrap().volume.range();
                v.volume.settings.iso_threshold = r.normalize(300.0);
                // clip away the scanner table behind the patient
                v.volume.clip.clip_box = ClipBox { min: Vec3::new(0.04, 0.0, 0.0), max: Vec3::new(0.96, 0.74, 1.0) };
                v.volume.settings.ambient_occlusion = true;
                v.volume.settings.quality = 0.8;
                v.volume.camera.rotate(-0.45, -0.15);
            },
        },
        Scene {
            name: "volume_mip",
            setup: |v| {
                reset(v);
                v.view_mode = ViewMode::Volume3d;
                v.volume.settings.mode = RenderMode::Mip;
                v.volume.settings.brightness = 0.55;
                v.volume.settings.quality = 0.8;
            },
        },
        Scene {
            name: "slice_measurements",
            setup: |v| {
                reset(v);
                v.view_mode = ViewMode::Slice2d;
                v.slices.axis = SliceAxis::Axial;
                v.apply_window_preset(WindowPreset::Lung);
                let volume = v.dataset().unwrap().volume.clone();
                let k = (SliceAxis::Axial.slice_count(&volume) as f32 * 0.62) as u32;
                v.set_slice_index(SliceAxis::Axial, k);
                let key = SliceKey::new(SliceAxis::Axial, k);
                let size = SliceAxis::Axial.plane_size_mm(&volume);
                let p = |x: f32, y: f32| Vec2::new(x, y) * size;
                v.add_annotation(key, Annotation::Distance { a: p(0.22, 0.52), b: p(0.46, 0.52) });
                v.add_annotation(key, Annotation::Angle { a: p(0.62, 0.30), vertex: p(0.56, 0.45), b: p(0.74, 0.47) });
                v.add_annotation(
                    key,
                    Annotation::Polygon { points: vec![p(0.60, 0.55), p(0.72, 0.53), p(0.76, 0.66), p(0.64, 0.70)] },
                );
                v.add_annotation(key, Annotation::Text { pos: p(0.25, 0.40), text: "right lung".into() });
            },
        },
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = PathBuf::from(args.get(1).expect("usage: showcase <volume> <out_dir>"));
    let out = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("docs/images"));
    std::fs::create_dir_all(&out).unwrap();

    let mut setup = egui_kittest::wgpu::default_wgpu_setup();
    if let egui_wgpu::WgpuSetup::CreateNew(c) = &mut setup {
        c.device_descriptor = Arc::new(mri_render::gpu::device_descriptor);
    }
    let rs = egui_kittest::wgpu::create_render_state(setup, egui_wgpu::RendererOptions::PREDICTABLE);
    let repo = Arc::new(mri_io::CompositeRepository::default());
    let mut app = ViewerApp::new(Some(&rs), repo, vec![input]);
    app.viewer.wait_idle();
    assert!(app.viewer.dataset().is_some(), "{:?}", app.viewer.status);

    let renderer = egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs.clone());
    let rs2 = rs.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .renderer(renderer)
        .build_ui_state(move |ui, app: &mut ViewerApp| app.show(ui, Some(&rs2)), app);
    for scene in scenes() {
        (scene.setup)(&mut h.state_mut().viewer);
        h.run_steps(2); // requests background work (e.g. ambient occlusion)
        h.state_mut().viewer.wait_idle();
        h.run_steps(3);
        let img = h.render().expect("render");
        let path = out.join(format!("{}.png", scene.name));
        img.save(&path).unwrap();
        println!("wrote {}", path.display());
    }
}
