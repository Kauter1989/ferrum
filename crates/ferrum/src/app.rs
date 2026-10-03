//! The eframe application: composition root of the presentation layer.
//!
//! Layout (top to bottom): a header with the study, the view-mode switch
//! and file actions; a toolbar with the tools of the current mode; the
//! studies sidebar on the left, the viewports in the centre and the
//! settings panel on the right; a status bar at the bottom.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Align, Align2, Color32, Id, Layout, Order, Pos2, Rect, RichText, Stroke, UiBuilder, Vec2};
use egui_phosphor::light as icon;
use ferrum_app::{GpuSyncState, ToolKind, ViewMode, Viewer};
use ferrum_domain::{SliceAxis, ViewPreset, VolumeRepository};
use ferrum_render::gpu::VolumeRenderer;

use crate::gpu_bridge::{self, RendererSink};
use crate::ui::panels::{self, PanelState, PanelTab};
use crate::ui::recent::RecentFiles;
use crate::ui::slice_view::{self, SliceViewState};
use crate::ui::theme::{self, ACCENT, ACCENT_SOFT, BORDER, DANGER, OVERLAY, TEXT, TEXT_DIM};
use crate::ui::volume_view::{self, VolumeViewState};
use crate::ui::widgets::{self, segmented, tool_button, tool_button_enabled, toolbar_separator};

/// Top-level application.
pub struct ViewerApp {
    /// Application layer.
    pub viewer: Viewer,
    sync: GpuSyncState,
    /// Settings panel state (tabs, editors, AI connection).
    pub panel: PanelState,
    slice_states: [SliceViewState; 3],
    volume_state: VolumeViewState,
    show_panel: bool,
    show_studies: bool,
    show_about: bool,
    text_input: String,
    pending_screenshot: Option<PathBuf>,
    last_frame: Option<std::time::Instant>,
    frame_ms: f32,
    recent: RecentFiles,
    current: Option<PathBuf>,
    styled: bool,
}

/// Icon of a 2D tool.
pub(crate) fn tool_icon(t: ToolKind) -> &'static str {
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
        ToolKind::AiPoint => icon::CURSOR_CLICK,
        ToolKind::AiBox => icon::BOUNDING_BOX,
        ToolKind::AiScribble => icon::SCRIBBLE,
        ToolKind::AiLasso => icon::LASSO,
        ToolKind::Region => icon::PAINT_BUCKET,
    }
}

