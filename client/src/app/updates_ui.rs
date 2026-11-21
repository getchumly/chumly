//! Update notices (design sections 34–37): the banner on the start screen,
//! the dot on Settings while connected, "What's new", the Updates block in
//! Settings → General and the hint in the "incompatible versions" dialog.
//! The check itself is `crate::updates`.

use egui::text::{LayoutJob, TextFormat};
use egui::{CornerRadius, FontFamily, FontId, Margin, Rect, Sense, Stroke, pos2, vec2};
use shared::release::Release;

use super::*;
use crate::icons;
use crate::self_update::{Failure, Install};
use crate::theme::{self, Button, Dialog, pal};
use crate::updates::{self, Status};

/// The executable the user replaces: "chumly.exe" or "chumly".
fn exe_name() -> String {
    format!("{}{}", shared::APP_NAME.to_lowercase(), std::env::consts::EXE_SUFFIX)
}

/// "18 ноября 2026" / "18 Nov 2026" from "2026-11-18"; as is if unreadable.
fn fmt_date(lang: Language, date: &str) -> String {
    const RU: [&str; 12] = [
        "января",
        "февраля",
        "марта",
        "апреля",
        "мая",
        "июня",
        "июля",
        "августа",
        "сентября",
        "октября",
        "ноября",
        "декабря",
    ];
    const EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut it = date.split('-').map(|p| p.parse::<usize>().ok());
    match (it.next().flatten(), it.next().flatten(), it.next().flatten()) {
        (Some(y), Some(m @ 1..=12), Some(d)) => {
            let month = match lang {
                Language::Ru => RU[m - 1],
                Language::En => EN[m - 1],
            };
            format!("{d} {month} {y}")
        }
        _ => date.to_string(),
    }
}

