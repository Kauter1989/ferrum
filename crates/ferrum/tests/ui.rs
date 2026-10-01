//! End-to-end UI tests driving the real egui application with
//! egui_kittest: navigation through the accessibility tree without a GPU,
//! and full-window renders through wgpu (lavapipe in CI) to verify that
//! 2D, 3D and MPR views actually draw the volume.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use ferrum::ViewerApp;
use ferrum_app::{ToolKind, ViewMode};
use ferrum_domain::{Dims3, RenderMode, Volume};
use glam::Vec3;

/// Writes a sphere phantom as NIfTI and returns its path.
fn phantom_file(dir: &Path) -> PathBuf {
    let dims = Dims3::new(48, 48, 40);
    let c = (dims.as_vec3() - Vec3::ONE) * 0.5;
    let mut vals = Vec::with_capacity(dims.voxel_count());
    for k in 0..dims.z {
        for j in 0..dims.y {
            for i in 0..dims.x {
                let r = ((Vec3::new(i as f32, j as f32, k as f32) - c) / c).length();
                vals.push(if r < 0.5 {
                    1000.0
                } else if r < 0.8 {
                    300.0
                } else {
                    0.0
                });
            }
        }
    }
    let v = Volume::from_physical(dims, Vec3::ONE, &vals).unwrap();
    let path = dir.join("phantom.nii");
    ferrum_io::write_nifti(&v, &path).unwrap();
    path
}

fn loaded_app(dir: &Path, render_state: Option<&egui_wgpu::RenderState>) -> ViewerApp {
    let repo = Arc::new(ferrum_io::CompositeRepository::default());
    let mut app = ViewerApp::new(render_state, repo, vec![phantom_file(dir)]);
    app.viewer.wait_idle();
    assert!(app.viewer.dataset().is_some(), "{:?}", app.viewer.status);
    app
}

#[test]
fn welcome_screen_without_data() {
    let repo = Arc::new(ferrum_io::CompositeRepository::default());
    let app = ViewerApp::new(None, repo, vec![]);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();
    h.get_by_label("Open folder");
    h.get_by_label("Open files");
    assert!(h.query_by_label("Settings panel").is_none(), "docks hidden without data");
}

#[test]
fn switching_modes_and_tools_through_the_ui() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path(), None);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();
    assert_eq!(h.state().viewer.view_mode, ViewMode::Slice2d);

    h.get_by_label("Distance").click();
    h.run();
    assert_eq!(h.state().viewer.tool, ToolKind::Distance);

    h.get_by_label("3D").click();
    h.run();
    assert_eq!(h.state().viewer.view_mode, ViewMode::Volume3d);
    h.get_by_label("Isosurface").click();
    h.run();
    assert_eq!(h.state().viewer.volume.settings.mode, RenderMode::Isosurface);

    h.get_by_label("MPR").click();
    h.run();
    assert_eq!(h.state().viewer.view_mode, ViewMode::Mpr);

    h.get_by_label("Settings panel").click();
    h.run();
    assert!(h.query_by_label("Isosurface").is_none(), "panel hidden");
    h.get_by_label("Settings panel").click();
    h.run();

    h.get_by_label("Info").click();
    h.run();
    h.get_by_label("Series information");
}

fn gpu_render_state() -> Option<egui_wgpu::RenderState> {
    if let Err(e) = ferrum_render::gpu::GpuContext::headless() {
        assert!(std::env::var("FERRUM_REQUIRE_GPU").is_err(), "GPU required: {e}");
        eprintln!("skipping GPU UI test: {e}");
        return None;
    }
    let mut setup = egui_kittest::wgpu::default_wgpu_setup();
    if let egui_wgpu::WgpuSetup::CreateNew(c) = &mut setup {
        c.device_descriptor = Arc::new(ferrum_render::gpu::device_descriptor);
    }
    Some(egui_kittest::wgpu::create_render_state(setup, egui_wgpu::RendererOptions::PREDICTABLE))
}

/// Fraction of pixels inside `rect` that are clearly brighter than the
/// dark view background.
fn lit_fraction(img: &image::RgbaImage, rect: egui::Rect) -> f64 {
    let mut lit = 0usize;
    let mut total = 0usize;
    for y in rect.min.y as u32..rect.max.y as u32 {
        for x in rect.min.x as u32..rect.max.x as u32 {
            if let Some(p) = img.get_pixel_checked(x, y) {
                total += 1;
                if p.0[0].max(p.0[1]).max(p.0[2]) > 60 {
                    lit += 1;
                }
            }
        }
    }
    lit as f64 / total.max(1) as f64
}

#[test]
fn every_view_mode_renders_the_volume() {
    let Some(rs) = gpu_render_state() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path(), Some(&rs));
    let renderer = egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs.clone());
    let rs2 = rs.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .renderer(renderer)
        .build_ui_state(move |ui, app: &mut ViewerApp| app.show(ui, Some(&rs2)), app);
    let out_dir = std::env::var_os("FERRUM_SNAPSHOT_DIR").map(PathBuf::from);
    // central area: left of the 320 pt settings panel, between bars
    let central = egui::Rect::from_min_max(egui::pos2(10.0, 40.0), egui::pos2(850.0, 760.0));
    for (mode, render) in [
        (ViewMode::Slice2d, RenderMode::Tissue),
        (ViewMode::Volume3d, RenderMode::Isosurface),
        (ViewMode::Volume3d, RenderMode::Tissue),
        (ViewMode::Volume3d, RenderMode::Mip),
        (ViewMode::Volume3d, RenderMode::TransferFunction),
        (ViewMode::Mpr, RenderMode::Isosurface),
    ] {
        h.state_mut().viewer.view_mode = mode;
        h.state_mut().viewer.volume.settings.mode = render;
        h.run();
        let img = h.render().expect("render");
        let name = format!("{mode:?}_{render:?}").to_lowercase();
        if let Some(d) = &out_dir {
            std::fs::create_dir_all(d).unwrap();
            img.save(d.join(format!("{name}.png"))).unwrap();
        }
        let lit = lit_fraction(&img, central);
        eprintln!("{name}: {:.1}% lit", lit * 100.0);
        assert!(lit > 0.03, "{name}: view looks empty ({:.2}% lit)", lit * 100.0);
    }
}

#[test]
fn start_screen_renders_with_recent_files() {
    let Some(rs) = gpu_render_state() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut recent = ferrum::ui::recent::RecentFiles::load(&dir.path().join("recent.txt"));
    recent.record(&[PathBuf::from("/data/chest_ct/lung_053.nii.gz")]);
    recent.record(&[PathBuf::from("/data/knee_mri/series_3")]);
    let repo = Arc::new(ferrum_io::CompositeRepository::default());
    let app = ViewerApp::new(Some(&rs), repo, vec![]).with_recent(recent);
    let renderer = egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs.clone());
    let rs2 = rs.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .renderer(renderer)
        .build_ui_state(move |ui, app: &mut ViewerApp| app.show(ui, Some(&rs2)), app);
    h.run();
    h.get_by_label("Open folder");
    let img = h.render().expect("render");
    if let Some(d) = std::env::var_os("FERRUM_SNAPSHOT_DIR").map(PathBuf::from) {
        std::fs::create_dir_all(&d).unwrap();
        img.save(d.join("start_screen.png")).unwrap();
    }
    let whole = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1600.0, 960.0));
    assert!(lit_fraction(&img, whole) > 0.005, "start screen is empty");
}
