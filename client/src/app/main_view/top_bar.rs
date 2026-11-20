//! Top bar: logo, server name, connection indicator with its details
//! popover; on the right the role chip, admin entry points, settings and
//! the window buttons.

use egui::{CornerRadius, FontFamily, Margin, Pos2, Rect, Sense, Stroke, pos2, vec2};

use super::*;
use crate::app::popover::HoverPopover;
use crate::app::ui_util::{dot, font, galley, put, put_right};
use crate::app::{link, roles};
use crate::icons;
use crate::theme::{self, Button, pal};

impl App {
    pub(super) fn top_bar(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let custom_frame = !self.cfg.system_frame;
        let right = if custom_frame { 0 } else { 12 };
        let frame = egui::Frame::new().fill(pal(root).bg2).inner_margin(Margin { left: 14, right, top: 0, bottom: 0 });
        egui::Panel::top("top").exact_size(theme::TOP_BAR).frame(frame).show(root, |ui| {
            window_frame::title_bar(ui, custom_frame, |ui| {
                // The right side is laid out first; the server name and the link
                // indicator get what is left and shrink to fit.
                let full = ui.max_rect();
                let right_layout = egui::Layout::right_to_left(egui::Align::Center);
                let mut right_ui = ui.new_child(egui::UiBuilder::new().max_rect(full).layout(right_layout));
                self.top_bar_right(&mut right_ui, custom_frame, actions);

                let right_w = right_ui.min_rect().width();
                let left_rect =
                    Rect::from_min_max(full.min, pos2((full.right() - right_w - 16.0).max(full.left()), full.bottom()));
                let left_layout = egui::Layout::left_to_right(egui::Align::Center);
                let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(left_rect).layout(left_layout));
                self.top_bar_left(&mut left_ui);
            });
        });
    }

    /// Laid out right to left: window buttons, "Server users", the role
    /// chip (or the admin login), settings.
    fn top_bar_right(&self, ui: &mut egui::Ui, custom_frame: bool, actions: &mut Vec<Action>) {
        let Some(s) = &self.session else { return };
        let t = self.t();
        let p = pal(ui);
        if custom_frame {
            ui.spacing_mut().item_spacing.x = 0.0;
            window_frame::window_buttons(ui, t);
            ui.add_space(8.0);
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        if s.my_perms().intersects(Perms::PANEL) {
            let users = Button::secondary(t.server_users).with_icon(icons::USERS_THREE).small();
            if ui.add(users).clicked() {
                actions.push(Action::OpenAdminPanel);
            }
        }
        role_chips(ui, t, s, actions);
        let resp = ui.add(Button::ghost(t.settings).with_icon(icons::GEAR_SIX).small());
        // A waiting update: a quiet dot, no toast over the game.
        let resp = match self.updates.offer(self.cfg.skipped_version.as_deref()) {
            Some(r) => {
                crate::app::updates_ui::icon_dot(ui, resp.rect, if resp.hovered() { p.hover } else { p.bg2 });
                resp.on_hover_text(fill(t.update_tip, &[("v", &r.version)]))
            }
            None => resp,
        };
        if resp.clicked() {
            actions.push(Action::OpenSettings);
        }
    }

    /// Logo, server name and the connection indicator (or the status
    /// while not connected), sharing the width left by the right side.
    fn top_bar_left(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let logo = self.logo(ui.ctx());
        let (status, status_color) = self.status_text();
        let connected = self.conn == ConnState::Connected;
        let Some(s) = &self.session else { return };
        let p = pal(ui);
        ui.spacing_mut().item_spacing.x = 10.0;
        if let Some(logo) = &logo {
            ui.add(egui::Image::new((logo.id(), vec2(22.0, 22.0))).corner_radius(CornerRadius::same(theme::R_SM)));
        }
        let gap = ui.spacing().item_spacing.x;
        let avail = ui.available_width();
        let name_w = galley(ui, &s.server_name, font(14.0, theme::semibold()), p.t1).size().x;
        // The name keeps up to 120 px; the indicator takes what it needs
        // of the rest (shortening itself), the name gets the remainder.
        // A poor connection matters more than the full name.
        let poor = s.link.level() == Some(theme::Quality::Poor);
        let name_min = name_w.min(if poor { 60.0 } else { 120.0 });
        let indicator = connected.then(|| LinkIndicator::new(ui, t, s, avail - name_min - gap));
        let status_w = if connected {
            0.0
        } else {
            13.0 + galley(ui, &status, font(12.0, FontFamily::Proportional), p.t2).size().x
        };
        let right_part = indicator.as_ref().map_or(status_w, LinkIndicator::width);
        let name_max = (avail - right_part - gap).max(name_min.min(avail));
        ui.scope(|ui| {
            ui.set_max_width(name_max);
            ui.add(egui::Label::new(theme::text_semibold(&s.server_name, 14.0)).truncate())
                .on_hover_text(fill(t.server_version_tip, &[("version", server_version(s))]));
        });
        match indicator {
            Some(ind) => ind.show(ui, t, s, &mut self.link_popover),
            None => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (r, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
                    dot(ui, r.center(), 3.5, status_color);
                    ui.add(egui::Label::new(theme::text(status, 12.0).color(p.t2)).truncate());
                });
            }
        }
    }
}

