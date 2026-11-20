//! Viewers of a screen share: the eye counter and the popover with their
//! names. The streamer sees it in the "me" block (with how long each has
//! been watching, a click on a name opens the user menu), a viewer in the
//! stream window (themselves first, as "you").

use egui::{Align2, CornerRadius, FontFamily, Margin, Rect, Sense, Stroke, pos2, vec2};

use super::popover::HoverPopover;
use super::ui_util::{font, galley, put, put_right};
use super::*;
use crate::icons;
use crate::theme::{self, pal};

/// The counter is tinted this long after a viewer joins.
pub(super) const FLASH: Duration = Duration::from_secs(2);
const POPOVER_W: f32 = 250.0;
const ROW_H: f32 = 30.0;
/// The list scrolls above this height (8 rows).
const LIST_MAX_H: f32 = 240.0;

/// The eye with the number of viewers. `flash`: a viewer just joined
/// (green with "+1"); with nobody watching it stays, muted, so the
/// streamer knows it is there. Returns the click/hover response.
pub(super) fn eye_pill(ui: &mut egui::Ui, n: usize, flash: bool, height: f32) -> egui::Response {
    let p = pal(ui);
    let (bg, border, color) = if flash {
        let a = p.accent;
        (a.gamma_multiply(0.14), a.gamma_multiply(0.4), a)
    } else if n > 0 {
        (p.sel, p.border, p.t2)
    } else {
        (Color32::TRANSPARENT, p.border, p.t3)
    };
    let eye = galley(ui, icons::EYE, font(13.0, FontFamily::Proportional), color);
    let num = galley(ui, &n.to_string(), font(11.5, FontFamily::Monospace), color);
    let plus = flash.then(|| galley(ui, "+1", font(11.0, FontFamily::Monospace), color));
    let w = 7.0 + eye.size().x + 5.0 + num.size().x + plus.as_ref().map_or(0.0, |g| g.size().x + 5.0) + 7.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    ui.painter().rect(rect, CornerRadius::same(5), bg, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    let mut x = put(ui, rect, rect.left() + 7.0, eye) + 5.0;
    x = put(ui, rect, x, num) + 5.0;
    if let Some(g) = plus {
        put(ui, rect, x, g);
    }
    resp
}

/// "now" or "12 min".
fn watched_for(t: &Tr, since: Option<Instant>) -> String {
    let min = since.map_or(0, |s| s.elapsed().as_secs() / 60);
    if min == 0 { t.viewer_now.into() } else { fill(t.viewer_minutes, &[("n", &min.to_string())]) }
}

/// Who watches `streamer`'s share: the popover for `pill` (see `HoverPopover`).
/// `as_streamer`: the streamer's own list (watch time, user menu on a
/// click, an empty state); otherwise a viewer's (themselves first).
#[allow(clippy::too_many_arguments)]
pub(super) fn popover(
    ui: &egui::Ui,
    t: &Tr,
    s: &Session,
    cfg: &ClientConfig,
    streamer: UserId,
    as_streamer: bool,
    pop: &mut HoverPopover,
    pill: &egui::Response,
    above: bool,
    actions: &mut Vec<Action>,
) {
    if !pop.update(ui, pill) {
        return;
    }
    let p = pal(ui);
    let mut viewers = s.viewers_of(streamer);
    if !as_streamer && let Some(i) = viewers.iter().position(|u| u.id == s.my_id) {
        let me = viewers.remove(i);
        viewers.insert(0, me);
    }
    let (at, pivot) = if above {
        (pill.rect.right_top() - vec2(0.0, 6.0), Align2::RIGHT_BOTTOM)
    } else {
        (pill.rect.left_bottom() + vec2(0.0, 6.0), Align2::LEFT_TOP)
    };
    let frame = egui::Frame::new()
        .fill(p.pop)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(theme::R_MD))
        .inner_margin(Margin::same(6))
        .shadow(egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(90) });
    let area = egui::Area::new(egui::Id::unique(("viewers-popover", streamer, as_streamer)))
        .order(egui::Order::Foreground)
        .pivot(pivot)
        .fixed_pos(at)
        .show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                ui.set_width(POPOVER_W - 12.0);
                ui.spacing_mut().item_spacing.y = 1.0;
                // "WATCHING · 3"
                let (head, _) = ui.allocate_exact_size(vec2(POPOVER_W - 12.0, 26.0), Sense::hover());
                let x = put(
                    ui,
                    head,
                    head.left() + 8.0,
                    galley(
                        ui,
                        &format!("{} · ", t.viewers_caption.to_uppercase()),
                        font(11.0, theme::semibold()),
                        p.t3,
                    ),
                );
                put(ui, head, x, galley(ui, &viewers.len().to_string(), font(11.0, FontFamily::Monospace), p.t3));
                if viewers.is_empty() {
                    empty(ui, t);
                    return;
                }
                egui::ScrollArea::vertical().max_height(LIST_MAX_H).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for u in &viewers {
                        let me = u.id == s.my_id;
                        let right = if as_streamer {
                            watched_for(t, s.watch_since.get(&u.id).copied())
                        } else if me {
                            t.you.into()
                        } else {
                            String::new()
                        };
                        let resp = row(ui, t, s, u, &right, as_streamer && !me);
                        if as_streamer && !me {
                            let volume = cfg.user_volume(&u.nickname);
                            let perms = s.my_perms();
                            egui::Popup::menu(&resp).show(|ui| {
                                super::user_menu::user_menu(ui, t, &s.roles, s.my_role(), perms, u, volume, actions);
                            });
                        }
                    }
                });
                if as_streamer {
                    ui.add_space(4.0);
                    let (r, _) = ui.allocate_exact_size(vec2(POPOVER_W - 12.0, 26.0), Sense::hover());
                    ui.painter().hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, p.border));
                    put(
                        ui,
                        r,
                        r.left() + 8.0,
                        galley(ui, t.viewer_menu_hint, font(11.5, FontFamily::Proportional), p.t3),
                    );
                }
            });
        });
    pop.set_rect(area.response.rect);
    // Minutes tick: keep the times fresh while it is open.
    if as_streamer {
        ui.ctx().request_repaint_after(Duration::from_secs(10));
    }
}

