//! Update notices (design sections 34–37): the banner on the start screen,
//! the dot on Settings while connected, "What's new", the Updates block in
//! Settings → General and the hint in the "incompatible versions" dialog.
//! The check itself is `crate::updates`.

use egui::text::{LayoutJob, TextFormat};
use egui::{CornerRadius, FontFamily, FontId, Margin, Rect, Sense, Stroke, pos2, vec2};
use shared::release::Release;

use super::*;
use crate::icons;
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

    /// The start screen banner above the server list: "Chumly 0.2.0 is
    /// available", What's new, Skip, Download, ×.
    pub(super) fn update_banner(&mut self, ui: &mut egui::Ui) {
        if self.updates.banner_closed {
            return;
        }
        let Some(r) = self.update_offer().cloned() else { return };
        let t = self.t();
        let p = pal(ui);
        let dark = ui.visuals().dark_mode;
        let (bg, border) = if dark {
            (p.accent.gamma_multiply(0.08), p.accent.gamma_multiply(0.35))
        } else {
            (p.accent.gamma_multiply(0.12), p.accent_text.gamma_multiply(0.3))
        };
        let mut skip = false;
        let mut close = false;
        let mut notes = false;
        let mut download = false;
        egui::Frame::new()
            .fill(bg)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(CornerRadius::same(theme::R_MD))
            .inner_margin(Margin { left: 14, right: 8, top: 10, bottom: 10 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    ui.label(
                        RichText::new(icons::FILL_ARROW_CIRCLE_DOWN)
                            .family(theme::icons_fill())
                            .size(20.0)
                            .color(p.accent_text),
                    );
                    let right_w = 28.0 + 8.0 + theme::button_width(ui, t.download, Some(icons::ARROW_SQUARE_OUT));
                    ui.allocate_ui(vec2((ui.available_width() - right_w - 12.0).max(80.0), 40.0), |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(theme::text_semibold(fill(t.update_title, &[("v", &r.version)]), 13.0));
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let have = format!("{} · ", fill(t.update_you_have, &[("v", shared::VERSION)]));
                                ui.label(theme::text(have, 12.0).color(p.t2));
                                notes = link(ui, t.whats_new, 12.0, p.accent_text, false);
                                ui.label(theme::text(" · ", 12.0).color(p.t2));
                                skip = link(ui, t.skip_version, 12.0, p.t2, true);
                            });
                        });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if ui.add(Button::icon(icons::X).height(28.0)).clicked() {
                            close = true;
                        }
                        download =
                            ui.add(Button::primary(t.download).with_icon(icons::ARROW_SQUARE_OUT).small()).clicked();
                    });
                });
            });
        if download {
            self.open(updates::download_url(&r));
        }
        if notes {
            self.updates.notes_open = true;
        }
        if skip {
            self.cfg.skipped_version = Some(r.version.clone());
            self.cfg_dirty = true;
        }
        if close {
            self.updates.banner_closed = true;
        }
    }

    /// "What's new in 0.2.0": the release notes, Download, View on GitHub.
    pub(super) fn draw_whats_new(&mut self, ctx: &egui::Context) {
        if !self.updates.notes_open {
            return;
        }
        let r = match &self.updates.status {
            Status::Available(r) => r.clone(),
            _ => {
                self.updates.notes_open = false;
                return;
            }
        };
        let t = self.t();
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let lang = self.cfg.language;
        let mut close = false;
        let mut download = false;
        let mut page = false;
        let resp =
            Dialog::new("whats-new", fill(t.whats_new_title, &[("v", &r.version)])).width(520.0).closable().show(
                ctx,
                |ui| {
                    let meta = format!("{} → {} · {}", shared::VERSION, r.version, fmt_date(lang, &r.date));
                    ui.label(theme::mono(meta, 12.0).color(p.t3));
                    egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                        markdown(ui, &r.notes);
                    });
                    egui::Frame::new()
                        .fill(p.bg2)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(theme::R_SM))
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                ui.label(theme::text(icons::INFO, 14.0).color(p.t3));
                                let hint = fill(t.update_replace_hint, &[("file", &exe_name())]);
                                ui.add(egui::Label::new(theme::text(hint, 12.0).color(p.t2)).wrap());
                            });
                        });
                },
                |ui| {
                    download = ui.add(Button::primary(t.download).with_icon(icons::ARROW_SQUARE_OUT)).clicked();
                    close = ui.add(Button::secondary(t.close)).clicked();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        page = link(ui, t.open_on_github, 12.0, p.t3, false);
                    });
                },
            );
        if download {
            self.open(updates::download_url(&r));
        }
        if page {
            self.open(&r.page);
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
        let waiting = self.update_offer().is_some();
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
        let checking = matches!(self.updates.status, Status::Checking);
        let mut action: Option<UpdateClick> = None;
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
                Status::Available(r) => {
                    ui.label(
                        RichText::new(icons::FILL_ARROW_CIRCLE_DOWN)
                            .family(theme::icons_fill())
                            .size(15.0)
                            .color(p.accent_text),
                    );
                    ui.label(theme::text(fill(t.update_newer, &[("v", &r.version)]), 12.5).color(p.t1));
                    if link(ui, t.download, 12.5, p.accent_text, false) {
                        action = Some(UpdateClick::Download(updates::download_url(r).to_string()));
                    }
                    ui.label(theme::text("·", 12.5).color(p.t3));
                    if link(ui, t.whats_new, 12.5, p.accent_text, false) {
                        action = Some(UpdateClick::Notes);
                    }
                }
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
            Some(UpdateClick::Download(url)) => self.open(&url),
            Some(UpdateClick::Notes) => self.updates.notes_open = true,
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
    Download(String),
    Notes,
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