/// A clickable text link; returns whether it was clicked.
fn link(ui: &mut egui::Ui, text: &str, size: f32, color: Color32, underline: bool) -> bool {
    let mut rt = RichText::new(text).size(size).color(color).family(theme::medium());
    if underline {
        rt = rt.underline();
    }
    let resp = ui.add(egui::Label::new(rt).sense(Sense::click()));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// The 6 px accent dot on a button's icon (an update is waiting).
pub(super) fn icon_dot(ui: &egui::Ui, button: Rect, ring: Color32) {
    let p = pal(ui);
    // The icon starts 12 px in and is about 15 px wide (see `theme::Button`).
    let c = pos2(button.left() + 12.0 + 14.0, button.center().y - 6.0);
    ui.painter().circle_filled(c, 5.0, ring);
    ui.painter().circle_filled(c, 3.0, p.accent);
}

impl App {
    /// The offered release (newer and not skipped).
    pub(super) fn update_offer(&self) -> Option<&Release> {
        self.updates.offer(self.cfg.skipped_version.as_deref())
    }

    fn open(&self, url: &str) {
        self.ctx.open_url(egui::OpenUrl::new_tab(url));
    }

    /// The newer release being offered or installed (skipped or not).
    fn update_release(&self) -> Option<Release> {
        match &self.updates.status {
            Status::Available(r) => Some(r.clone()),
            _ => self.updates.latest.clone().filter(|r| shared::release::is_newer(&r.version, shared::VERSION)),
        }
    }

    /// The Settings tooltip while an update waits: available or ready.
    pub(super) fn update_tooltip(&self) -> Option<String> {
        let t = self.t();
        if let Install::Ready { version } = self.updates.install.state() {
            return Some(fill(t.update_ready_tip, &[("v", &version)]));
        }
        self.update_offer().map(|r| fill(t.update_tip, &[("v", &r.version)]))
    }

    /// What the banner (section 45) or the Settings box (section 46) shows for
    /// the current state; `None` when there is nothing to show.
    fn update_view(&self, compact: bool) -> Option<UpdateView> {
        let t = self.t();
        let lang = self.cfg.language;
        let v = match self.updates.install.state() {
            Install::Idle => {
                let r = self.update_offer()?;
                let mut links = vec![(t.whats_new, true, Act::Notes)];
                if !compact {
                    links.push((t.skip_version, false, Act::Skip));
                }
                UpdateView {
                    glyph: Glyph::Icon(icons::FILL_ARROW_CIRCLE_DOWN, true),
                    title: if compact {
                        fill(t.update_available_short, &[("v", &r.version)])
                    } else {
                        fill(t.update_title, &[("v", &r.version)])
                    },
                    sub: (!compact).then(|| fill(t.update_you_have, &[("v", shared::VERSION)])),
                    closable: true,
                    primary: Some((t.update_and_restart, Act::Install)),
                    links,
                    ..UpdateView::new(Tone::Accent)
                }
            }
            Install::Downloading { done, total } => {
                let title = match total {
                    Some(total) => {
                        fill(t.update_downloading, &[("done", &mb(done, lang, 1)), ("total", &mb(total, lang, 0))])
                    }
                    None => fill(t.update_downloading_unknown, &[("done", &mb(done, lang, 1))]),
                };
                let progress = total.filter(|t| *t > 0).map(|total| {
                    let f = (done as f32 / total as f32).clamp(0.0, 1.0);
                    (f, format!("{:.0}%", f * 100.0))
                });
                UpdateView {
                    glyph: Glyph::Spinner,
                    title,
                    progress,
                    secondary: Some((t.cancel, icons::X, Act::Cancel)),
                    ..UpdateView::new(Tone::Accent)
                }
            }
            Install::Checking { total } => UpdateView {
                glyph: Glyph::Spinner,
                title: t.update_checking_file.to_string(),
                sub: (!compact).then(|| t.update_checking_sub.to_string()),
                progress: Some((1.0, format!("{} {}", mb(total, lang, 0), t.megabytes))),
                ..UpdateView::new(Tone::Accent)
            },
            Install::Ready { version } => UpdateView {
                glyph: Glyph::Icon(icons::FILL_CHECK_CIRCLE, true),
                title: fill(t.update_ready, &[("v", &version)]),
                closable: true,
                primary: Some((t.update_restart, Act::Restart)),
                note: Some(t.update_later_note),
                ..UpdateView::new(Tone::Accent)
            },
            Install::Failed(Failure::Network) => UpdateView {
                glyph: Glyph::Icon(icons::CLOUD_SLASH, false),
                title: t.update_err_net.to_string(),
                sub: (!compact).then(|| t.update_err_net_sub.to_string()),
                closable: true,
                secondary: Some((t.update_retry, icons::ARROWS_CLOCKWISE, Act::Install)),
                links: vec![(t.update_manual, false, Act::Manual)],
                ..UpdateView::new(Tone::Neutral)
            },
            Install::Failed(Failure::Signature) => UpdateView {
                glyph: Glyph::Icon(icons::FILL_SHIELD_WARNING, true),
                title: t.update_err_sig.to_string(),
                sub: (!compact).then(|| t.update_err_sig_sub.to_string()),
                closable: true,
                secondary: Some((t.update_manual, icons::ARROW_SQUARE_OUT, Act::Manual)),
                ..UpdateView::new(Tone::Warn)
            },
            Install::Failed(Failure::Folder) => UpdateView {
                glyph: Glyph::Icon(icons::FOLDER_LOCK, false),
                title: t.update_err_dir.to_string(),
                sub: (!compact).then(|| {
                    fill(t.update_err_dir_sub, &[("dir", &self.updates.install.folder()), ("file", &exe_name())])
                }),
                closable: true,
                secondary: Some((t.update_manual, icons::ARROW_SQUARE_OUT, Act::Manual)),
                ..UpdateView::new(Tone::Neutral)
            },
        };
        Some(v)
    }

    /// Carries out a click on the banner, the Settings box or "What's new".
    fn update_act(&mut self, act: Act) {
        match act {
            Act::Install => {
                if let Some(r) = self.update_release() {
                    let ctx = self.ctx.clone();
                    self.updates.install.start(&r, &ctx);
                }
            }
            Act::Restart => {
                // In a call nothing restarts without a confirmation (section 47).
                if self.session.is_some() {
                    self.updates.confirm_restart = true;
                } else {
                    self.restart_for_update();
                }
            }
            Act::Cancel => self.updates.install.cancel(),
            Act::Manual => {
                if let Some(r) = self.update_release() {
                    self.open(updates::download_url(&r));
                }
            }
            Act::Notes => self.updates.notes_open = true,
            Act::Skip => {
                if let Some(r) = self.update_offer() {
                    self.cfg.skipped_version = Some(r.version.clone());
                    self.cfg_dirty = true;
                }
            }
        }
    }

    /// Restarts into the installed update: disconnects like closing the app
    /// does, so the new instance can connect with the same nickname, saves
    /// the settings, starts the new executable and closes this one.
    fn restart_for_update(&mut self) {
        if self.conn != ConnState::Idle {
            let _ = self.net.send(NetCommand::Disconnect);
            std::thread::sleep(Duration::from_millis(400));
        }
        self.save_config();
        match self.updates.install.restart() {
            Ok(()) => self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => {
                tracing::error!("restart after update: {e}");
                let msg = fill(self.t().restart_failed, &[("err", &e.to_string())]);
                self.toast_err(msg);
            }
        }
    }

    /// The start screen banner above the server list (section 45), or the
    /// one-time "Updated to …" line after an update (section 48).
    pub(super) fn update_banner(&mut self, ui: &mut egui::Ui) {
        if self.updates.just_updated {
            self.updated_line(ui);
            return;
        }
        if self.updates.banner_closed {
            return;
        }
        let Some(view) = self.update_view(false) else { return };
        match draw_update_view(ui, &view, false) {
            Some(Click::Close) => {
                // × on Ready hides the banner; the update still runs on the next start.
                self.updates.install.dismiss();
                self.updates.banner_closed = true;
            }
            Some(Click::Act(act)) => self.update_act(act),
            None => {}
        }
    }

    /// "✓ Updated to 0.3.0 · What's new ×" in the banner slot (section 48).
    fn updated_line(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let p = pal(ui);
        let (bg, border) = Tone::Accent.colors(ui);
        let mut notes = false;
        let mut close = false;
        egui::Frame::new()
            .fill(bg)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(CornerRadius::same(theme::R_MD))
            .inner_margin(Margin { left: 12, right: 6, top: 0, bottom: 0 })
            .show(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
                let mut row = ui.new_child(
                    egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                row.spacing_mut().item_spacing.x = 10.0;
                row.label(
                    RichText::new(icons::FILL_CHECK_CIRCLE).family(theme::icons_fill()).size(18.0).color(p.accent_text),
                );
                row.label(theme::text_semibold(fill(t.updated_to, &[("v", shared::VERSION)]), 13.0));
                notes = link(&mut row, t.whats_new, 12.5, p.accent_text, false);
                row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.add(Button::icon(icons::X).height(28.0)).clicked();
                });
            });
        if notes {
            // The notes of this version, if the check already fetched them.
            if self.updates.latest.as_ref().is_some_and(|r| r.version == shared::VERSION) {
                self.updates.notes_open = true;
            } else {
                self.open(&updates::current_release_page());
            }
        }
        if close {
            self.updates.just_updated = false;
        }
    }

    /// The restart confirmation while connected (section 47). Later is the
    /// default: the safe choice is not to drop the call.
    pub(super) fn draw_restart_confirm(&mut self, ctx: &egui::Context) {
        if !self.updates.confirm_restart {
            return;
        }
        let Some(server) = self.session.as_ref().map(|s| s.server_name.clone()) else {
            // Disconnected meanwhile: nothing to confirm.
            self.updates.confirm_restart = false;
            self.restart_for_update();
            return;
        };
        let t = self.t();
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let mut later = false;
        let mut restart = false;
        let resp = Dialog::new("restart-confirm", t.restart_title)
            .icon(icons::ARROWS_CLOCKWISE, p.accent_text)
            .width(440.0)
            .closable()
            .show(
                ctx,
                |ui| {
                    ui.spacing_mut().item_spacing.y = 12.0;
                    let mut job = LayoutJob::default();
                    let font = FontId::new(13.0, FontFamily::Proportional);
                    job.append(&format!("{} ", t.restart_lead), 0.0, TextFormat::simple(font.clone(), p.t1));
                    job.append(&server, 0.0, TextFormat::simple(FontId::new(13.0, theme::semibold()), p.t1));
                    job.append(".", 0.0, TextFormat::simple(font, p.t1));
                    job.wrap.max_width = ui.available_width();
                    ui.label(job);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(theme::text(icons::INFO, 15.0).color(p.t3));
                        ui.add(egui::Label::new(theme::text(t.restart_sub, 12.5).color(p.t2)).wrap());
                    });
                },
                |ui| {
                    later = ui.add(Button::primary(t.restart_later)).clicked();
                    restart = ui.add(Button::secondary(t.restart_now).with_icon(icons::ARROWS_CLOCKWISE)).clicked();
                },
            );
        if later || resp.close_requested {
            self.updates.confirm_restart = false;
        }
        if restart {
            self.updates.confirm_restart = false;
            self.restart_for_update();
        }
    }

    /// "What's new in 0.2.0": the release notes of the offered release, or of
    /// this version after an update. Update and restart, Close, GitHub.
    pub(super) fn draw_whats_new(&mut self, ctx: &egui::Context) {
        if !self.updates.notes_open {
            return;
        }
        let r = match &self.updates.status {
            Status::Available(r) => Some(r.clone()),
            _ => self.updates.latest.clone().filter(|r| r.version == shared::VERSION),
        };
        let Some(r) = r else {
            self.updates.notes_open = false;
            return;
        };
        let t = self.t();
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let lang = self.cfg.language;
        let newer = shared::release::is_newer(&r.version, shared::VERSION);
        let action = match self.updates.install.state() {
            Install::Idle | Install::Failed(_) if newer => Some((t.update_and_restart, Act::Install)),
            Install::Ready { .. } => Some((t.update_restart, Act::Restart)),
            _ => None,
        };
        let mut close = false;
        let mut act = None;
        let mut page = false;
        let resp =
            Dialog::new("whats-new", fill(t.whats_new_title, &[("v", &r.version)])).width(520.0).closable().show(
                ctx,
                |ui| {
                    let meta = if newer {
                        format!("{} → {} · {}", shared::VERSION, r.version, fmt_date(lang, &r.date))
                    } else {
                        format!("{} · {}", r.version, fmt_date(lang, &r.date))
                    };
                    ui.label(theme::mono(meta, 12.0).color(p.t3));
                    egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                        markdown(ui, &r.notes);
                    });
                },
                |ui| {
                    if let Some((label, a)) = action
                        && ui.add(Button::primary(label).with_icon(icons::ARROWS_CLOCKWISE)).clicked()
                    {
                        act = Some(a);
                    }
                    close = ui.add(Button::secondary(t.close)).clicked();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        page = link(ui, t.open_on_github, 12.0, p.t3, false);
                    });
                },
            );
        if page {
            self.open(&r.page);
        }
        if let Some(a) = act {
            self.updates.notes_open = false;
            self.update_act(a);
        }
        if close || resp.close_requested {
            self.updates.notes_open = false;
        }
    }

    /// Settings → General → Updates: version, the auto-check switch, Check now
    /// and the state of the last check.
    pub(super) fn settings_updates(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let p = pal(ui);
        let waiting = self.update_tooltip().is_some();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(theme::text_semibold(t.updates_title, 13.0));
            if waiting {
                let (r, _) = ui.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.0, p.accent);
            }
        });
        egui::Grid::new("settings-updates")
            .num_columns(2)
            .spacing([14.0, 12.0])
            .min_col_width(150.0)
            .striped(false)
            .show(ui, |ui| {
                ui.label(theme::text(t.current_version, 13.0).color(p.t2));
                ui.label(theme::mono(shared::VERSION, 12.5).color(p.t1));
                ui.end_row();
                ui.label(theme::text(t.check_updates, 13.0).color(p.t2));
                if theme::toggle(ui, &mut self.cfg.check_updates).changed() {
                    self.cfg_dirty = true;
                }
                ui.end_row();
            });
        ui.add(egui::Label::new(theme::text(t.updates_privacy, 12.0).color(p.t3)).wrap());
        let mut action: Option<UpdateClick> = None;
        // The offered update and its install, the same state as the banner.
        if let Some(view) = self.update_view(true)
            && let Some(Click::Act(act)) = draw_update_view(ui, &view, true)
        {
            action = Some(UpdateClick::Box(act));
        }
        let checking = matches!(self.updates.status, Status::Checking) || self.updates.install.busy();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let now = Button::secondary(t.check_now).with_icon(icons::ARROWS_CLOCKWISE).small();
            if ui.add_enabled(!checking, now).clicked() {
                action = Some(UpdateClick::Check);
            }
            ui.spacing_mut().item_spacing.x = 7.0;
            let skipped = self.cfg.skipped_version.as_deref();
            match &self.updates.status {
                Status::Checking => {
                    ui.add(egui::Spinner::new().size(13.0).color(p.t2));
                    ui.label(theme::text(t.update_checking, 12.5).color(p.t2));
                }
                Status::UpToDate => {
                    ui.label(theme::text(icons::CHECK_CIRCLE, 15.0).color(p.accent_text));
                    ui.label(theme::text(t.update_latest, 12.5).color(p.t2));
                }
                Status::Available(r) if skipped == Some(r.version.as_str()) => {
                    ui.label(theme::text(icons::EYE_SLASH, 15.0).color(p.t3));
                    ui.label(theme::text(fill(t.update_skipped, &[("v", &r.version)]), 12.5).color(p.t2));
                    if link(ui, t.update_unskip, 12.5, p.accent_text, false) {
                        action = Some(UpdateClick::Unskip);
                    }
                }
                // Shown in the box above.
                Status::Available(_) => {}
                Status::Failed => {
                    ui.label(theme::text(icons::CLOUD_SLASH, 15.0).color(p.t3));
                    ui.label(theme::text(t.update_failed, 12.5).color(p.t2));
                }
                Status::Idle if !self.cfg.check_updates => {
                    ui.label(theme::text(t.update_auto_off, 12.5).color(p.t3));
                }
                Status::Idle => {}
            }
        });
        match action {
            Some(UpdateClick::Check) => {
                let ctx = ui.ctx().clone();
                self.updates.check(&ctx);
            }
            Some(UpdateClick::Unskip) => {
                self.cfg.skipped_version = None;
                self.cfg_dirty = true;
            }
            Some(UpdateClick::Box(act)) => self.update_act(act),
            None => {}
        }
    }

    /// The hint in the "incompatible versions" dialog: both versions, and
    /// Download when there is a newer client, or "ask the owner" when the
    /// server is the older one. `server`: its version from the status
    /// probe, empty if unknown.
    pub(super) fn incompatible_body(&self, ui: &mut egui::Ui, server: &str) -> Option<String> {
        let t = self.t();
        let p = pal(ui);
        ui.add(egui::Label::new(theme::text(t.incompatible_short, 13.0)).wrap());
        egui::Frame::new()
            .fill(p.field)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(theme::R_SM))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new("incompatible-versions").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    let majors: Vec<String> = shared::PROTOCOL_MAJORS.iter().map(|m| format!("chumly/{m}")).collect();
                    ui.label(theme::text(t.incompat_you, 12.0).color(p.t3));
                    ui.label(theme::mono(format!("{} · {}", shared::VERSION, majors.join(", ")), 12.0).color(p.t2));
                    ui.end_row();
                    ui.label(theme::text(t.incompat_server, 12.0).color(p.t3));
                    ui.label(theme::mono(if server.is_empty() { "?" } else { server }, 12.0).color(p.t2));
                    ui.end_row();
                });
            });
        let server_newer = !server.is_empty() && shared::release::is_newer(server, shared::VERSION);
        let server_older = !server.is_empty() && shared::release::is_newer(shared::VERSION, server);
        // A newer client is offered even if the user skipped it: they need it now.
        let newer = match &self.updates.status {
            Status::Available(r) if !server_older => Some(r),
            _ => None,
        };
        let (icon, text, accent) = match newer {
            Some(r) => (icons::FILL_ARROW_CIRCLE_DOWN, fill(t.incompat_update, &[("v", &r.version)]), true),
            None if server_older => (icons::INFO, t.incompat_server_old.to_string(), false),
            None if server_newer => (icons::INFO, t.incompat_client_old.to_string(), false),
            None => return None,
        };
        let (bg, border, icon_color) = if accent {
            (p.accent.gamma_multiply(0.1), p.accent.gamma_multiply(0.35), p.accent_text)
        } else {
            (p.field, p.border, p.t3)
        };
        egui::Frame::new()
            .fill(bg)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(CornerRadius::same(theme::R_SM))
            .inner_margin(Margin::symmetric(11, 9))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let family = if accent { theme::icons_fill() } else { FontFamily::Proportional };
                    ui.label(RichText::new(icon).family(family).size(15.0).color(icon_color));
                    ui.add(egui::Label::new(theme::text(text, 12.5).color(p.t1)).wrap());
                });
            });
        newer.map(|r| r.version.clone())
    }

    /// Opens the download of the offered (or skipped) newer release.
    pub(super) fn open_update_download(&self) {
        if let Status::Available(r) = &self.updates.status {
            self.open(updates::download_url(r));
        }
    }
}

