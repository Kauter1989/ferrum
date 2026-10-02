//! "Segments" section of the settings panel: the segment list with colour,
//! visibility, opacity, volume and name, plus label-map import/export.
//! Segments are created with the segmentation tools of the toolbar (or
//! imported); there is no empty "new segment", which nothing could fill.

use std::collections::HashMap;

use egui::{RichText, Slider};
use egui_phosphor::light as icon;
use ferrum_app::{SegmentSummary, Viewer};
use ferrum_domain::{Author, Provenance, ReviewStatus};

use super::theme::{DANGER, SURFACE, TEXT_DIM, WARN};
use super::widgets::tool_button;

/// UI-only state of the section.
#[derive(Debug, Clone, Default)]
pub struct SegmentsPanelState {
    /// Names being edited (kept while the field has focus).
    name_edits: HashMap<u8, String>,
    /// Set when the user asks to import a label map; the application shows
    /// the file dialog and resets the flag.
    pub import_labels: bool,
    /// Set when the user asks to export the label map.
    pub export_labels: bool,
}

/// What the user did on one row.
enum RowAction {
    None,
    Rename(String),
    Color([u8; 3]),
    Visible(bool),
    Opacity(f32),
    Review(ReviewStatus),
    Delete,
}

/// Draws the section body.
/// Review decisions are recorded in the name of `reviewer` (and, with a
/// workspace open, saved and logged there).
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut SegmentsPanelState, reviewer: Option<&str>) {
    let rows = viewer.segment_summaries();
    ui.horizontal_wrapped(|ui| {
        if ui.button(format!("{} Import", icon::UPLOAD_SIMPLE)).clicked() {
            state.import_labels = true;
        }
        if ui.add_enabled(!rows.is_empty(), egui::Button::new(format!("{} Export", icon::DOWNLOAD_SIMPLE))).clicked() {
            state.export_labels = true;
        }
        if ui
            .add_enabled(
                viewer.can_undo_segmentation(),
                egui::Button::new(format!("{} Undo edit", icon::ARROW_U_UP_LEFT)),
            )
            .clicked()
        {
            viewer.undo_segmentation();
        }
    });
    if rows.is_empty() {
        let text = if viewer.can_segment().is_ok() {
            "No segments yet. Choose Region or an AI tool in the toolbar and click on a slice, or import a label \
             map (NIfTI)."
        } else {
            "No segments yet. Create them in the 2D or MPR view, or import a label map (NIfTI)."
        };
        ui.label(RichText::new(text).size(12.0).color(TEXT_DIM));
        return;
    }
    let mut show = viewer.segmentation().show;
    if ui.checkbox(&mut show, "Show segments").changed() {
        viewer.set_segments_shown(show);
    }
    for row in rows {
        let label = row.segment.label;
        let result = match segment_row(ui, &row, state) {
            RowAction::None => Ok(()),
            RowAction::Rename(name) => {
                viewer.rename_segment(label, &name);
                Ok(())
            }
            RowAction::Color(c) => viewer.set_segment_color(label, c),
            RowAction::Visible(v) => viewer.set_segment_visible(label, v),
            RowAction::Opacity(o) => viewer.set_segment_opacity(label, o),
            RowAction::Review(status) => {
                if let Err(e) = viewer.decide(ferrum_domain::ReviewItem::Segment(label), status, reviewer) {
                    viewer.status.errors.push(e);
                }
                Ok(())
            }
            RowAction::Delete => {
                state.name_edits.remove(&label);
                viewer.remove_segment(label)
            }
        };
        if let Err(e) = result {
            viewer.status.errors.push(e.to_string());
        }
    }
}

