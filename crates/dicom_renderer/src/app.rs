//! The eframe application: composition root of the presentation layer.
//!
//! Layout: the viewports fill the whole window; controls float above them
//! in "glass" docks — brand (top-left), tools (top-centre), actions
//! (top-right), view modes (left), zoom (bottom-left), settings panel
//! (right) and a status HUD (bottom-centre).

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align2, Color32, Id, Order, Pos2, Rect, RichText, Stroke, Vec2};
use egui_phosphor::light as icon;
use mri_app::{GpuSyncState, ToolKind, ViewMode, Viewer};
use mri_domain::{SliceAxis, ViewPreset, VolumeRepository};
use mri_render::gpu::VolumeRenderer;

use crate::gpu_bridge::{self, RendererSink};
use crate::ui::panels::{self, PanelState};
use crate::ui::recent::RecentFiles;
use crate::ui::slice_view::{self, SliceViewState};
use crate::ui::theme::{self, ACCENT, HUD, TEXT, TEXT_DIM};
use crate::ui::volume_view::{self, VolumeViewState};
use crate::ui::widgets::{self, dock_separator, round_button, round_text_button};

/// Top-level application.
pub struct ViewerApp {
    /// Application layer.
    pub viewer: Viewer,
    sync: GpuSyncState,
    panel: PanelState,
    slice_states: [SliceViewState; 3],
    volume_state: VolumeViewState,
    show_panel: bool,
    show_info: bool,
    show_about: bool,
    text_input: String,
    pending_screenshot: Option<PathBuf>,
    last_frame: Option<std::time::Instant>,
    frame_ms: f32,
    recent: RecentFiles,
    styled: bool,
}

/// Icon of a 2D tool.
fn tool_icon(t: ToolKind) -> &'static str {
    match t {
        ToolKind::Pan => icon::HAND,
        ToolKind::WindowLevel => icon::CIRCLE_HALF,
        ToolKind::Probe => icon::CROSSHAIR,
        ToolKind::Distance => icon::RULER,
        ToolKind::Angle => icon::ANGLE,
        ToolKind::Area => icon::POLYGON,
        ToolKind::Rect => icon::SQUARE,
        ToolKind::Text => icon::TEXT_T,
        ToolKind::Move => icon::ARROWS_OUT_CARDINAL,
        ToolKind::Delete => icon::X,
    }
}

