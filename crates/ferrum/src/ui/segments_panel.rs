//! "Segments" section of the settings panel: the segment list with colour,
//! visibility, opacity, volume and name, plus label-map import/export.

use std::collections::HashMap;

use egui::{RichText, Slider};
use egui_phosphor::light as icon;
use ferrum_app::{SegmentSummary, Viewer};

use super::theme::{SURFACE, TEXT_DIM};
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
    Delete,
}

/// Draws the section body.
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut SegmentsPanelState) {
    let rows = viewer.segment_summaries();
    ui.horizontal_wrapped(|ui| {
        if ui.button(format!("{} Add", icon::PLUS)).clicked() {
            if let Ok(label) = viewer.add_segment("") {
                state.name_edits.remove(&label);
            }
        }
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
        ui.label(
            RichText::new("Import a label map (NIfTI) or add a segment. Segmentation engines fill segments.")
                .size(12.0)
                .color(TEXT_DIM),
        );
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
    });
    action
}
