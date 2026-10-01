//! 2D slice view: GPU-rendered image, tool input and annotation overlay.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};
use ferrum_app::{InputKind, ViewMode, Viewer};
use ferrum_domain::{Annotation, SliceAxis, SliceKey};

use super::{theme, widgets};
use crate::gpu_bridge::{view_ids, SliceCallback};

/// Per-view interaction state.
#[derive(Debug, Default, Clone)]
pub struct SliceViewState {
    pressed: bool,
    last_pointer: Option<Pos2>,
    scroll_acc: f32,
}

const ANNOTATION_COLOR: Color32 = Color32::from_rgb(255, 214, 10);
const PREVIEW_COLOR: Color32 = Color32::from_rgb(120, 200, 255);

fn g2e(v: glam::Vec2) -> Pos2 {
    Pos2::new(v.x, v.y)
}

fn e2g(p: Pos2) -> glam::Vec2 {
    glam::Vec2::new(p.x, p.y)
}

/// Draws the slice of `axis` into the remaining space of `ui`.
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, axis: SliceAxis, state: &mut SliceViewState) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let scrub_rect =
        Rect::from_min_max(Pos2::new(rect.max.x - 30.0, rect.min.y + 40.0), Pos2::new(rect.max.x, rect.max.y - 40.0));
    let scrub = ui.interact(scrub_rect, ui.id().with(("scrub", axis.id())), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::CANVAS);
    let Some(dataset) = viewer.dataset() else {
        painter.text(rect.center(), Align2::CENTER_CENTER, "No data", FontId::proportional(14.0), Color32::GRAY);
        return;
    };
    let volume = dataset.volume.clone();
    let ppp = ui.ctx().pixels_per_point();
    let rect_px = (e2g(rect.min) * ppp, e2g(rect.max) * ppp);
    if let Some(params) = viewer.slice_params(axis, rect_px, ppp) {
        let cb = SliceCallback { id: view_ids::slice(axis.id()), params };
        painter.add(egui_wgpu::Callback::new_paint_callback(rect, cb));
    }

    // ---------------------------------------------------------------- input
    let viewport = e2g(rect.size().to_pos2());
    let local = |p: Pos2| e2g(p) - e2g(rect.min);
    let (pressed, released, down, pointer, scroll, ctrl, keys) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_released(),
            i.pointer.primary_down(),
            i.pointer.hover_pos(),
            i.smooth_scroll_delta.y,
            i.modifiers.command,
            (
                i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::PageDown),
            ),
        )
    });
    if response.hovered() && !scrub.hovered() && !scrub.dragged() {
        if let Some(p) = pointer {
            if pressed {
                state.pressed = true;
                viewer.slice_input(axis, InputKind::Press, local(p), viewport);
            } else if !down {
                viewer.slice_input(axis, InputKind::Hover, local(p), viewport);
            }
            if scroll != 0.0 {
                if ctrl {
                    viewer.slice_input(axis, InputKind::Scroll { amount: scroll / 50.0 }, local(p), viewport);
                } else {
                    state.scroll_acc += scroll;
                    while state.scroll_acc.abs() >= 20.0 {
                        let step = state.scroll_acc.signum();
                        viewer.step_slice(axis, step as i32);
                        state.scroll_acc -= step * 20.0;
                    }
                }
            }
        }
        if keys.0 {
            viewer.step_slice(axis, 1);
        }
        if keys.1 {
            viewer.step_slice(axis, -1);
        }
    }
    if state.pressed {
        if let Some(p) = pointer {
            if let Some(last) = state.last_pointer {
                let delta = e2g(p) - e2g(last);
                if down && delta != glam::Vec2::ZERO {
                    viewer.slice_input(axis, InputKind::Drag { delta }, local(p), viewport);
                }
            }
        }
        if released {
            state.pressed = false;
            if let Some(p) = pointer {
                viewer.slice_input(axis, InputKind::Release, local(p), viewport);
            }
        }
    }
    state.last_pointer = pointer;
    if response.double_clicked() {
        if let Some(p) = pointer {
            viewer.slice_input(axis, InputKind::DoubleClick, local(p), viewport);
        }
    }
    if response.secondary_clicked() && viewer.view_mode == ViewMode::Mpr {
        if let Some(p) = pointer {
            viewer.navigate_to(axis, local(p), viewport);
        }
    }

    // -------------------------------------------------------------- overlay
    let index = viewer.slices.index(axis);
    let image_mm = axis.plane_size_mm(&volume);
    let view = viewer.slices.views[axis.normal_axis()];
    let to_screen = |mm: glam::Vec2| g2e(e2g(rect.min) + view.mm_to_screen(mm, viewport, image_mm));
    let key = SliceKey::new(axis, index);
    for (_, a) in viewer.annotations().on_slice(key) {
        draw_annotation(&painter, a, &to_screen, ANNOTATION_COLOR);
    }
    if let Some(a) = viewer.annotation_preview() {
        draw_annotation(&painter, &a, &to_screen, PREVIEW_COLOR);
    }
    let (lo, hi) = view.image_rect(viewport, image_mm);
    let img = Rect::from_min_max(g2e(e2g(rect.min) + lo), g2e(e2g(rect.min) + hi));
    if viewer.view_mode == ViewMode::Mpr {
        for other in SliceAxis::ALL.into_iter().filter(|a| *a != axis) {
            let mut t = glam::Vec3::splat(0.5);
            t[other.normal_axis()] = other.slice_position(&volume, viewer.slices.index(other));
            let uv = axis.uv_of_tex(t);
            let stroke = Stroke::new(1.0, axis_color(other).gamma_multiply(0.85));
            if axis.screen_axis(0) == other.normal_axis() {
                let x = img.min.x + uv.x * img.width();
                painter.line_segment([Pos2::new(x, img.min.y), Pos2::new(x, img.max.y)], stroke);
            } else {
                let y = img.min.y + uv.y * img.height();
                painter.line_segment([Pos2::new(img.min.x, y), Pos2::new(img.max.x, y)], stroke);
            }
        }
    }
    // HUD frame in the orientation's colour
    widgets::hud_corners(&painter, rect, axis_color(axis).gamma_multiply(0.9));

    // anatomical orientation markers at the image edges
    let [l, r, t, b] = axis.edge_labels();
    let inner = rect.shrink(18.0);
    let mid = img.center();
    let clampx = |x: f32| x.clamp(inner.min.x, inner.max.x);
    let clampy = |y: f32| y.clamp(inner.min.y, inner.max.y);
    let font = theme::hud_font(15.0);
    let lc = theme::HUD;
    painter.text(Pos2::new(clampx(img.min.x + 10.0), clampy(mid.y)), Align2::LEFT_CENTER, l, font.clone(), lc);
    painter.text(Pos2::new(clampx(img.max.x - 10.0), clampy(mid.y)), Align2::RIGHT_CENTER, r, font.clone(), lc);
    painter.text(Pos2::new(clampx(mid.x), clampy(img.min.y + 10.0)), Align2::CENTER_TOP, t, font.clone(), lc);
    painter.text(Pos2::new(clampx(mid.x), clampy(img.max.y - 10.0)), Align2::CENTER_BOTTOM, b, font, lc);

    // read-outs
    let n = axis.slice_count(&volume);
    let w = viewer.slices.window;
    widgets::hud_label(
        &painter,
        rect.left_top() + egui::vec2(14.0, 14.0),
        Align2::LEFT_TOP,
        &format!("{}  {:>3}/{}", axis.label().to_uppercase(), index + 1, n),
        axis_color(axis),
        12.0,
    );
    widgets::hud_label(
        &painter,
        rect.left_bottom() + egui::vec2(14.0, -14.0),
        Align2::LEFT_BOTTOM,
        &format!("W {:.0}  L {:.0}  ·  {:.0}%", w.width, w.center, view.zoom * 100.0),
        theme::TEXT_DIM,
        11.5,
    );

    // slice scrubber along the right edge
    if n > 1 {
        let track = Rect::from_min_max(
            Pos2::new(rect.max.x - 20.0, rect.min.y + 48.0),
            Pos2::new(rect.max.x - 12.0, rect.max.y - 48.0),
        );
        if track.height() > 40.0 {
            let frac = index as f32 / (n - 1) as f32;
            painter.rect_filled(track, 4.0, Color32::from_white_alpha(14));
            let y = track.max.y - frac * track.height();
            let filled = Rect::from_min_max(Pos2::new(track.min.x, y), track.max);
            painter.rect_filled(filled, 4.0, axis_color(axis).gamma_multiply(0.35));
            painter.circle_filled(Pos2::new(track.center().x, y), 6.0, axis_color(axis));
            if let Some(p) = scrub.interact_pointer_pos() {
                let f = ((track.max.y - p.y) / track.height()).clamp(0.0, 1.0);
                viewer.set_slice_index(axis, (f * (n - 1) as f32).round() as u32);
            }
        }
    }
}

