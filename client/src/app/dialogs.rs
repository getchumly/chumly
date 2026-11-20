//! Modal dialogs: admin setup code, kick/ban, channel edit/delete, certificate change, info.

use super::*;
use crate::icons;
use crate::theme::{self, Button, Dialog, Kind, pal};

/// Modal dialogs; each is open while its value is `Some`.
#[derive(Default)]
pub(super) struct Dialogs {
    /// Text of the owner setup code being entered.
    pub(super) admin_code: Option<String>,
    /// Kick or ban with a reason.
    pub(super) moderate: Option<ModDialog>,
    /// Create or edit a channel.
    pub(super) channel: Option<ChannelDialog>,
    /// Channel awaiting delete confirmation.
    pub(super) channel_delete: Option<(ChannelId, String)>,
    /// The server's certificate changed.
    pub(super) cert: Option<CertPrompt>,
    /// A message after a disconnect (rejected, kicked, failed).
    pub(super) info: Option<String>,
    /// When `info` closes, open this bookmark in the editor with the error
    /// under the field to fix (wrong password, nickname taken…).
    pub(super) then_edit: Option<(usize, super::servers::EditorField, String)>,
    /// Banned during the session or refused on connect for a ban.
    pub(super) banned: Option<super::bans_ui::BannedDialog>,
    /// The server refused us for incompatible versions: its version from
    /// the status probe ("" if unknown) for the dialog.
    pub(super) incompatible: Option<String>,
}

impl Dialogs {
    /// Closes the dialogs that act on the server we were connected to.
    pub(super) fn close_session_dialogs(&mut self) {
        self.admin_code = None;
        self.moderate = None;
        self.channel = None;
        self.channel_delete = None;
    }
}

pub(super) struct ModDialog {
    pub(super) user_id: UserId,
    pub(super) name: String,
    pub(super) ban: bool,
    pub(super) reason: String,
    /// Ban only: seconds, `None`: permanent.
    pub(super) duration: Option<u64>,
}

/// Admin creating (`id: None`) or editing a channel.
pub(super) struct ChannelDialog {
    pub(super) id: Option<ChannelId>,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) limited: bool,
    pub(super) max_users: u32,
}

pub(super) struct CertPrompt {
    pub(super) bookmark: usize,
    pub(super) saved: String,
    pub(super) actual: String,
}

impl App {
    pub(super) fn draw_dialogs(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let p = theme::palette(ctx.global_style().visuals.dark_mode);

        if let Some(mut code) = self.dialogs.admin_code.take() {
            let mut submit = false;
            let mut enter = false;
            let mut cancel = false;
            let r = Dialog::new("admin-setup", t.admin_login).fill_icon(icons::FILL_STAR, p.gold).show(
                ctx,
                |ui| {
                    ui.add(egui::Label::new(theme::text(t.admin_code, 13.0).color(p.t2)).wrap());
                    let edit = theme::text_field(&mut code)
                        .hint_text(theme::mono("XXXX-XXXX", 12.0).color(p.t3))
                        .font(egui::FontId::new(12.0, egui::FontFamily::Monospace))
                        .desired_width(ui.available_width());
                    let resp = ui.add(edit);
                    resp.request_focus();
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        enter = true;
                    }
                },
                |ui| {
                    submit |= ui.add(Button::primary(t.login)).clicked();
                    cancel |= ui.add(Button::secondary(t.cancel)).clicked();
                },
            );
            if submit || enter {
                self.send(ClientMsg::AdminSetup { code });
            } else if !cancel && !r.close_requested {
                self.dialogs.admin_code = Some(code);
            }
        }

