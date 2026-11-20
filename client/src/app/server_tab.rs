//! The owner's "Server" tab of the admin panel: the server password. Set or
//! change it (typed twice), or remove it (with a confirmation). The server
//! never sends the password back, only whether there is one.

use egui::{CornerRadius, Margin, Stroke};

use super::*;
use crate::icons;
use crate::theme::{self, Button, Dialog, pal};

#[derive(Default)]
pub(super) struct ServerTab {
    /// From the server: whether a password is set; `None` until it answers.
    pub(super) password_set: Option<bool>,
    set: Option<SetDialog>,
    remove: bool,
    /// What was asked, for the toast when the answer comes: `Some(true)`
    /// set, `Some(false)` removed.
    awaiting: Option<bool>,
}

#[derive(Default)]
struct SetDialog {
    password: String,
    repeat: String,
    error: Option<&'static str>,
}

impl ServerTab {
    pub(super) fn dialog_open(&self) -> bool {
        self.set.is_some() || self.remove
    }

    /// The server's answer to a request. Returns the toast to show for a
    /// change this tab asked for.
    pub(super) fn on_settings(&mut self, t: &Tr, password_set: bool) -> Option<&'static str> {
        self.password_set = Some(password_set);
        match self.awaiting.take() {
            Some(true) if password_set => Some(t.password_changed),
            Some(false) if !password_set => Some(t.password_removed),
            _ => None,
        }
    }

    /// The tab's content.
    pub(super) fn show(&mut self, ui: &mut egui::Ui, t: &Tr) {
        let p = pal(ui);
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.add_space(8.0);
        egui::Frame::new()
            .fill(p.field)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(theme::R_MD))
            .inner_margin(Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(theme::text_semibold(t.server_password, 14.0));
                let (icon, color, text) = match self.password_set {
                    None => (icons::CIRCLE_NOTCH, p.t3, t.loading),
                    Some(true) => (icons::LOCK_KEY, p.accent_text, t.password_is_set),
                    Some(false) => (icons::WARNING_CIRCLE, p.warn, t.password_is_none),
                };
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(theme::text(icon, 15.0).color(color));
                    ui.label(theme::text(text, 13.0).color(p.t1));
                });
                ui.label(theme::text(t.password_note, 12.0).color(p.t3));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let known = self.password_set.is_some();
                    let set = self.password_set == Some(true);
                    let label = if set { t.password_change } else { t.password_set };
                    let button = if set { Button::secondary(label) } else { Button::primary(label) };
                    if ui.add_enabled(known, button.with_icon(icons::KEY).small()).clicked() {
                        self.set = Some(SetDialog::default());
                    }
                    if set && ui.add(Button::ghost(t.password_remove).with_icon(icons::TRASH).small()).clicked() {
                        self.remove = true;
                    }
                });
            });
    }

    /// The set and remove dialogs, drawn over the panel.
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, t: &Tr, send: &mut Vec<ClientMsg>) {
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        if let Some(mut d) = self.set.take() {
            let mut submit = false;
            let mut enter = false;
            let mut cancel = false;
            let title = if self.password_set == Some(true) { t.password_change } else { t.password_set };
            let r = Dialog::new("server-password", title).icon(icons::KEY, p.t2).width(440.0).show(
                ctx,
                |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    theme::field_label(ui, t.password_new);
                    let first =
                        ui.add(theme::text_field(&mut d.password).password(true).desired_width(ui.available_width()));
                    if !first.has_focus() && d.password.is_empty() && d.repeat.is_empty() {
                        first.request_focus();
                    }
                    ui.add_space(4.0);
                    theme::field_label(ui, t.password_repeat);
                    let second =
                        ui.add(theme::text_field(&mut d.repeat).password(true).desired_width(ui.available_width()));
                    if first.changed() || second.changed() {
                        d.error = None;
                    }
                    if second.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        enter = true;
                    }
                    if let Some(e) = d.error {
                        ui.add_space(4.0);
                        theme::note(ui, theme::Note::Error, e);
                    }
                    ui.add_space(2.0);
                    ui.add(egui::Label::new(theme::text(t.password_hint_rules, 12.0).color(p.t3)).wrap());
                },
                |ui| {
                    submit |= ui.add(Button::primary(t.save)).clicked();
                    cancel |= ui.add(Button::secondary(t.cancel)).clicked();
                },
            );
            if submit || enter {
                d.error = if d.password.is_empty() {
                    Some(t.password_empty)
                } else if d.password != d.repeat {
                    Some(t.password_mismatch)
                } else if !shared::validate_password(&d.password) {
                    Some(t.err_invalid_password)
                } else {
                    None
                };
                if d.error.is_none() {
                    send.push(ClientMsg::SetPassword { password: d.password.clone() });
                    self.awaiting = Some(true);
                    return;
                }
            }
            if !cancel && !r.close_requested {
                self.set = Some(d);
            }
        }
        if self.remove {
            let mut keep = true;
            let r = Dialog::new("server-password-remove", t.password_remove_title).icon(icons::WARNING, p.warn).show(
                ctx,
                |ui| {
                    ui.add(egui::Label::new(theme::text(t.password_remove_text, 13.0).color(p.t2)).wrap());
                },
                |ui| {
                    if ui.add(Button::danger(t.password_remove)).clicked() {
                        send.push(ClientMsg::SetPassword { password: String::new() });
                        self.awaiting = Some(false);
                        keep = false;
                    }
                    if ui.add(Button::secondary(t.cancel)).clicked() {
                        keep = false;
                    }
                },
            );
            if !keep || r.close_requested {
                self.remove = false;
            }
        }
    }
}
