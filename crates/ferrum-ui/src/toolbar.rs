//! The tool bar of the views: tools of the 2D and MPR views (measurements,
//! segmentation, AI prompts), the 3D view's camera and eraser, zoom, and the
//! plane switch of the 2D view.

use egui::{Align, Layout, RichText};
use egui_phosphor::light as icon;
use ferrum_app::{ToolKind, ViewMode, Viewer};
use ferrum_domain::{SliceAxis, ViewPreset};

use crate::theme::TEXT_DIM;
use crate::widgets::{row_in, segmented, tool_button, tool_button_enabled, toolbar_separator};

/// Icon of a 2D tool.
pub fn tool_icon(t: ToolKind) -> &'static str {
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

/// The view-mode switch (2D, 3D, MPR), at the start of the widget's tool
/// bar; the desktop app has it in its header.
pub fn mode_switch(ui: &mut egui::Ui, viewer: &mut Viewer) {
    let modes = [ViewMode::Slice2d, ViewMode::Volume3d, ViewMode::Mpr];
    let rect = ui.max_rect();
    row_in(ui, rect, Layout::left_to_right(Align::Center), |ui| {
        let current = modes.iter().position(|m| *m == viewer.view_mode());
        if let Some(i) = segmented(ui, &["2D", "3D", "MPR"], current) {
            viewer.set_view_mode(modes[i]);
        }
    });
}

/// Draws the tool bar into `ui` (one row, the height of `ui`).
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer) {
    let rect = ui.max_rect();
    let mode = viewer.view_mode();
    row_in(ui, rect, Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if mode == ViewMode::Volume3d {
            let erasing = viewer.volume.eraser_enabled;
            if tool_button(ui, icon::CUBE_FOCUS, "Rotate", !erasing).clicked() {
                viewer.volume.eraser_enabled = false;
            }
            if tool_button(ui, icon::ERASER, "Volume eraser", erasing).clicked() {
                viewer.volume.eraser_enabled = !erasing;
            }
            toolbar_separator(ui);
            for p in ViewPreset::ALL {
                if tool_button(ui, p.label(), p.name(), false).clicked() {
                    viewer.volume.camera.look_from(p);
                }
            }
            toolbar_separator(ui);
            if tool_button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Reset camera", false).clicked() {
                viewer.reset_view_3d();
            }
            let can_undo = viewer.erase_history_len() > 0;
            if tool_button_enabled(ui, icon::ARROW_U_UP_LEFT, "Undo erase", false, can_undo).clicked() {
                viewer.undo_erase();
            }
        } else {
            for t in ToolKind::ALL {
                tool_select(ui, viewer, t);
            }
            toolbar_separator(ui);
            ui.label(RichText::new("Segment").size(12.0).color(TEXT_DIM));
            for t in ToolKind::SEGMENT {
                tool_select(ui, viewer, t);
            }
            toolbar_separator(ui);
            let any = !viewer.annotations().is_empty();
            if tool_button_enabled(ui, icon::TRASH, "Clear annotations", false, any).clicked() {
                viewer.clear_annotations();
            }
            if mode == ViewMode::Slice2d {
                toolbar_separator(ui);
                zoom_buttons(ui, viewer);
            }
        }
    });
    if mode == ViewMode::Slice2d {
        row_in(ui, rect, Layout::right_to_left(Align::Center), |ui| {
            let current = SliceAxis::ALL.iter().position(|a| *a == viewer.slices.axis);
            let labels = SliceAxis::ALL.map(|a| a.label());
            if let Some(i) = segmented(ui, &labels, current) {
                viewer.slices.axis = SliceAxis::ALL[i];
            }
        });
    }
}

/// A toolbar button selecting `t`; disabled, with the reason on hover,
/// when the tool cannot be used now (e.g. an AI tool without engine).
fn tool_select(ui: &mut egui::Ui, viewer: &mut Viewer, t: ToolKind) {
    let available = viewer.tool_availability(t);
    let active = viewer.tool == t && available.is_ok();
    let resp = tool_button_enabled(ui, tool_icon(t), t.label(), active, available.is_ok());
    match available {
        Ok(()) => {
            if resp.on_hover_text(t.hint()).clicked() {
                viewer.select_tool(t);
            }
        }
        Err(reason) => {
            resp.on_hover_text(format!("{}: {reason}", t.label()));
        }
    }
}

fn zoom_buttons(ui: &mut egui::Ui, viewer: &mut Viewer) {
    let axis = viewer.slices.axis;
    let view = &mut viewer.slices.views[axis.normal_axis()];
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
