//! AI engine part of the "Segmentation" section: connection, Include /
//! Exclude, the current object (Accept, Discard, Undo prompt) and automatic
//! segmentation. The AI prompt tools are in the toolbar; they stay
//! disabled until a `ferrum-engine/1` engine is connected.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{Color32, RichText};
use egui_phosphor::light as icon;
use ferrum_app::{AiStatus, Viewer};
use ferrum_engines::{HttpConfig, HttpEngine};

use super::theme::{ACCENT, DANGER, TEXT, TEXT_DIM};
use super::widgets::segmented;

/// Engine URL used when `FERRUM_ENGINE_URL` is not set.
pub const DEFAULT_ENGINE_URL: &str = "http://127.0.0.1:8765";

/// UI-only state of the section.
#[derive(Debug, Clone)]
pub struct AiPanelState {
    /// Engine base URL being edited.
    pub url: String,
    /// Structures chosen for automatic segmentation (empty = all).
    pub selected: BTreeSet<String>,
    /// Filter of the structure list.
    pub filter: String,
}

impl Default for AiPanelState {
    fn default() -> Self {
        Self {
            url: std::env::var("FERRUM_ENGINE_URL").unwrap_or_else(|_| DEFAULT_ENGINE_URL.to_owned()),
            selected: BTreeSet::new(),
            filter: String::new(),
        }
    }
}

fn status_line(viewer: &Viewer) -> (Color32, String) {
    let ai = viewer.ai();
    let name = ai.info().map(|i| format!("{} {}", i.name, i.version)).unwrap_or_default();
    match ai.status() {
        AiStatus::Disconnected => (TEXT_DIM, "No engine connected".into()),
        AiStatus::Connecting => (ACCENT, format!("Connecting to {}…", ai.engine_label())),
        AiStatus::Connected => (Color32::from_rgb(90, 190, 120), format!("{name} — connected")),
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
            // the access token comes from the environment, never from the UI
            let token = std::env::var("FERRUM_ENGINE_TOKEN").ok().filter(|t| !t.is_empty());
            viewer.connect_engine(Arc::new(HttpEngine::new(HttpConfig { token, ..HttpConfig::new(&url) })), &url);
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

/// Automatic segmentation: structure choice, Run, progress and Cancel.
fn automatic(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut AiPanelState) {
    let names: Vec<String> =
        viewer.ai().info().map(|i| i.labels.iter().map(|l| l.name.clone()).collect()).unwrap_or_default();
    ui.label(RichText::new(format!("{} Automatic", icon::MAGIC_WAND)).strong().color(TEXT));
    if let Some(job) = viewer.ai().job() {
        let text = if job.message.is_empty() { job.state.as_str().to_owned() } else { job.message.clone() };
        ui.add(egui::ProgressBar::new(job.progress).text(text).desired_width(ui.available_width() - 90.0));
        if ui.button(format!("{} Cancel", icon::X_CIRCLE)).clicked() {
            viewer.ai_cancel_job();
        }
        return;
    }
    if names.len() > 1 {
        let filter = ui.add(egui::TextEdit::singleline(&mut state.filter).hint_text("Filter structures"));
        filter.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Filter structures"));
        let needle = state.filter.to_lowercase();
        egui::ScrollArea::vertical().id_salt("ai-labels").max_height(140.0).show(ui, |ui| {
            for name in names.iter().filter(|n| n.to_lowercase().contains(&needle)) {
                let mut on = state.selected.contains(name);
                if ui.checkbox(&mut on, name).changed() {
                    if on {
                        state.selected.insert(name.clone());
                    } else {
                        state.selected.remove(name);
                    }
                }
            }
        });
    }
    // forget choices the engine does not offer (e.g. after reconnecting)
    state.selected.retain(|n| names.contains(n));
    let label = if state.selected.is_empty() {
        "Segment all structures".to_owned()
    } else {
        format!("Segment {} structure(s)", state.selected.len())
    };
    let enabled = viewer.dataset().is_some() && !viewer.ai().is_busy();
    if ui.add_enabled(enabled, egui::Button::new(format!("{} {label}", icon::PLAY))).clicked() {
        let labels = (!state.selected.is_empty()).then(|| state.selected.iter().cloned().collect());
        viewer.ai_run_automatic(labels);
    }
}

/// Draws the section body.
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut AiPanelState) {
    connection(ui, viewer, state);
    ui.add_space(4.0);
    let interactive = viewer.ai().info().is_some_and(|i| i.capabilities.interactive);
    if interactive {
        tools(ui, viewer);
    }
    if viewer.ai().supports_automatic() {
        ui.add_space(4.0);
        automatic(ui, viewer, state);
    }
    if !viewer.ai().status().is_connected() {
        ui.label(
            RichText::new(
                "AI tools need a ferrum-engine/1 engine, e.g. an nnInteractive, TotalSegmentator or MONAI Label \
                 bridge.",
            )
            .size(12.0)
            .color(TEXT_DIM),
        );
    } else if interactive {
        ui.label(RichText::new("Choose an AI tool in the toolbar and draw on a slice.").size(12.0).color(TEXT_DIM));
    }
}
