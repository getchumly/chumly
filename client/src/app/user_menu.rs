//! A user's context menu: private message, watch, volume, roles,
//! registration, kick and ban.

use egui::{CornerRadius, FontFamily, Rect, Sense, pos2, vec2};

use super::roles;
use super::ui_util::{font, galley, menu_item, menu_item_danger, menu_title, put, put_right};
use super::*;
use crate::icons;
use crate::theme::{self, pal};

/// A user's menu: right click in the channel list, a click in a viewer list.
#[allow(clippy::too_many_arguments)]
pub(super) fn user_menu(
    ui: &mut egui::Ui,
    t: &Tr,
    roles: &shared::Roles,
    my_role: Option<RoleId>,
    perms: Perms,
    u: &UserInfo,
    volume: f32,
    actions: &mut Vec<Action>,
) {
    menu_title(ui, &u.nickname);
    if ui.add(menu_item(icons::CHAT_CIRCLE, t.private_message)).clicked() {
        actions.push(Action::OpenPm(u.id, u.nickname.clone()));
        ui.close();
    }
    let live = u.streams.contains(&MediaKind::Screen);
    if live && ui.add_enabled(screen::SUPPORTED, menu_item(icons::MONITOR_PLAY, t.watch)).clicked() {
        actions.push(Action::Watch(u.id, u.nickname.clone()));
        ui.close();
    }
    ui.separator();
    volume_slider(ui, t, u, volume, actions);
    // Everything below acts on the user: only on those strictly
    // below, see `Roles::outranks`.
    if !roles.outranks(my_role, u.role) {
        return;
    }
    rights_items(ui, t, roles, my_role, perms, u, actions);
    moderation_items(ui, t, perms, u, actions);
}

fn volume_slider(ui: &mut egui::Ui, t: &Tr, u: &UserInfo, volume: f32, actions: &mut Vec<Action>) {
    ui.label(theme::text(theme::with_icon(icons::SPEAKER_HIGH, t.volume), 12.0).color(pal(ui).t2));
    let mut v = volume;
    let slider = egui::Slider::new(&mut v, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0));
    if ui.add(slider).changed() {
        actions.push(Action::SetVolume(u.id, u.nickname.clone(), v));
    }
}

/// "Assign role" and "Register nickname": both need the user's key.
fn rights_items(
    ui: &mut egui::Ui,
    t: &Tr,
    roles: &shared::Roles,
    my_role: Option<RoleId>,
    perms: Perms,
    u: &UserInfo,
    actions: &mut Vec<Action>,
) {
    // "Assign role" only with a role to give.
    let can_give =
        perms.contains(Perms::ASSIGN_ROLES) && roles.list.iter().any(|r| roles.can_assign(my_role, Some(r.id)));
    let can_register = perms.contains(Perms::REGISTER);
    if !can_give && !can_register {
        return;
    }
    ui.separator();
    if !u.has_key {
        ui.label(theme::text(t.no_key_tip, 12.0).color(pal(ui).t3));
        return;
    }
    if can_give {
        role_menu(ui, t, roles, my_role, u, actions);
    }
    if can_register {
        let (icon, label, value) = if u.registered {
            (icons::USER_MINUS, t.unregister_nick, false)
        } else {
            (icons::USER_PLUS, t.register_nick, true)
        };
        if ui.add(menu_item(icon, label)).on_hover_text(t.register_tip).clicked() {
            actions.push(Action::SetRegistered(u.id, value));
            ui.close();
        }
    }
}

fn moderation_items(ui: &mut egui::Ui, t: &Tr, perms: Perms, u: &UserInfo, actions: &mut Vec<Action>) {
    if !perms.intersects(Perms::KICK | Perms::BAN) {
        return;
    }
    ui.separator();
    let moderate = |ban| Action::Moderate { user_id: u.id, name: u.nickname.clone(), ban };
    let kick = menu_item_danger(ui, icons::SIGN_OUT, &format!("{}…", t.kick));
    if perms.contains(Perms::KICK) && ui.add(kick).clicked() {
        actions.push(moderate(false));
        ui.close();
    }
    if perms.contains(Perms::BAN) {
        let ban = menu_item_danger(ui, icons::PROHIBIT, &format!("{}…", t.ban));
        if ui.add(ban).clicked() {
            actions.push(moderate(true));
            ui.close();
        }
    }
}

/// "Assign role ▸" with the roles the viewer may give (below their own),
/// a check at the current one, and "Remove role".
fn role_menu(
    ui: &mut egui::Ui,
    t: &Tr,
    roles: &shared::Roles,
    my_role: Option<RoleId>,
    u: &UserInfo,
    actions: &mut Vec<Action>,
) {
    ui.menu_button(theme::with_icon(icons::IDENTIFICATION_BADGE, t.give_role), |ui| {
        for r in roles.list.iter().filter(|r| roles.can_assign(my_role, Some(r.id))) {
            let current = u.role == Some(r.id);
            if role_item(ui, t, r, current).clicked() {
                if !current {
                    actions.push(Action::SetRole(u.id, Some(r.id)));
                }
                ui.close();
            }
        }
        if u.role.is_some() && roles.can_assign(my_role, None) {
            ui.separator();
            if ui.add(menu_item(icons::USER_MINUS, t.remove_role)).clicked() {
                actions.push(Action::SetRole(u.id, None));
                ui.close();
            }
        }
    });
}

/// One role in "Assign role ▸": its icon and name in its color, a check
/// at the current one.
fn role_item(ui: &mut egui::Ui, t: &Tr, r: &shared::RoleInfo, current: bool) -> egui::Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(200.0, 30.0), Sense::click());
    if resp.hovered() || current {
        ui.painter().rect_filled(rect, CornerRadius::same(5), p.hover);
    }
    let color = roles::color(p, &r.color);
    let icon = Rect::from_min_size(pos2(rect.left() + 10.0, rect.top()), vec2(16.0, rect.height()));
    if let Some((glyph, family)) = roles::icon(&r.icon) {
        let g = galley(ui, glyph, font(14.0, family), color);
        ui.painter().galley(icon.center() - g.size() / 2.0, g, color);
    }
    let right = if current {
        let check = galley(ui, icons::CHECK, font(14.0, FontFamily::Proportional), p.t1);
        put_right(ui, rect, rect.right() - 10.0, check).left() - 6.0
    } else {
        rect.right() - 10.0
    };
    let mut job = egui::text::LayoutJob::simple_singleline(roles::name(t, r), font(13.0, theme::medium()), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(right - icon.right() - 10.0);
    put(ui, rect, icon.right() + 10.0, ui.painter().layout_job(job));
    resp
}