/// One viewer: role icon, nickname in the role color, and on the right the
/// watch time or "you".
fn row(ui: &mut egui::Ui, t: &Tr, s: &Session, u: &UserInfo, right: &str, clickable: bool) -> egui::Response {
    let p = pal(ui);
    let sense = if clickable { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
    if clickable && resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(5), p.hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let role = u.role.and_then(|id| s.roles.get(id));
    let color = role.map_or(p.t1, |r| super::roles::color(p, &r.color));
    let icon = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top()), vec2(14.0, ROW_H));
    if let Some((glyph, family)) = role.and_then(|r| super::roles::icon(&r.icon)) {
        let g = galley(ui, glyph, font(12.0, family), color);
        ui.painter().galley(icon.center() - g.size() / 2.0, g, color);
    }
    let mut end = rect.right() - 12.0;
    if !right.is_empty() {
        let family = if right == t.you { FontFamily::Proportional } else { FontFamily::Monospace };
        end = put_right(ui, rect, end, galley(ui, right, font(11.5, family), p.t3)).left() - 8.0;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(u.nickname.clone(), font(13.0, theme::medium()), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width((end - icon.right() - 8.0).max(20.0));
    put(ui, rect, icon.right() + 8.0, ui.painter().layout_job(job));
    resp
}

/// Nobody is watching yet.
fn empty(ui: &mut egui::Ui, t: &Tr) {
    let p = pal(ui);
    ui.add_space(6.0);
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(theme::text(icons::EYE_SLASH, 20.0).color(p.t3));
        ui.label(theme::text(t.nobody_watching, 13.0).color(p.t2));
        ui.add(egui::Label::new(theme::text(t.nobody_watching_hint, 12.0).color(p.t3)).wrap());
    });
    ui.add_space(8.0);
}

/// Tooltip of a LIVE badge: who watches and "click to watch".
pub(super) fn live_tip(t: &Tr, name: &str, viewers: &[&UserInfo]) -> String {
    if viewers.is_empty() {
        return t.live_tip.into();
    }
    let names: Vec<&str> = viewers.iter().map(|u| u.nickname.as_str()).collect();
    let head = fill(t.live_viewers_title, &[("name", name), ("n", &viewers.len().to_string())]);
    format!("{head}\n{} · {}", names.join(", "), t.live_click_to_watch)
}
