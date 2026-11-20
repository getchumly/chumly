//! Left panel: channels with the users in them, and the "me" block at the
//! bottom (voice state, mute / deafen, screen share, disconnect).

use egui::{CornerRadius, FontFamily, Margin, Pos2, Rect, Sense, Stroke, pos2, vec2};

use super::*;
use crate::app::ui_util::{dot, font, galley, menu_item, menu_item_danger, menu_title, put, put_right};
use crate::app::user_menu::user_menu;
use crate::app::{roles, viewers};
use crate::icons;
use crate::theme::{self, Button, Kind, pal};

/// Fade-in and fade-out of the speaking indicator, seconds. The mixer
/// already holds "speaking" for 300 ms after the last packet.
const SPEAK_FADE_IN: f32 = 0.06;
const SPEAK_FADE_OUT: f32 = 0.15;

/// Font size of the buttons in the "me" block.
const ME_BUTTON_FONT: f32 = 12.5;

impl App {
    pub(super) fn channel_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let p = pal(root);
        egui::Panel::left("channels")
            .resizable(true)
            .default_size(310.0)
            .size_range(260.0..=480.0)
            .frame(egui::Frame::new().fill(p.bg2))
            .show(root, |ui| {
                let me_frame = egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 12 });
                egui::Panel::bottom("me").frame(me_frame).show(ui, |ui| self.me_block(ui, actions));
                // Border between the channel list and the "me" block.
                let r = ui.max_rect();
                ui.painter().hline(r.x_range(), ui.available_rect_before_wrap().bottom(), Stroke::new(1.0, p.border));
                self.channel_list(ui, actions);
            });
    }

    /// We are sending voice right now.
    fn me_talking(&self) -> bool {
        self.audio.shared.transmitting.load(Ordering::Relaxed) && !self.muted
    }

    fn me_block(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.spacing_mut().item_spacing = vec2(6.0, 9.0);
        self.me_line(ui);
        let half = (ui.available_width() - 6.0) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            self.voice_buttons(ui, half, actions);
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            self.share_and_disconnect(ui, half, actions);
        });
        if self.screen.sharer.is_some() {
            self.sharing_status(ui, actions);
        }
    }

    /// Speaking dot, nickname, "transmitting", and the voice mode on the right.
    fn me_line(&self, ui: &mut egui::Ui) {
        let Some(me) = self.session.as_ref().and_then(Session::me) else { return };
        let t = self.t();
        let p = pal(ui);
        let talking = self.me_talking();
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
        let level = speak_level(ui, egui::Id::unique(("speak", me.id)), talking);
        paint_speak_dot(ui, pos2(rect.left() + 4.0, rect.center().y), level);
        let x = put(ui, rect, rect.left() + 16.0, galley(ui, &me.nickname, font(13.0, theme::semibold()), p.t1));
        if talking {
            put(ui, rect, x + 8.0, galley(ui, t.transmitting, font(12.0, theme::medium()), p.speak));
        }
        let mode = match self.cfg.audio.mode {
            VoiceMode::VoiceActivation => t.mode_vad.to_string(),
            VoiceMode::PushToTalk => format!("{}: {}", t.mode_ptt, self.cfg.audio.ptt_key),
        };
        put_right(ui, rect, rect.right(), galley(ui, &mode, font(11.0, FontFamily::Proportional), p.t3));
    }

    fn voice_buttons(&self, ui: &mut egui::Ui, width: f32, actions: &mut Vec<Action>) {
        let t = self.t();
        let (icon, label, kind) = if self.muted {
            (icons::MICROPHONE_SLASH, t.unmute, Kind::ToggleOff)
        } else {
            (icons::MICROPHONE, t.mute, Kind::Secondary)
        };
        let resp = ui.add(Button::new(kind, label).with_icon(icon).width(width).font_size(ME_BUTTON_FONT));
        if with_hotkey(resp, &self.cfg.hotkeys.toggle_mute).clicked() {
            actions.push(Action::ToggleMute);
        }
        let (icon, label, kind) = if self.deafened {
            (icons::SPEAKER_SIMPLE_SLASH, t.undeafen, Kind::ToggleOff)
        } else {
            (icons::HEADPHONES, t.deafen, Kind::Secondary)
        };
        let resp = ui.add(Button::new(kind, label).with_icon(icon).width(width).font_size(ME_BUTTON_FONT));
        if with_hotkey(resp, &self.cfg.hotkeys.toggle_deafen).clicked() {
            actions.push(Action::ToggleDeafen);
        }
    }

    fn share_and_disconnect(&self, ui: &mut egui::Ui, width: f32, actions: &mut Vec<Action>) {
        let t = self.t();
        if self.screen.sharer.is_some() {
            let stop =
                Button::danger(t.stop_share).with_fill_icon(icons::FILL_STOP).width(width).font_size(ME_BUTTON_FONT);
            if ui.add(stop).clicked() {
                actions.push(Action::StopShare);
            }
        } else {
            let share =
                Button::secondary(t.share_screen).with_icon(icons::SCREENCAST).width(width).font_size(ME_BUTTON_FONT);
            let resp = ui.add_enabled(screen::SUPPORTED, share);
            if resp.on_disabled_hover_text(t.share_unsupported).clicked() {
                actions.push(Action::OpenShare);
            }
        }
        let disconnect = Button::new(Kind::SecondaryDanger, t.disconnect)
            .with_icon(icons::SIGN_OUT)
            .width(width)
            .font_size(ME_BUTTON_FONT);
        if ui.add(disconnect).clicked() {
            actions.push(Action::Disconnect);
        }
    }

    /// "• You are sharing" (a drawn red dot) with the viewer count on the right; the stats
    /// always on their own line, so nothing wraps.
    fn sharing_status(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let (Some(s), Some(sh)) = (&self.session, &self.screen.sharer) else { return };
        let t = self.t();
        let p = pal(ui);
        let stats = fill(
            t.stream_stats,
            &[
                ("fps", &format!("{:.0}", self.screen.fps.fps)),
                ("lost", &sh.counters().lost.load(Ordering::Relaxed).to_string()),
            ],
        );
        let viewer_count = s.viewers_of(s.my_id).len();
        let flash_at = s.viewer_flash.filter(|at| at.elapsed() < viewers::FLASH);
        if let Some(at) = flash_at {
            ui.ctx().request_repaint_after(viewers::FLASH.saturating_sub(at.elapsed()));
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
            let layout = egui::Layout::right_to_left(egui::Align::Center);
            let mut right = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
            let pill = viewers::eye_pill(&mut right, viewer_count, flash_at.is_some(), 22.0);
            // The dot is drawn, not a "●" in the text: the bundled fonts are
            // subsets without it, and it showed as a box.
            dot(ui, pos2(rect.left() + 3.5, rect.center().y), 3.5, p.danger);
            let text_x = rect.left() + 12.0;
            let mut job = egui::text::LayoutJob::simple_singleline(
                t.sharing_now.to_owned(),
                font(12.0, theme::medium()),
                p.danger,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width((pill.rect.left() - text_x - 8.0).max(20.0));
            put(ui, rect, text_x, ui.painter().layout_job(job));
            viewers::popover(
                ui,
                t,
                s,
                &self.cfg,
                s.my_id,
                true,
                &mut self.screen.viewers_popover,
                &pill,
                true,
                actions,
            );
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
            put(ui, rect, rect.left(), galley(ui, &stats, font(11.0, FontFamily::Monospace), p.t3));
        });
    }

    /// "CHANNELS" with "+", then every channel with its users.
    fn channel_list(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(s) = &self.session else { return };
        let t = self.t();
        // Only for what to show: the server checks every request.
        let perms = s.my_perms();
        let my_role = s.my_role();
        let my_ch = s.my_channel();
        let me_talking = self.me_talking();
        let speaking: HashSet<UserId> = {
            let mx = lock_mixer(&self.audio.mixer);
            s.users.keys().copied().filter(|id| mx.is_speaking(*id)).collect()
        };

        ui.add_space(6.0);
        channels_header(ui, t, perms, actions);
        ui.add_space(6.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin::symmetric(8, 0)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for ch in &s.channels {
                    let users = s.users_in(ch.id);
                    let mine = my_ch == Some(ch.id);
                    let resp = channel_row(ui, ch, users.len(), mine);
                    let resp = if ch.description.is_empty() { resp } else { resp.on_hover_text(&ch.description) };
                    if resp.double_clicked() && !mine {
                        actions.push(Action::Join(ch.id));
                    }
                    resp.context_menu(|ui| channel_menu(ui, t, s, ch, mine, actions));
                    for u in users {
                        let is_me = u.id == s.my_id;
                        let talking = if is_me { me_talking } else { speaking.contains(&u.id) };
                        let row = user_row(ui, t, &s.roles, u, &s.viewers_of(u.id), is_me, talking);
                        if row.live_clicked {
                            actions.push(Action::Watch(u.id, u.nickname.clone()));
                        }
                        if is_me {
                            continue;
                        }
                        if row.response.double_clicked() {
                            actions.push(Action::OpenPm(u.id, u.nickname.clone()));
                        }
                        let volume = self.cfg.user_volume(&u.nickname);
                        row.response.context_menu(|ui| user_menu(ui, t, &s.roles, my_role, perms, u, volume, actions));
                    }
                }
                ui.add_space(8.0);
            });
        });
    }
}

