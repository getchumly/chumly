//! `Button` in the design's variants, and the badge.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Response, Sense, Stroke, Ui, vec2};

use crate::theme::*;

/// Button variants from the design.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Filled accent: the main action of a screen or dialog.
    Primary,
    /// Field fill with a border: ordinary actions.
    Secondary,
    /// Filled red: destructive confirmation.
    Danger,
    /// Red outline on transparent: a risky but not default action.
    DangerOutline,
    /// No fill until hovered: toolbar actions.
    Ghost,
    /// Tinted red: a voice toggle that is currently off (muted, deafened).
    ToggleOff,
    /// Secondary look with red text: e.g. Disconnect.
    SecondaryDanger,
}

/// A button drawn per design: optional leading icon, fixed height,
/// and its own hover/pressed/disabled colors.
pub struct Button<'a> {
    kind: Kind,
    icon: Option<&'a str>,
    text: String,
    height: f32,
    min_width: f32,
    icon_only: bool,
    icon_filled: bool,
    radius: u8,
    icon_color: Option<Color32>,
    exact_width: Option<f32>,
    font_size: f32,
}

impl<'a> Button<'a> {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        Self {
            kind,
            icon: None,
            text: text.into(),
            height: BUTTON_H,
            min_width: 0.0,
            icon_only: false,
            icon_filled: false,
            radius: R_SM,
            icon_color: None,
            exact_width: None,
            font_size: 13.0,
        }
    }
    pub fn primary(text: impl Into<String>) -> Self {
        Self::new(Kind::Primary, text)
    }
    pub fn secondary(text: impl Into<String>) -> Self {
        Self::new(Kind::Secondary, text)
    }
    pub fn ghost(text: impl Into<String>) -> Self {
        Self::new(Kind::Ghost, text)
    }
    pub fn danger(text: impl Into<String>) -> Self {
        Self::new(Kind::Danger, text)
    }
    /// A square ghost button with only an icon; give it a tooltip.
    pub fn icon(icon: &'a str) -> Self {
        Self { icon: Some(icon), icon_only: true, ..Self::new(Kind::Ghost, "") }
    }
    pub fn with_icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }
    /// Leading icon from the Fill set (`icons::FILL_*`).
    pub fn with_fill_icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self.icon_filled = true;
        self
    }
    pub fn icon_color(mut self, color: Color32) -> Self {
        self.icon_color = Some(color);
        self
    }
    /// Fixed width regardless of the label (labels are centered).
    pub fn width(mut self, width: f32) -> Self {
        self.exact_width = Some(width);
        self
    }
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }
    pub fn radius(mut self, radius: u8) -> Self {
        self.radius = radius;
        self
    }
    pub fn small(mut self) -> Self {
        self.height = BUTTON_H_SMALL;
        self
    }
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }
}

/// Width a `Button` with this label (and a leading icon, if any) takes
/// at the default 13 px size: for reserving room, e.g. for a label that toggles.
pub fn button_width(ui: &Ui, text: &str, icon: Option<&str>) -> f32 {
    let text_w =
        ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, medium()), Color32::PLACEHOLDER).size().x;
    let icon_w = icon.map_or(0.0, |i| {
        ui.painter()
            .layout_no_wrap(i.to_owned(), FontId::new(15.0, FontFamily::Proportional), Color32::PLACEHOLDER)
            .size()
            .x
            + 6.0
    });
    text_w + icon_w + 24.0
}

