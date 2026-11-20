//! Modal `Dialog` and the frame and header of non-modal windows.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Sense, Shadow, Stroke, Ui, Vec2, vec2};

use crate::theme::*;

/// Header icon of a dialog.
pub struct DialogIcon<'a> {
    pub glyph: &'a str,
    pub color: Color32,
    pub filled: bool,
}

/// A centered modal dialog per design: header with icon and title,
/// body, and a footer with right-aligned buttons (add the main action first).
pub struct Dialog<'a> {
    id: &'a str,
    title: String,
    icon: Option<DialogIcon<'a>>,
    width: f32,
    closable: bool,
    danger: bool,
}

/// What happened to a dialog this frame.
pub struct DialogResponse {
    /// Esc or the header's close button.
    pub close_requested: bool,
}

impl<'a> Dialog<'a> {
    pub fn new(id: &'a str, title: impl Into<String>) -> Self {
        Self { id, title: title.into(), icon: None, width: 400.0, closable: false, danger: false }
    }
    pub fn icon(mut self, glyph: &'a str, color: Color32) -> Self {
        self.icon = Some(DialogIcon { glyph, color, filled: false });
        self
    }
    pub fn fill_icon(mut self, glyph: &'a str, color: Color32) -> Self {
        self.icon = Some(DialogIcon { glyph, color, filled: true });
        self
    }
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
    /// Show an × in the header.
    pub fn closable(mut self) -> Self {
        self.closable = true;
        self
    }
    /// Red frame and header: security warnings.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn show(self, ctx: &egui::Context, body: impl FnOnce(&mut Ui), footer: impl FnOnce(&mut Ui)) -> DialogResponse {
        let p = palette(ctx.global_style().visuals.dark_mode);
        let border = if self.danger { p.danger.gamma_multiply(0.5) } else { p.border };
        let frame = egui::Frame::new()
            .fill(p.pop)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(CornerRadius::same(R_LG))
            .shadow(Shadow { offset: [0, 24], blur: 64, spread: 0, color: Color32::from_black_alpha(128) });
        let mut close = false;
        let width = self.width.min(ctx.content_rect().width() - 32.0);
        egui::Modal::new(egui::Id::unique(self.id)).frame(frame).backdrop_color(Color32::from_black_alpha(110)).show(
            ctx,
            |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                // Header.
                let header_fill = if self.danger { p.danger.gamma_multiply(0.12) } else { Color32::TRANSPARENT };
                let header_h = if self.danger { 66.0 } else { 48.0 };
                let (rect, _) = ui.allocate_exact_size(vec2(width, header_h), Sense::hover());
                let radius = CornerRadius { nw: R_LG, ne: R_LG, sw: 0, se: 0 };
                ui.painter().rect_filled(rect, radius, header_fill);
                let line = if self.danger { p.danger.gamma_multiply(0.3) } else { p.border };
                ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, line));
                let mut x = rect.left() + 20.0;
                if let Some(icon) = &self.icon {
                    let family = if icon.filled { icons_fill() } else { FontFamily::Proportional };
                    if self.danger {
                        let tile = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - 17.0), vec2(34.0, 34.0));
                        ui.painter().rect_filled(tile, CornerRadius::same(R_MD), p.danger);
                        let g = ui.painter().layout_no_wrap(
                            icon.glyph.to_owned(),
                            FontId::new(20.0, family),
                            Color32::WHITE,
                        );
                        ui.painter().galley(tile.center() - g.size() / 2.0, g, Color32::WHITE);
                        x = tile.right() + 12.0;
                    } else {
                        let g =
                            ui.painter().layout_no_wrap(icon.glyph.to_owned(), FontId::new(16.0, family), icon.color);
                        let w = g.size().x;
                        ui.painter().galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, icon.color);
                        x += w + 8.0;
                    }
                }
                let title_max = rect.right() - x - if self.closable { 44.0 } else { 20.0 };
                let g = ui.painter().layout(self.title.clone(), FontId::new(16.0, semibold()), p.t1, title_max);
                ui.painter().galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, p.t1);
                if self.closable {
                    let r = egui::Rect::from_center_size(
                        egui::pos2(rect.right() - 26.0, rect.center().y),
                        Vec2::splat(28.0),
                    );
                    if close_button(ui, r) {
                        close = true;
                    }
                }
                // Body.
                egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 18 }).show(ui, |ui| {
                    ui.set_width(width - 40.0);
                    ui.spacing_mut().item_spacing.y = 10.0;
                    body(ui);
                });
                // Footer.
                let top = ui.cursor().top();
                ui.painter().hline(ui.max_rect().x_range(), top, Stroke::new(1.0, p.border));
                egui::Frame::new().inner_margin(Margin::symmetric(20, 14)).show(ui, |ui| {
                    let layout = egui::Layout::right_to_left(egui::Align::Center);
                    ui.allocate_ui_with_layout(vec2(width - 40.0, 34.0), layout, |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        footer(ui);
                    });
                });
            },
        );
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        DialogResponse { close_requested: close }
    }
}

/// Frame of a non-modal window (settings, admin panel).
pub fn window_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.pop)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(R_LG))
        .shadow(Shadow { offset: [0, 24], blur: 64, spread: 0, color: Color32::from_black_alpha(140) })
}

/// 48 px window header with a title and ×. Returns true if × was clicked.
pub fn window_header(ui: &mut Ui, width: f32, title: &str) -> bool {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 48.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.border));
    let g = ui.painter().layout_no_wrap(title.to_owned(), FontId::new(16.0, semibold()), p.t1);
    ui.painter().galley(egui::pos2(rect.left() + 20.0, rect.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
    let x = egui::Rect::from_center_size(egui::pos2(rect.right() - 26.0, rect.center().y), vec2(28.0, 28.0));
    close_button(ui, x)
}

/// The × button at `rect`. Drawn in a child UI: `Ui::put` would move the
/// parent's layout cursor to the button's bottom, and whatever comes after
/// the header would start inside it.
fn close_button(ui: &mut Ui, rect: egui::Rect) -> bool {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.add(Button::icon(crate::icons::X).height(28.0)).clicked()
}
