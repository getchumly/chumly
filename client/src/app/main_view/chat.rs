//! Chat: the input at the bottom, the tabs (channel and private
//! conversations) and the messages.

use egui::text::{LayoutJob, TextFormat};
use egui::{CornerRadius, FontFamily, Margin, Rect, Sense, Stroke, pos2, vec2};

use super::*;
use crate::app::ui_util::{dot, font, galley, put};
use crate::icons;
use crate::theme::{self, Button, pal};

impl App {
    pub(super) fn chat_input(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(s) = &self.session else { return };
        let t = self.t();
        let p = pal(root);
        let (hint, enabled) = input_hint(t, s, self.target);
        let input = &mut self.input;
        let frame = egui::Frame::new().fill(p.bg1).inner_margin(Margin { left: 14, right: 14, top: 10, bottom: 14 });
        egui::Panel::bottom("input").frame(frame).show_separator_line(false).show(root, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let send = Button::primary(t.send)
                    .with_fill_icon(icons::FILL_PAPER_PLANE_RIGHT)
                    .height(36.0)
                    .radius(theme::R_MD);
                let send_clicked = ui.add_enabled(enabled, send).clicked();
                let field = egui::Frame::new()
                    .fill(p.field)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(theme::R_MD))
                    .inner_margin(Margin::symmetric(12, 9));
                let edit = egui::TextEdit::singleline(input)
                    .hint_text(theme::text(hint, 13.0).color(p.t3))
                    .frame(field)
                    .desired_width(ui.available_width());
                let resp = ui.add_enabled(enabled, edit);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if send_clicked || enter {
                    actions.push(Action::SendChat);
                    resp.request_focus();
                }
            });
        });
    }

    pub(super) fn chat_area(&self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(s) = &self.session else { return };
        let t = self.t();
        let target = self.target;
        let my_ch = s.my_channel();
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal(root).bg1)).show(root, |ui| {
            chat_tabs(ui, t, s, target, actions);
            let lines: &[ChatLine] = match target {
                ChatTarget::Channel => my_ch.and_then(|c| s.channel_chat.get(&c)).map(Vec::as_slice).unwrap_or(&[]),
                ChatTarget::Private(id) => s.pms.get(&id).map(|c| c.lines.as_slice()).unwrap_or(&[]),
            };
            chat_messages(ui, lines, ("chat", target, my_ch));
        });
    }
}

/// Name of the channel we are in, "" outside channels.
fn current_channel_name(s: &Session) -> String {
    s.my_channel().map(|c| s.channel_name(c)).unwrap_or_default()
}

/// The input's placeholder for `target`, and whether a message can be sent
/// there (we are in a channel, the conversation partner is online).
fn input_hint(t: &Tr, s: &Session, target: ChatTarget) -> (String, bool) {
    match target {
        ChatTarget::Channel => {
            (fill(t.message_channel, &[("channel", &current_channel_name(s))]), s.my_channel().is_some())
        }
        ChatTarget::Private(id) => {
            let name = s.pms.get(&id).map(|c| c.name.clone()).unwrap_or_default();
            (fill(t.message_user, &[("name", &name)]), s.users.contains_key(&id))
        }
    }
}

/// The channel tab and one tab per private conversation.
fn chat_tabs(ui: &mut egui::Ui, t: &Tr, s: &Session, target: ChatTarget, actions: &mut Vec<Action>) {
    let p = pal(ui);
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, p.border));
    let channel = TabSpec {
        prefix: "#",
        name: &current_channel_name(s),
        suffix: None,
        active: target == ChatTarget::Channel,
        unread: s.channel_unread,
        closable: false,
    };
    let tab = chat_tab(ui, bar, bar.left() + 12.0, channel);
    if tab.clicked {
        actions.push(Action::Select(ChatTarget::Channel));
    }
    let mut x = tab.right + 2.0;
    for (id, conv) in &s.pms {
        let online = s.users.contains_key(id);
        let spec = TabSpec {
            prefix: "@",
            name: &conv.name,
            suffix: (!online).then_some(t.offline),
            active: target == ChatTarget::Private(*id),
            unread: conv.unread,
            closable: true,
        };
        let tab = chat_tab(ui, bar, x, spec);
        if tab.clicked {
            actions.push(Action::Select(ChatTarget::Private(*id)));
        }
        if tab.closed {
            actions.push(Action::ClosePm(*id));
        }
        x = tab.right + 2.0;
    }
}