/// Shows the hotkey as the tooltip, if one is set.
fn with_hotkey(resp: egui::Response, key: &str) -> egui::Response {
    if key.is_empty() { resp } else { resp.on_hover_text(key) }
}

/// Animated 0..=1 level of a speaking indicator.
fn speak_level(ui: &egui::Ui, id: egui::Id, speaking: bool) -> f32 {
    let time = if speaking { SPEAK_FADE_IN } else { SPEAK_FADE_OUT };
    ui.ctx().animate_value_with_time(id, if speaking { 1.0 } else { 0.0 }, time)
}

fn paint_speak_dot(ui: &egui::Ui, center: Pos2, level: f32) {
    let p = pal(ui);
    if level > 0.0 {
        dot(ui, center, 7.0, p.speak.gamma_multiply(0.28 * level));
    }
    dot(ui, center, 4.0, theme::lerp_color(p.dot_idle, p.speak, level));
}

fn channels_header(ui: &mut egui::Ui, t: &Tr, perms: Perms, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.set_height(22.0);
        ui.add_space(14.0);
        ui.label(theme::caption(ui, t.channels));
        if perms.contains(Perms::CHANNELS) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                let add = Button::icon(icons::PLUS).height(22.0);
                if ui.add(add).on_hover_text(t.create_channel).clicked() {
                    actions.push(Action::EditChannel(None));
                }
            });
        }
    });
}