        if let Some(mut d) = self.dialogs.moderate.take() {
            let mut submit = false;
            let mut enter = false;
            let mut cancel = false;
            let title = fill(if d.ban { t.ban_title } else { t.kick_title }, &[("name", &d.name)]);
            let icon = if d.ban { icons::PROHIBIT } else { icons::SIGN_OUT };
            let lang = self.cfg.language;
            let r = Dialog::new("moderate", title).icon(icon, p.danger).width(440.0).show(
                ctx,
                |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    if d.ban {
                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing.y = 8.0;
                            super::bans_ui::duration_picker(ui, t, lang, &mut d.duration);
                        });
                        ui.add_space(8.0);
                    }
                    theme::field_label(ui, t.reason);
                    let resp = ui.add(theme::text_field(&mut d.reason).desired_width(ui.available_width()));
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        enter = true;
                    }
                    let hint = if d.ban { t.reason_hint } else { t.kick_hint };
                    ui.label(theme::text(hint, 12.0).color(p.t3));
                },
                |ui| {
                    submit |= ui.add(Button::danger(if d.ban { t.ban } else { t.kick })).clicked();
                    cancel |= ui.add(Button::secondary(t.cancel)).clicked();
                },
            );
            if submit || enter {
                let reason = d.reason.trim().to_string();
                self.send(if d.ban {
                    ClientMsg::Ban { user_id: d.user_id, reason, duration_secs: d.duration }
                } else {
                    ClientMsg::Kick { user_id: d.user_id, reason }
                });
            } else if !cancel && !r.close_requested {
                self.dialogs.moderate = Some(d);
            }
        }

        if let Some(mut d) = self.dialogs.channel.take() {
            let mut submit = false;
            let mut cancel = false;
            let inside =
                d.id.and_then(|id| Some(self.session.as_ref()?.users.values().filter(|u| u.channel_id == id).count()))
                    .unwrap_or(0);
            let title = match d.id {
                Some(_) => t.edit_channel,
                None => t.create_channel,
            };
            let valid = shared::validate_channel_name(d.name.trim());
            let r = Dialog::new("channel-edit", title).width(440.0).closable().show(
                ctx,
                |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    theme::field_label(ui, t.name);
                    let resp = ui.add(
                        theme::text_field(&mut d.name)
                            .char_limit(shared::MAX_CHANNEL_NAME_CHARS)
                            .hint_text(theme::text("#", 13.0).color(p.t3))
                            .desired_width(ui.available_width()),
                    );
                    if d.id.is_none() && d.name.is_empty() {
                        resp.request_focus();
                    }
                    ui.add_space(6.0);
                    theme::field_label(ui, t.description);
                    ui.add(
                        theme::text_field(&mut d.description)
                            .char_limit(shared::MAX_CHANNEL_DESCRIPTION_CHARS)
                            .desired_width(ui.available_width()),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        theme::checkbox(ui, &mut d.limited, t.limit_users, None);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_enabled(d.limited, egui::DragValue::new(&mut d.max_users).range(1..=999).speed(0.2));
                        });
                    });
                    if d.limited && inside > d.max_users as usize {
                        ui.add_space(4.0);
                        theme::note(ui, theme::Note::Warn, &fill(t.limit_over, &[("n", &inside.to_string())]));
                    }
                },
                |ui| {
                    let label = if d.id.is_some() { t.save } else { t.create };
                    submit |= ui.add_enabled(valid, Button::primary(label)).clicked();
                    cancel |= ui.add(Button::secondary(t.cancel)).clicked();
                },
            );
            if submit {
                let name = d.name.trim().to_string();
                let description = d.description.trim().to_string();
                let max_users = d.limited.then_some(d.max_users);
                self.send(match d.id {
                    Some(channel_id) => ClientMsg::EditChannel { channel_id, name, description, max_users },
                    None => ClientMsg::CreateChannel { name, description, max_users },
                });
            } else if !cancel && !r.close_requested {
                self.dialogs.channel = Some(d);
            }
        }

        if let Some((id, name)) = self.dialogs.channel_delete.take() {
            let mut keep = true;
            let r = Dialog::new("channel-delete", fill(t.delete_channel_title, &[("name", &name)]))
                .icon(icons::TRASH, p.danger)
                .show(
                    ctx,
                    |ui| {
                        ui.add(egui::Label::new(theme::text(t.delete_channel_text, 13.0).color(p.t2)).wrap());
                    },
                    |ui| {
                        if ui.add(Button::danger(t.delete_channel)).clicked() {
                            self.send(ClientMsg::DeleteChannel { channel_id: id });
                            keep = false;
                        }
                        if ui.add(Button::secondary(t.cancel)).clicked() {
                            keep = false;
                        }
                    },
                );
            if keep && !r.close_requested {
                self.dialogs.channel_delete = Some((id, name));
            }
        }

        if let Some(prompt) = self.dialogs.cert.take() {
            let mut keep = true;
            let mut trust = false;
            let server = self
                .cfg
                .servers
                .get(prompt.bookmark)
                .map(|b| format!("{} ({})", b.name, b.address))
                .unwrap_or_default();
            let r = Dialog::new("cert-changed", t.cert_changed_title)
                .fill_icon(icons::FILL_SHIELD_WARNING, p.danger)
                .width(620.0)
                .danger()
                .show(
                    ctx,
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 14.0;
                        if !server.is_empty() {
                            ui.label(theme::text_semibold(&server, 13.0));
                        }
                        ui.add(egui::Label::new(theme::text(t.cert_changed_text, 13.0).color(p.t1)).wrap());
                        ui.columns(2, |cols| {
                            fingerprint_box(&mut cols[0], t.cert_saved, &prompt.saved, false);
                            fingerprint_box(&mut cols[1], t.cert_new, &prompt.actual, true);
                        });
                    },
                    |ui| {
                        // The safe action is the filled one; the risky one is outlined.
                        let cancel = Button::new(Kind::Primary, t.cancel).height(34.0);
                        if ui.add(cancel).clicked() {
                            keep = false;
                        }
                        let risky = Button::new(Kind::DangerOutline, t.trust_and_connect).height(34.0);
                        if ui.add(risky).clicked() {
                            trust = true;
                        }
                    },
                );
            if trust {
                if let Some(bm) = self.cfg.servers.get_mut(prompt.bookmark) {
                    bm.cert_fingerprint = Some(prompt.actual.clone());
                    self.save_config();
                    self.connect(prompt.bookmark);
                }
            } else if keep && !r.close_requested {
                self.dialogs.cert = Some(prompt);
            }
        }

        if let Some(server) = self.dialogs.incompatible.take() {
            let mut keep = true;
            let mut download = false;
            let newer: std::cell::RefCell<Option<String>> = Default::default();
            let r = Dialog::new("incompatible", t.disconnected).icon(icons::PLUGS, p.t2).width(440.0).show(
                ctx,
                |ui| *newer.borrow_mut() = self.incompatible_body(ui, &server),
                |ui| {
                    if let Some(v) = newer.borrow().as_ref() {
                        let dl =
                            Button::primary(fill(t.incompat_download, &[("v", v)])).with_icon(icons::ARROW_SQUARE_OUT);
                        download = ui.add(dl).clicked();
                    }
                    if ui.add(Button::secondary(t.ok).min_width(64.0)).clicked() {
                        keep = false;
                    }
                },
            );
            if download {
                self.open_update_download();
            }
            if keep && !r.close_requested {
                self.dialogs.incompatible = Some(server);
            }
        }

        if let Some(d) = self.dialogs.banned.take()
            && d.show(ctx, t, self.cfg.language)
        {
            self.dialogs.banned = Some(d);
        }

        if let Some(text) = self.dialogs.info.take() {
            let mut keep = true;
            let r = Dialog::new("disconnected", t.disconnected).icon(icons::PLUGS, p.t2).show(
                ctx,
                |ui| {
                    ui.add(egui::Label::new(theme::text(&text, 13.0)).wrap());
                },
                |ui| {
                    if ui.add(Button::secondary(t.ok).min_width(64.0)).clicked() {
                        keep = false;
                    }
                },
            );
            if keep && !r.close_requested {
                self.dialogs.info = Some(text);
            } else if let Some((index, field, error)) = self.dialogs.then_edit.take()
                && let Some(data) = self.cfg.servers.get(index).cloned()
            {
                self.server_list.editor =
                    Some(super::servers::BookmarkEditor { index: Some(index), data, error: Some((field, error)) });
            }
        }

        self.toasts.retain(|toast| {
            let ttl = if toast.kind == ToastKind::Error { TOAST_ERROR_TIME } else { TOAST_TIME };
            toast.at.elapsed() < ttl
        });
        if !self.toasts.is_empty() {
            // Above the chat input when connected, in the corner otherwise.
            let offset = if self.session.is_some() { [-14.0, -64.0] } else { [-16.0, -16.0] };
            egui::Area::new(egui::Id::unique("toasts"))
                .anchor(Align2::RIGHT_BOTTOM, offset)
                .order(egui::Order::Foreground)
                .interactable(false)
                .show(ctx, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                        ui.spacing_mut().item_spacing.y = 10.0;
                        for toast in &self.toasts {
                            theme::toast(ui, toast.kind, &toast.text);
                        }
                    });
                });
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

fn fingerprint_box(ui: &mut egui::Ui, title: &str, fingerprint: &str, new: bool) {
    let p = pal(ui);
    let (fill, stroke, title_color, text_color) = if new {
        (p.danger.gamma_multiply(0.08), p.danger.gamma_multiply(0.4), p.danger, p.t1)
    } else {
        (p.field, p.border, p.t3, p.t2)
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(egui::CornerRadius::same(theme::R_MD))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(RichText::new(title.to_uppercase()).family(theme::semibold()).size(11.0).color(title_color));
            ui.add(
                egui::Label::new(theme::mono(fingerprint, 12.0).color(text_color)).wrap_mode(egui::TextWrapMode::Wrap),
            );
        });
}
