//! Admin panel: server users (registered and online), bans and roles.

use egui::{CornerRadius, FontFamily, FontId, Margin, Rect, Sense, Stroke, pos2, vec2};

use super::*;
use crate::icons;
use crate::theme::{self, Button, pal};

const PANEL_WIDTH: f32 = 820.0;
/// Height of the tab bar (tabs, hint, Refresh).
const TAB_BAR_H: f32 = 44.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AdminTab {
    Registered,
    Bans,
    Roles,
    /// Owner only: the server password.
    Server,
}

/// Admin panel: registered users (online or not) and bans. Filled by the
/// server's answers; the server checks admin rights for every request.
pub(super) struct AdminPanel {
    pub(super) tab: AdminTab,
    pub(super) users: Option<Vec<PanelUser>>,
    pub(super) bans: Option<Vec<BanInfo>>,
    /// Someone joined, left or changed role: ask for the list again.
    pub(super) stale: bool,
    pub(super) last_request: Instant,
    pub(super) roles: super::role_editor::RoleTab,
    pub(super) server: super::server_tab::ServerTab,
}

/// The panel refreshes itself at most this often: a burst of joins (e.g.
/// after a server restart) must not turn into a burst of requests, which
/// the server's control message limit would punish.
pub(super) const PANEL_REFRESH: Duration = Duration::from_secs(1);

pub(super) fn fmt_date(ts: i64) -> String {
    local_time(ts, "%d.%m.%Y")
}

/// Key fingerprint "AB:CD:...:YZ" shortened to "ABCD…WXYZ".
pub(super) fn short_key(key: &str) -> String {
    let hex: String = key.chars().filter(|c| *c != ':').collect();
    if hex.len() <= 8 {
        return hex;
    }
    format!("{}…{}", &hex[..4], &hex[hex.len() - 4..])
}

/// Column widths for a row: `None` is the flexible column taking the rest.
fn column_rects(row: Rect, widths: &[Option<f32>], gap: f32) -> Vec<Rect> {
    let fixed: f32 = widths.iter().flatten().sum();
    let gaps = gap * (widths.len().saturating_sub(1)) as f32;
    let flex = (row.width() - 16.0 - fixed - gaps).max(40.0);
    let mut x = row.left() + 8.0;
    widths
        .iter()
        .map(|w| {
            let w = w.unwrap_or(flex);
            let r = Rect::from_min_max(pos2(x, row.top()), pos2(x + w, row.bottom()));
            x += w + gap;
            r
        })
        .collect()
}

/// A table row: allocates the row and gives a left-to-right child UI per column.
fn table_row(
    ui: &mut egui::Ui,
    height: f32,
    border: bool,
    widths: &[Option<f32>],
    mut cell: impl FnMut(usize, &mut egui::Ui),
) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    if border {
        ui.painter().hline(row.x_range(), row.top() + 0.5, Stroke::new(1.0, pal(ui).border));
    }
    for (i, rect) in column_rects(row, widths, 10.0).into_iter().enumerate() {
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        cell(i, &mut child);
    }
}

fn header_row(ui: &mut egui::Ui, widths: &[Option<f32>], titles: &[&str]) {
    table_row(ui, 36.0, false, widths, |i, ui| {
        if let Some(title) = titles.get(i) {
            ui.label(theme::caption(ui, title));
        }
    });
}

pub(super) fn empty_state(ui: &mut egui::Ui, text: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(theme::text(text, 13.0).color(pal(ui).t3));
    });
    ui.add_space(40.0);
}

