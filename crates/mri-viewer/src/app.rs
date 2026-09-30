//! The eframe application: composition root of the presentation layer.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, RichText};
use mri_app::{GpuSyncState, ViewMode, Viewer};
use mri_domain::{SliceAxis, VolumeRepository};
use mri_render::gpu::VolumeRenderer;

use crate::gpu_bridge::{self, RendererSink};
use crate::ui::panels::{self, PanelState};
use crate::ui::slice_view::{self, SliceViewState};
use crate::ui::volume_view::{self, VolumeViewState};

/// Top-level application.
pub struct ViewerApp {
    /// Application layer.
    pub viewer: Viewer,
    sync: GpuSyncState,
    panel: PanelState,
    slice_states: [SliceViewState; 3],
    volume_state: VolumeViewState,
    show_info: bool,
    show_about: bool,
    text_input: String,
    pending_screenshot: Option<PathBuf>,
    last_frame: Option<std::time::Instant>,
    frame_ms: f32,
}

impl ViewerApp {
    /// Creates the app, installing the GPU renderer if eframe runs on wgpu.
    pub fn new(
        render_state: Option<&egui_wgpu::RenderState>,
        repo: Arc<dyn VolumeRepository>,
        paths: Vec<PathBuf>,
    ) -> Self {
        if let Some(rs) = render_state {
            gpu_bridge::install(rs);
        }
        let mut viewer = Viewer::new(repo);
        viewer.open_paths(paths);
        Self {
            viewer,
            sync: GpuSyncState::default(),
            panel: PanelState::default(),
            slice_states: Default::default(),
            volume_state: VolumeViewState::default(),
            show_info: false,
            show_about: false,
            text_input: String::new(),
            pending_screenshot: None,
            last_frame: None,
            frame_ms: 0.0,
        }
    }

    fn sync_gpu(&mut self, render_state: Option<&egui_wgpu::RenderState>) {
        let Some(rs) = render_state else {
            return;
        };
        let mut guard = rs.renderer.write();
        if let Some(renderer) = guard.callback_resources.get_mut::<VolumeRenderer>() {
            let mut sink = RendererSink { device: &rs.device, queue: &rs.queue, renderer };
            self.viewer.sync_gpu(&mut self.sync, &mut sink);
        }
    }

    /// Draws the whole UI. Separated from [`eframe::App`] so that UI tests
    /// can drive it without a window.
    pub fn show(&mut self, ui: &mut egui::Ui, render_state: Option<&egui_wgpu::RenderState>) {
        let ctx = ui.ctx().clone();
        let now = std::time::Instant::now();
        if let Some(prev) = self.last_frame {
            self.frame_ms = self.frame_ms * 0.9 + now.duration_since(prev).as_secs_f32() * 1000.0 * 0.1;
        }
        self.last_frame = Some(now);

        self.viewer.poll();
        self.handle_dropped_files(&ctx);
        self.handle_shortcuts(&ctx);
        self.handle_screenshot_events(&ctx);
        self.sync_gpu(render_state);

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if self.viewer.dataset().is_some() {
            egui::Panel::right("settings").resizable(true).default_size(320.0).show(ui, |ui| {
                panels::show(ui, &mut self.viewer, &mut self.panel);
            });
        }
        egui::CentralPanel::default().show(ui, |ui| self.main_area(ui));
        self.dialogs(&ctx);

        if self.viewer.is_loading() || self.viewer.is_computing() || self.viewer.volume.interacting {
            ctx.request_repaint();
        }
    }

