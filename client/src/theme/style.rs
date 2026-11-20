//! The egui style of a theme, built from its palette.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, vec2};

use super::*;

pub(super) fn style(p: &Palette, dark: bool) -> egui::Style {
    let text_styles = [
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, medium())),
        (TextStyle::Heading, FontId::new(20.0, semibold())),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ]
    .into();
    let mut style = egui::Style { text_styles, ..Default::default() };

    let s = &mut style.spacing;
    s.item_spacing = vec2(8.0, 6.0);
    s.button_padding = vec2(12.0, 6.0);
    s.interact_size = vec2(40.0, BUTTON_H_SMALL);
    s.window_margin = Margin::same(16);
    s.menu_margin = Margin::same(6);
    s.indent = 16.0;
    s.slider_rail_height = 4.0;
    s.icon_width = 18.0;
    s.icon_width_inner = 10.0;
    s.icon_spacing = 8.0;
    s.combo_height = 300.0;
    s.text_edit_width = 240.0;
    s.menu_spacing = 2.0;

    let v = &mut style.visuals;
    *v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.override_text_color = Some(p.t1);
    v.weak_text_color = Some(p.t3);
    v.hyperlink_color = p.accent;
    v.faint_bg_color = p.bg2;
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.code_bg_color = p.field;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.danger;
    v.panel_fill = p.bg1;
    v.window_fill = p.pop;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(R_LG);
    v.menu_corner_radius = CornerRadius::same(R_MD);
    let shadow_alpha = if dark { 140 } else { 40 };
    v.window_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(shadow_alpha) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(shadow_alpha) };
    v.selection.bg_fill = p.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.slider_trailing_fill = true;
    v.striped = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.window_highlight_topmost = false;

    let radius = CornerRadius::same(R_SM);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg1;
    w.noninteractive.weak_bg_fill = p.bg2;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.t1);
    w.noninteractive.corner_radius = radius;

    // Secondary button look; also used by combo boxes, checkboxes and sliders.
    w.inactive.bg_fill = p.sel;
    w.inactive.weak_bg_fill = p.field;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.t1);
    w.inactive.corner_radius = radius;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = p.border_strong;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.5, p.t1);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.accent;
    w.active.weak_bg_fill = p.sel;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.5, p.t1);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.sel;
    w.open.weak_bg_fill = p.sel;
    w.open.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.open.fg_stroke = Stroke::new(1.0, p.t1);
    w.open.corner_radius = radius;

    style
}
