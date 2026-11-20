//! Form controls: notes, labels, text field, toggle, slider, segmented
//! control, hotkey field, checkbox.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Response, RichText, Sense, Stroke, Ui, vec2,
};

use crate::theme::*;

/// Severity of an inline note in a form.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Note {
    Warn,
    Error,
}

/// A tinted box with an icon and wrapped text, e.g. a form warning.
pub fn note(ui: &mut Ui, kind: Note, text: &str) {
    let p = pal(ui);
    let (color, icon) = match kind {
        Note::Warn => (p.warn, crate::icons::WARNING),
        Note::Error => (p.danger, crate::icons::WARNING_CIRCLE),
    };
    let (fill, stroke) = (color.gamma_multiply(0.10), color.gamma_multiply(0.30));
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(R_SM))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new(icon).size(14.0).color(color));
                ui.add(egui::Label::new(RichText::new(text).size(12.0).color(p.t1)).wrap());
            });
        });
}

/// Label above or beside a form field.
pub fn field_label(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).size(12.0).color(pal(ui).t2))
}

/// A 32 px single-line text field per design.
pub fn text_field(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(Margin::symmetric(10, 7)).min_size(vec2(0.0, BUTTON_H))
}

/// On/off switch, 34×20.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = pal(ui);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(34.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let radius = CornerRadius::same(10);
    if *on {
        ui.painter().rect_filled(rect, radius, p.accent.gamma_multiply(t.max(0.3)));
    }
    if t < 1.0 {
        ui.painter().rect_filled(rect, radius, p.sel.gamma_multiply(1.0 - t));
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, p.border.gamma_multiply(1.0 - t)),
            egui::StrokeKind::Inside,
        );
    }
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 10.0, t);
    let knob = lerp_color(p.t2, p.on_accent, t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 7.0, knob);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Horizontal slider per design: 4 px rail, accent fill, white knob.
/// `mark` draws a tick at that value (e.g. 100% on a 0–200% slider).
pub fn slider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    width: f32,
    mark: Option<f32>,
) -> Response {
    let p = pal(ui);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 16.0), Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    let inner = rect.shrink2(vec2(7.0, 0.0));
    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let frac = ((pos.x - inner.left()) / inner.width()).clamp(0.0, 1.0);
        let v = lo + frac * (hi - lo);
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            resp.mark_changed();
        }
    }
    let frac = ((*value - lo) / (hi - lo)).clamp(0.0, 1.0);
    let x = inner.left() + frac * inner.width();
    let y = rect.center().y;
    let rail = egui::Rect::from_min_max(egui::pos2(rect.left(), y - 2.0), egui::pos2(rect.right(), y + 2.0));
    ui.painter().rect_filled(rail, CornerRadius::same(2), p.sel);
    let filled = egui::Rect::from_min_max(rail.min, egui::pos2(x, rail.max.y));
    ui.painter().rect_filled(filled, CornerRadius::same(2), p.accent);
    if let Some(m) = mark {
        let mx = inner.left() + ((m - lo) / (hi - lo)).clamp(0.0, 1.0) * inner.width();
        ui.painter().vline(mx, (y - 5.0)..=(y + 5.0), Stroke::new(1.0, p.t3));
    }
    let active = resp.hovered() || resp.dragged();
    if active {
        ui.painter().circle_filled(egui::pos2(x, y), 10.0, p.accent.gamma_multiply(0.3));
    }
    ui.painter().circle_filled(egui::pos2(x, y + 1.0), 7.0, Color32::from_black_alpha(70));
    ui.painter().circle_filled(egui::pos2(x, y), 7.0, Color32::WHITE);
    resp
}

