//! "Segmentation" section of the settings panel: what to do next, the
//! settings of the built-in region tool and the AI engine.
//!
//! The segmentation tools themselves sit in the toolbar next to the
//! measurement tools, because they act on clicks in the slice views like
//! every other tool. The section guides the user through one path for
//! every method: choose a tool, draw on a slice, check the result, keep it.

use egui::RichText;
use egui_phosphor::light as icon;
use ferrum_app::{SegmentationStep, ToolKind, ViewMode, Viewer};

use super::ai_panel::{self, AiPanelState};
use super::theme::{ACCENT, ACCENT_SOFT, TEXT, TEXT_DIM};
use super::widgets::slider_row;

/// What the user should do next, in words.
pub fn next_step(viewer: &Viewer) -> String {
    match viewer.segmentation_step() {
        SegmentationStep::Unavailable(reason) => reason,
        SegmentationStep::ChooseTool => "Choose a segmentation tool in the toolbar: Region works right away; the AI \
                                         tools need an engine (below)."
            .into(),
        SegmentationStep::Draw(ToolKind::Region) => format!(
            "Click inside the structure on a slice. Its connected voxels within ±{:.0} of the clicked value become \
             a new segment.",
            viewer.segmentation().region.tolerance
        ),
        SegmentationStep::Draw(tool) => format!(
            "{}: {} on a slice. Include marks the object, Exclude removes parts of it.",
            tool.label(),
            tool.hint()
        ),
        SegmentationStep::Working => "The engine is working…".into(),
        SegmentationStep::ReviewObject => "Check the object on the slices (and in 3D in the MPR view). Add prompts \
                                           to refine it, then Accept or Discard."
            .into(),
    }
}

/// A one-line reminder for the slice views while a segmentation tool is
/// active (the panel has the full guide).
pub fn short_step(viewer: &Viewer) -> Option<String> {
    match viewer.segmentation_step() {
        SegmentationStep::Draw(tool) => Some(format!("{}: {}", tool.label(), tool.hint())),
        SegmentationStep::Working => Some("Segmenting…".into()),
        SegmentationStep::ReviewObject => Some("Refine with more prompts, or Accept / Discard in the panel".into()),
        SegmentationStep::Unavailable(_) | SegmentationStep::ChooseTool => None,
    }
}

/// The guide card: the next step, with a way out of the 3D view.
fn guide(ui: &mut egui::Ui, viewer: &mut Viewer) {
    let unavailable = viewer.can_segment().is_err();
    egui::Frame::new().fill(ACCENT_SOFT).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(icon::SIGNPOST).size(15.0).color(ACCENT));
            ui.label(RichText::new(next_step(viewer)).size(12.5).color(TEXT));
        });
        if unavailable && viewer.dataset().is_some() && ui.button(format!("{} Segment in 2D", icon::SQUARE)).clicked() {
            viewer.set_view_mode(ViewMode::Slice2d);
        }
    });
}

/// Tolerance and size limit of the region tool.
fn region_settings(ui: &mut egui::Ui, viewer: &mut Viewer) {
    let Some(span) = viewer.dataset().map(|d| d.volume.range().span().max(1.0)) else {
        return;
    };
    ui.label(RichText::new(format!("{} Region tool", icon::PAINT_BUCKET)).strong().color(TEXT));
    let region = viewer.region_settings_mut();
    slider_row(ui, "Tolerance ±", &mut region.tolerance, 0.0..=span * 0.5);
    slider_row(ui, "Max volume, ml", &mut region.max_ml, 1.0..=20000.0);
    slider_row(ui, "Noise smoothing, mm", &mut region.smoothing_mm, 0.0..=6.0);
    slider_row(ui, "Cut thin bridges, mm", &mut region.opening_mm, 0.0..=15.0);
    ui.checkbox(&mut region.fill_holes, "Fill vessels and holes");
    ui.label(
        RichText::new("Whole organs: raise the tolerance. Strong smoothing shrinks small structures. Larger regions have leaked and are not created.")
            .size(11.5)
            .color(TEXT_DIM),
    );
}

/// Draws the section body (2D and MPR views).
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, ai: &mut AiPanelState) {
    guide(ui, viewer);
    if viewer.can_segment().is_err() {
        return;
    }
    if viewer.tool == ToolKind::Region {
        // settings of the active tool only, to keep the panel short
        ui.add_space(6.0);
        region_settings(ui, viewer);
    }
    ui.add_space(6.0);
    ui.label(RichText::new(format!("{} AI engine", icon::MAGIC_WAND)).strong().color(TEXT));
    ai_panel::show(ui, viewer, ai);
}

/// Note shown in the 3D view, where segments are displayed but not
/// created.
pub fn volume_hint(ui: &mut egui::Ui, viewer: &mut Viewer) {
    if viewer.view_mode() != ViewMode::Volume3d {
        return;
    }
    egui::Frame::new().fill(ACCENT_SOFT).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new(format!(
                "{}  The 3D view shows segments. To create or edit them, switch to 2D or MPR and choose a \
                 segmentation tool in the toolbar.",
                icon::INFO
            ))
            .size(12.5)
            .color(TEXT),
        );
        if ui.button(format!("{} Segment in 2D", icon::SQUARE)).clicked() {
            viewer.set_view_mode(ViewMode::Slice2d);
        }
    });
}
