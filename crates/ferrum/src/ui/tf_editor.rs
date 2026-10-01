//! Transfer function editor: histogram background, draggable control
//! points (x = intensity, y = opacity) and per-point colour editing.

use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use ferrum_domain::{Rgb, TransferFunction};

/// Interaction state kept between frames.
#[derive(Debug, Default, Clone)]
pub struct TfEditorState {
    selected: Option<usize>,
    dragging: Option<usize>,
}

const POINT_RADIUS: f32 = 5.0;
const HIT_RADIUS: f32 = 9.0;

fn to_color(c: Rgb, a: f32) -> Color32 {
    let u = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(u(c.r), u(c.g), u(c.b), u(a))
}

/// Shows the editor. Returns the edited function if the user changed it.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut TfEditorState,
    tf: &TransferFunction,
    histogram: &[f32],
) -> Option<TransferFunction> {
    let width = ui.available_width().max(120.0);
    let (response, painter) = ui.allocate_painter(Vec2::new(width, 150.0), Sense::click_and_drag());
    let rect = response.rect;
    let plot = Rect::from_min_max(rect.min + Vec2::new(4.0, 4.0), rect.max - Vec2::new(4.0, 18.0));
    let strip = Rect::from_min_max(Pos2::new(plot.min.x, plot.max.y + 4.0), Pos2::new(plot.max.x, rect.max.y - 2.0));
    let to_screen = |x: f32, y: f32| Pos2::new(plot.min.x + x * plot.width(), plot.max.y - y * plot.height());
    let from_screen = |p: Pos2| ((p.x - plot.min.x) / plot.width(), (plot.max.y - p.y) / plot.height());

    painter.rect_filled(rect, 4.0, Color32::from_gray(18));
    // histogram
    if !histogram.is_empty() {
        let bw = plot.width() / histogram.len() as f32;
        for (i, h) in histogram.iter().enumerate() {
            let x0 = plot.min.x + i as f32 * bw;
            let top = plot.max.y - h.clamp(0.0, 1.0) * plot.height();
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x0, top), Pos2::new(x0 + bw.max(1.0), plot.max.y)),
                0.0,
                Color32::from_gray(55),
            );
        }
    }
    // colour strip
    let n = 128;
    for i in 0..n {
        let x = i as f32 / (n - 1) as f32;
        let (c, a) = tf.sample(x);
        let x0 = strip.min.x + strip.width() * i as f32 / n as f32;
        let x1 = strip.min.x + strip.width() * (i + 1) as f32 / n as f32;
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(x0, strip.min.y), Pos2::new(x1, strip.max.y)),
            0.0,
            to_color(c, a.max(0.15)),
        );
    }
    painter.rect_stroke(plot, 0.0, Stroke::new(1.0, Color32::from_gray(80)), StrokeKind::Inside);

    // curve
    let pts: Vec<Pos2> = tf.points().iter().map(|p| to_screen(p.position, p.opacity)).collect();
    painter.add(egui::Shape::line(pts.clone(), Stroke::new(1.5, Color32::from_gray(220))));
    for (i, (p, cp)) in pts.iter().zip(tf.points()).enumerate() {
        let selected = state.selected == Some(i);
        painter.circle_filled(*p, POINT_RADIUS, to_color(cp.color, 1.0));
        painter.circle_stroke(
            *p,
            POINT_RADIUS,
            Stroke::new(if selected { 2.5 } else { 1.0 }, if selected { Color32::YELLOW } else { Color32::WHITE }),
        );
    }

    let hit = |pos: Pos2| -> Option<usize> {
        pts.iter()
            .enumerate()
            .map(|(i, p)| (i, p.distance(pos)))
            .filter(|(_, d)| *d <= HIT_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };

    let mut edited: Option<TransferFunction> = None;
    let pointer = response.interact_pointer_pos().or(response.hover_pos());
    if response.drag_started() {
        state.dragging = pointer.and_then(hit);
        if state.dragging.is_some() {
            state.selected = state.dragging;
        }
    }
    if response.dragged() {
        if let (Some(i), Some(pos)) = (state.dragging, pointer) {
            let (x, y) = from_screen(pos);
            let mut t = tf.clone();
            if t.move_point(i, x, y).is_ok() {
                edited = Some(t);
            }
        }
    }
    if response.drag_stopped() {
        state.dragging = None;
    }
    if response.clicked() {
        state.selected = pointer.and_then(hit);
    }
    if response.double_clicked() {
        if let Some(pos) = pointer.filter(|p| hit(*p).is_none()) {
            let (x, y) = from_screen(pos);
            let mut t = tf.clone();
            let i = t.insert_point(x);
            let _ = t.move_point(i, x, y);
            state.selected = Some(i);
            edited = Some(t);
        }
    }
    if response.secondary_clicked() {
        if let Some(i) = pointer.and_then(hit) {
            let mut t = tf.clone();
            if t.remove_point(i).is_ok() {
                state.selected = None;
                edited = Some(t);
            }
        }
    }
    response.on_hover_text("Drag points to shape opacity · double-click to add · right-click to remove");

    // selected point details
    let current = edited.clone().unwrap_or_else(|| tf.clone());
    if let Some(i) = state.selected.filter(|&i| i < current.points().len()) {
        let p = current.points()[i];
        ui.horizontal(|ui| {
            ui.label(format!("Point {i}: x {:.2}, α {:.2}", p.position, p.opacity));
            let mut rgb = [p.color.r, p.color.g, p.color.b];
            if egui::color_picker::color_edit_button_rgb(ui, &mut rgb).changed() {
                let mut t = current.clone();
                if t.set_color(i, Rgb::new(rgb[0], rgb[1], rgb[2])).is_ok() {
                    edited = Some(t);
                }
            }
        });
    }
    edited
}