/// A tab in the panel's tab bar with a count after the label.
fn panel_tab(ui: &mut egui::Ui, label: &str, count: Option<usize>, active: bool) -> egui::Response {
    let p = pal(ui);
    let family = if active { theme::semibold() } else { theme::medium() };
    let lg = ui.painter().layout_no_wrap(label.to_owned(), FontId::new(13.0, family), if active { p.t1 } else { p.t2 });
    let cg =
        count.map(|n| ui.painter().layout_no_wrap(n.to_string(), FontId::new(13.0, FontFamily::Proportional), p.t3));
    let w = 24.0 + lg.size().x + cg.as_ref().map_or(0.0, |g| g.size().x + 6.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ui.available_height().min(TAB_BAR_H)), Sense::click());
    if resp.hovered() && !active {
        ui.painter().rect_filled(rect, CornerRadius { nw: theme::R_SM, ne: theme::R_SM, sw: 0, se: 0 }, p.hover);
    }
    if active {
        let line = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max);
        ui.painter().rect_filled(line, CornerRadius::ZERO, p.accent);
    }
    let lw = lg.size().x;
    ui.painter().galley(pos2(rect.left() + 12.0, rect.center().y - lg.size().y / 2.0), lg, Color32::PLACEHOLDER);
    if let Some(g) = cg {
        ui.painter().galley(pos2(rect.left() + 18.0 + lw, rect.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
    }
    resp
}

impl App {
    pub(super) fn draw_admin_panel(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let lang = self.cfg.language;
        let Some(s) = self.session.as_ref() else {
            self.admin_panel = None;
            return;
        };
        let perms = s.my_perms();
        if !perms.intersects(Perms::PANEL) {
            // Lost the rights: the server would refuse anyway.
            self.admin_panel = None;
        }
        let Some(panel) = self.admin_panel.as_mut() else { return };
        let roles = &s.roles;
        let me = (s.my_role(), s.my_id);
        // The bans tab needs the ban right, the roles tab the edit right.
        let owner = me.0 == Some(shared::OWNER_ROLE);
        let lost = match panel.tab {
            AdminTab::Bans => !perms.contains(Perms::BAN),
            AdminTab::Roles => !perms.contains(Perms::EDIT_ROLES),
            AdminTab::Server => !owner,
            AdminTab::Registered => false,
        };
        if lost {
            panel.tab = AdminTab::Registered;
        }
        let my_nick = s.users.get(&s.my_id).map_or("", |u| u.nickname.as_str());
        let view = super::role_editor::View { t, roles, my_role: me.0, perms, users: panel.users.as_deref(), my_nick };
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let mut close = false;
        let mut send: Vec<ClientMsg> = Vec::new();
        if panel.stale {
            if panel.last_request.elapsed() >= PANEL_REFRESH {
                panel.stale = false;
                panel.last_request = Instant::now();
                send.push(ClientMsg::ListUsers);
            } else {
                ctx.request_repaint_after(PANEL_REFRESH);
            }
        }
        let width = PANEL_WIDTH.min(ctx.content_rect().width() - 24.0);
        let max_h = (ctx.content_rect().height() - 140.0).max(200.0);
        egui::Window::new("admin-panel")
            .title_bar(false)
            .frame(theme::window_frame(p))
            .resizable(false)
            .min_width(width)
            .max_width(width)
            .default_pos(pos2(ctx.content_rect().center().x - width / 2.0, ctx.content_rect().top() + 60.0))
            .collapsible(false)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                if theme::window_header(ui, width, t.server_users) {
                    close = true;
                }
                // Tabs, hint and Refresh. The bar is reserved with an exact size first:
                // layout helpers would clamp its height to what the window has left.
                let (bar, _) = ui.allocate_exact_size(vec2(width, TAB_BAR_H), Sense::hover());
                ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, p.border));
                let inner = bar.shrink2(vec2(20.0, 0.0));
                let mut left = ui.new_child(
                    egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                left.spacing_mut().item_spacing.x = 2.0;
                let users = panel.users.as_ref().map(Vec::len);
                let bans = panel.bans.as_ref().map(Vec::len);
                if panel_tab(&mut left, t.tab_registered, users, panel.tab == AdminTab::Registered).clicked() {
                    panel.tab = AdminTab::Registered;
                }
                if perms.contains(Perms::BAN)
                    && panel_tab(&mut left, t.tab_bans, bans, panel.tab == AdminTab::Bans).clicked()
                {
                    panel.tab = AdminTab::Bans;
                }
                let role_count = Some(roles.list.len());
                if perms.contains(Perms::EDIT_ROLES)
                    && panel_tab(&mut left, t.tab_roles, role_count, panel.tab == AdminTab::Roles).clicked()
                {
                    panel.tab = AdminTab::Roles;
                }
                if owner && panel_tab(&mut left, t.tab_server, None, panel.tab == AdminTab::Server).clicked() {
                    panel.tab = AdminTab::Server;
                    send.push(ClientMsg::GetServerSettings);
                }
                let mut right = ui.new_child(
                    egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                right.spacing_mut().item_spacing.x = 14.0;
                // At most once per PANEL_REFRESH: an auto-clicker on this
                // button would otherwise hit the server's control message
                // limit and get the admin disconnected.
                let ready = panel.last_request.elapsed() >= PANEL_REFRESH;
                let refresh = Button::ghost(t.refresh).with_icon(icons::ARROWS_CLOCKWISE).small();
                if right.add_enabled(ready, refresh).clicked() {
                    panel.stale = false;
                    panel.last_request = Instant::now();
                    send.push(ClientMsg::ListUsers);
                    if perms.contains(Perms::BAN) {
                        send.push(ClientMsg::ListBans);
                    }
                }
                if !ready {
                    right.ctx().request_repaint_after(PANEL_REFRESH);
                }
                let hint = format!("{}  {}", icons::INFO, t.rights_by_key);
                right.label(theme::text(hint, 12.0).color(p.t3)).on_hover_text(t.server_users_hint);

                if panel.tab == AdminTab::Roles {
                    let height = (max_h + 40.0).min(660.0);
                    panel.roles.show(ui, &view, height, &mut send);
                    return;
                }
                egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 8, bottom: 14 }).show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(
                        ui,
                        |ui| match panel.tab {
                            AdminTab::Registered => {
                                users_table(ui, t, roles, perms, panel.users.as_deref(), me, &mut send)
                            }
                            AdminTab::Bans => super::bans_ui::bans_table(ui, t, lang, panel.bans.as_deref(), &mut send),
                            AdminTab::Roles => {}
                            AdminTab::Server => panel.server.show(ui, t),
                        },
                    );
                });
            });
        // Esc belongs to an open dialog of the roles tab.
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape))
            && !panel.roles.dialog_open()
            && !panel.server.dialog_open();
        let mut close_now = panel.roles.dialogs(ctx, &view, &mut send);
        panel.server.dialogs(ctx, t, &mut send);
        if close || (esc && !self.settings.open) {
            // Unsaved role edits ask first.
            close_now |= panel.roles.request(super::role_editor::Pending::Close, &view, &mut send);
        }
        if close_now {
            self.admin_panel = None;
        }
        for m in send {
            self.send(m);
        }
    }
}