fn dock(ctx: &egui::Context, id: &str, align: Align2, offset: Vec2, add: impl FnOnce(&mut egui::Ui)) -> Rect {
    egui::Area::new(Id::new(id))
        .anchor(align, offset)
        .order(Order::Foreground)
        .show(ctx, |ui| theme::glass().show(ui, add).response.rect)
        .inner
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
            show_panel: true,
            show_info: false,
            show_about: false,
            text_input: String::new(),
            pending_screenshot: None,
            last_frame: None,
            frame_ms: 0.0,
            recent: RecentFiles::in_memory(),
            styled: false,
        }
    }

    /// Uses a persistent "recently opened" list.
    pub fn with_recent(mut self, recent: RecentFiles) -> Self {
        self.recent = recent;
        self
    }

    fn open(&mut self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            self.recent.record(&paths);
            self.viewer.open_paths(paths);
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
        if !self.styled {
            theme::install(&ctx);
            self.styled = true;
        }
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

        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CANVAS)).show(ui, |ui| self.main_area(ui));

        if self.viewer.dataset().is_some() {
            self.tool_dock(&ctx);
            self.action_dock(&ctx);
            self.mode_dock(&ctx);
            if self.viewer.view_mode == ViewMode::Slice2d {
                self.zoom_dock(&ctx);
            }
            if self.show_panel {
                self.settings_panel(&ctx);
            }
            self.status_hud(&ctx);
        }
        if self.viewer.is_loading() {
            self.loading_overlay(&ctx);
        }
        self.drop_overlay(&ctx);
        self.dialogs(&ctx);

        if self.viewer.is_loading() || self.viewer.is_computing() || self.viewer.volume.interacting {
            ctx.request_repaint();
        }
    }

    // ------------------------------------------------------------ viewports

    fn main_area(&mut self, ui: &mut egui::Ui) {
        if self.viewer.dataset().is_none() {
            self.start_screen(ui);
            return;
        }
        // Keep the images clear of the floating docks and the settings panel.
        let full = ui.max_rect();
        let right = if self.show_panel { 364.0 } else { 16.0 };
        let inner = Rect::from_min_max(full.min + Vec2::new(84.0, 76.0), full.max - Vec2::new(right, 60.0));
        let inner = if inner.width() > 200.0 && inner.height() > 150.0 { inner } else { full };
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| self.viewports(ui));
    }

    fn viewports(&mut self, ui: &mut egui::Ui) {
        match self.viewer.view_mode {
            ViewMode::Slice2d => {
                let axis = self.viewer.slices.axis;
                slice_view::show(ui, &mut self.viewer, axis, &mut self.slice_states[axis.normal_axis()]);
            }
            ViewMode::Volume3d => volume_view::show(ui, &mut self.viewer, &mut self.volume_state),
            ViewMode::Mpr => {
                let full = ui.available_rect_before_wrap();
                let gap = 4.0;
                let half = (full.size() - Vec2::splat(gap)) * 0.5;
                let cell = |col: f32, row: f32| {
                    Rect::from_min_size(full.min + Vec2::new(col * (half.x + gap), row * (half.y + gap)), half)
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

    // ---------------------------------------------------------------- docks

    fn tool_dock(&mut self, ctx: &egui::Context) {
        let mode = self.viewer.view_mode;
        dock(ctx, "tools", Align2::CENTER_TOP, Vec2::new(0.0, 14.0), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if mode == ViewMode::Volume3d {
                    let erasing = self.viewer.volume.eraser_enabled;
                    if round_button(ui, icon::CUBE_FOCUS, "Rotate", !erasing).clicked() {
                        self.viewer.volume.eraser_enabled = false;
                    }
                    if round_button(ui, icon::ERASER, "Volume eraser", erasing).clicked() {
                        self.viewer.volume.eraser_enabled = !erasing;
                    }
                    dock_separator(ui, false);
                    for p in ViewPreset::ALL {
                        if round_text_button(ui, p.label(), p.name(), false).clicked() {
                            self.viewer.volume.camera.look_from(p);
                        }
                    }
                    dock_separator(ui, false);
                    if round_button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Reset camera", false).clicked() {
                        self.viewer.reset_view_3d();
                    }
                    let can_undo = self.viewer.erase_history_len() > 0;
                    if widgets::round_button_sized(
                        ui,
                        icon::ARROW_U_UP_LEFT,
                        "Undo erase",
                        false,
                        widgets::ROUND,
                        can_undo,
                    )
                    .clicked()
                    {
                        self.viewer.undo_erase();
                    }
                } else {
                    for t in ToolKind::ALL {
                        if round_button(ui, tool_icon(t), t.label(), self.viewer.tool == t)
                            .on_hover_text(t.hint())
                            .clicked()
                        {
                            self.viewer.select_tool(t);
                        }
                    }
                    dock_separator(ui, false);
                    let any = !self.viewer.annotations().is_empty();
                    if widgets::round_button_sized(ui, icon::TRASH, "Clear annotations", false, widgets::ROUND, any)
                        .clicked()
                    {
                        self.viewer.clear_annotations();
                    }
                }
            });
        });
    }

    fn action_dock(&mut self, ctx: &egui::Context) {
        dock(ctx, "actions", Align2::RIGHT_TOP, Vec2::new(-14.0, 14.0), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if round_button(ui, icon::FOLDER_OPEN, "Open folder", false).clicked() {
                    self.pick_folder();
                }
                if round_button(ui, icon::FILE_PLUS, "Open files", false).clicked() {
                    self.pick_files();
                }
                if round_button(ui, icon::DOWNLOAD_SIMPLE, "Export NIfTI", false).clicked() {
                    self.export_nifti();
                }
                if round_button(ui, icon::CAMERA, "Screenshot", false).clicked() {
                    self.request_screenshot(ui.ctx());
                }
                if round_button(ui, icon::INFO, "Info", self.show_info).clicked() {
                    self.show_info = !self.show_info;
                }
                dock_separator(ui, false);
                if round_button(ui, icon::SLIDERS_HORIZONTAL, "Settings panel", self.show_panel).clicked() {
                    self.show_panel = !self.show_panel;
                }
                if round_button(ui, icon::QUESTION, "About", self.show_about).clicked() {
                    self.show_about = !self.show_about;
                }
            });
        });
    }

    fn mode_dock(&mut self, ctx: &egui::Context) {
        dock(ctx, "modes", Align2::LEFT_TOP, Vec2::new(14.0, 14.0), |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(widgets::ROUND), egui::Sense::click());
                widgets::logo(ui.painter(), r.center(), 30.0, TEXT);
                if resp.on_hover_text("About dicom_renderer").clicked() {
                    self.show_about = true;
                }
                dock_separator(ui, true);
                for (m, label) in [(ViewMode::Slice2d, "2D"), (ViewMode::Volume3d, "3D"), (ViewMode::Mpr, "MPR")] {
                    if round_text_button(ui, label, label, self.viewer.view_mode == m).clicked() {
                        self.viewer.view_mode = m;
                    }
                }
                if self.viewer.view_mode == ViewMode::Slice2d {
                    dock_separator(ui, true);
                    for a in SliceAxis::ALL {
                        let active = self.viewer.slices.axis == a;
                        if round_text_button(ui, &a.label()[..1], a.label(), active).clicked() {
                            self.viewer.slices.axis = a;
                        }
                    }
                }
            });
        });
    }

    fn zoom_dock(&mut self, ctx: &egui::Context) {
        let axis = self.viewer.slices.axis;
        dock(ctx, "zoom", Align2::LEFT_BOTTOM, Vec2::new(14.0, -14.0), |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let view = &mut self.viewer.slices.views[axis.normal_axis()];
                let mut scale = |f: f32| {
                    let z = (view.zoom * f).clamp(mri_domain::SliceView::MIN_ZOOM, mri_domain::SliceView::MAX_ZOOM);
                    view.pan *= z / view.zoom;
                    view.zoom = z;
                };
                if round_button(ui, icon::MAGNIFYING_GLASS_PLUS, "Zoom in", false).clicked() {
                    scale(1.25);
                }
                if round_button(ui, icon::MAGNIFYING_GLASS_MINUS, "Zoom out", false).clicked() {
                    scale(0.8);
                }
                if round_button(ui, icon::ARROWS_IN, "Fit to view", false).clicked() {
                    *view = mri_domain::SliceView::default();
                }
            });
        });
    }

    fn settings_panel(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let max_h = (screen.height() - 160.0).max(200.0);
        egui::Area::new(Id::new("settings")).anchor(Align2::RIGHT_TOP, [-14.0, 76.0]).order(Order::Foreground).show(
            ctx,
            |ui| {
                theme::panel().show(ui, |ui| {
                    ui.set_width(318.0);
                    ui.set_max_height(max_h);
                    panels::show(ui, &mut self.viewer, &mut self.panel);
                });
            },
        );
    }

    fn status_hud(&mut self, ctx: &egui::Context) {
        egui::Area::new(Id::new("status")).anchor(Align2::CENTER_BOTTOM, [0.0, -14.0]).order(Order::Foreground).show(
            ctx,
            |ui| {
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(170))
                    .stroke(Stroke::new(1.0, theme::HAIRLINE))
                    .corner_radius(14)
                    .inner_margin(egui::Margin::symmetric(14, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if self.viewer.is_computing() {
                                ui.spinner();
                            }
                            ui.label(RichText::new(&self.viewer.status.message).size(12.5).color(TEXT_DIM));
                            if let Some(p) = self.viewer.probe {
                                ui.label(
                                    RichText::new(format!(
                                        "· ({}, {}, {}) = {:.1}",
                                        p.voxel.x, p.voxel.y, p.voxel.z, p.value
                                    ))
                                    .monospace()
                                    .size(12.0)
                                    .color(HUD),
                                );
                            }
                            ui.label(
                                RichText::new(format!("· {:.1} ms", self.frame_ms))
                                    .monospace()
                                    .size(11.5)
                                    .color(TEXT_DIM),
                            );
                        });
                    });
            },
        );
    }

    fn loading_overlay(&mut self, ctx: &egui::Context) {
        egui::Area::new(Id::new("loading")).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).order(Order::Tooltip).show(
            ctx,
            |ui| {
                theme::panel().show(ui, |ui| {
                    ui.set_width(360.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Loading").size(16.0).strong().color(TEXT));
                    });
                    ui.label(RichText::new(&self.viewer.status.message).color(TEXT_DIM));
                    if let Some(p) = self.viewer.status.progress {
                        ui.add(egui::ProgressBar::new(p).desired_height(6.0).fill(ACCENT));
                    }
                    if ui.button(format!("{} Cancel", icon::X)).clicked() {
                        self.viewer.cancel_loading();
                    }
                });
            },
        );
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }
        let screen = ctx.content_rect();
        let p = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("drop")));
        p.rect_filled(screen, 0.0, Color32::from_black_alpha(170));
        let card = Rect::from_center_size(screen.center(), Vec2::new(420.0, 180.0));
        p.rect_stroke(card, 24.0, Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
        p.text(
            card.center() - Vec2::new(0.0, 20.0),
            Align2::CENTER_CENTER,
            icon::DOWNLOAD_SIMPLE,
            egui::FontId::proportional(40.0),
            ACCENT,
        );
        p.text(
            card.center() + Vec2::new(0.0, 30.0),
            Align2::CENTER_CENTER,
            "Drop to open",
            egui::FontId::proportional(18.0),
            TEXT,
        );
    }

    // --------------------------------------------------------- start screen

    fn start_screen(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let p = ui.painter();
        // background: faint grid and a glow behind the mark
        let grid = Color32::from_rgba_unmultiplied(92, 225, 255, 6);
        let step = 48.0;
        let mut x = rect.min.x - (rect.min.x % step);
        while x < rect.max.x {
            p.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)], Stroke::new(1.0, grid));
            x += step;
        }
        let mut y = rect.min.y - (rect.min.y % step);
        while y < rect.max.y {
            p.line_segment([Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)], Stroke::new(1.0, grid));
            y += step;
        }
        let top = rect.center_top() + Vec2::new(0.0, (rect.height() * 0.16).max(40.0));
        for (r, a) in [(140.0, 6u8), (95.0, 10), (60.0, 16)] {
            p.circle_filled(
                top + Vec2::new(0.0, 50.0),
                r,
                Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), a),
            );
        }
        widgets::logo(p, top + Vec2::new(0.0, 50.0), 92.0, TEXT);

        ui.vertical_centered(|ui| {
            ui.add_space((rect.height() * 0.16).max(40.0) + 124.0);
            ui.label(RichText::new("dicom_renderer").size(34.0).color(TEXT).strong());
            ui.label(
                RichText::new("GPU volume rendering · MPR · measurements for DICOM and NIfTI")
                    .size(15.0)
                    .color(TEXT_DIM),
            );
            ui.add_space(28.0);

            let card_w = 560.0f32.min(ui.available_width() - 40.0);
            theme::panel().show(ui, |ui| {
                ui.set_width(card_w);
                let (r, _) = ui.allocate_exact_size(Vec2::new(card_w, 130.0), egui::Sense::hover());
                dashed_rect(ui.painter(), r.shrink(4.0), 18.0, TEXT_DIM.gamma_multiply(0.6));
                ui.painter().text(
                    r.center() - Vec2::new(0.0, 16.0),
                    Align2::CENTER_CENTER,
                    icon::FOLDER_OPEN,
                    egui::FontId::proportional(34.0),
                    ACCENT,
                );
                ui.painter().text(
                    r.center() + Vec2::new(0.0, 28.0),
                    Align2::CENTER_CENTER,
                    "Drop a DICOM folder or a NIfTI file here",
                    egui::FontId::proportional(15.0),
                    TEXT,
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space((card_w - 2.0 * 170.0 - 12.0).max(0.0) / 2.0);
                    if pill_button(ui, icon::FOLDER_OPEN, "Open folder").clicked() {
                        self.pick_folder();
                    }
                    if pill_button(ui, icon::FILE_PLUS, "Open files").clicked() {
                        self.pick_files();
                    }
                });
                if !self.recent.items().is_empty() {
                    ui.add_space(10.0);
                    widgets::section_title(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Recent");
                    let mut chosen = None;
                    for p in self.recent.items() {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let r = ui
                            .add(
                                egui::Button::new(RichText::new(format!("{}  {name}", icon::FILE_TEXT)).color(TEXT))
                                    .frame(false),
                            )
                            .on_hover_text(p.to_string_lossy());
                        if r.clicked() {
                            chosen = Some(p.clone());
                        }
                    }
                    if let Some(p) = chosen {
                        self.open(vec![p]);
                    }
                }
            });
            ui.add_space(14.0);
            ui.label(
                RichText::new("DICOM (JPEG · JPEG 2000 · RLE) · NIfTI-1 · Ctrl+O to open a folder")
                    .size(12.0)
                    .color(TEXT_DIM),
            );
        });
    }

    // -------------------------------------------------------------- dialogs

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(series) = self.viewer.series_choice.clone() {
            egui::Window::new("Select series")
                .collapsible(false)
                .resizable(true)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(RichText::new("Several series were found").color(TEXT_DIM));
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        for s in series {
                            ui.horizontal(|ui| {
                                if ui.button(format!("{} Load", icon::CARET_RIGHT)).clicked() {
                                    self.viewer.load_series(s.clone());
                                }
                                ui.label(
                                    RichText::new(format!("{} · {}", s.modality, s.format)).color(HUD).monospace(),
                                );
                                ui.label(&s.description);
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
            egui::Window::new("Annotation text").collapsible(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(
                ctx,
                |ui| {
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
                },
            );
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
            egui::Window::new("Series information").open(&mut open).default_pos([80.0, 90.0]).show(ctx, |ui| {
                if let Some(d) = self.viewer.dataset() {
                    let v = &d.volume;
                    egui::Grid::new("info").striped(true).show(ui, |ui| {
                        let dims = v.dims();
                        let sp = v.spacing();
                        ui.label("Dimensions");
                        ui.label(RichText::new(format!("{} × {} × {}", dims.x, dims.y, dims.z)).color(HUD).monospace());
                        ui.end_row();
                        ui.label("Spacing");
                        ui.label(
                            RichText::new(format!("{:.3} × {:.3} × {:.3} mm", sp.x, sp.y, sp.z)).color(HUD).monospace(),
                        );
                        ui.end_row();
                        ui.label("Intensity range");
                        ui.label(
                            RichText::new(format!("{:.1} … {:.1}", v.range().min, v.range().max))
                                .color(HUD)
                                .monospace(),
                        );
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
            egui::Window::new("Errors").collapsible(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                for e in &self.viewer.status.errors {
                    ui.colored_label(ACCENT, format!("{} {e}", icon::WARNING));
                }
                if ui.button("Dismiss").clicked() {
                    self.viewer.status.errors.clear();
                }
            });
        }
        if self.show_about {
            let mut open = true;
            egui::Window::new("About").open(&mut open).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.heading("dicom_renderer");
                ui.label("GPU volume rendering and 2D/MPR viewing of DICOM and NIfTI data.");
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(TEXT_DIM));
                ui.hyperlink_to("Source code", "https://github.com/Kauter1989/dicom_renderer");
                ui.add_space(6.0);
                ui.label(RichText::new("Shortcuts").strong());
                for (k, v) in [
                    ("F2 / F3 / F4", "2D / 3D / MPR"),
                    ("Wheel · ↑ ↓", "change slice"),
                    ("Ctrl + wheel", "zoom slice"),
                    ("Ctrl+O", "open folder"),
                    ("Ctrl+Z", "undo erase"),
                    ("Tab", "toggle settings panel"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(k).monospace().color(HUD));
                        ui.label(RichText::new(v).color(TEXT_DIM));
                    });
                }
            });
            self.show_about = open;
        }
    }

    // --------------------------------------------------------------- actions

    fn pick_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            self.open(vec![dir]);
        }
    }

    fn pick_files(&mut self) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("Medical images", &["dcm", "nii", "gz", "ima"])
            .add_filter("All files", &["*"])
            .pick_files()
        {
            self.open(files);
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
        if let Some(path) = rfd::FileDialog::new().set_file_name("dicom_renderer.png").save_file() {
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
        self.open(paths);
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (open, undo, v2, v3, vm, tab) = ctx.input(|i| {
            let typing = i.focused;
            (
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::Z),
                !typing && i.key_pressed(egui::Key::F2),
                !typing && i.key_pressed(egui::Key::F3),
                !typing && i.key_pressed(egui::Key::F4),
                !typing && i.key_pressed(egui::Key::Tab),
            )
        });
        if open {
            self.pick_folder();
        }
        if undo {
            self.viewer.undo_erase();
        }
        if tab {
            self.show_panel = !self.show_panel;
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

/// Large rounded button with icon and text, used on the start screen.
fn pill_button(ui: &mut egui::Ui, icon_str: &str, text: &str) -> egui::Response {
    let r = ui.add(
        egui::Button::new(RichText::new(format!("{icon_str}   {text}")).size(15.0).color(TEXT))
            .min_size(Vec2::new(170.0, 42.0))
            .corner_radius(21.0)
            .fill(theme::CONTROL),
    );
    let label = text.to_string();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    r
}

fn dashed_rect(p: &egui::Painter, rect: Rect, radius: f32, color: Color32) {
    let stroke = Stroke::new(1.2, color);
    let r = rect.shrink(radius);
    let mut path = Vec::new();
    let arc = |c: Pos2, a0: f32, path: &mut Vec<Pos2>| {
        for i in 0..=8 {
            let a = a0 + i as f32 / 8.0 * std::f32::consts::FRAC_PI_2;
            path.push(c + Vec2::new(a.cos(), a.sin()) * radius);
        }
    };
    use std::f32::consts::PI;
    arc(r.right_bottom(), 0.0, &mut path);
    arc(r.left_bottom(), PI * 0.5, &mut path);
    arc(r.left_top(), PI, &mut path);
    arc(r.right_top(), PI * 1.5, &mut path);
    path.push(path[0]);
    p.add(egui::Shape::dashed_line(&path, stroke, 7.0, 5.0));
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let rs = frame.wgpu_render_state().cloned();
        self.show(ui, rs.as_ref());
    }
}