/// The entry recorded for an open action: the folder, or the single file.
fn study_entry(paths: &[PathBuf]) -> Option<PathBuf> {
    let first = paths.first()?;
    Some(if paths.len() > 1 { first.parent().map(Path::to_path_buf).unwrap_or(first.clone()) } else { first.clone() })
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Runs `add` in a horizontal, vertically centred child of `ui` limited to `rect`.
fn row_in(ui: &mut egui::Ui, rect: Rect, layout: Layout, add: impl FnOnce(&mut egui::Ui)) {
    ui.scope_builder(UiBuilder::new().max_rect(rect).layout(layout), add);
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
        let current = study_entry(&paths);
        let mut viewer = Viewer::new(repo);
        viewer.open_paths(paths);
        Self {
            viewer,
            sync: GpuSyncState::default(),
            panel: PanelState::default(),
            slice_states: Default::default(),
            volume_state: VolumeViewState::default(),
            show_panel: true,
            show_studies: true,
            show_about: false,
            text_input: String::new(),
            pending_screenshot: None,
            last_frame: None,
            frame_ms: 0.0,
            recent: RecentFiles::in_memory(),
            current,
            styled: false,
        }
    }

    /// Uses a persistent "recently opened" list. Paths passed to
    /// [`ViewerApp::new`] are recorded in it.
    pub fn with_recent(mut self, recent: RecentFiles) -> Self {
        self.recent = recent;
        if let Some(c) = self.current.clone() {
            self.recent.record(&[c]);
        }
        self
    }

    fn open(&mut self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            self.recent.record(&paths);
            self.current = study_entry(&paths);
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

        let has_data = self.viewer.dataset().is_some();
        egui::Panel::top("header").exact_size(56.0).frame(theme::bar()).show(ui, |ui| self.header(ui));
        if has_data {
            egui::Panel::top("toolbar").exact_size(48.0).frame(theme::bar()).show(ui, |ui| self.toolbar(ui));
            egui::Panel::bottom("status").exact_size(28.0).frame(theme::bar()).show(ui, |ui| self.status_bar(ui));
            if self.show_studies {
                egui::Panel::left("studies")
                    .exact_size(232.0)
                    .resizable(false)
                    .frame(theme::side())
                    .show(ui, |ui| self.studies(ui));
            }
            if self.show_panel {
                egui::Panel::right("settings")
                    .exact_size(324.0)
                    .resizable(false)
                    .frame(theme::side())
                    .show(ui, |ui| panels::show(ui, &mut self.viewer, &mut self.panel));
                if std::mem::take(&mut self.panel.export_annotations) {
                    self.export_annotations();
                }
                if std::mem::take(&mut self.panel.segments.import_labels) {
                    self.import_labels();
                }
                if std::mem::take(&mut self.panel.segments.export_labels) {
                    self.export_labels();
                }
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::same(8)))
            .show(ui, |ui| self.main_area(ui));

        if self.viewer.is_loading() {
            self.loading_overlay(&ctx);
        }
        self.drop_overlay(&ctx);
        self.dialogs(&ctx);

        if self.viewer.is_loading()
            || self.viewer.is_computing()
            || self.viewer.ai().is_busy()
            || self.viewer.volume.interacting
        {
            ctx.request_repaint();
        }
    }

    // ------------------------------------------------------------ viewports

    fn main_area(&mut self, ui: &mut egui::Ui) {
        if self.viewer.dataset().is_none() {
            self.start_screen(ui);
            return;
        }
        match self.viewer.view_mode() {
            ViewMode::Slice2d => {
                let axis = self.viewer.slices.axis;
                slice_view::show(ui, &mut self.viewer, axis, &mut self.slice_states[axis.normal_axis()]);
            }
            ViewMode::Volume3d => volume_view::show(ui, &mut self.viewer, &mut self.volume_state),
            ViewMode::Mpr => {
                let full = ui.available_rect_before_wrap();
                let gap = 8.0;
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
                    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| match axis {
                        Some(a) => slice_view::show(ui, &mut self.viewer, a, &mut self.slice_states[a.normal_axis()]),
                        None => volume_view::show(ui, &mut self.viewer, &mut self.volume_state),
                    });
                }
                ui.allocate_rect(full, egui::Sense::hover());
            }
        }
    }

    // ----------------------------------------------------------------- bars

    fn header(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let has_data = self.viewer.dataset().is_some();
        let left = Layout::left_to_right(Align::Center);

        // brand and study
        row_in(ui, rect, left, |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if has_data && tool_button(ui, icon::SIDEBAR_SIMPLE, "Studies panel", false).clicked() {
                self.show_studies = !self.show_studies;
            }
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::click());
            widgets::logo(ui.painter(), r);
            if resp.on_hover_text("About FERRUM").clicked() {
                self.show_about = true;
            }
            ui.label(RichText::new("FERRUM").size(16.0).strong().color(TEXT));
            if let Some(d) = self.viewer.dataset() {
                let (sep, _) = ui.allocate_exact_size(Vec2::new(12.0, 28.0), egui::Sense::hover());
                ui.painter().line_segment([sep.center_top(), sep.center_bottom()], Stroke::new(1.0, BORDER));
                let v = &d.volume;
                let dims = v.dims();
                let title = if d.metadata.description.is_empty() {
                    self.current.as_deref().map(file_name).unwrap_or_else(|| "Study".into())
                } else {
                    d.metadata.description.clone()
                };
                let mut sub = format!("{} × {} × {}", dims.x, dims.y, dims.z);
                if !d.metadata.modality.is_empty() {
                    sub = format!("{} · {sub}", d.metadata.modality);
                }
                ui.vertical(|ui| {
                    ui.add_space(9.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(title).size(13.5).strong().color(TEXT));
                    ui.label(RichText::new(sub).size(11.5).color(TEXT_DIM));
                });
            }
        });

        // view-mode switch in the middle
        if has_data {
            let mid = Rect::from_center_size(rect.center(), Vec2::new(190.0, rect.height()));
            row_in(ui, mid, Layout::left_to_right(Align::Center), |ui| {
                let modes = [ViewMode::Slice2d, ViewMode::Volume3d, ViewMode::Mpr];
                let current = modes.iter().position(|m| *m == self.viewer.view_mode());
                if let Some(i) = segmented(ui, &["2D", "3D", "MPR"], current) {
                    self.viewer.set_view_mode(modes[i]);
                }
            });
        }

        // actions
        row_in(ui, rect, Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if tool_button(ui, icon::QUESTION, "About", self.show_about).clicked() {
                self.show_about = !self.show_about;
            }
            if !has_data {
                return;
            }
            if tool_button(ui, icon::SLIDERS_HORIZONTAL, "Settings panel", self.show_panel).clicked() {
                self.show_panel = !self.show_panel;
            }
            let details = self.show_panel && self.panel.tab == PanelTab::Details;
            if tool_button(ui, icon::INFO, "Info", details).clicked() {
                if details {
                    self.show_panel = false;
                } else {
                    self.show_panel = true;
                    self.panel.tab = PanelTab::Details;
                }
            }
            toolbar_separator(ui);
            if tool_button(ui, icon::CAMERA, "Screenshot", false).clicked() {
                self.request_screenshot(ui.ctx());
            }
            if tool_button(ui, icon::DOWNLOAD_SIMPLE, "Export NIfTI", false).clicked() {
                self.export_nifti();
            }
            if tool_button(ui, icon::FILE_PLUS, "Open files", false).clicked() {
                self.pick_files();
            }
            if tool_button(ui, icon::FOLDER_OPEN, "Open folder", false).clicked() {
                self.pick_folder();
            }
            if tool_button(ui, icon::CLIPBOARD_TEXT, "Open workspace", false).clicked() {
                self.pick_workspace();
            }
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let mode = self.viewer.view_mode();
        row_in(ui, rect, Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if mode == ViewMode::Volume3d {
                let erasing = self.viewer.volume.eraser_enabled;
                if tool_button(ui, icon::CUBE_FOCUS, "Rotate", !erasing).clicked() {
                    self.viewer.volume.eraser_enabled = false;
                }
                if tool_button(ui, icon::ERASER, "Volume eraser", erasing).clicked() {
                    self.viewer.volume.eraser_enabled = !erasing;
                }
                toolbar_separator(ui);
                for p in ViewPreset::ALL {
                    if tool_button(ui, p.label(), p.name(), false).clicked() {
                        self.viewer.volume.camera.look_from(p);
                    }
                }
                toolbar_separator(ui);
                if tool_button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Reset camera", false).clicked() {
                    self.viewer.reset_view_3d();
                }
                let can_undo = self.viewer.erase_history_len() > 0;
                if tool_button_enabled(ui, icon::ARROW_U_UP_LEFT, "Undo erase", false, can_undo).clicked() {
                    self.viewer.undo_erase();
                }
            } else {
                for t in ToolKind::ALL {
                    self.tool_button(ui, t);
                }
                toolbar_separator(ui);
                ui.label(RichText::new("Segment").size(12.0).color(TEXT_DIM));
                for t in ToolKind::SEGMENT {
                    self.tool_button(ui, t);
                }
                toolbar_separator(ui);
                let any = !self.viewer.annotations().is_empty();
                if tool_button_enabled(ui, icon::TRASH, "Clear annotations", false, any).clicked() {
                    self.viewer.clear_annotations();
                }
                if mode == ViewMode::Slice2d {
                    toolbar_separator(ui);
                    self.zoom_buttons(ui);
                }
            }
        });
        if mode == ViewMode::Slice2d {
            row_in(ui, rect, Layout::right_to_left(Align::Center), |ui| {
                let current = SliceAxis::ALL.iter().position(|a| *a == self.viewer.slices.axis);
                let labels = SliceAxis::ALL.map(|a| a.label());
                if let Some(i) = segmented(ui, &labels, current) {
                    self.viewer.slices.axis = SliceAxis::ALL[i];
                }
            });
        }
    }

    /// A toolbar button selecting `t`; disabled, with the reason on hover,
    /// when the tool cannot be used now (e.g. an AI tool without engine).
    fn tool_button(&mut self, ui: &mut egui::Ui, t: ToolKind) {
        let available = self.viewer.tool_availability(t);
        let active = self.viewer.tool == t && available.is_ok();
        let resp = tool_button_enabled(ui, tool_icon(t), t.label(), active, available.is_ok());
        match available {
            Ok(()) => {
                if resp.on_hover_text(t.hint()).clicked() {
                    self.viewer.select_tool(t);
                }
            }
            Err(reason) => {
                resp.on_hover_text(format!("{}: {reason}", t.label()));
            }
        }
    }

    fn zoom_buttons(&mut self, ui: &mut egui::Ui) {
        let axis = self.viewer.slices.axis;
        let view = &mut self.viewer.slices.views[axis.normal_axis()];
        let mut scale = |f: f32| {
            let z = (view.zoom * f).clamp(ferrum_domain::SliceView::MIN_ZOOM, ferrum_domain::SliceView::MAX_ZOOM);
            view.pan *= z / view.zoom;
            view.zoom = z;
        };
        if tool_button(ui, icon::MAGNIFYING_GLASS_PLUS, "Zoom in", false).clicked() {
            scale(1.25);
        }
        if tool_button(ui, icon::MAGNIFYING_GLASS_MINUS, "Zoom out", false).clicked() {
            scale(0.8);
        }
        if tool_button(ui, icon::ARROWS_IN, "Fit to view", false).clicked() {
            *view = ferrum_domain::SliceView::default();
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        row_in(ui, rect, Layout::left_to_right(Align::Center), |ui| {
            if self.viewer.is_computing() {
                ui.spinner();
            }
            ui.label(RichText::new(&self.viewer.status.message).size(12.0).color(TEXT_DIM));
        });
        row_in(ui, rect, Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            ui.label(RichText::new(format!("{:.1} ms", self.frame_ms)).monospace().size(11.5).color(TEXT_DIM));
            if let Some(p) = self.viewer.probe {
                ui.label(
                    RichText::new(format!("({}, {}, {}) = {:.1}", p.voxel.x, p.voxel.y, p.voxel.z, p.value))
                        .monospace()
                        .size(11.5)
                        .color(OVERLAY),
                );
            }
        });
    }

    // -------------------------------------------------------------- studies

    fn studies(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Studies").size(13.5).strong().color(TEXT));
        ui.add_space(2.0);
        let mut chosen = None;
        if let Some(d) = self.viewer.dataset() {
            let v = &d.volume;
            let dims = v.dims();
            let title = self.current.as_deref().map(file_name).unwrap_or_else(|| "Current study".into());
            let mut sub = format!("{} × {} × {}", dims.x, dims.y, dims.z);
            if !d.metadata.modality.is_empty() {
                sub = format!("{} · {sub}", d.metadata.modality);
            }
            study_row(ui, &title, &sub, true);
        }
        let others: Vec<PathBuf> =
            self.recent.items().iter().filter(|p| Some(*p) != self.current.as_ref()).cloned().collect();
        if !others.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{}  Recent", icon::CLOCK_COUNTER_CLOCKWISE)).size(12.0).color(TEXT_DIM));
            for p in others {
                let parent = p.parent().map(|q| q.to_string_lossy().into_owned()).unwrap_or_default();
                if study_row(ui, &file_name(&p), &parent, false).on_hover_text(p.to_string_lossy()).clicked() {
                    chosen = Some(p);
                }
            }
        }
        if let Some(p) = chosen {
            self.open(vec![p]);
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.label(
                RichText::new("Drop a DICOM folder or a NIfTI file anywhere to open it.").size(11.5).color(TEXT_DIM),
            );
        });
    }

    fn loading_overlay(&mut self, ctx: &egui::Context) {
        egui::Area::new(Id::new("loading")).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).order(Order::Tooltip).show(
            ctx,
            |ui| {
                theme::overlay().show(ui, |ui| {
                    ui.set_width(360.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Loading").size(15.0).strong().color(TEXT));
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
        p.rect_filled(screen, 0.0, Color32::from_black_alpha(160));
        let card = Rect::from_center_size(screen.center(), Vec2::new(420.0, 180.0));
        p.rect_filled(card, 16.0, theme::SURFACE);
        dashed_rect(&p, card.shrink(8.0), 12.0, ACCENT);
        p.text(
            card.center() - Vec2::new(0.0, 20.0),
            Align2::CENTER_CENTER,
            icon::DOWNLOAD_SIMPLE,
            egui::FontId::proportional(38.0),
            ACCENT,
        );
        p.text(
            card.center() + Vec2::new(0.0, 30.0),
            Align2::CENTER_CENTER,
            "Drop to open",
            egui::FontId::proportional(17.0),
            TEXT,
        );
    }

    // --------------------------------------------------------- start screen

    fn start_screen(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.vertical_centered(|ui| {
            ui.add_space((rect.height() * 0.12).max(24.0));
            let (r, _) = ui.allocate_exact_size(Vec2::splat(76.0), egui::Sense::hover());
            widgets::logo(ui.painter(), r);
            ui.add_space(10.0);
            ui.label(RichText::new("FERRUM").size(30.0).strong().color(TEXT));
            ui.label(RichText::new("High-performance medical imaging").size(15.0).color(TEXT_DIM));
            ui.add_space(24.0);

            let card_w = 520.0f32.min(ui.available_width() - 40.0);
            theme::card().inner_margin(egui::Margin::same(18)).show(ui, |ui| {
                ui.set_width(card_w);
                let (r, _) = ui.allocate_exact_size(Vec2::new(card_w, 120.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 10.0, theme::BG);
                dashed_rect(ui.painter(), r.shrink(1.0), 10.0, BORDER.gamma_multiply(1.6));
                ui.painter().text(
                    r.center() - Vec2::new(0.0, 16.0),
                    Align2::CENTER_CENTER,
                    icon::FOLDER_OPEN,
                    egui::FontId::proportional(30.0),
                    ACCENT,
                );
                ui.painter().text(
                    r.center() + Vec2::new(0.0, 24.0),
                    Align2::CENTER_CENTER,
                    "Drop a DICOM folder or a NIfTI file here",
                    egui::FontId::proportional(14.0),
                    TEXT_DIM,
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space((card_w - 2.0 * 168.0 - 10.0).max(0.0) / 2.0);
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if pill_button(ui, icon::FOLDER_OPEN, "Open folder", true).clicked() {
                        self.pick_folder();
                    }
                    if pill_button(ui, icon::FILE_PLUS, "Open files", false).clicked() {
                        self.pick_files();
                    }
                });
                if !self.recent.items().is_empty() {
                    ui.add_space(14.0);
                    ui.with_layout(Layout::top_down(Align::Min), |ui| self.recent_studies(ui));
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

    /// "Recent studies" list of the start screen; a click opens the study.
    fn recent_studies(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(format!("{}  Recent studies", icon::CLOCK_COUNTER_CLOCKWISE)).size(12.5).color(TEXT_DIM),
        );
        let mut chosen = None;
        for p in self.recent.items() {
            let parent = p.parent().map(|q| q.to_string_lossy().into_owned()).unwrap_or_default();
            if study_row(ui, &file_name(p), &parent, false).on_hover_text(p.to_string_lossy()).clicked() {
                chosen = Some(p.clone());
            }
        }
        if let Some(p) = chosen {
            self.open(vec![p]);
        }
    }

    // -------------------------------------------------------------- dialogs

    /// Series picker shown when a folder holds more than one series.
    fn series_dialog(&mut self, ctx: &egui::Context, series: Vec<ferrum_domain::SeriesDescriptor>) {
        egui::Window::new("Select series")
            .collapsible(false)
            .resizable(true)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new("Several series were found").color(TEXT_DIM));
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for s in series {
                        if series_row(ui, &s) {
                            self.viewer.load_series(s);
                        }
                    }
                });
                if ui.button("Cancel").clicked() {
                    self.viewer.series_choice = None;
                }
            });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(series) = self.viewer.series_choice.clone() {
            self.series_dialog(ctx, series);
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
        if !self.viewer.status.errors.is_empty() {
            egui::Window::new("Errors").collapsible(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                for e in &self.viewer.status.errors {
                    ui.colored_label(DANGER, format!("{} {e}", icon::WARNING));
                }
                if ui.button("Dismiss").clicked() {
                    self.viewer.status.errors.clear();
                }
            });
        }
        if self.show_about {
            let mut open = true;
            egui::Window::new("About").open(&mut open).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), egui::Sense::hover());
                    widgets::logo(ui.painter(), r);
                    ui.vertical(|ui| {
                        ui.heading("FERRUM");
                        ui.label(RichText::new("High-performance medical imaging").color(TEXT_DIM));
                    });
                });
                ui.label("GPU volume rendering and 2D/MPR viewing of DICOM and NIfTI data.");
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(TEXT_DIM));
                ui.hyperlink_to("Source code", "https://github.com/Kauter1989/ferrum");
                ui.add_space(6.0);
                ui.label(RichText::new("Shortcuts").strong());
                for (k, v) in [
                    ("F2 / F3 / F4", "2D / 3D / MPR"),
                    ("Wheel · ↑ ↓", "change slice"),
                    ("Ctrl + wheel", "zoom slice"),
                    ("Ctrl+O", "open folder"),
                    ("Ctrl+Z", "undo: eraser (3D), segmentation (2D, MPR)"),
                    ("Tab", "toggle the settings panel"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(k).monospace().color(OVERLAY));
                        ui.label(RichText::new(v).color(TEXT_DIM));
                    });
                }
            });
            self.show_about = open;
        }
    }

    // --------------------------------------------------------------- actions

    /// Ctrl+Z: undoes what the current view edits — an eraser stroke in
    /// 3D; on slices the last AI prompt of the current object, or else (no
    /// object in progress, whose mask the engine owns) the last
    /// segmentation edit.
    fn undo(&mut self) {
        if self.viewer.view_mode() == ViewMode::Volume3d {
            self.viewer.undo_erase();
        } else if self.viewer.ai().target().is_some() {
            self.viewer.ai_undo();
        } else {
            self.viewer.undo_segmentation();
        }
    }

    fn pick_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            self.open(vec![dir]);
        }
    }

    fn pick_workspace(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Open workspace").pick_folder() {
            self.open_workspace(&dir);
        }
    }

    /// Opens a workspace written by the agent skill (`ferrum-cli`): its
    /// series is loaded and its proposals appear in the Review section.
    pub fn open_workspace(&mut self, dir: &std::path::Path) {
        let generator = format!("FERRUM {}", env!("CARGO_PKG_VERSION"));
        let result = ferrum_io::WorkspaceStore::open(dir, &generator).map_err(|e| e.to_string()).and_then(|store| {
            use ferrum_domain::ResultStore as _;
            let sources = store.source_paths();
            self.viewer.open_workspace(std::sync::Arc::new(store)).map(|()| sources)
        });
        match result {
            Ok(sources) => self.current = study_entry(&sources),
            Err(e) => self.viewer.status.errors.push(format!("Open workspace: {e}")),
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
            match ferrum_io::write_nifti(&volume, &path) {
                Ok(()) => self.viewer.status.message = format!("Exported {}", path.display()),
                Err(e) => self.viewer.status.errors.push(e.to_string()),
            }
        }
    }

    fn export_annotations(&mut self) {
        let Some(report) = self.viewer.annotation_report() else {
            return;
        };
        let stem = report
            .source
            .file_stem()
            .map(|s| s.to_string_lossy().trim_end_matches(".nii").to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "study".into());
        let generator = format!("FERRUM {}", env!("CARGO_PKG_VERSION"));
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_file_name(format!("{stem}_annotations.json"))
            .save_file()
        {
            match ferrum_io::write_annotation_report(&report, &generator, &path) {
                Ok(()) => {
                    self.viewer.status.message =
                        format!("Exported {} annotations to {}", report.annotations.len(), path.display())
                }
                Err(e) => self.viewer.status.errors.push(e.to_string()),
            }
        }
    }

    fn import_labels(&mut self) {
        let Some(volume) = self.viewer.dataset().map(|d| d.volume.clone()) else {
            return;
        };
        let Some(path) = rfd::FileDialog::new().add_filter("NIfTI label map", &["nii", "gz"]).pick_file() else {
            return;
        };
        let result = ferrum_io::read_label_nifti(&path, &volume)
            .map_err(|e| e.to_string())
            .and_then(|labels| self.viewer.import_label_map(labels).map_err(|e| e.to_string()));
        if let Err(e) = result {
            self.viewer.status.errors.push(format!("Label map import failed: {e}"));
        }
    }

    fn export_labels(&mut self) {
        let Some(volume) = self.viewer.dataset().map(|d| d.volume.clone()) else {
            return;
        };
        let Some(set) = self.viewer.segmentation().set() else {
            return;
        };
        if let Some(path) = rfd::FileDialog::new().set_file_name("segments.nii.gz").save_file() {
            match ferrum_io::write_label_nifti(set.labels(), &volume, &path) {
                Ok(()) => self.viewer.status.message = format!("Exported segments to {}", path.display()),
                Err(e) => self.viewer.status.errors.push(e.to_string()),
            }
        }
    }

    fn request_screenshot(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new().set_file_name("ferrum.png").save_file() {
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
            self.undo();
        }
        if tab {
            self.show_panel = !self.show_panel;
        }
        if self.viewer.dataset().is_some() {
            if v2 {
                self.viewer.set_view_mode(ViewMode::Slice2d);
            }
            if v3 {
                self.viewer.set_view_mode(ViewMode::Volume3d);
            }
            if vm {
                self.viewer.set_view_mode(ViewMode::Mpr);
            }
        }
    }
}

