//! Reusable widgets of the design system.

use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Layout, Pos2, Rect, Response, RichText, Sense, Stroke,
    Ui, Vec2, WidgetInfo, WidgetType,
};

use super::theme::{ACCENT, BORDER, CONTROL, CONTROL_HOVER, OVERLAY, TEXT, TEXT_DIM};

/// Side of square toolbar buttons.
pub const TOOL: f32 = 34.0;

/// Square icon button of the toolbars. `label` is the tooltip and the
/// accessible name.
pub fn tool_button(ui: &mut Ui, icon: &str, label: &str, active: bool) -> Response {
    tool_button_enabled(ui, icon, label, active, true)
}

/// [`tool_button`] that can be disabled.
pub fn tool_button_enabled(ui: &mut Ui, icon: &str, label: &str, active: bool, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(TOOL), sense);
    let label_owned = label.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &label_owned));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hovered = resp.hovered() && enabled;
        if active {
            p.rect_filled(rect, 8.0, ACCENT);
        } else if hovered {
            p.rect_filled(rect, 8.0, CONTROL_HOVER);
        }
        let fg = if !enabled {
            TEXT_DIM.linear_multiply(0.45)
        } else if active {
            Color32::WHITE
        } else if hovered {
            TEXT
        } else {
            TEXT.linear_multiply(0.85)
        };
        p.text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(19.0), fg);
    }
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label)
    } else {
        resp.on_hover_text(label)
    }
}

/// Thin vertical separator between toolbar groups.
pub fn toolbar_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, TOOL), Sense::hover());
    let c = rect.center();
    ui.painter().line_segment([c - Vec2::new(0.0, 10.0), c + Vec2::new(0.0, 10.0)], Stroke::new(1.0, BORDER));
}

/// Segmented control. Each item is a button whose accessible name is its
/// text. Returns the index of the clicked item.
pub fn segmented(ui: &mut Ui, items: &[&str], selected: Option<usize>) -> Option<usize> {
    let mut clicked = None;
    egui::Frame::new().fill(CONTROL).corner_radius(CornerRadius::same(9)).inner_margin(egui::Margin::same(3)).show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, text) in items.iter().enumerate() {
                    let on = selected == Some(i);
                    let galley =
                        ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.0), Color32::WHITE);
                    let size = Vec2::new(galley.size().x + 22.0, 26.0);
                    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                    let label = text.to_string();
                    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, &label));
                    let fill = if on {
                        ACCENT
                    } else if resp.hovered() {
                        CONTROL_HOVER
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(rect, 7.0, fill);
                    let color = if on { Color32::WHITE } else { TEXT_DIM };
                    ui.painter().galley(rect.center() - galley.size() * 0.5, galley, color);
                    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        },
    );
    clicked
}

/// Slider row with a leading icon.
pub fn icon_slider(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> Response {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(16.0).color(TEXT_DIM)).on_hover_text(label);
        let r = ui.add(egui::Slider::new(value, range).show_value(true).max_decimals(2));
        r.widget_info(|| WidgetInfo::slider(true, f64::from(*value), label));
        r
    })
    .inner
}

/// Labelled slider in the style of a form row: the label on the left,
/// the editable value on the right and a full-width slider beneath.
/// Returns `true` when the value changed.
pub fn slider_row(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT_DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let speed = (range.end() - range.start()) / 400.0;
            changed |= ui.add(egui::DragValue::new(value).speed(speed).range(range.clone()).max_decimals(0)).changed();
        });
    });
    let r = ui.scope(|ui| {
        ui.spacing_mut().slider_width = ui.available_width();
        ui.add(egui::Slider::new(value, range).show_value(false))
    });
    let r = r.inner;
    r.widget_info(|| WidgetInfo::slider(true, f64::from(*value), label));
    changed |= r.changed();
    changed
}

/// Section heading with an icon.
pub fn section_title(ui: &mut Ui, icon: &str, title: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(15.0).color(ACCENT));
        ui.label(RichText::new(title).size(13.5).strong().color(TEXT));
    });
}

/// Rounded selectable chip.
pub fn chip(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let rich = RichText::new(text).size(12.5).color(if selected { Color32::WHITE } else { TEXT });
    let fill = if selected { ACCENT } else { CONTROL };
    let r = ui
        .add(egui::Button::new(rich).fill(fill).corner_radius(8.0).stroke(Stroke::NONE).min_size(Vec2::new(0.0, 26.0)));
    let label = text.to_string();
    r.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, &label));
    r.on_hover_cursor(CursorIcon::PointingHand)
}

/// Thin frame around a view; highlighted when the view is active.
pub fn view_frame(p: &egui::Painter, rect: Rect, active: bool) {
    let color = if active { ACCENT.linear_multiply(0.8) } else { BORDER };
    p.rect_stroke(rect.shrink(0.5), 6.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
}

/// Text drawn over an image, with a soft shadow for legibility.
pub fn overlay_text(p: &egui::Painter, pos: Pos2, align: Align2, text: &str, color: Color32, size: f32) -> Rect {
    let font = FontId::proportional(size);
    p.text(pos + Vec2::new(1.0, 1.0), align, text, font.clone(), Color32::from_black_alpha(200));
    p.text(pos, align, text, font, color)
}

/// Multi-line overlay block anchored at a view corner.
pub fn overlay_lines(p: &egui::Painter, corner: Pos2, align: Align2, lines: &[(String, Color32)], size: f32) {
    let step = size + 5.0;
    let n = lines.len() as f32;
    for (i, (text, color)) in lines.iter().enumerate() {
        let dy = if align.y() == Align::Max { -(n - 1.0 - i as f32) * step } else { i as f32 * step };
        overlay_text(p, corner + Vec2::new(0.0, dy), align, text, *color, size);
    }
}

/// The product mark: the "Fe" tile of the periodic table — iron, the
/// metal behind Rust — with its atomic number.
pub fn logo(p: &egui::Painter, rect: Rect) {
    let s = rect.height();
    p.rect_filled(rect, s * 0.22, ACCENT);
    p.rect_stroke(rect, s * 0.22, Stroke::new(1.0, Color32::from_white_alpha(40)), egui::StrokeKind::Inside);
    p.text(
        rect.left_top() + Vec2::new(s * 0.14, s * 0.1),
        Align2::LEFT_TOP,
        "26",
        FontId::proportional(s * 0.2),
        Color32::from_white_alpha(190),
    );
    p.text(
        rect.center() + Vec2::new(0.0, s * 0.06),
        Align2::CENTER_CENTER,
        "Fe",
        FontId::proportional(s * 0.48),
        OVERLAY,
    );
}