/// Colour coding of orientations (sagittal red, coronal green, axial blue).
pub fn axis_color(axis: SliceAxis) -> Color32 {
    match axis {
        SliceAxis::Sagittal => Color32::from_rgb(230, 80, 80),
        SliceAxis::Coronal => Color32::from_rgb(90, 200, 90),
        SliceAxis::Axial => Color32::from_rgb(90, 150, 255),
    }
}

fn draw_annotation(painter: &egui::Painter, a: &Annotation, to_screen: &dyn Fn(glam::Vec2) -> Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let pts: Vec<Pos2> = a.points().into_iter().map(to_screen).collect();
    match a {
        Annotation::Distance { .. } | Annotation::Angle { .. } => {
            painter.add(egui::Shape::line(pts.clone(), stroke));
        }
        Annotation::Polygon { .. } => {
            painter.add(egui::Shape::closed_line(pts.clone(), stroke));
        }
        Annotation::Rect { .. } => {
            if let [p, q] = pts[..] {
                painter.rect_stroke(Rect::from_two_pos(p, q), 0.0, stroke, egui::StrokeKind::Middle);
            }
        }
        Annotation::Text { .. } => {}
    }
    if !matches!(a, Annotation::Text { .. }) {
        for p in &pts {
            painter.circle_filled(*p, 2.5, color);
        }
    }
    let label = a.label();
    if !label.is_empty() {
        let pos = to_screen(a.label_anchor()) + egui::vec2(6.0, -6.0);
        let galley = painter.layout_no_wrap(label, FontId::proportional(13.0), color);
        let bg = Rect::from_min_size(pos - egui::vec2(2.0, galley.size().y), galley.size() + egui::vec2(4.0, 2.0));
        painter.rect_filled(bg, 2.0, Color32::from_black_alpha(170));
        painter.galley(bg.min + egui::vec2(2.0, 1.0), galley, color);
    }
}