/// One row of the series picker. Returns `true` when "Load" was clicked.
fn series_row(ui: &mut egui::Ui, s: &ferrum_domain::SeriesDescriptor) -> bool {
    ui.horizontal(|ui| {
        let load = ui.button(format!("{} Load", icon::CARET_RIGHT)).clicked();
        ui.label(RichText::new(format!("{} · {}", s.modality, s.format)).color(OVERLAY).monospace());
        ui.label(&s.description);
        load
    })
    .inner
}

/// Large button with icon and text, used on the start screen.
fn pill_button(ui: &mut egui::Ui, icon_str: &str, text: &str, primary: bool) -> egui::Response {
    let color = if primary { Color32::WHITE } else { TEXT };
    let r = ui.add(
        egui::Button::new(RichText::new(format!("{icon_str}   {text}")).size(14.0).color(color))
            .min_size(Vec2::new(168.0, 40.0))
            .corner_radius(10.0)
            .fill(if primary { ACCENT } else { theme::CONTROL }),
    );
    let label = text.to_string();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    r
}

/// Row of the studies list: file name and a dim second line.
fn study_row(ui: &mut egui::Ui, title: &str, subtitle: &str, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 46.0), egui::Sense::click());
    let label = title.to_string();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &label));
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 8.0, ACCENT_SOFT);
        p.rect_stroke(rect, 8.0, Stroke::new(1.0, ACCENT.gamma_multiply(0.8)), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(rect, 8.0, theme::CONTROL);
    }
    let tile = Rect::from_min_size(rect.min + Vec2::new(8.0, 7.0), Vec2::splat(32.0));
    p.rect_filled(tile, 6.0, theme::CANVAS);
    p.text(tile.center(), Align2::CENTER_CENTER, icon::IMAGE, egui::FontId::proportional(17.0), TEXT_DIM);
    let text_x = tile.max.x + 10.0;
    let clip = Rect::from_min_max(Pos2::new(text_x, rect.min.y), rect.max - Vec2::new(6.0, 0.0));
    let painter = p.with_clip_rect(clip);
    painter.text(Pos2::new(text_x, rect.min.y + 8.0), Align2::LEFT_TOP, title, egui::FontId::proportional(13.0), TEXT);
    painter.text(
        Pos2::new(text_x, rect.min.y + 26.0),
        Align2::LEFT_TOP,
        subtitle,
        egui::FontId::proportional(11.0),
        TEXT_DIM,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
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