/// `me`: the viewer's role and session id. Only users strictly below the
/// viewer get controls (see `Roles::outranks`), and only those the viewer's
/// rights allow; the role list holds only roles the viewer may give
/// (`Roles::can_assign`).
fn users_table(
    ui: &mut egui::Ui,
    t: &Tr,
    roles: &shared::Roles,
    perms: Perms,
    users: Option<&[PanelUser]>,
    me: (Option<RoleId>, UserId),
    send: &mut Vec<ClientMsg>,
) {
    let (my_role, my_id) = (me.0, Some(me.1));
    let p = pal(ui);
    let role_text = |role: Option<RoleId>, lock: bool| super::roles::label(t, p, roles, role, lock);
    let users = match users {
        None => return empty_state(ui, t.loading),
        Some([]) => return empty_state(ui, t.no_registered),
        Some(u) => u,
    };
    let widths = [Some(18.0), None, Some(150.0), Some(110.0), Some(64.0), Some(125.0), Some(28.0)];
    header_row(ui, &widths, &["", t.nickname, t.col_role, t.col_key, t.col_client, t.col_registered_at, ""]);
    for (row, u) in users.iter().enumerate() {
        let below = u.online.is_none_or(|id| Some(id) != my_id) && roles.outranks(my_role, u.role);
        let can_give = below && perms.contains(Perms::ASSIGN_ROLES);
        let can_register = below && perms.contains(Perms::REGISTER);
        table_row(ui, 38.0, true, &widths, |col, ui| match col {
            0 => {
                let color = if u.online.is_some() { p.speak } else { p.dot_idle };
                let (r, resp) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, color);
                if u.online.is_some() {
                    resp.on_hover_text(t.online);
                }
            }
            1 => {
                ui.add(egui::Label::new(theme::text(&u.nickname, 13.0).color(p.t1)).truncate());
            }
            // Registered: by key, online or not. Unregistered: only online
            // ones are listed, by session id; giving a role registers them.
            2 => match (&u.key, u.registered, u.online) {
                (None, _, _) => {
                    ui.label(theme::text(t.no_key, 12.0).color(p.t3)).on_hover_text(t.no_key_tip);
                }
                // A role the viewer cannot change: plain text with a lock.
                (Some(_), _, _) if !can_give => {
                    ui.add_space(9.0);
                    ui.label(role_text(u.role, perms.contains(Perms::ASSIGN_ROLES)));
                }
                (Some(key), registered, online) => {
                    let mut role = u.role;
                    egui::ComboBox::from_id_salt(("role", row, key))
                        .width(140.0)
                        .selected_text(role_text(role, false))
                        .show_ui(ui, |ui| {
                            let options = roles.list.iter().map(|r| Some(r.id)).chain([None]);
                            for r in options.filter(|r| roles.can_assign(my_role, *r)) {
                                ui.selectable_value(&mut role, r, role_text(r, false));
                            }
                        });
                    if role != u.role {
                        match (registered, online) {
                            (true, _) => send.push(ClientMsg::SetRoleByKey { key: key.clone(), role }),
                            (false, Some(user_id)) => send.push(ClientMsg::SetRole { user_id, role }),
                            (false, None) => {}
                        }
                    }
                }
            },
            3 => match &u.key {
                Some(k) => {
                    ui.label(theme::mono(short_key(k), 12.0).color(p.t2)).on_hover_text(k);
                }
                None => {
                    ui.label(theme::text("—", 12.0).color(p.t3));
                }
            },
            // Client version of an online user: the owner sees old builds without
            // any automatic warnings.
            4 => {
                let v = if u.client_version.is_empty() { "—" } else { u.client_version.as_str() };
                ui.label(theme::mono(v, 12.0).color(p.t3));
            }
            5 => {
                if u.registered {
                    ui.label(theme::text(fmt_date(u.created), 12.0).color(p.t3));
                } else {
                    ui.label(RichText::new(t.not_registered).size(12.0).italics().color(p.t3));
                }
            }
            _ => match (&u.key, u.registered, u.online) {
                _ if !can_register => {}
                (Some(key), true, _) => {
                    if ui.add(Button::icon(icons::USER_MINUS).height(28.0)).on_hover_text(t.unregister_nick).clicked() {
                        send.push(ClientMsg::UnregisterKey { key: key.clone() });
                    }
                }
                (Some(_), false, Some(user_id)) => {
                    let tip = format!("{}\n{}", t.register_nick, t.register_tip);
                    if ui.add(Button::icon(icons::USER_PLUS).height(28.0)).on_hover_text(tip).clicked() {
                        send.push(ClientMsg::SetRegistered { user_id, registered: true });
                    }
                }
                _ => {}
            },
        });
    }
}