/// Right click on a channel: join, and for those with the right edit,
/// create and delete.
fn channel_menu(ui: &mut egui::Ui, t: &Tr, s: &Session, ch: &ChannelInfo, mine: bool, actions: &mut Vec<Action>) {
    menu_title(ui, &format!("# {}", ch.name));
    if ui.add_enabled(!mine, menu_item(icons::SIGN_IN, t.join_channel)).clicked() {
        actions.push(Action::Join(ch.id));
        ui.close();
    }
    if !s.my_perms().contains(Perms::CHANNELS) {
        return;
    }
    ui.separator();
    if ui.add(menu_item(icons::PENCIL_SIMPLE, t.edit_channel)).clicked() {
        actions.push(Action::EditChannel(Some(ch.id)));
        ui.close();
    }
    if ui.add(menu_item(icons::PLUS, t.create_channel)).clicked() {
        actions.push(Action::EditChannel(None));
        ui.close();
    }
    let last = s.channels.len() == 1;
    let del = menu_item_danger(ui, icons::TRASH, t.delete_channel);
    if ui.add_enabled(!last, del).on_disabled_hover_text(t.err_last_channel).clicked() {
        actions.push(Action::DeleteChannel(ch.id, ch.name.clone()));
        ui.close();
    }
}

fn channel_row(ui: &mut egui::Ui, ch: &ChannelInfo, count: usize, mine: bool) -> egui::Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), theme::ROW_CHANNEL), Sense::click());
    let bg = if mine {
        p.sel
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(theme::R_SM), bg);
    let mut x =
        put(ui, rect, rect.left() + 8.0, galley(ui, icons::HASH, font(14.0, FontFamily::Proportional), p.t3)) + 6.0;
    let (family, color) = if mine {
        (theme::semibold(), p.t1)
    } else if count > 0 {
        (theme::medium(), p.t1)
    } else {
        (theme::medium(), p.t2)
    };
    x = put(ui, rect, x, galley(ui, &ch.name, font(13.0, family), color)) + 6.0;
    let count_text = match ch.max_users {
        Some(m) => format!("({count}/{m})"),
        None => format!("({count})"),
    };
    put(ui, rect, x, galley(ui, &count_text, font(12.0, FontFamily::Proportional), p.t3));
    resp
}

struct UserRow {
    response: egui::Response,
    live_clicked: bool,
}

/// `viewers`: who watches the user's screen share (the LIVE badge shows the count).
fn user_row(
    ui: &mut egui::Ui,
    t: &Tr,
    roles: &shared::Roles,
    u: &UserInfo,
    viewers: &[&UserInfo],
    is_me: bool,
    talking: bool,
) -> UserRow {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), theme::ROW_USER), Sense::click());
    if resp.hovered() && !is_me {
        ui.painter().rect_filled(rect, CornerRadius::same(theme::R_SM), p.hover);
    }
    let level = speak_level(ui, egui::Id::unique(("speak", u.id)), talking);
    paint_speak_dot(ui, pos2(rect.left() + 34.0, rect.center().y), level);

    // Right side first, so the nickname can be clipped before it.
    let mut right = rect.right() - 8.0;
    let mut live_clicked = false;
    if u.streams.contains(&MediaKind::Screen) {
        let badge = live_badge(ui, t, u, viewers, rect, right, is_me);
        live_clicked = badge.clicked;
        right = badge.left - 6.0;
    }
    if let Some(left) = voice_state_icon(ui, t, u, rect, right) {
        right = left - 6.0;
    }
    user_name(ui, t, roles, u, rect, right, is_me);
    UserRow { response: resp, live_clicked }
}

