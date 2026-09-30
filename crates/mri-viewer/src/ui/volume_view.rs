//! 3D view: GPU ray casting with orbit/pan/zoom navigation and the volume
//! eraser.

use egui::{Align2, Color32, FontId, Pos2, Sense, Stroke};
use glam::{UVec2, Vec2};
use mri_app::Viewer;

use crate::gpu_bridge::{view_ids, VolumeCallback};

/// Per-view interaction state.
#[derive(Debug, Default, Clone)]
pub struct VolumeViewState {
    last_erase: Option<Pos2>,
    was_interacting: bool,
}

/// Radians of rotation per point of mouse movement.
const ROTATE_SPEED: f32 = 0.008;

fn ndc(rect: egui::Rect, p: Pos2) -> Vec2 {
    Vec2::new((p.x - rect.min.x) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.min.y) / rect.height() * 2.0)
}

/// Draws the 3D view into the remaining space of `ui`.
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut VolumeViewState) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    if viewer.dataset().is_none() {
        painter.text(rect.center(), Align2::CENTER_CENTER, "No data", FontId::proportional(14.0), Color32::GRAY);
        return;
    }
    let aspect = rect.width() / rect.height().max(1.0);

    // ---------------------------------------------------------------- input
    let pointer = response.interact_pointer_pos().or(response.hover_pos());
    let erasing = viewer.volume.eraser_enabled;
    if erasing && response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) {
        if let Some(p) = pointer {
            let moved = state.last_erase.is_none_or(|l| l.distance(p) > 4.0);
            if moved && viewer.erase_at(ndc(rect, p), aspect) {
                state.last_erase = Some(p);
            }
        }
    } else {
        state.last_erase = None;
    }
    if response.dragged_by(egui::PointerButton::Primary) && !erasing {
        let d = response.drag_delta();
        viewer.volume.camera.rotate(d.x * ROTATE_SPEED, d.y * ROTATE_SPEED);
    }
    if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) {
        let d = response.drag_delta();
        viewer.volume.camera.pan(d.x / rect.height(), d.y / rect.height());
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            viewer.volume.camera.zoom(scroll * 0.002);
        }
    }
    if response.double_clicked() && !erasing {
        viewer.volume.camera.reset();
    }
    let interacting = response.dragged() && !erasing;
    viewer.volume.interacting = interacting;
    if state.was_interacting && !interacting {
        ui.ctx().request_repaint(); // re-render at full resolution
    }
    state.was_interacting = interacting;

    // --------------------------------------------------------------- render
    let ppp = ui.ctx().pixels_per_point();
    let scale = viewer.volume.render_scale();
    let size = UVec2::new(
        ((rect.width() * ppp * scale).round() as u32).max(1),
        ((rect.height() * ppp * scale).round() as u32).max(1),
    );
    if let Some(params) = viewer.frame_params(Vec2::new(rect.width(), rect.height())) {
        let cb = VolumeCallback { id: view_ids::VOLUME, params, size };
        painter.add(egui_wgpu::Callback::new_paint_callback(rect, cb));
    }

    // -------------------------------------------------------------- overlay
    let s = &viewer.volume.settings;
    let mut info = format!("{} · quality {:.0}%", s.mode.label(), s.quality * 100.0);
    if viewer.is_computing() {
        info.push_str(" · computing…");
    }
    painter.text(
        rect.min + egui::vec2(8.0, 6.0),
        Align2::LEFT_TOP,
        info,
        FontId::monospace(12.0),
        Color32::from_gray(210),
    );
    if erasing {
        if let Some(p) = response.hover_pos() {
            // approximate on-screen brush radius
            let d = viewer.dataset().map(|d| d.volume.physical_size().max_element()).unwrap_or(1.0);
            let h = 2.0 * viewer.volume.camera.distance * (viewer.volume.camera.fov_y * 0.5).tan();
            let r = viewer.volume.brush.radius_mm / d / h * rect.height();
            painter.circle_stroke(p, r.max(2.0), Stroke::new(1.5, Color32::from_rgb(255, 90, 90)));
        }
    }
}