/// Messages, aligned to the bottom like a messenger.
fn chat_messages(ui: &mut egui::Ui, lines: &[ChatLine], id_salt: impl std::hash::Hash + std::fmt::Debug) {
    egui::ScrollArea::vertical().id_salt(id_salt).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(Margin { left: 18, right: 18, top: 16, bottom: 10 }).show(ui, |ui| {
            // The content height from the last frame gives the free
            // space to push the messages down with.
            let height_id = ui.scope_id().with("content-height");
            let prev: f32 = ui.data(|d| d.get_temp(height_id)).unwrap_or(0.0);
            let free = (ui.available_height() - prev - 26.0).max(0.0);
            ui.add_space(free);
            let top = ui.cursor().top();
            ui.spacing_mut().item_spacing.y = 7.0;
            for line in lines {
                chat_line(ui, line);
            }
            let used = ui.cursor().top() - top;
            if (used - prev).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(height_id, used));
                ui.ctx().request_repaint();
            }
        });
    });
}

/// What a chat tab shows: "# general", "@ nick (offline)".
struct TabSpec<'a> {
    prefix: &'a str,
    name: &'a str,
    /// Shown dimmed after the name; a tab with it is dimmed too.
    suffix: Option<&'a str>,
    active: bool,
    unread: bool,
    closable: bool,
}

struct Tab {
    right: f32,
    clicked: bool,
    closed: bool,
}

/// One tab starting at `x` in the tab `bar`.
fn chat_tab(ui: &mut egui::Ui, bar: Rect, x: f32, spec: TabSpec) -> Tab {
    let TabSpec { prefix, name, suffix, active, unread, closable } = spec;
    let p = pal(ui);
    let name_color = if active {
        p.t1
    } else if suffix.is_some() {
        p.t3
    } else {
        p.t2
    };
    let family = if active { theme::semibold() } else { theme::medium() };
    let pg = galley(ui, prefix, font(13.0, family.clone()), p.t3);
    let ng = galley(ui, name, font(13.0, family), name_color);
    let sg = suffix.map(|s| galley(ui, &format!("({s})"), font(12.0, FontFamily::Proportional), p.t3));
    let xg = closable.then(|| galley(ui, icons::X, font(12.0, FontFamily::Proportional), p.t3));
    let mut w = 12.0 + pg.size().x + 4.0 + ng.size().x + 12.0;
    if let Some(g) = &sg {
        w += 6.0 + g.size().x;
    }
    if unread {
        w += 12.0;
    }
    if let Some(g) = &xg {
        w += 10.0 + g.size().x;
    }
    let rect = Rect::from_min_max(pos2(x, bar.top() + 1.0), pos2(x + w, bar.bottom() - 1.0));
    let resp = ui.interact(rect, ui.scope_id().with(("tab", prefix, name)), Sense::click());
    if resp.hovered() && !active {
        let r = CornerRadius { nw: theme::R_SM, ne: theme::R_SM, sw: 0, se: 0 };
        ui.painter().rect_filled(rect, r, p.hover);
    }
    if active {
        let line = Rect::from_min_max(pos2(rect.left(), bar.bottom() - 2.0), pos2(rect.right(), bar.bottom()));
        ui.painter().rect_filled(line, CornerRadius::ZERO, p.accent);
    }
    let mut cx = put(ui, rect, rect.left() + 12.0, pg) + 4.0;
    cx = put(ui, rect, cx, ng);
    if let Some(g) = sg {
        cx = put(ui, rect, cx + 6.0, g);
    }
    if unread {
        dot(ui, pos2(cx + 9.0, rect.center().y), 3.0, p.gold);
        cx += 12.0;
    }
    let mut closed = false;
    if let Some(g) = xg {
        let size = g.size();
        let r = Rect::from_min_size(pos2(cx + 10.0, rect.center().y - size.y / 2.0), size).expand(3.0);
        let close = ui.interact(r, ui.scope_id().with(("tab-close", name)), Sense::click());
        if close.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(theme::R_XS), p.sel);
        }
        put(ui, rect, cx + 10.0, g);
        closed = close.clicked();
    }
    Tab { right: rect.right(), clicked: resp.clicked(), closed }
}

fn chat_line(ui: &mut egui::Ui, line: &ChatLine) {
    let p = pal(ui);
    let (ts, body): (i64, LayoutJob) = match line {
        ChatLine::Msg { author, text, ts } => {
            let mut job = LayoutJob::default();
            let nick = TextFormat {
                font_id: font(13.0, theme::semibold()),
                color: theme::nick_color(ui, author),
                ..Default::default()
            };
            job.append(author, 0.0, nick);
            let body = TextFormat { font_id: font(13.0, FontFamily::Proportional), color: p.t1, ..Default::default() };
            job.append(text, 8.0, body);
            (*ts, job)
        }
        ChatLine::System { text, ts } => {
            let fmt = TextFormat {
                font_id: font(13.0, FontFamily::Proportional),
                color: p.t3,
                italics: true,
                ..Default::default()
            };
            (*ts, LayoutJob::single_section(text.clone(), fmt))
        }
    };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.add_sized([36.0, 18.0], egui::Label::new(theme::mono(local_time(ts, "%H:%M"), 11.0).color(p.t3)));
        ui.add(egui::Label::new(body).wrap());
    });
}
