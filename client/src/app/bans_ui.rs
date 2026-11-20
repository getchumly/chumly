//! Temporary bans in the client: the duration in the
//! ban dialog, the dialog a banned user sees, the bans tab of the admin panel.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{CornerRadius, FontFamily, FontId, Margin, Rect, Sense, Stroke, pos2, vec2};

use super::admin_panel::short_key;
use super::*;
use crate::icons;
use crate::theme::{self, Button, Dialog, pal};
use crate::timefmt;

/// The duration preselected in the ban dialog: one day.
pub(super) const DEFAULT_BAN: Option<u64> = Some(86_400);

/// Ban durations in the dialog, `None`: permanent.
fn options(t: &Tr) -> [(Option<u64>, &'static str); 5] {
    let [h, d, w, m] = shared::BAN_DURATIONS;
    [(Some(h), t.dur_hour), (Some(d), t.dur_day), (Some(w), t.dur_week), (Some(m), t.dur_month), (None, t.dur_forever)]
}

/// A segmented control over the full width, then a line that
/// spells out the result in local time ("Until Fri, 21 Nov, 15:00").
pub(super) fn duration_picker(ui: &mut egui::Ui, t: &Tr, lang: Language, value: &mut Option<u64>) {
    let p = pal(ui);
    theme::field_label(ui, t.ban_duration);
    let opts = options(t);
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    ui.painter().rect(outer, CornerRadius::same(7), p.field, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    let inner = outer.shrink(3.0);
    let gap = 2.0;
    let texts: Vec<String> = opts
        .iter()
        .map(|(opt, label)| if opt.is_none() { format!("{} {label}", icons::INFINITY) } else { (*label).to_owned() })
        .collect();
    // Each segment gets its text (measured in the bold, selected weight, so
    // nothing moves on selection) plus padding; the rest of the width is
    // shared equally. Equal fifths left "∞ Навсегда" touching the edge.
    let natural: Vec<f32> = texts
        .iter()
        .map(|s| ui.painter().layout_no_wrap(s.clone(), FontId::new(12.5, theme::semibold()), p.t1).size().x + 16.0)
        .collect();
    let spare = (inner.width() - gap * (opts.len() - 1) as f32 - natural.iter().sum::<f32>()).max(0.0);
    let extra = spare / opts.len() as f32;
    let mut x = inner.left();
    for (i, ((opt, _), text)) in opts.iter().zip(texts).enumerate() {
        let w = natural[i] + extra;
        let rect = Rect::from_min_size(pos2(x, inner.top()), vec2(w, inner.height()));
        x += w + gap;
        let resp = ui.interact(rect, ui.scope_id().with(("ban-duration", i)), Sense::click());
        let selected = *value == *opt;
        if selected {
            ui.painter().rect(rect, CornerRadius::same(5), p.sel, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(5), p.hover);
        }
        let family = if selected { theme::semibold() } else { theme::medium() };
        let color = if selected { p.t1 } else { p.t2 };
        let g = ui.painter().layout_no_wrap(text, FontId::new(12.5, family), color);
        ui.painter().galley(rect.center() - g.size() / 2.0, g, color);
        if resp.clicked() {
            *value = *opt;
        }
    }
    let (icon, line) = match value {
        Some(secs) => (icons::CALENDAR_BLANK, fill(t.ban_until, &[("date", &until(lang, *secs))])),
        None => (icons::INFINITY, t.ban_forever_result.to_string()),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        ui.label(theme::text(icon, 14.0).color(p.t3));
        ui.label(theme::text(line, 12.5).color(p.t2));
    });
}

/// The local end of a ban of `secs` starting now.
fn until(lang: Language, secs: u64) -> String {
    timefmt::date_long(lang, shared::unix_now() + secs as i64)
}

/// What the banned user is told.
pub(super) struct BannedDialog {
    /// The server's name in the list (or its address).
    pub(super) server: String,
    /// Banned during the session (`true`) or refused on connect (`false`).
    pub(super) just_now: bool,
    /// The ban's length (`just_now`) or the time left; `None`: permanent.
    pub(super) secs: Option<u64>,
    pub(super) reason: String,
    /// When the dialog opened: the end is computed from it and does not tick.
    pub(super) at: i64,
}

impl BannedDialog {
    pub(super) fn new(server: String, just_now: bool, secs: Option<u64>, reason: String) -> Self {
        Self { server, just_now, secs, reason, at: shared::unix_now() }
    }

    /// Draws the dialog; false once it is closed.
    pub(super) fn show(&self, ctx: &egui::Context, t: &Tr, lang: Language) -> bool {
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let (title, lead) =
            if self.just_now { (t.banned_title, t.banned_lead) } else { (t.banned_now_title, t.banned_now_lead) };
        let tail = match (self.just_now, self.secs) {
            (true, Some(s)) => fill(t.banned_for, &[("duration", &timefmt::duration(lang, s))]),
            (true, None) => t.banned_forever.to_string(),
            (false, _) => ".".to_string(),
        };
        let mut keep = true;
        let r = Dialog::new("banned", title).icon(icons::PROHIBIT, p.t2).width(440.0).show(
            ctx,
            |ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                let mut job = LayoutJob::default();
                let plain = TextFormat::simple(FontId::new(13.0, FontFamily::Proportional), p.t1);
                let bold = TextFormat::simple(FontId::new(13.0, theme::semibold()), p.t1);
                job.append(&format!("{lead} "), 0.0, plain.clone());
                job.append(&self.server, 0.0, bold);
                job.append(&tail, 0.0, plain);
                job.wrap.max_width = ui.available_width();
                ui.label(job);

                egui::Frame::new()
                    .fill(p.field)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(theme::R_SM))
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            let (icon, when, left) = match self.secs {
                                Some(s) => (
                                    icons::CLOCK,
                                    fill(t.ban_until, &[("date", &timefmt::date_long(lang, self.at + s as i64))]),
                                    Some(fill(t.time_left, &[("left", &timefmt::left(lang, s))])),
                                ),
                                None => (icons::INFINITY, t.ban_permanently.to_string(), None),
                            };
                            ui.label(theme::text(icon, 18.0).color(p.t2));
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(theme::text_semibold(when, 13.0).color(p.t1));
                                if let Some(left) = left {
                                    ui.label(theme::text(left, 12.0).color(p.t2));
                                }
                            });
                        });
                    });

                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(theme::text(t.reason, 12.0).color(p.t3));
                    let reason = if self.reason.is_empty() {
                        theme::text(t.no_reason, 13.0).color(p.t3).italics()
                    } else {
                        theme::text(&self.reason, 13.0).color(p.t1)
                    };
                    ui.add(egui::Label::new(reason).wrap());
                });
            },
            |ui| {
                if ui.add(Button::secondary(t.ok).min_width(64.0)).clicked() {
                    keep = false;
                }
            },
        );
        keep && !r.close_requested
    }
}