enum UpdateClick {
    Check,
    Unskip,
    Box(Act),
}

/// The tint of the update banner by state (section 45): good news is the
/// accent, a network or folder problem is neutral, a failed signature warns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Accent,
    Neutral,
    Warn,
}

impl Tone {
    /// Fill and border.
    fn colors(self, ui: &egui::Ui) -> (Color32, Color32) {
        let p = pal(ui);
        let dark = ui.visuals().dark_mode;
        match self {
            Tone::Accent if dark => (p.accent.gamma_multiply(0.08), p.accent.gamma_multiply(0.35)),
            Tone::Accent => (p.accent.gamma_multiply(0.12), p.accent_text.gamma_multiply(0.3)),
            Tone::Neutral => (p.field, p.border),
            Tone::Warn if dark => (p.warn.gamma_multiply(0.08), p.warn.gamma_multiply(0.45)),
            Tone::Warn => (p.warn.gamma_multiply(0.07), p.warn.gamma_multiply(0.4)),
        }
    }

    fn icon(self, ui: &egui::Ui, compact: bool) -> Color32 {
        let p = pal(ui);
        match self {
            Tone::Accent => p.accent_text,
            Tone::Neutral if compact => p.t3,
            Tone::Neutral => p.t2,
            Tone::Warn => p.warn,
        }
    }
}

