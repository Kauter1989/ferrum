//! An example host (24.4): FERRUM's viewer next to the host's own panel.
//!
//! ```text
//! cargo run -p ferrum-ui --example embed_host -- [DICOM folder or NIfTI file]
//! ```
//!
//! Without a path a synthetic phantom is generated. The left panel is the
//! host's: three buttons that call `Viewer` use cases, the log of the events
//! the widget returns, and FERRUM's AI widget. The centre is the widget.
//! The layout lives in [`host_ui`], which `tests/embed.rs` drives with
//! `egui_kittest`.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use ferrum_app::{ViewMode, Viewer};
use ferrum_domain::{Dims3, Volume, WindowPreset};
use ferrum_ui::ai_panel::{self, AiPanelState};
use ferrum_ui::ViewerWidget;
use glam::Vec3;

/// The host's state: the viewer, the widget, the event log, the AI panel.
pub struct HostState {
    /// The study and its use cases.
    pub viewer: Viewer,
    /// FERRUM's views.
    pub widget: ViewerWidget,
    /// The AI widget's UI state.
    pub ai: AiPanelState,
    /// One line per event the widget returned.
    pub events: Vec<String>,
}

impl HostState {
    /// A host around `viewer`.
    pub fn new(viewer: Viewer) -> Self {
        Self { viewer, widget: ViewerWidget::new(), ai: AiPanelState::default(), events: Vec::new() }
    }
}

/// The host's layout: its own left panel, the widget in the centre. The
/// events of each frame go to the log; the buttons call `Viewer` use cases,
/// and their effect comes back as events too.
pub fn host_ui(ui: &mut egui::Ui, host: &mut HostState, render_state: Option<&egui_wgpu::RenderState>) {
    egui::Panel::left("host-panel").exact_size(300.0).show(ui, |ui| {
        ui.heading("Host");
        if ui.button("Next slice").clicked() {
            let axis = host.viewer.slices.axis;
            let next = host.viewer.slice_index(axis).saturating_add(1);
            host.viewer.set_slice_index(axis, next);
        }
        if ui.button("Lung window").clicked() {
            host.viewer.apply_window_preset(WindowPreset::Lung);
        }
        if ui.button("3D view").clicked() {
            host.viewer.set_view_mode(ViewMode::Volume3d);
        }
        ui.separator();
        ui.label("Events");
        egui::ScrollArea::vertical().id_salt("event-log").max_height(220.0).show(ui, |ui| {
            for line in &host.events {
                ui.label(line);
            }
        });
        ui.separator();
        ai_panel::show(ui, &mut host.viewer, &mut host.ai);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        for event in host.widget.show(ui, &mut host.viewer, render_state) {
            host.events.push(format!("{event:?}"));
        }
    });
}

/// A sphere phantom, for a run without a study.
fn phantom() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let dir = std::env::temp_dir().join("ferrum-embed-host");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("phantom.nii");
    let dims = Dims3::new(64, 64, 48);
    let c = (dims.as_vec3() - Vec3::ONE) * 0.5;
    let mut vals = Vec::with_capacity(dims.voxel_count());
    for k in 0..dims.z {
        for j in 0..dims.y {
            for i in 0..dims.x {
                let r = ((Vec3::new(i as f32, j as f32, k as f32) - c) / c).length();
                vals.push(if r < 0.6 { 400.0 } else { -1000.0 });
            }
        }
    }
    let volume =
        Volume::from_physical(dims, Vec3::new(1.0, 1.0, 2.0), &vals).map_err(|e| format!("phantom volume: {e}"))?;
    ferrum_io::write_nifti(&volume, &path)?;
    Ok(path)
}

struct HostApp {
    host: HostState,
}

impl eframe::App for HostApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        host_ui(ui, &mut self.host, frame.wgpu_render_state());
    }
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let path = match std::env::args().nth(1) {
        Some(p) => PathBuf::from(p),
        None => phantom().map_err(eframe::Error::AppCreation)?,
    };

    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(create) = &mut wgpu_options.wgpu_setup {
        create.device_descriptor = Arc::new(ferrum_render::gpu::device_descriptor);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FERRUM embedded — example host")
            .with_inner_size([1400.0, 900.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };
    eframe::run_native(
        "ferrum-embed-host",
        options,
        Box::new(move |cc| {
            if let Some(rs) = &cc.wgpu_render_state {
                ferrum_ui::install(rs);
            }
            let mut viewer = Viewer::new(Arc::new(ferrum_io::CompositeRepository::default()));
            viewer.open_paths(vec![path]);
            viewer.wait_idle();
            let _ = viewer.declare_modality("CT");
            Ok(Box::new(HostApp { host: HostState::new(viewer) }))
        }),
    )
}
