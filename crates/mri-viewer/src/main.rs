//! Desktop entry point.
//!
//! ```text
//! mri-viewer [PATH...]
//! ```
//! Paths (DICOM folders/files or NIfTI files) given on the command line are
//! opened at start-up.

use std::path::PathBuf;
use std::sync::Arc;

use mri_viewer::ViewerApp;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();

    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(create) = &mut wgpu_options.wgpu_setup {
        create.device_descriptor = Arc::new(mri_render::gpu::device_descriptor);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("dicom_renderer")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };
    eframe::run_native(
        "MRI Viewer",
        options,
        Box::new(move |cc| {
            let repo = Arc::new(mri_io::CompositeRepository::default());
            Ok(Box::new(
                ViewerApp::new(cc.wgpu_render_state.as_ref(), repo, paths)
                    .with_recent(mri_viewer::ui::recent::RecentFiles::load_default()),
            ))
        }),
    )
}