enum Glyph {
    Spinner,
    /// An icon; `true`: from the Fill set.
    Icon(&'static str, bool),
}

/// What a button or link of the update banner does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    Install,
    Restart,
    Cancel,
    Manual,
    Notes,
    Skip,
}

enum Click {
    Close,
    Act(Act),
}

/// One state of the update banner / Settings box: icon slot, title (and a
/// second line), progress, actions. The layout is the same in every state, so
/// it does not jump when the state changes.
struct UpdateView {
    tone: Tone,
    glyph: Glyph,
    title: String,
    sub: Option<String>,
    /// × in the corner (the banner only).
    closable: bool,
    /// Done fraction and the label right of the bar.
    progress: Option<(f32, String)>,
    primary: Option<(&'static str, Act)>,
    secondary: Option<(&'static str, &'static str, Act)>,
    /// Text links: label, strong (accent) or quiet (underlined), action.
    links: Vec<(&'static str, bool, Act)>,
    note: Option<&'static str>,
}

impl UpdateView {
    fn new(tone: Tone) -> Self {
        Self {
            tone,
            glyph: Glyph::Spinner,
            title: String::new(),
            sub: None,
            closable: false,
            progress: None,
            primary: None,
            secondary: None,
            links: Vec::new(),
            note: None,
        }
    }
}

/// "6.2" / "6,2": megabytes with `decimals` digits in the language's style.
fn mb(bytes: u64, lang: Language, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, bytes as f64 / (1024.0 * 1024.0));
    match lang {
        Language::Ru => s.replace('.', ","),
        Language::En => s,
    }
}

/// Draws the update banner (section 45) or, `compact`, the box in Settings
/// (section 46).
fn draw_update_view(ui: &mut egui::Ui, v: &UpdateView, compact: bool) -> Option<Click> {
    let p = pal(ui);
    let (bg, border) = v.tone.colors(ui);
    let icon_color = v.tone.icon(ui, compact);
    let (slot, gap, indent) = if compact { (15.0, 8.0, 21.0) } else { (20.0, 12.0, 32.0) };
    let closable = v.closable && !compact;
    let mut click = None;
    let margin = if compact { Margin::symmetric(12, 10) } else { Margin { left: 14, right: 8, top: 12, bottom: 12 } };
    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(theme::R_MD))
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = if compact { 8.0 } else { 9.0 };
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let (r, _) = ui.allocate_exact_size(vec2(slot, slot + 2.0), Sense::hover());
                match v.glyph {
                    Glyph::Spinner => {
                        let s = if compact { 13.0 } else { 16.0 };
                        egui::Spinner::new()
                            .size(s)
                            .color(p.accent_text)
                            .paint_at(ui, Rect::from_center_size(r.center(), vec2(s, s)));
                    }
                    Glyph::Icon(g, filled) => {
                        let family = if filled { theme::icons_fill() } else { FontFamily::Proportional };
                        let g = ui.painter().layout_no_wrap(g.to_owned(), FontId::new(slot, family), icon_color);
                        ui.painter().galley(r.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
                    }
                }
                let text_w = (ui.available_width() - if closable { 28.0 + gap } else { 0.0 }).max(60.0);
                ui.vertical(|ui| {
                    ui.set_width(text_w);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    let title = if compact {
                        let color = if v.tone == Tone::Neutral { p.t2 } else { p.t1 };
                        theme::text(&v.title, 12.5).color(color)
                    } else {
                        theme::text_semibold(&v.title, 13.0)
                    };
                    ui.add(egui::Label::new(title).wrap());
                    if let Some(sub) = &v.sub {
                        ui.add(egui::Label::new(theme::text(sub, 12.0).color(p.t2)).wrap());
                    }
                });
                if closable {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui.add(Button::icon(icons::X).height(28.0)).clicked() {
                            click = Some(Click::Close);
                        }
                    });
                }
            });
            if let Some((done, label)) = &v.progress {
                let thick: f32 = if compact { 4.0 } else { 6.0 };
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), thick.max(14.0)), Sense::hover());
                let mut right = row.right() - if compact { 0.0 } else { 6.0 };
                if !compact {
                    let g = ui.painter().layout_no_wrap(label.clone(), FontId::new(11.0, FontFamily::Monospace), p.t2);
                    right -= g.size().x;
                    ui.painter().galley(pos2(right, row.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
                    right -= 10.0;
                }
                let track = Rect::from_min_max(
                    pos2(row.left() + indent, row.center().y - thick / 2.0),
                    pos2(right, row.center().y + thick / 2.0),
                );
                let radius = CornerRadius::same((thick / 2.0) as u8);
                ui.painter().rect_filled(track, radius, p.sel);
                let mut bar = track;
                bar.set_width(track.width() * done);
                ui.painter().rect_filled(bar, radius, p.accent);
            }
            let has_actions = v.primary.is_some() || v.secondary.is_some() || !v.links.is_empty() || v.note.is_some();
            if has_actions {
                egui::Frame::new().inner_margin(Margin { left: indent as i8, ..Default::default() }).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                        let (h, size) = if compact { (28.0, 12.5) } else { (30.0, 13.0) };
                        if let Some((label, act)) = v.primary {
                            let b = Button::primary(label).with_icon(icons::ARROWS_CLOCKWISE).height(h).font_size(size);
                            if ui.add(b).clicked() {
                                click = Some(Click::Act(act));
                            }
                        }
                        if let Some((label, icon, act)) = v.secondary {
                            let b = Button::secondary(label).with_icon(icon).height(h).font_size(size);
                            if ui.add(b).clicked() {
                                click = Some(Click::Act(act));
                            }
                        }
                        for (label, strong, act) in &v.links {
                            let color = if *strong { p.accent_text } else { p.t2 };
                            if link(ui, label, 12.5, color, !strong) {
                                click = Some(Click::Act(*act));
                            }
                        }
                        if let Some(note) = v.note {
                            ui.label(theme::text(note, 12.0).color(p.t3));
                        }
                    });
                });
            }
        });
    click
}

