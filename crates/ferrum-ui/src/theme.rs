//! Visual identity: calm navy surfaces, a single blue accent and quiet
//! light-grey read-outs on a black image canvas.

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Stroke, TextStyle, Visuals};

/// Primary accent: active tools, selected items, slider fills.
pub const ACCENT: Color32 = Color32::from_rgb(47, 111, 235);
/// Muted accent for selected list rows.
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(27, 49, 92);
/// Application background (bars, side panels).
pub const BG: Color32 = Color32::from_rgb(12, 19, 32);
/// Card and panel surface.
pub const SURFACE: Color32 = Color32::from_rgb(17, 26, 42);
/// Raised control fill.
pub const CONTROL: Color32 = Color32::from_rgb(26, 38, 60);
/// Hovered control fill.
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(36, 51, 79);
/// Borders and separators.
pub const BORDER: Color32 = Color32::from_rgb(34, 48, 72);
/// Image canvas behind the views.
pub const CANVAS: Color32 = Color32::from_rgb(2, 4, 8);
/// Primary text.
pub const TEXT: Color32 = Color32::from_rgb(226, 232, 242);
/// Secondary text.
pub const TEXT_DIM: Color32 = Color32::from_rgb(134, 149, 173);
/// Text drawn over images (orientation labels, read-outs).
pub const OVERLAY: Color32 = Color32::from_rgb(222, 228, 236);
/// Pending review (proposals of agents and engines), research-only badges.
pub const WARN: Color32 = Color32::from_rgb(240, 190, 60);
/// Errors and destructive actions.
pub const DANGER: Color32 = Color32::from_rgb(232, 104, 96);

/// Installs fonts (with Phosphor icons as a fallback glyph source) and
/// the dark visuals.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Light);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let mut v = Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = Color32::from_rgb(9, 14, 24);
        v.faint_bg_color = Color32::from_rgb(20, 30, 48);
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) };
        v.popup_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(120) };
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.hyperlink_color = Color32::from_rgb(110, 160, 250);
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        let r = CornerRadius::same(8);
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
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.inactive.bg_fill = CONTROL;
        v.widgets.inactive.weak_bg_fill = CONTROL;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.bg_fill = CONTROL_HOVER;
        v.widgets.hovered.weak_bg_fill = CONTROL_HOVER;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.linear_multiply(0.5));
        v.widgets.active.bg_fill = ACCENT;
        v.widgets.active.weak_bg_fill = ACCENT;
        v.widgets.open.weak_bg_fill = CONTROL_HOVER;
        style.visuals = v;
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.slider_width = 150.0;
        style.spacing.interact_size.y = 22.0;
        style.spacing.slider_rail_height = 4.0;
        style.text_styles.insert(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(13.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(11.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional));
    });
}

/// Frame of the top bars and the status bar.
pub fn bar() -> Frame {
    Frame::new().fill(BG).inner_margin(Margin::symmetric(12, 0))
}

/// Frame of the side panels.
pub fn side() -> Frame {
    Frame::new().fill(BG).inner_margin(Margin::symmetric(14, 12))
}

/// Card inside a side panel or on the start screen.
pub fn card() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(14))
}

/// Floating overlays (loading progress).
pub fn overlay() -> Frame {
    card().shadow(Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(150) })
}

/// Font for numeric read-outs.
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}
