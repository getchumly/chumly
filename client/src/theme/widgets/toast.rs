//! Toast cards.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, Margin, RichText, Shadow, Stroke, Ui};

use crate::theme::*;

/// A toast's look: icon and accent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Info,
    Success,
    Admin,
    Announce,
    Error,
}

/// Paints one toast card (the caller positions and stacks them).
pub fn toast(ui: &mut Ui, kind: ToastKind, text: &str) {
    let p = pal(ui);
    let (icon, color, filled) = match kind {
        ToastKind::Info => (crate::icons::INFO, p.t2, false),
        ToastKind::Success => (crate::icons::FILL_CHECK_CIRCLE, p.speak, true),
        ToastKind::Admin => (crate::icons::FILL_STAR, p.gold, true),
        ToastKind::Announce => (crate::icons::FILL_MEGAPHONE, p.accent, true),
        ToastKind::Error => (crate::icons::FILL_WARNING_CIRCLE, p.danger, true),
    };
    let stroke = if kind == ToastKind::Error { p.danger.gamma_multiply(0.4) } else { p.border };
    egui::Frame::new()
        .fill(p.pop)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(R_MD))
        .inner_margin(Margin::symmetric(12, 10))
        .shadow(Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(100) })
        .show(ui, |ui| {
            ui.set_max_width(340.0);
            // Align::Min: a centered cross axis would stretch the card to the
            // full height of the area.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let family = if filled { icons_fill() } else { FontFamily::Proportional };
                ui.label(RichText::new(icon).family(family).size(16.0).color(color));
                ui.add(egui::Label::new(RichText::new(text).size(13.0).color(p.t1)).wrap());
            });
        });
}