    fn main_area(&mut self, ui: &mut egui::Ui) {
        if self.viewer.dataset().is_none() {
            welcome(ui, &mut self.viewer);
            return;
        }
        match self.viewer.view_mode {
            ViewMode::Slice2d => {
                let axis = self.viewer.slices.axis;
                slice_view::show(ui, &mut self.viewer, axis, &mut self.slice_states[axis.normal_axis()]);
            }
            ViewMode::Volume3d => volume_view::show(ui, &mut self.viewer, &mut self.volume_state),
            ViewMode::Mpr => {
                let full = ui.available_rect_before_wrap();
                let half = full.size() * 0.5;
                let cell = |col: f32, row: f32| {
                    egui::Rect::from_min_size(full.min + egui::vec2(col * half.x, row * half.y), half).shrink(1.0)
                };
                let layout = [
                    (cell(0.0, 0.0), Some(SliceAxis::Axial)),
                    (cell(1.0, 0.0), None),
                    (cell(0.0, 1.0), Some(SliceAxis::Coronal)),
                    (cell(1.0, 1.0), Some(SliceAxis::Sagittal)),
                ];
                for (rect, axis) in layout {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| match axis {
                        Some(a) => slice_view::show(ui, &mut self.viewer, a, &mut self.slice_states[a.normal_axis()]),
                        None => volume_view::show(ui, &mut self.viewer, &mut self.volume_state),
                    });
                }
                ui.allocate_rect(full, egui::Sense::hover());
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("MRI Viewer").strong().color(Color32::from_rgb(120, 180, 255)));
            ui.separator();
            if ui.button("📂 Open folder").on_hover_text("Ctrl+O").clicked() {
                self.pick_folder();
            }
            if ui.button("📄 Open files").clicked() {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("Medical images", &["dcm", "nii", "gz", "ima"])
                    .add_filter("All files", &["*"])
                    .pick_files()
                {
                    self.viewer.open_paths(files);
                }
            }
            let loaded = self.viewer.dataset().is_some();
            ui.add_enabled_ui(loaded, |ui| {
                if ui.button("💾 Export NIfTI").clicked() {
                    self.export_nifti();
                }
                if ui.button("📷 Screenshot").clicked() {
                    self.request_screenshot(ui.ctx());
                }
                if ui.button("ℹ Info").clicked() {
                    self.show_info = true;
                }
            });
            ui.separator();
            ui.add_enabled_ui(loaded, |ui| {
                for m in ViewMode::ALL {
                    ui.selectable_value(&mut self.viewer.view_mode, m, m.label());
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("?").clicked() {
                    self.show_about = true;
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(p) = self.viewer.status.progress {
                ui.add(egui::ProgressBar::new(p).desired_width(160.0).show_percentage());
                if ui.small_button("Cancel").clicked() {
                    self.viewer.cancel_loading();
                }
            }
            ui.label(&self.viewer.status.message);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{:.1} ms/frame", self.frame_ms)).weak());
                if let Some(p) = self.viewer.probe {
                    ui.label(format!("({}, {}, {}) = {:.1}", p.voxel.x, p.voxel.y, p.voxel.z, p.value));
                }
            });
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(series) = self.viewer.series_choice.clone() {
            egui::Window::new("Select series").collapsible(false).resizable(true).show(ctx, |ui| {
                ui.label("Several series were found:");
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for s in series {
                        ui.horizontal(|ui| {
                            if ui.button("Load").clicked() {
                                self.viewer.load_series(s.clone());
                            }
                            ui.label(format!("[{}] {} {}", s.format, s.modality, s.description));
                        });
                    }
                });
                if ui.button("Cancel").clicked() {
                    self.viewer.series_choice = None;
                }
            });
        }
        if self.viewer.pending_text.is_some() {
            let mut close = None;
            egui::Window::new("Annotation text").collapsible(false).show(ctx, |ui| {
                let r = ui.text_edit_singleline(&mut self.text_input);
                r.request_focus();
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        close = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        close = Some(false);
                    }
                });
            });
            match close {
                Some(true) => {
                    let text = std::mem::take(&mut self.text_input);
                    self.viewer.commit_text(&text);
                }
                Some(false) => {
                    self.text_input.clear();
                    self.viewer.pending_text = None;
                }
                None => {}
            }
        }
        if self.show_info {
            let mut open = true;
            egui::Window::new("Series information").open(&mut open).show(ctx, |ui| {
                if let Some(d) = self.viewer.dataset() {
                    let v = &d.volume;
                    egui::Grid::new("info").striped(true).show(ui, |ui| {
                        let dims = v.dims();
                        let sp = v.spacing();
                        ui.label("Dimensions");
                        ui.label(format!("{} × {} × {}", dims.x, dims.y, dims.z));
                        ui.end_row();
                        ui.label("Spacing");
                        ui.label(format!("{:.3} × {:.3} × {:.3} mm", sp.x, sp.y, sp.z));
                        ui.end_row();
                        ui.label("Intensity range");
                        ui.label(format!("{:.1} … {:.1}", v.range().min, v.range().max));
                        ui.end_row();
                        for (k, val) in &d.metadata.attributes {
                            ui.label(k);
                            ui.label(val);
                            ui.end_row();
                        }
                    });
                }
            });
            self.show_info = open;
        }
        if !self.viewer.status.errors.is_empty() {
            egui::Window::new("Errors").collapsible(false).show(ctx, |ui| {
                for e in &self.viewer.status.errors {
                    ui.colored_label(Color32::from_rgb(255, 120, 120), e);
                }
                if ui.button("Dismiss").clicked() {
                    self.viewer.status.errors.clear();
                }
            });
        }
        if self.show_about {
            let mut open = true;
            egui::Window::new("About").open(&mut open).show(ctx, |ui| {
                ui.heading("MRI Viewer (Rust)");
                ui.label("GPU volume rendering and 2D/MPR viewing of DICOM and NIfTI data.");
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.hyperlink_to("Source code", "https://github.com/Kauter1989/dicom_renderer");
            });
            self.show_about = open;
        }
    }

    fn pick_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            self.viewer.open_paths(vec![dir]);
        }
    }

    fn export_nifti(&mut self) {
        let Some(volume) = self.viewer.dataset().map(|d| d.volume.clone()) else {
            return;
        };
        if let Some(path) = rfd::FileDialog::new().set_file_name("volume.nii.gz").save_file() {
            match mri_io::write_nifti(&volume, &path) {
                Ok(()) => self.viewer.status.message = format!("Exported {}", path.display()),
                Err(e) => self.viewer.status.errors.push(e.to_string()),
            }
        }
    }

    fn request_screenshot(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new().set_file_name("mri-viewer.png").save_file() {
            self.pending_screenshot = Some(path);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }

    fn handle_screenshot_events(&mut self, ctx: &egui::Context) {
        let shots: Vec<Arc<egui::ColorImage>> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
                .collect()
        });
        for img in shots {
            if let Some(path) = self.pending_screenshot.take() {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                match image::save_buffer(&path, &bytes, w as u32, h as u32, image::ExtendedColorType::Rgba8) {
                    Ok(()) => self.viewer.status.message = format!("Saved {}", path.display()),
                    Err(e) => self.viewer.status.errors.push(e.to_string()),
                }
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let paths: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !paths.is_empty() {
            self.viewer.open_paths(paths);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (open, undo, v2, v3, vm) = ctx.input(|i| {
            let typing = i.focused;
            (
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::Z),
                !typing && i.key_pressed(egui::Key::F2),
                !typing && i.key_pressed(egui::Key::F3),
                !typing && i.key_pressed(egui::Key::F4),
            )
        });
        if open {
            self.pick_folder();
        }
        if undo {
            self.viewer.undo_erase();
        }
        if self.viewer.dataset().is_some() {
            if v2 {
                self.viewer.view_mode = ViewMode::Slice2d;
            }
            if v3 {
                self.viewer.view_mode = ViewMode::Volume3d;
            }
            if vm {
                self.viewer.view_mode = ViewMode::Mpr;
            }
        }
    }
}

fn welcome(ui: &mut egui::Ui, viewer: &mut Viewer) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.3);
        ui.heading("MRI Viewer");
        ui.label("High-performance 2D / MPR / 3D viewer for DICOM and NIfTI volumes");
        ui.add_space(12.0);
        if viewer.is_loading() {
            ui.spinner();
        } else {
            ui.label("Drop a DICOM folder or NIfTI file here, or use “Open folder” (Ctrl+O).");
        }
    });
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let rs = frame.wgpu_render_state().cloned();
        self.show(ui, rs.as_ref());
    }
}
