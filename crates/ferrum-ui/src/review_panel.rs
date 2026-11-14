//! "Review" section: the queue of proposals (from agents and engines)
//! with Confirm / Reject, the reviewer's name, and the open workspace.

use egui::RichText;
use egui_phosphor::light as icon;
use ferrum_app::Viewer;
use ferrum_domain::ReviewItem;

use super::segments_panel::provenance_line;
use super::theme::{SURFACE, TEXT_DIM, WARN};

/// UI-only state of the section.
#[derive(Debug, Clone, Default)]
pub struct ReviewPanelState {
    /// Name recorded with decisions (may be empty).
    pub reviewer: String,
}

/// The reviewer's name, if one was entered.
pub fn reviewer(state: &ReviewPanelState) -> Option<&str> {
    Some(state.reviewer.trim()).filter(|r| !r.is_empty())
}

/// `true` when the section has something to show.
pub fn relevant(viewer: &Viewer) -> bool {
    viewer.workspace().is_some() || !viewer.review_queue().is_empty()
}

/// Draws the section body.
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut ReviewPanelState) {
    if let Some(ws) = viewer.workspace() {
        ui.label(RichText::new(format!("{} Workspace {ws}", icon::FOLDER_SIMPLE)).size(12.0).color(TEXT_DIM));
        ui.horizontal_wrapped(|ui| {
            let dirty = viewer.workspace_dirty();
            let label = if dirty { "Save to workspace •" } else { "Saved" };
            if ui.add_enabled(dirty, egui::Button::new(format!("{} {label}", icon::FLOPPY_DISK))).clicked() {
                if let Err(e) = viewer.save_workspace() {
                    viewer.status.errors.push(e);
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Reviewer").size(12.0).color(TEXT_DIM));
        let edit = ui.add(egui::TextEdit::singleline(&mut state.reviewer).hint_text("your name").desired_width(160.0));
        edit.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Reviewer name"));
    });
    let queue = viewer.review_queue();
    if queue.is_empty() {
        ui.label(RichText::new(format!("{} Nothing waits for review.", icon::CHECK_CIRCLE)).size(12.0).color(TEXT_DIM));
        return;
    }
    ui.label(
        RichText::new(format!("{} {} proposal(s) are not findings until confirmed.", icon::WARNING, queue.len()))
            .size(12.0)
            .color(WARN),
    );
    for entry in queue {
        let frame = egui::Frame::new().fill(SURFACE).corner_radius(8).inner_margin(egui::Margin::symmetric(8, 6));
        let mut decision = None;
        let mut go_to = false;
        frame.show(ui, |ui| {
            ui.set_width((ui.available_width() - 16.0).max(140.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} · {}", entry.kind, entry.name)).strong());
                if let ReviewItem::Annotation(_) = entry.item {
                    go_to = super::widgets::tool_button(
                        ui,
                        icon::ARROW_SQUARE_OUT,
                        &format!("Go to {}", entry.name),
                        false,
                    )
                    .clicked();
                }
            });
            decision = provenance_line(ui, &entry.provenance, &entry.name);
        });
        if let (true, ReviewItem::Annotation(id)) = (go_to, entry.item) {
            viewer.go_to_annotation(id);
        }
        if let Some(status) = decision {
            if let Err(e) = viewer.decide(entry.item, status, reviewer(state)) {
                viewer.status.errors.push(e);
            }
        }
    }
}
