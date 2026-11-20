//! Small painting and layout helpers shared by the app's screens: text laid
//! out and placed by hand inside custom rows, dots, context menu items.

use egui::{FontFamily, FontId, Pos2, Rect, pos2, vec2};

use super::*;
use crate::theme::{self, pal};

/// Lays out one line of text for painting by hand inside custom rows.
pub(super) fn galley(ui: &egui::Ui, text: &str, font: FontId, color: Color32) -> Arc<egui::Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font, color)
}

pub(super) fn font(size: f32, family: FontFamily) -> FontId {
    FontId::new(size, family)
}

/// Paints `g` at `x`, vertically centered in `rect`; returns the x after it.
pub(super) fn put(ui: &egui::Ui, rect: Rect, x: f32, g: Arc<egui::Galley>) -> f32 {
    let w = g.size().x;
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
    x + w
}

/// Paints `g` so that it ends at `right`; returns its rect.
pub(super) fn put_right(ui: &egui::Ui, rect: Rect, right: f32, g: Arc<egui::Galley>) -> Rect {
    let size = g.size();
    let min = pos2(right - size.x, rect.center().y - size.y / 2.0);
    ui.painter().galley(min, g, Color32::PLACEHOLDER);
    Rect::from_min_size(min, size)
}

pub(super) fn dot(ui: &egui::Ui, center: Pos2, radius: f32, color: Color32) {
    ui.painter().circle_filled(center, radius, color);
}

/// The caption at the top of a context menu.
pub(super) fn menu_title(ui: &mut egui::Ui, text: &str) {
    ui.label(theme::caption(ui, text));
}

pub(super) fn menu_item(icon: &str, text: &str) -> egui::Button<'static> {
    egui::Button::new(theme::with_icon(icon, text)).frame_when_inactive(false).min_size(vec2(200.0, 28.0))
}

pub(super) fn menu_item_danger(ui: &egui::Ui, icon: &str, text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(theme::with_icon(icon, text)).color(pal(ui).danger))
        .frame_when_inactive(false)
        .min_size(vec2(200.0, 28.0))
}
