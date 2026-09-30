//! Reusable widgets of the design system.

use egui::{
    Align2, Color32, CursorIcon, FontId, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2, WidgetInfo, WidgetType,
};

use super::theme::{ACCENT, CONTROL, CONTROL_HOVER, HUD, TEXT, TEXT_DIM};

/// Diameter of round dock buttons.
pub const ROUND: f32 = 40.0;

/// Round icon button. `label` is the tooltip and the accessible name.
pub fn round_button(ui: &mut Ui, icon: &str, label: &str, active: bool) -> Response {
    round_button_sized(ui, icon, label, active, ROUND, true)
}

/// Round icon button with explicit diameter; `enabled = false` greys it out.
pub fn round_button_sized(ui: &mut Ui, icon: &str, label: &str, active: bool, size: f32, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let label_owned = label.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &label_owned));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let c = rect.center();
        let r = size * 0.5;
        let hovered = resp.hovered() && enabled;
        if active {
            // soft glow
            for (i, a) in [(6.0, 18u8), (3.0, 40)] {
                p.circle_filled(c, r + i, Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), a));
            }
            p.circle_filled(c, r, ACCENT);
        } else {
            p.circle_filled(c, r, if hovered { CONTROL_HOVER } else { CONTROL });
            if hovered {
                p.circle_stroke(c, r - 0.5, Stroke::new(1.0, ACCENT.linear_multiply(0.7)));
            }
        }
        let fg = if !enabled {
            TEXT_DIM.linear_multiply(0.5)
        } else if active {
            Color32::WHITE
        } else {
            TEXT
        };
        p.text(c, Align2::CENTER_CENTER, icon, FontId::proportional(size * 0.5), fg);
    }
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label)
    } else {
        resp.on_hover_text(label)
    }
}

/// Round button showing a short text instead of an icon (e.g. "2D").
pub fn round_text_button(ui: &mut Ui, text: &str, label: &str, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(ROUND), Sense::click());
    let label_owned = label.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label_owned));
    let p = ui.painter();
    let c = rect.center();
    let hovered = resp.hovered();
    if active {
        p.circle_filled(c, ROUND * 0.5 + 4.0, Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 30));
        p.circle_stroke(c, ROUND * 0.5 - 1.0, Stroke::new(1.5, ACCENT));
        p.circle_filled(c, ROUND * 0.5 - 2.5, CONTROL);
    } else {
        p.circle_filled(c, ROUND * 0.5, if hovered { CONTROL_HOVER } else { CONTROL });
    }
    p.text(c, Align2::CENTER_CENTER, text, FontId::proportional(14.0), if active { ACCENT } else { TEXT });
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label)
}

/// Thin vertical separator for docks.
pub fn dock_separator(ui: &mut Ui, vertical_dock: bool) {
    let size = if vertical_dock { Vec2::new(ROUND, 9.0) } else { Vec2::new(9.0, ROUND) };
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let c = rect.center();
    let d = if vertical_dock { Vec2::new(ROUND * 0.3, 0.0) } else { Vec2::new(0.0, ROUND * 0.3) };
    ui.painter().line_segment([c - d, c + d], Stroke::new(1.0, TEXT_DIM.linear_multiply(0.4)));
}

/// Slider row with a leading icon (the original viewer's style).
pub fn icon_slider(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> Response {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(icon).size(17.0).color(TEXT_DIM)).on_hover_text(label);
        let r = ui.add(egui::Slider::new(value, range).show_value(true).max_decimals(2));
        r.widget_info(|| WidgetInfo::slider(true, f64::from(*value), label));
        r
    })
    .inner
}

/// Section title with an icon.
pub fn section_title(ui: &mut Ui, icon: &str, title: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(icon).size(16.0).color(ACCENT));
        ui.label(egui::RichText::new(title.to_uppercase()).size(11.5).strong().color(TEXT_DIM));
    });
}

/// Pill-shaped selectable chip.
pub fn chip(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let rich = egui::RichText::new(text).size(12.5).color(if selected { Color32::WHITE } else { TEXT });
    let fill = if selected { ACCENT } else { CONTROL };
    let r = ui.add(
        egui::Button::new(rich).fill(fill).corner_radius(12.0).stroke(Stroke::NONE).min_size(Vec2::new(0.0, 24.0)),
    );
    let label = text.to_string();
    r.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, &label));
    r.on_hover_cursor(CursorIcon::PointingHand)
}

/// Futuristic corner brackets around `rect`.
pub fn hud_corners(p: &egui::Painter, rect: Rect, color: Color32) {
    let len = 16.0f32.min(rect.width() * 0.2).min(rect.height() * 0.2);
    let s = Stroke::new(1.5, color);
    let r = rect.shrink(6.0);
    let corners = [
        (r.left_top(), Vec2::new(len, 0.0), Vec2::new(0.0, len)),
        (r.right_top(), Vec2::new(-len, 0.0), Vec2::new(0.0, len)),
        (r.left_bottom(), Vec2::new(len, 0.0), Vec2::new(0.0, -len)),
        (r.right_bottom(), Vec2::new(-len, 0.0), Vec2::new(0.0, -len)),
    ];
    for (c, a, b) in corners {
        p.line_segment([c, c + a], s);
        p.line_segment([c, c + b], s);
    }
}

/// Label with a dark rounded backdrop, used for on-image read-outs.
pub fn hud_label(p: &egui::Painter, pos: Pos2, align: Align2, text: &str, color: Color32, size: f32) {
    let galley = p.layout_no_wrap(text.to_string(), super::theme::hud_font(size), color);
    let rect = align.anchor_size(pos, galley.size() + Vec2::new(10.0, 6.0));
    p.rect_filled(rect, 6.0, Color32::from_black_alpha(150));
    p.galley(rect.min + Vec2::new(5.0, 3.0), galley, color);
}

/// The product mark: three converging beams (a nod to the original logo)
/// drawn as vectors so it scales crisply.
pub fn logo(p: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    let s = Stroke::new((size / 22.0).max(1.2), color);
    let u = size / 2.0;
    for i in 0..3 {
        let o = (i as f32 - 1.0) * u * 0.14;
        // vertical beam down to the junction
        p.line_segment([center + Vec2::new(-u * 0.25 + o, -u), center + Vec2::new(-u * 0.25 + o, o)], s);
        // horizontal beam to the right
        p.line_segment([center + Vec2::new(-u * 0.25 + o, o), center + Vec2::new(u, o)], s);
        // diagonal beam to the lower left
        p.line_segment([center + Vec2::new(-u * 0.25 + o, o), center + Vec2::new(-u + o, u * 0.75 + o)], s);
    }
    p.circle_filled(center + Vec2::new(-u * 0.25, 0.0), size * 0.06, HUD);
}