/// Bans tab columns: user · key · IP, reason, ends, banned by,
/// Unban. Close to the design (1.3fr 1.5fr 200 1fr 92, gap 14), with a little
/// width moved to "banned by" so that «консоль сервера» fits.
fn columns(row: Rect) -> [Rect; 5] {
    const GAP: f32 = 14.0;
    let (ends, unban) = (176.0, 92.0);
    let inner = row.shrink2(vec2(8.0, 0.0));
    let flex = (inner.width() - ends - unban - GAP * 4.0).max(120.0);
    let fr = flex / 3.8;
    let widths = [1.3 * fr, 1.4 * fr, ends, 1.1 * fr, unban];
    let mut x = inner.left();
    widths.map(|w| {
        let r = Rect::from_min_max(pos2(x, row.top()), pos2(x + w, row.bottom()));
        x += w + GAP;
        r
    })
}

/// A single line of text cut with "…" to `width`.
fn line(ui: &egui::Ui, text: &str, font: FontId, color: Color32, width: f32, italic: bool) -> Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat { font_id: font, color, italics: italic, ..Default::default() });
    job.wrap = TextWrapping::truncate_at_width(width);
    ui.painter().layout_job(job)
}

/// Paints galleys stacked with `gap`, centered vertically in `cell`.
fn stack(ui: &egui::Ui, cell: Rect, gap: f32, galleys: Vec<Arc<egui::Galley>>) {
    let total: f32 = galleys.iter().map(|g| g.size().y).sum::<f32>() + gap * galleys.len().saturating_sub(1) as f32;
    let mut y = cell.center().y - total / 2.0;
    for g in galleys {
        let h = g.size().y;
        ui.painter().galley(pos2(cell.left(), y), g, Color32::PLACEHOLDER);
        y += h + gap;
    }
}