/// A subset of markdown for release notes: `#` headings, `-` / `*` bullets
/// (indented lines continue the bullet), `code`; links show their text,
/// `**` is dropped. Everything else is plain text.
fn markdown(ui: &mut egui::Ui, text: &str) {
    let p = pal(ui);
    let width = ui.available_width();
    ui.spacing_mut().item_spacing.y = 6.0;
    let mut blocks: Vec<(bool, bool, String)> = Vec::new(); // (heading, bullet, text)
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(h) = trimmed.strip_prefix('#') {
            blocks.push((true, false, h.trim_start_matches('#').trim().to_string()));
        } else if let Some(b) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            blocks.push((false, true, b.to_string()));
        } else if line.starts_with(' ') && blocks.last().is_some_and(|b| b.1) {
            let last = blocks.last_mut().expect("checked");
            last.2.push(' ');
            last.2.push_str(trimmed);
        } else {
            blocks.push((false, false, trimmed.to_string()));
        }
    }
    for (i, (heading, bullet, text)) in blocks.iter().enumerate() {
        if *heading {
            if i > 0 {
                ui.add_space(4.0);
            }
            ui.label(theme::text_semibold(strip_inline(text), 14.0));
        } else if *bullet {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(4.0, 18.0), Sense::hover());
                ui.painter().circle_filled(pos2(r.center().x, r.top() + 9.0), 2.0, p.t3);
                ui.add(egui::Label::new(inline(ui, text, width - 12.0)).wrap());
            });
        } else {
            ui.add(egui::Label::new(inline(ui, text, width)).wrap());
        }
    }
}