fn server_version(s: &Session) -> &str {
    if s.server_version.is_empty() { "?" } else { s.server_version.as_str() }
}

/// The user's role chip, and for the owner a warning when the server lacks
/// features this client has. Without a role, the admin login while the
/// server waits for its owner.
fn role_chips(ui: &mut egui::Ui, t: &Tr, s: &Session, actions: &mut Vec<Action>) {
    let p = pal(ui);
    let roles = &s.roles;
    match s.my_role().and_then(|id| roles.get(id)) {
        Some(role) => {
            let tip = roles::tip(t, roles, role);
            role_chip(ui, roles::icon(&role.icon), &roles::name(t, role), roles::color(p, &role.color))
                .on_hover_text(tip);
            // Only the owner can update the server, so only they hear
            // about it, and only for features this client cannot use.
            let missing = shared::missing_on_server(shared::FEATURES, &s.server_features);
            if role.id == shared::OWNER_ROLE && !missing.is_empty() {
                let names: Vec<String> = missing.iter().map(|f| feature_label(t, f.name)).collect();
                let tip = fill(t.server_missing, &[("features", &names.join(", ")), ("version", server_version(s))]);
                role_chip(ui, Some((icons::WARNING, FontFamily::Proportional)), t.server_outdated, p.warn)
                    .on_hover_text(tip);
            }
            separator_v(ui);
        }
        // The button is offered only while the server waits for its
        // owner: after that rights are given from the user menu.
        None if s.admin_setup => {
            let b = Button::secondary(t.admin_login).with_icon(icons::KEY).small();
            if ui.add(b).clicked() {
                actions.push(Action::OpenAdmin);
            }
            separator_v(ui);
        }
        None => {}
    }
}

/// Human name of an optional feature (`shared::FEATURES`) for the owner's
/// "the server does not support ..." note. Unknown names are shown as is.
fn feature_label(_t: &Tr, name: &str) -> String {
    // No optional features yet; each new one gets a translated label here.
    name.to_string()
}

/// A chip with an icon (glyph and its font family) and a label, at most
/// 220 px wide (a long role name is cut with "…").
fn role_chip(ui: &mut egui::Ui, icon: Option<(&str, FontFamily)>, text: &str, color: Color32) -> egui::Response {
    let ig = icon.map(|(g, family)| galley(ui, g, font(12.0, family), color));
    let icon_w = ig.as_ref().map_or(0.0, |g| g.size().x + 5.0);
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font(12.0, theme::semibold()), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(220.0 - 16.0 - icon_w);
    let tg = ui.painter().layout_job(job);
    let size = vec2(icon_w + tg.size().x + 16.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(5), roles::chip_bg(pal(ui), color));
    let mut x = rect.left() + 8.0;
    if let Some(g) = ig {
        x = put(ui, rect, x, g) + 5.0;
    }
    put(ui, rect, x, tg);
    resp
}

fn separator_v(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 18.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, pal(ui).border));
}

/// "● Connected · 23 ms" in the top bar, a red pill when the connection is
/// poor, and the details popover on hover (300 ms) or click (pinned).
/// In a narrow window it drops pieces: the loss, then the label, then the ping.
struct LinkIndicator {
    label: Option<Arc<egui::Galley>>,
    numbers: Option<Arc<egui::Galley>>,
    poor: bool,
    color: Color32,
}

impl LinkIndicator {
    fn new(ui: &egui::Ui, t: &Tr, s: &Session, max_w: f32) -> Self {
        let p = pal(ui);
        let avg = s.link.average();
        let level = s.link.level();
        let poor = level == Some(theme::Quality::Poor);
        let ms = |v: f32| fill(t.link_ms, &[("ms", &format!("{v:.0}"))]);
        let (label, label_color) = if poor { (t.link_poor, p.danger) } else { (t.status_connected, p.t2) };
        let label_font = if poor { font(12.0, theme::semibold()) } else { font(12.0, FontFamily::Proportional) };
        let num_color = match level {
            Some(theme::Quality::Good) | None => p.t2,
            Some(q) => q.color(p),
        };
        let numbers = |with_label: bool, with_loss: bool| {
            avg.map(|a| {
                let mut n = ms(a.rtt_ms);
                if with_loss {
                    n = format!("{n} · {}", fill(t.link_loss, &[("loss", &format!("{:.0}", a.loss * 100.0))]));
                }
                if with_label {
                    n = format!("· {n}");
                }
                galley(ui, &n, font(11.5, FontFamily::Monospace), num_color)
            })
        };
        let color = level.map_or(p.speak, |q| q.color(p));
        // From the fullest to the shortest; the first that fits wins.
        let variants = [(true, poor), (true, false), (false, false)];
        for (with_label, with_loss) in variants {
            let ind = Self {
                label: with_label.then(|| galley(ui, label, label_font.clone(), label_color)),
                numbers: numbers(with_label, with_loss),
                poor,
                color,
            };
            if ind.width() <= max_w {
                return ind;
            }
        }
        Self { label: None, numbers: None, poor, color }
    }