pub(super) fn bans_table(
    ui: &mut egui::Ui,
    t: &Tr,
    lang: Language,
    bans: Option<&[BanInfo]>,
    send: &mut Vec<ClientMsg>,
) {
    let p = pal(ui);
    let bans = match bans {
        None => return super::admin_panel::empty_state(ui, t.loading),
        Some([]) => return super::admin_panel::empty_state(ui, t.no_bans),
        Some(b) => b,
    };
    let now = shared::unix_now();
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    for (rect, title) in columns(head).iter().zip([t.col_user_key_ip, t.col_reason, t.col_ends, t.col_banned_by]) {
        let g = ui.painter().layout_no_wrap(title.to_uppercase(), FontId::new(11.0, theme::semibold()), p.t3);
        ui.painter().galley(pos2(rect.left(), rect.center().y - g.size().y / 2.0), g, p.t3);
    }
    // Ending soonest first, permanent last (the server sends them so).
    for b in bans {
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
        ui.painter().hline(row.x_range(), row.top() + 0.5, Stroke::new(1.0, p.border));
        let [user, reason, ends, by, unban] = columns(row);
        let mono = |s: &str, w: f32| line(ui, s, FontId::monospace(11.0), p.t3, w, false);

        // User, then key · IP; the full values in a hover card.
        let key = b.key.as_deref().map(short_key).unwrap_or_else(|| "—".into());
        let ip = b.ip.as_deref().unwrap_or("—");
        stack(
            ui,
            user,
            2.0,
            vec![
                line(
                    ui,
                    b.nickname.as_deref().unwrap_or("—"),
                    FontId::new(13.0, theme::medium()),
                    p.t1,
                    user.width(),
                    false,
                ),
                mono(&format!("{key} · {ip}"), user.width()),
            ],
        );
        let card = ui.interact(user, ui.scope_id().with(("ban-user", b.created, &b.key, &b.ip)), Sense::hover());
        card.on_hover_ui(|ui| {
            egui::Grid::new("ban-card").num_columns(2).spacing(vec2(12.0, 5.0)).show(ui, |ui| {
                ui.label(theme::text(t.card_key, 12.0).color(p.t3));
                let full = b.key.as_deref().map(|k| k.replace(':', "").to_lowercase()).unwrap_or_else(|| "—".into());
                ui.add(egui::Label::new(theme::mono(full, 11.0)).wrap());
                ui.end_row();
                ui.label(theme::text("IP", 12.0).color(p.t3));
                ui.label(theme::mono(ip, 11.0));
                ui.end_row();
                ui.label(theme::text(t.card_banned, 12.0).color(p.t3));
                ui.label(theme::mono(timefmt::date_full(b.created), 11.0));
                ui.end_row();
            });
        });

        // Reason, up to two lines.
        let mut job = LayoutJob::default();
        let (text, color, italic) =
            if b.reason.is_empty() { (t.no_reason, p.t3, true) } else { (b.reason.as_str(), p.t1, false) };
        job.append(
            text,
            0.0,
            TextFormat {
                font_id: FontId::new(12.5, FontFamily::Proportional),
                color,
                italics: italic,
                ..Default::default()
            },
        );
        job.wrap = TextWrapping {
            max_width: reason.width(),
            max_rows: 2,
            break_anywhere: false,
            overflow_character: Some('…'),
        };
        stack(ui, reason, 0.0, vec![ui.painter().layout_job(job)]);

        // Ends: a chip with the time left, then the exact end.
        let left = b.expires.map(|e| u64::try_from(e - now).unwrap_or(0));
        let (icon, chip_text, fg, bg) = match left {
            None => (icons::INFINITY, t.ban_permanent.to_string(), p.t2, p.sel),
            Some(s) if s < 3600 => (
                icons::HOURGLASS_MEDIUM,
                fill(t.time_left, &[("left", &timefmt::left(lang, s))]),
                p.warn,
                p.warn.gamma_multiply(0.14),
            ),
            Some(s) => (icons::CLOCK, fill(t.time_left, &[("left", &timefmt::left(lang, s))]), p.t1, p.sel),
        };
        let chip = ui.painter().layout_no_wrap(format!("{icon} {chip_text}"), FontId::new(11.5, theme::medium()), fg);
        let mut parts = vec![chip.clone()];
        if let Some(e) = b.expires {
            parts.push(mono(&fill(t.until_short, &[("date", &timefmt::date_short(e))]), ends.width()));
        }
        let total: f32 = 20.0 + parts.get(1).map_or(0.0, |g| 3.0 + g.size().y);
        let top = ends.center().y - total / 2.0;
        let chip_rect = Rect::from_min_size(pos2(ends.left(), top), vec2(chip.size().x + 14.0, 20.0));
        ui.painter().rect_filled(chip_rect, CornerRadius::same(4), bg);
        ui.painter().galley(chip_rect.center() - chip.size() / 2.0, chip, fg);
        if let Some(until) = parts.get(1) {
            ui.painter().galley(pos2(ends.left(), chip_rect.bottom() + 3.0), until.clone(), p.t3);
        }

        // Banned by (a moderator or the console), then when.
        let (by_icon, by_text, by_color, by_italic) = match &b.by {
            Some(nick) => (icons::USER, nick.as_str(), p.t1, false),
            None => (icons::TERMINAL_WINDOW, t.by_console, p.t2, true),
        };
        stack(
            ui,
            by,
            2.0,
            vec![
                line(
                    ui,
                    &format!("{by_icon} {by_text}"),
                    FontId::new(12.5, FontFamily::Proportional),
                    by_color,
                    by.width(),
                    by_italic,
                ),
                mono(&timefmt::date_past(b.created, now), by.width()),
            ],
        );

        let button = Rect::from_center_size(unban.center(), vec2(unban.width(), theme::BUTTON_H_SMALL));
        if ui.put(button, Button::secondary(t.unban).small().width(unban.width())).clicked() {
            send.push(ClientMsg::Unban { ban: b.clone() });
        }
    }
}
