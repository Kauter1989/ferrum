// Release builds on Windows are GUI applications without a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop entry point.
//!
//! ```text
//! ferrum [PATH...]
//! ```
//! Paths (DICOM folders/files or NIfTI files) given on the command line are
//! opened at start-up.

use std::path::PathBuf;
use std::sync::Arc;

use ferrum::ViewerApp;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();

    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(create) = &mut wgpu_options.wgpu_setup {
        create.device_descriptor = Arc::new(ferrum_render::gpu::device_descriptor);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FERRUM")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };
    eframe::run_native(
        "ferrum",
        options,
        Box::new(move |cc| {
            let repo = Arc::new(ferrum_io::CompositeRepository::default());
            Ok(Box::new(
                ViewerApp::new(cc.wgpu_render_state.as_ref(), repo, paths)
                    .with_recent(ferrum::ui::recent::RecentFiles::load_default()),
            ))
        }),
    )
}