/// Links as their text, no `**`.
fn strip_inline(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s.replace("**", "");
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](").map(|m| open + m) else { break };
        let Some(close) = rest[mid..].find(')').map(|c| mid + c) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..mid]);
        rest = rest[close + 1..].to_string();
    }
    out.push_str(&rest);
    out
}

/// A paragraph with `code` spans.
fn inline(ui: &egui::Ui, s: &str, width: f32) -> LayoutJob {
    let p = pal(ui);
    let text = strip_inline(s);
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let plain = TextFormat::simple(FontId::new(13.0, FontFamily::Proportional), p.t1);
    let code = TextFormat { background: p.sel, ..TextFormat::simple(FontId::new(11.5, FontFamily::Monospace), p.t1) };
    for (i, part) in text.split('`').enumerate() {
        if !part.is_empty() {
            job.append(part, 0.0, if i % 2 == 1 { code.clone() } else { plain.clone() });
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_inline_markdown() {
        assert_eq!(fmt_date(Language::Ru, "2026-11-18"), "18 ноября 2026");
        assert_eq!(fmt_date(Language::En, "2026-01-05"), "5 Jan 2026");
        assert_eq!(fmt_date(Language::En, ""), "");
        assert_eq!(strip_inline("**Update:** see [the guide](https://x) now"), "Update: see the guide now");
    }
}