/// Segmented control: one of several options. Returns true if changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, Option<&str>, &str)]) -> bool {
    let p = pal(ui);
    let mut changed = false;
    egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            let galleys: Vec<_> = options
                .iter()
                .map(|(opt, icon, label)| {
                    let selected = *value == *opt;
                    let font = FontId::new(13.0, if selected { semibold() } else { FontFamily::Proportional });
                    let text = match icon {
                        Some(i) => format!("{i}  {label}"),
                        None => (*label).to_owned(),
                    };
                    ui.painter().layout_no_wrap(text, font, if selected { p.t1 } else { p.t2 })
                })
                .collect();
            let total: f32 = galleys.iter().map(|g| g.size().x + 24.0).sum();
            // Always left to right, even inside a right-to-left parent.
            let layout = egui::Layout::left_to_right(egui::Align::Center);
            ui.allocate_ui_with_layout(vec2(total, 26.0), layout, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for ((opt, _, _), g) in options.iter().zip(galleys) {
                    let selected = *value == *opt;
                    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 24.0, 26.0), Sense::click());
                    if selected {
                        ui.painter().rect_filled(rect, CornerRadius::same(5), p.sel);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(5), p.hover);
                    }
                    ui.painter().galley(rect.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
                    if resp.clicked() && !selected {
                        *value = *opt;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// A hotkey shown as key caps inside a field ("Ctrl" "M").
/// `state`: capturing draws an accent border and the prompt instead.
pub fn hotkey_field(ui: &mut Ui, width: f32, combo: &str, placeholder: &str, capturing: Option<&str>, error: bool) {
    let p = pal(ui);
    let border = if capturing.is_some() {
        p.accent
    } else if error {
        p.danger
    } else {
        p.border
    };
    let (rect, _) = ui.allocate_exact_size(vec2(width, BUTTON_H), Sense::hover());
    if capturing.is_some() {
        ui.painter().rect_filled(rect.expand(3.0), CornerRadius::same(R_SM + 3), p.accent.gamma_multiply(0.18));
    }
    ui.painter().rect_filled(rect, CornerRadius::same(R_SM), p.field);
    ui.painter().rect_stroke(rect, CornerRadius::same(R_SM), Stroke::new(1.0, border), egui::StrokeKind::Inside);
    let painter = ui.painter_at(rect.shrink(1.0));
    let mut x = rect.left() + 8.0;
    if let Some(prompt) = capturing {
        painter.circle_filled(egui::pos2(x + 4.0, rect.center().y), 3.5, p.accent);
        let g = painter.layout_no_wrap(prompt.to_owned(), FontId::new(13.0, FontFamily::Proportional), p.accent);
        painter.galley(egui::pos2(x + 14.0, rect.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
        return;
    }
    if combo.is_empty() {
        let g = painter.layout_no_wrap(placeholder.to_owned(), FontId::new(13.0, FontFamily::Proportional), p.t3);
        painter.galley(egui::pos2(x + 2.0, rect.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
        return;
    }
    for key in combo.split('+') {
        let g = painter.layout_no_wrap(key.to_owned(), FontId::new(11.5, FontFamily::Monospace), p.t1);
        let cap = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - 10.0), vec2(g.size().x + 14.0, 20.0));
        painter.rect_filled(cap, CornerRadius::same(R_XS), p.sel);
        painter.galley(cap.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
        x = cap.right() + 6.0;
    }
}

/// Checkbox per design: 18 px box, accent fill with a check when on.
/// `hint` is drawn under the label in small muted text.
pub fn checkbox(ui: &mut Ui, on: &mut bool, label: &str, hint: Option<&str>) -> Response {
    let p = pal(ui);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (rect, mut resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
        let radius = CornerRadius::same(R_XS);
        if *on {
            ui.painter().rect_filled(rect, radius, p.accent);
            let g = ui.painter().layout_no_wrap(crate::icons::CHECK.to_owned(), FontId::new(13.0, bold()), p.on_accent);
            ui.painter().galley(rect.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
        } else {
            let stroke = if resp.hovered() { p.t2 } else { p.t3 };
            ui.painter().rect_stroke(rect, radius, Stroke::new(1.5, stroke), egui::StrokeKind::Inside);
        }
        let text = ui
            .vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                let l = ui.add(egui::Label::new(RichText::new(label).size(13.0).color(p.t1)).sense(Sense::click()));
                if let Some(h) = hint {
                    ui.add(egui::Label::new(RichText::new(h).size(12.0).color(p.t3)).wrap());
                }
                l
            })
            .inner;
        if resp.clicked() || text.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        resp
    })
    .inner
}