fn segment_row(ui: &mut egui::Ui, row: &SegmentSummary, state: &mut SegmentsPanelState) -> RowAction {
    let mut action = RowAction::None;
    let s = &row.segment;
    let frame = egui::Frame::new().fill(SURFACE).corner_radius(8).inner_margin(egui::Margin::symmetric(8, 6));
    let inner_width = (ui.available_width() - 16.0).max(140.0);
    frame.show(ui, |ui| {
        ui.set_width(inner_width);
        ui.horizontal(|ui| {
            let mut color = s.color;
            let swatch = ui.color_edit_button_srgb(&mut color);
            swatch.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::ColorButton, true, format!("Colour of {}", s.name))
            });
            if swatch.changed() {
                action = RowAction::Color(color);
            }
            let buf = state.name_edits.entry(s.label).or_insert_with(|| s.name.clone());
            let used = ui.min_rect().width() + 2.0 * super::widgets::TOOL + 3.0 * ui.spacing().item_spacing.x;
            let edit = ui.add(egui::TextEdit::singleline(buf).desired_width((inner_width - used).max(60.0)));
            edit.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Segment name"));
            if edit.changed() && !buf.trim().is_empty() {
                action = RowAction::Rename(buf.clone());
            }
            if !edit.has_focus() && !edit.changed() {
                state.name_edits.remove(&s.label);
            }
            let (eye, verb) = if s.visible { (icon::EYE, "Hide") } else { (icon::EYE_SLASH, "Show") };
            if tool_button(ui, eye, &format!("{verb} {}", s.name), false).clicked() {
                action = RowAction::Visible(!s.visible);
            }
            if tool_button(ui, icon::TRASH, &format!("Delete {}", s.name), false).clicked() {
                action = RowAction::Delete;
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{:.2} ml · {} vox", row.volume_ml, row.voxels)).size(12.0).color(TEXT_DIM));
            ui.scope(|ui| {
                ui.spacing_mut().slider_width = (inner_width - 170.0).max(50.0);
                let mut opacity = s.opacity;
                let slider = ui.add(Slider::new(&mut opacity, 0.0..=1.0).show_value(false));
                slider.widget_info(|| {
                    egui::WidgetInfo::slider(true, f64::from(opacity), format!("Opacity of {}", s.name))
                });
                if slider.changed() {
                    action = RowAction::Opacity(opacity);
                }
            });
        });
        if let Some(review) = provenance_line(ui, &s.provenance, &s.name) {
            action = RowAction::Review(review);
        }
    });
    action
}

/// Who proposed an item and its review state, with Confirm / Reject for
/// proposals. Nothing is shown for items drawn and confirmed by a person.
/// Returns the decision the user took.
pub fn provenance_line(ui: &mut egui::Ui, p: &Provenance, name: &str) -> Option<ReviewStatus> {
    if p.author == Author::Human && p.status == ReviewStatus::Confirmed {
        return None;
    }
    let mut decision = None;
    ui.horizontal_wrapped(|ui| {
        let (text, color) = match p.status {
            ReviewStatus::Proposed => (format!("{} Proposed by {}", icon::HOURGLASS_MEDIUM, p.author.describe()), WARN),
            ReviewStatus::Confirmed => {
                (format!("{} Confirmed · {}", icon::CHECK_CIRCLE, p.author.describe()), TEXT_DIM)
            }
            ReviewStatus::Rejected => (format!("{} Rejected · {}", icon::X_CIRCLE, p.author.describe()), DANGER),
        };
        let mut hover = String::new();
        if let Some(t) = p.created {
            hover.push_str(&format!("Created {t}"));
        }
        if let (Some(t), by) = (p.reviewed, &p.reviewed_by) {
            hover.push_str(&format!("\nReviewed {t}{}", by.as_deref().map(|b| format!(" by {b}")).unwrap_or_default()));
        }
        if matches!(p.author, Author::Engine { research_only: true, .. }) {
            hover.push_str("\nResearch use only");
        }
        let label = ui.label(RichText::new(text).size(12.0).color(color));
        if !hover.is_empty() {
            label.on_hover_text(hover.trim_start());
        }
        if p.status == ReviewStatus::Proposed {
            if ui.small_button("Confirm").on_hover_text(format!("Confirm {name}")).clicked() {
                decision = Some(ReviewStatus::Confirmed);
            }
            if ui.small_button("Reject").on_hover_text(format!("Reject {name}")).clicked() {
                decision = Some(ReviewStatus::Rejected);
            }
        } else if ui.small_button("Reopen").on_hover_text(format!("Mark {name} as proposed again")).clicked() {
            decision = Some(ReviewStatus::Proposed);
        }
    });
    decision
}