    fn pad(&self) -> f32 {
        if self.poor { 8.0 } else { 0.0 }
    }

    fn width(&self) -> f32 {
        let piece = |g: &Option<Arc<egui::Galley>>| g.as_ref().map_or(0.0, |g| g.size().x + 6.0);
        self.pad() * 2.0 + 7.0 + piece(&self.label) + piece(&self.numbers)
    }

    fn show(self, ui: &mut egui::Ui, t: &Tr, s: &Session, pop: &mut HoverPopover) {
        let p = pal(ui);
        let pad = self.pad();
        let (rect, resp) = ui.allocate_exact_size(vec2(self.width(), 24.0), Sense::click());
        if self.poor {
            ui.painter().rect_filled(rect, CornerRadius::same(theme::R_SM), p.danger.gamma_multiply(0.14));
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(theme::R_SM),
                Stroke::new(1.0, p.danger.gamma_multiply(0.4)),
                egui::StrokeKind::Inside,
            );
        }
        let mut x = rect.left() + pad;
        dot(ui, pos2(x + 3.5, rect.center().y), 3.5, self.color);
        x += 7.0 + 6.0;
        if let Some(g) = self.label {
            x = put(ui, rect, x, g) + 6.0;
        }
        if let Some(g) = self.numbers {
            put(ui, rect, x, g);
        }
        if pop.update(ui, &resp) {
            pop.set_rect(link_popover(ui, t, s, rect.left_bottom() + vec2(0.0, 6.0)));
        }
    }
}

/// The details card: level, ping, losses, traffic, server version, protocol.
fn link_popover(ui: &egui::Ui, t: &Tr, s: &Session, at: Pos2) -> Rect {
    let p = pal(ui);
    let level = s.link.level();
    let frame = egui::Frame::new()
        .fill(p.pop)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(theme::R_MD))
        .inner_margin(Margin::same(12))
        .shadow(egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(90) });
    let area = egui::Area::new(egui::Id::unique("link-popover"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                ui.set_width(266.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    theme::signal_bars(ui, level);
                    let (title, color) = match level {
                        Some(theme::Quality::Good) | None => (t.link_good, p.speak),
                        Some(theme::Quality::Fair) => (t.link_fair, p.warn),
                        Some(theme::Quality::Poor) => (t.link_poor, p.danger),
                    };
                    ui.label(theme::text_semibold(title, 13.0).color(color));
                });
                ui.add_space(6.0);
                for (label, value, color) in link_rows(ui, t, s) {
                    let (r, _) = ui.allocate_exact_size(vec2(266.0, 26.0), Sense::hover());
                    put(ui, r, r.left(), galley(ui, label, font(12.0, FontFamily::Proportional), p.t2));
                    put_right(ui, r, r.right(), galley(ui, &value, font(12.0, FontFamily::Monospace), color));
                }
                if level == Some(theme::Quality::Poor) {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(p.danger.gamma_multiply(0.12))
                        .corner_radius(CornerRadius::same(theme::R_SM))
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(246.0);
                            ui.add(egui::Label::new(theme::text(t.link_poor_hint, 12.0).color(p.t1)).wrap());
                        });
                }
            });
        });
    area.response.rect
}

/// The rows of the details card: label, value and its color.
fn link_rows(ui: &egui::Ui, t: &Tr, s: &Session) -> [(&'static str, String, Color32); 6] {
    let p = pal(ui);
    let avg = s.link.average().unwrap_or_default();
    let pct = |v: f32| format!("{:.1} %", v * 100.0);
    // A value over its threshold takes the level colour.
    let tint = |q: theme::Quality| if q == theme::Quality::Good { p.t1 } else { q.color(p) };
    let voice_quality = link::loss_quality(avg.voice_lost.max(avg.voice_late));
    let traffic = fill(t.link_kbps, &[("rx", &format!("{:.0}", avg.rx_kbps)), ("tx", &format!("{:.0}", avg.tx_kbps))]);
    [
        (t.link_ping, fill(t.link_ms, &[("ms", &format!("{:.0}", avg.rtt_ms))]), tint(link::ping_quality(avg.rtt_ms))),
        (t.link_packet_loss, pct(avg.loss), tint(link::loss_quality(avg.loss))),
        (t.link_voice, format!("{} / {}", pct(avg.voice_lost), pct(avg.voice_late)), tint(voice_quality)),
        (t.link_traffic, traffic, p.t1),
        (t.link_server_version, server_version(s).to_string(), p.t1),
        (t.link_protocol, s.protocol.map_or("?".into(), |m| format!("chumly/{m}")), p.t1),
    ]
}
