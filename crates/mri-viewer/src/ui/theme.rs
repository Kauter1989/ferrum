//! Visual identity: a black full-bleed canvas with floating "glass" docks,
//! round icon buttons with a coral accent and cyan HUD read-outs.

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Stroke, TextStyle, Visuals};

/// Primary accent (active tools, slider fills).
pub const ACCENT: Color32 = Color32::from_rgb(255, 107, 87);
/// Secondary accent for data read-outs and HUD elements.
pub const HUD: Color32 = Color32::from_rgb(92, 225, 255);
/// Canvas background.
pub const CANVAS: Color32 = Color32::from_rgb(3, 4, 6);
/// Floating panel fill.
pub const GLASS: Color32 = Color32::from_rgba_premultiplied(18, 20, 25, 225);
/// Raised control fill.
pub const CONTROL: Color32 = Color32::from_rgb(38, 41, 48);
/// Hovered control fill.
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(56, 60, 70);
/// Hairline strokes.
pub const HAIRLINE: Color32 = Color32::from_rgba_premultiplied(40, 44, 52, 90);
/// Primary text.
pub const TEXT: Color32 = Color32::from_rgb(232, 234, 237);
/// Secondary text.
pub const TEXT_DIM: Color32 = Color32::from_rgb(138, 143, 152);

/// Installs fonts (with Phosphor icons as a fallback glyph source) and
/// the dark visuals.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Light);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let mut v = Visuals::dark();
        v.panel_fill = CANVAS;
        v.window_fill = GLASS;
        v.extreme_bg_color = Color32::from_rgb(10, 11, 14);
        v.faint_bg_color = Color32::from_rgb(24, 26, 31);
        v.window_stroke = Stroke::new(1.0, HAIRLINE);
        v.window_corner_radius = CornerRadius::same(18);
        v.menu_corner_radius = CornerRadius::same(12);
        v.window_shadow = Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(160) };
        v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(140) };
        v.selection.bg_fill = ACCENT.linear_multiply(0.85);
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.hyperlink_color = HUD;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };
        let r = CornerRadius::same(10);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
        }
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
        v.widgets.inactive.bg_fill = CONTROL;
        v.widgets.inactive.weak_bg_fill = CONTROL;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.bg_fill = CONTROL_HOVER;
        v.widgets.hovered.weak_bg_fill = CONTROL_HOVER;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.linear_multiply(0.6));
        v.widgets.active.bg_fill = ACCENT;
        v.widgets.active.weak_bg_fill = ACCENT;
        style.visuals = v;
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.slider_width = 170.0;
        style.spacing.interact_size.y = 24.0;
        style.text_styles.insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(11.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional));
    });
}

/// Frame of the floating docks and panels.
pub fn glass() -> Frame {
    Frame::new()
        .fill(GLASS)
        .stroke(Stroke::new(1.0, HAIRLINE))
        .corner_radius(CornerRadius::same(22))
        .inner_margin(Margin::same(8))
        .shadow(Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(150) })
}

/// Frame of the settings panel (more padding).
pub fn panel() -> Frame {
    glass().corner_radius(CornerRadius::same(20)).inner_margin(Margin::symmetric(16, 14))
}

/// Monospace font used by HUD read-outs.
pub fn hud_font(size: f32) -> FontId {
    FontId::monospace(size)
}