impl egui::Widget for Button<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = pal(ui);
        let font = match self.kind {
            Kind::Primary | Kind::Danger => FontId::new(self.font_size, semibold()),
            _ => FontId::new(self.font_size, medium()),
        };
        let icon_font = FontId::new(15.0, if self.icon_filled { icons_fill() } else { FontFamily::Proportional });
        // PLACEHOLDER color: the real one is picked after hover is known.
        let text_galley =
            (!self.text.is_empty()).then(|| ui.painter().layout_no_wrap(self.text.clone(), font, Color32::PLACEHOLDER));
        let icon_galley = self.icon.map(|i| ui.painter().layout_no_wrap(i.to_owned(), icon_font, Color32::PLACEHOLDER));
        let gap = if text_galley.is_some() && icon_galley.is_some() { 6.0 } else { 0.0 };
        let content_w =
            text_galley.as_ref().map_or(0.0, |g| g.size().x) + icon_galley.as_ref().map_or(0.0, |g| g.size().x) + gap;
        let pad = if self.icon_only { (self.height - content_w) / 2.0 } else { 12.0 };
        let mut width = (content_w + pad * 2.0).max(self.min_width);
        if let Some(w) = self.exact_width {
            width = w;
        }
        let (rect, response) = ui.allocate_exact_size(vec2(width, self.height), Sense::click());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let enabled = ui.is_enabled();
        let hovered = enabled && response.hovered();
        let pressed = enabled && response.is_pointer_button_down_on();
        let (fill, stroke, fg) = match self.kind {
            Kind::Primary => (
                if pressed {
                    p.accent_pressed
                } else if hovered {
                    p.accent_hover
                } else {
                    p.accent
                },
                None,
                p.on_accent,
            ),
            Kind::Danger => (
                if pressed {
                    p.danger_pressed
                } else if hovered {
                    p.danger_hover
                } else {
                    p.danger
                },
                None,
                Color32::WHITE,
            ),
            Kind::Secondary | Kind::SecondaryDanger => {
                let fill = if pressed {
                    p.sel
                } else if hovered {
                    p.hover
                } else {
                    p.field
                };
                let border = if hovered || pressed { p.border_strong } else { p.border };
                (fill, Some(border), if self.kind == Kind::Secondary { p.t1 } else { p.danger })
            }
            Kind::Ghost => {
                let fill = if pressed {
                    p.sel
                } else if hovered {
                    p.hover
                } else {
                    Color32::TRANSPARENT
                };
                (fill, None, if hovered || pressed { p.t1 } else { p.t2 })
            }
            Kind::DangerOutline => {
                let fill = if pressed {
                    p.danger.gamma_multiply(0.22)
                } else if hovered {
                    p.danger.gamma_multiply(0.14)
                } else {
                    Color32::TRANSPARENT
                };
                (fill, Some(p.danger.gamma_multiply(0.6)), p.danger)
            }
            Kind::ToggleOff => {
                let a = if pressed {
                    0.26
                } else if hovered {
                    0.20
                } else {
                    0.14
                };
                let b = if pressed {
                    0.5
                } else if hovered {
                    0.45
                } else {
                    0.35
                };
                (p.danger.gamma_multiply(a), Some(p.danger.gamma_multiply(b)), p.danger)
            }
        };
        let alpha = if enabled { 1.0 } else { 0.38 };
        let painter = ui.painter();
        let radius = CornerRadius::same(self.radius);
        painter.rect_filled(rect, radius, fill.gamma_multiply(alpha));
        if let Some(s) = stroke {
            painter.rect_stroke(rect, radius, Stroke::new(1.0, s.gamma_multiply(alpha)), egui::StrokeKind::Inside);
        }
        let fg = fg.gamma_multiply(alpha);
        let mut x = rect.center().x - content_w / 2.0;
        if let Some(g) = icon_galley {
            let pos = egui::pos2(x, rect.center().y - g.size().y / 2.0);
            x += g.size().x + gap;
            painter.galley(pos, g, self.icon_color.map_or(fg, |c| c.gamma_multiply(alpha)));
        }
        if let Some(g) = text_galley {
            let pos = egui::pos2(x, rect.center().y - g.size().y / 2.0);
            painter.galley(pos, g, fg);
        }
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response
    }
}

/// A small filled badge, e.g. LIVE or a role.
pub fn badge(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(10.0, bold()), fg);
    let size = vec2(galley.size().x + 10.0, 16.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(R_XS), bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    response
}