struct LiveBadge {
    left: f32,
    clicked: bool,
}

/// "LIVE · 3" ending at `right`: the count only when someone watches.
/// Clicking it on someone else opens their stream.
fn live_badge(
    ui: &mut egui::Ui,
    t: &Tr,
    u: &UserInfo,
    viewers: &[&UserInfo],
    row: Rect,
    right: f32,
    is_me: bool,
) -> LiveBadge {
    let p = pal(ui);
    let mut job = egui::text::LayoutJob::default();
    job.append(t.live, 0.0, egui::TextFormat::simple(font(10.0, theme::bold()), Color32::WHITE));
    if !viewers.is_empty() {
        job.append("·", 4.0, egui::TextFormat::simple(font(10.0, theme::bold()), Color32::WHITE.gamma_multiply(0.6)));
        job.append(
            &viewers.len().to_string(),
            4.0,
            egui::TextFormat::simple(font(10.0, FontFamily::Monospace), Color32::WHITE),
        );
    }
    let g = ui.painter().layout_job(job);
    let badge =
        Rect::from_center_size(pos2(right - (g.size().x + 10.0) / 2.0, row.center().y), vec2(g.size().x + 10.0, 16.0));
    ui.painter().rect_filled(badge, CornerRadius::same(theme::R_XS), p.danger);
    ui.painter().galley(badge.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
    let mut clicked = false;
    if !is_me {
        let live_tip = viewers::live_tip(t, &u.nickname, viewers);
        let tip = if screen::SUPPORTED { live_tip.as_str() } else { t.share_unsupported };
        let b = ui.interact(badge, egui::Id::unique(("live", u.id)), Sense::click()).on_hover_text(tip);
        if b.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        clicked = b.clicked();
    }
    LiveBadge { left: badge.left(), clicked }
}

/// The deafened or muted icon ending at `right`; returns its left edge.
fn voice_state_icon(ui: &mut egui::Ui, t: &Tr, u: &UserInfo, row: Rect, right: f32) -> Option<f32> {
    let (icon, tip) = if u.deafened {
        (icons::SPEAKER_SIMPLE_SLASH, t.deafened_tip)
    } else if u.muted {
        (icons::MICROPHONE_SLASH, t.muted_tip)
    } else {
        return None;
    };
    let r = put_right(ui, row, right, galley(ui, icon, font(14.0, FontFamily::Proportional), pal(ui).danger));
    ui.interact(r, egui::Id::unique(("state", u.id)), Sense::hover()).on_hover_text(tip);
    Some(r.left())
}

/// Nickname in the role's color, "(you)", and the role icon (or the
/// registered check), clipped before `right`.
fn user_name(ui: &mut egui::Ui, t: &Tr, roles: &shared::Roles, u: &UserInfo, row: Rect, right: f32, is_me: bool) {
    let p = pal(ui);
    let mut x = row.left() + 46.0;
    // The nick takes the role's color; without a role it stays plain.
    let role_info = u.role.and_then(|id| roles.get(id));
    let name_color = match role_info {
        _ if u.deafened => p.t3,
        Some(r) => roles::color(p, &r.color),
        None => p.t1,
    };
    let family = if is_me { theme::bold() } else { FontFamily::Proportional };
    let name = ui.painter().layout(u.nickname.clone(), font(13.0, family), name_color, (right - x).max(20.0));
    x = put(ui, row, x, name) + 8.0;
    if is_me {
        x = put(ui, row, x, galley(ui, &format!("({})", t.you), font(12.0, FontFamily::Proportional), p.t3)) + 8.0;
    }
    let role = match role_info {
        Some(r) => roles::icon(&r.icon)
            .map(|(glyph, family)| (glyph, family, roles::color(p, &r.color), roles::tip(t, roles, r))),
        None if u.registered => Some((icons::CHECK, FontFamily::Proportional, p.t3, t.registered_tip.to_string())),
        None => None,
    };
    if let Some((icon, family, color, tip)) = role
        && x < right
    {
        let g = galley(ui, icon, font(12.0, family), color);
        let r = Rect::from_min_size(pos2(x, row.center().y - g.size().y / 2.0), g.size());
        put(ui, row, x, g);
        ui.interact(r, egui::Id::unique(("role", u.id)), Sense::hover()).on_hover_text(tip);
    }
}
