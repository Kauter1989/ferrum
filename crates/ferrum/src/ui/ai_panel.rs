//! "AI segmentation" section of the settings panel. It is always visible;
//! its tools stay disabled until a `ferrum-engine/1` engine is connected.

use std::sync::Arc;

use egui::{Color32, RichText};
use egui_phosphor::light as icon;
use ferrum_app::{AiStatus, ToolKind, Viewer};
use ferrum_engines::{HttpConfig, HttpEngine};

use super::theme::{ACCENT, DANGER, TEXT, TEXT_DIM};
use super::widgets::{segmented, tool_button_enabled};

/// Engine URL used when `FERRUM_ENGINE_URL` is not set.
pub const DEFAULT_ENGINE_URL: &str = "http://127.0.0.1:8765";

/// UI-only state of the section.
#[derive(Debug, Clone)]
pub struct AiPanelState {
    /// Engine base URL being edited.
    pub url: String,
}

impl Default for AiPanelState {
    fn default() -> Self {
        Self { url: std::env::var("FERRUM_ENGINE_URL").unwrap_or_else(|_| DEFAULT_ENGINE_URL.to_owned()) }
    }
}

fn status_line(viewer: &Viewer) -> (Color32, String) {
    let ai = viewer.ai();
    let name = ai.info().map(|i| format!("{} {}", i.name, i.version)).unwrap_or_default();
    match ai.status() {
        AiStatus::Disconnected => (TEXT_DIM, "No engine connected".into()),
        AiStatus::Connecting => (ACCENT, format!("Connecting to {}…", ai.engine_label())),
        AiStatus::Connected => (Color32::from_rgb(90, 190, 120), format!("{name} — draw a prompt to start")),
        AiStatus::Uploading => (ACCENT, format!("{name} — uploading the volume…")),
        AiStatus::Ready => (Color32::from_rgb(90, 190, 120), format!("{name} — ready")),
        AiStatus::Working => (ACCENT, format!("{name} — segmenting…")),
        AiStatus::Failed(e) => (DANGER, e.clone()),
    }
}

fn connection(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut AiPanelState) {
    let connected = viewer.ai().status().is_connected() || *viewer.ai().status() == AiStatus::Connecting;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 110.0).max(80.0);
        let edit = ui.add_enabled(!connected, egui::TextEdit::singleline(&mut state.url).desired_width(width));
        edit.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Engine URL"));
        if connected {
            if ui.button(format!("{} Disconnect", icon::PLUG)).clicked() {
                viewer.disconnect_engine();
            }
        } else if ui.button(format!("{} Connect", icon::PLUGS_CONNECTED)).clicked() {
            let url = state.url.trim().to_owned();
            viewer.connect_engine(Arc::new(HttpEngine::new(HttpConfig::new(&url))), &url);
        }
    });
    let (color, text) = status_line(viewer);
    ui.horizontal_wrapped(|ui| {
        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.0, color);
        ui.label(RichText::new(text).size(12.0).color(if color == DANGER { DANGER } else { TEXT }));
    });
    if let Some(info) = viewer.ai().info() {
        if info.research_only {
            ui.label(
                RichText::new(format!("{} Research use only", icon::FLASK))
                    .strong()
                    .color(Color32::from_rgb(240, 190, 60)),
            );
        }
        if !info.license.is_empty() {
            ui.label(RichText::new(&info.license).size(11.0).color(TEXT_DIM));
        }
    }
}

fn tools(ui: &mut egui::Ui, viewer: &mut Viewer) {
    let positive = viewer.ai().positive;
    if let Some(i) = segmented(ui, &["Include", "Exclude"], Some(usize::from(!positive))) {
        viewer.set_ai_positive(i == 0);
    }
    let has_data = viewer.dataset().is_some();
    ui.horizontal(|ui| {
        for t in ToolKind::AI {
            let enabled = has_data && t.prompt_kind().is_some_and(|k| viewer.ai().supports(k));
            let resp = tool_button_enabled(ui, crate::app::tool_icon(t), t.label(), viewer.tool == t, enabled);
            if enabled && resp.on_hover_text(t.hint()).clicked() {
                viewer.select_tool(t);
            }
        }
    });
    let target = viewer.ai().target();
    let busy = viewer.ai().is_busy();
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(target.is_some() && !busy, egui::Button::new(format!("{} Accept", icon::CHECK))).clicked() {
            viewer.ai_accept();
        }
        if ui.add_enabled(target.is_some(), egui::Button::new(format!("{} Discard", icon::X_CIRCLE))).clicked() {
            viewer.ai_discard();
        }
        if ui
            .add_enabled(
                viewer.ai().can_undo() && !busy,
                egui::Button::new(format!("{} Undo prompt", icon::ARROW_U_UP_LEFT)),
            )
            .clicked()
        {
            viewer.ai_undo();
        }
    });
    if let Some(name) = target.and_then(|t| viewer.segmentation().set()?.segment(t).map(|s| s.name.clone())) {
        ui.label(RichText::new(format!("Editing {name}")).size(12.0).color(TEXT_DIM));
    }
}

/// Draws the section body.
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut AiPanelState) {
    connection(ui, viewer, state);
    ui.add_space(4.0);
    tools(ui, viewer);
    if !viewer.ai().status().is_connected() {
        ui.label(
            RichText::new(
                "To enable these tools, connect a segmentation engine that speaks ferrum-engine/1 — for example \
                 the nnInteractive bridge, or the mock engine: cargo run -p ferrum-engines --example mock_server",
            )
            .size(12.0)
            .color(TEXT_DIM),
        );
    } else {
        ui.label(RichText::new("Prompts are drawn in the 2D views.").size(12.0).color(TEXT_DIM));
    }
}
