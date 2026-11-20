//! The Roles tab of the admin panel: the role list (drag to reorder) and the
//! editor of the selected role.
//!
//! Name, color, icon and rights are saved with Save (a stray click on a live
//! server is costly); a move is saved at once. The server checks every change
//! again (`server/src/state/roles.rs`): the client only hides what would be
//! refused.

use egui::text::{LayoutJob, TextWrapping};
use egui::{CornerRadius, FontFamily, Margin, Rect, Sense, Stroke, Vec2, pos2, vec2};
use shared::{RoleInfo, Roles};

use super::roles;
use super::ui_util::{font, galley, put, put_right};
use super::*;
use crate::icons;
use crate::theme::{self, Button, Dialog, pal};

/// Width of the role list on the left.
const LIST_W: f32 = 268.0;
const ROW_H: f32 = 34.0;
const ROW_GAP: f32 = 2.0;
const FOOTER_H: f32 = 56.0;

/// What waits for the answer to "Save changes?".
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Pending {
    Select(RoleId),
    Create,
    Close,
}

struct RoleDrag {
    id: RoleId,
    grab: Vec2,
    size: Vec2,
}

#[derive(Default)]
pub(super) struct RoleTab {
    selected: Option<RoleId>,
    /// The selected role as the server has it and the edited copy. Both hold
    /// the shown name (default names translated), see `local`.
    base: Option<RoleInfo>,
    draft: Option<RoleInfo>,
    drag: Option<RoleDrag>,
    /// Name of a role just created: it is selected when it arrives.
    created: Option<String>,
    /// Waits for "Save changes?".
    pending: Option<Pending>,
    /// Waits for "Delete role?".
    delete: Option<RoleId>,
}

/// What the tab needs from the session.
pub(super) struct View<'a> {
    pub(super) t: &'static Tr,
    pub(super) roles: &'a Roles,
    pub(super) my_role: Option<RoleId>,
    pub(super) perms: Perms,
    pub(super) users: Option<&'a [PanelUser]>,
    pub(super) my_nick: &'a str,
}

impl View<'_> {
    fn can_edit(&self, id: RoleId) -> bool {
        self.perms.contains(Perms::EDIT_ROLES) && self.roles.can_edit(self.my_role, id)
    }

    /// Users with this role; `None` until the panel has the user list.
    fn count(&self, role: Option<RoleId>) -> Option<usize> {
        self.users.map(|u| u.iter().filter(|u| u.role == role).count())
    }

    /// Index of the topmost slot a role may move to: right below the viewer's.
    fn first_free_index(&self) -> usize {
        self.my_role.and_then(|r| self.roles.index(r)).map_or(usize::MAX, |i| i + 1)
    }
}

/// The role with its shown name: default names are translated until renamed.
fn local(t: &Tr, r: &RoleInfo) -> RoleInfo {
    RoleInfo { name: roles::name(t, r), ..r.clone() }
}

/// Rights in the editor's order with their titles and hints.
fn perm_rows(t: &Tr) -> [(Perms, &'static str, &'static str); 6] {
    [
        (Perms::KICK, t.perm_kick_title, t.perm_kick_desc),
        (Perms::BAN, t.perm_ban_title, t.perm_ban_desc),
        (Perms::CHANNELS, t.perm_channels_title, t.perm_channels_desc),
        (Perms::REGISTER, t.perm_register_title, t.perm_register_desc),
        (Perms::ASSIGN_ROLES, t.perm_assign_title, t.perm_assign_desc),
        (Perms::EDIT_ROLES, t.perm_edit_title, t.perm_edit_desc),
    ]
}

/// What is wrong with the draft's name, if anything. Names are compared
/// case-insensitively, both shown and stored ones (the server compares the
/// stored ones).
fn name_error(v: &View, draft: &RoleInfo) -> Option<String> {
    let t = v.t;
    if draft.name.is_empty() {
        return Some(t.role_name_empty.into());
    }
    if !shared::validate_role_name(&draft.name) {
        return Some(t.role_name_bad.into());
    }
    let lower = draft.name.to_lowercase();
    let taken = v
        .roles
        .list
        .iter()
        .any(|r| r.id != draft.id && (r.name.to_lowercase() == lower || roles::name(t, r).to_lowercase() == lower));
    taken.then(|| fill(t.role_name_taken, &[("name", &draft.name)]))
}

/// One line of text cut with "…" to `max_w`.
fn galley_cut(ui: &egui::Ui, text: &str, font: egui::FontId, color: Color32, max_w: f32) -> Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(max_w.max(10.0));
    ui.painter().layout_job(job)
}

/// A role's icon as a galley; an empty one for no icon.
fn icon_galley(ui: &egui::Ui, name: &str, size: f32, color: Color32) -> Option<Arc<egui::Galley>> {
    roles::icon(name).map(|(g, family)| galley(ui, g, font(size, family), color))
}

impl RoleTab {
    pub(super) fn dirty(&self) -> bool {
        matches!((&self.base, &self.draft), (Some(b), Some(d)) if b != d)
    }

    /// A dialog of the tab is open: Esc belongs to it.
    pub(super) fn dialog_open(&self) -> bool {
        self.pending.is_some() || self.delete.is_some()
    }

    /// Does `p` now, or asks to save first. Returns `true` for a close the
    /// caller should do now.
    pub(super) fn request(&mut self, p: Pending, v: &View, send: &mut Vec<ClientMsg>) -> bool {
        if self.dirty() {
            self.pending = Some(p);
            false
        } else {
            self.perform(p, v, send)
        }
    }

    fn perform(&mut self, p: Pending, v: &View, send: &mut Vec<ClientMsg>) -> bool {
        match p {
            Pending::Select(id) => self.select(v, id),
            Pending::Create => self.create(v, send),
            Pending::Close => return true,
        }
        false
    }

    fn select(&mut self, v: &View, id: RoleId) {
        self.selected = Some(id);
        self.base = v.roles.get(id).map(|r| local(v.t, r));
        self.draft = self.base.clone();
    }

    /// A new role at the bottom with a free name, no icon and no rights.
    fn create(&mut self, v: &View, send: &mut Vec<ClientMsg>) {
        let t = v.t;
        let free = |n: &str| {
            v.roles.list.iter().all(|r| !r.name.eq_ignore_ascii_case(n) && !roles::name(t, r).eq_ignore_ascii_case(n))
        };
        let name = (1..)
            .map(|i| if i == 1 { t.new_role_name.to_string() } else { format!("{} {i}", t.new_role_name) })
            .find(|n| free(n))
            .expect("some number is free");
        let role =
            RoleInfo { id: 0, name: name.clone(), color: "gray".into(), icon: String::new(), perms: Perms::NONE };
        send.push(ClientMsg::CreateRole { role });
        self.created = Some(name);
    }

    /// The draft as the server should store it: an untouched translated
    /// default name keeps its stored form.
    fn to_send(&self, v: &View) -> Option<RoleInfo> {
        let mut r = self.draft.clone()?;
        if let Some(cur) = v.roles.get(r.id)
            && r.name == roles::name(v.t, cur)
        {
            r.name = cur.name.clone();
        }
        Some(r)
    }

    fn save(&mut self, v: &View, send: &mut Vec<ClientMsg>) {
        if let Some(role) = self.to_send(v) {
            send.push(ClientMsg::EditRole { role });
        }
    }

    /// Follows the server's list: picks up a created role, drops a deleted
    /// one, and takes the server's version of the selected role unless there
    /// are edits (those stay until saved or cancelled; a save that the server
    /// accepted comes back equal to the draft).
    fn sync(&mut self, v: &View) {
        if let Some(name) = &self.created
            && let Some(r) = v.roles.list.iter().find(|r| &r.name == name)
        {
            let id = r.id;
            self.created = None;
            if !self.dirty() {
                self.select(v, id);
            }
        }
        if self.selected.is_some_and(|id| v.roles.get(id).is_none()) {
            self.selected = None;
        }
        let Some(id) = self.selected else {
            // The first role the viewer may edit, else their own, else the top.
            let first =
                v.roles.list.iter().find(|r| v.can_edit(r.id)).or_else(|| v.my_role.and_then(|id| v.roles.get(id)));
            if let Some(r) = first.or(v.roles.list.first()) {
                self.select(v, r.id);
            }
            return;
        };
        let current = v.roles.get(id).map(|r| local(v.t, r));
        if self.draft == current {
            self.base = current;
        } else if !self.dirty() {
            self.base = current.clone();
            self.draft = current;
        }
        if self.delete.is_some_and(|id| v.roles.get(id).is_none()) {
            self.delete = None;
        }
    }

    /// The tab's contents in `height`.
    pub(super) fn show(&mut self, ui: &mut egui::Ui, v: &View, height: f32, send: &mut Vec<ClientMsg>) {
        self.sync(v);
        let p = pal(ui);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let list = Rect::from_min_max(rect.min, pos2(rect.left() + LIST_W, rect.bottom()));
        let editor = Rect::from_min_max(pos2(list.right(), rect.top()), rect.max);
        let radius = CornerRadius { nw: 0, ne: 0, sw: theme::R_LG, se: 0 };
        ui.painter().rect_filled(list, radius, p.bg2);
        ui.painter().vline(list.right() - 0.5, list.y_range(), Stroke::new(1.0, p.border));
        self.role_list(ui, v, list, send);
        self.editor(ui, v, editor, send);
    }

    fn role_list(&mut self, ui: &mut egui::Ui, v: &View, rect: Rect, send: &mut Vec<ClientMsg>) {
        let t = v.t;
        let p = pal(ui);
        // Header: caption with the count, "Create role".
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), 46.0));
        let full = v.roles.list.len() >= shared::MAX_ROLES;
        let caption = galley(ui, &t.roles_caption.to_uppercase(), font(11.0, theme::semibold()), p.t3);
        let x = put(ui, header, header.left() + 14.0, caption) + 8.0;
        let count = format!("{} / {}", v.roles.list.len(), shared::MAX_ROLES);
        put(ui, header, x, galley(ui, &count, font(11.0, FontFamily::Monospace), if full { p.danger } else { p.t3 }));
        if v.perms.contains(Perms::EDIT_ROLES) {
            let w = theme::button_width(ui, t.create_role, Some(icons::PLUS));
            let b = Rect::from_min_size(pos2(header.right() - 10.0 - w, header.center().y - 14.0), vec2(w, 28.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(b));
            let resp = child.add_enabled(!full, Button::secondary(t.create_role).with_icon(icons::PLUS).small());
            if full {
                resp.on_disabled_hover_text(fill(t.roles_limit_tip, &[("n", &shared::MAX_ROLES.to_string())]));
            } else if resp.clicked() {
                self.request(Pending::Create, v, send);
            }
        }

        // "No role" at the bottom.
        let foot = Rect::from_min_max(
            pos2(rect.left() + 8.0, rect.bottom() - 48.0),
            pos2(rect.right() - 8.0, rect.bottom() - 8.0),
        );
        ui.painter().hline(foot.x_range(), foot.top() + 0.5, Stroke::new(1.0, p.border));
        let x = put(ui, foot, foot.left() + 12.0, galley(ui, icons::MINUS, font(14.0, FontFamily::Proportional), p.t3))
            + 26.0;
        let none = RichText::new(t.role_none).italics().size(13.0).color(p.t3);
        let g = egui::WidgetText::from(none).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        );
        ui.painter().galley(pos2(x, foot.center().y - g.size().y / 2.0), g, p.t3);
        if let Some(n) = v.count(None) {
            put_right(
                ui,
                foot,
                foot.right() - 8.0,
                galley(ui, &n.to_string(), font(11.5, FontFamily::Monospace), p.t3),
            );
        }

        // The rows.
        let body =
            Rect::from_min_max(pos2(rect.left() + 8.0, header.bottom()), pos2(rect.right() - 8.0, foot.top() - 4.0));
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(body.intersect(ui.clip_rect()));
        let mut rows: Vec<(RoleId, Rect, Option<egui::Response>)> = Vec::new();
        egui::ScrollArea::vertical().id_salt("role-list").auto_shrink([false, false]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            for r in &v.roles.list {
                if self.drag.as_ref().is_some_and(|d| d.id == r.id) {
                    let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                    super::servers::dashed_rect(ui, slot.shrink(0.5), p.border_strong);
                    rows.push((r.id, slot, None));
                    continue;
                }
                let (row, handle) = self.role_row(ui, v, r);
                if row.clicked() && self.selected != Some(r.id) {
                    self.request(Pending::Select(r.id), v, send);
                }
                rows.push((r.id, row.rect, handle));
            }
        });
        self.drag_rows(ui, v, &rows, send);
    }

    /// One row: handle or lock, icon, name, user count. Returns the row and,
    /// for a role the viewer may move, its handle.
    fn role_row(&self, ui: &mut egui::Ui, v: &View, r: &RoleInfo) -> (egui::Response, Option<egui::Response>) {
        let t = v.t;
        let p = pal(ui);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let selected = self.selected == Some(r.id);
        let editable = v.can_edit(r.id);
        let bg = if selected {
            p.sel
        } else if resp.hovered() {
            p.hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(theme::R_SM), bg);
        let lead = Rect::from_min_size(pos2(rect.left() + 4.0, rect.top()), vec2(16.0, ROW_H));
        let handle = if editable {
            let h = ui.interact(lead, egui::Id::unique(("role-handle", r.id)), Sense::drag());
            super::servers::drag_dots(ui, lead, h.hovered() || h.dragged());
            if h.hovered() || h.dragged() {
                ui.ctx().set_cursor_icon(if h.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
            }
            Some(h.on_hover_text(t.drag_to_reorder))
        } else {
            let g = galley(ui, icons::FILL_LOCK_SIMPLE, font(13.0, theme::icons_fill()), p.t3);
            ui.painter().galley(lead.center() - g.size() / 2.0, g, p.t3);
            None
        };
        draw_row_body(ui, v, r, rect, selected);
        let resp = if editable {
            resp
        } else if v.my_role == Some(r.id) {
            resp.on_hover_text(t.own_role_tip)
        } else {
            resp.on_hover_text(t.above_role_tip)
        };
        (resp, handle)
    }

    /// Drag and drop of role rows, as with server cards: the lifted row
    /// follows the cursor, its place is a dashed slot, an accent line marks
    /// the drop. The line never goes above the viewer's own role.
    fn drag_rows(
        &mut self,
        ui: &mut egui::Ui,
        v: &View,
        rows: &[(RoleId, Rect, Option<egui::Response>)],
        send: &mut Vec<ClientMsg>,
    ) {
        for (id, rect, handle) in rows {
            if let Some(h) = handle
                && h.drag_started()
                && let Some(pos) = h.interact_pointer_pos()
            {
                self.drag = Some(RoleDrag { id: *id, grab: pos - rect.min, size: rect.size() });
            }
        }
        let Some(drag) = self.drag.as_ref() else { return };
        let (id, grab, size) = (drag.id, drag.grab, drag.size);
        let released = ui.input(|i| i.pointer.any_released() || !i.pointer.any_down());
        let (Some(from), Some(pointer)) = (v.roles.index(id), ui.ctx().pointer_interact_pos()) else {
            if released || v.roles.get(id).is_none() {
                self.drag = None;
            }
            return;
        };
        let p = pal(ui);
        // Index among the other roles; moving the role out does not shift the
        // viewer's own (it is above), so the limit stays the same.
        let others: Vec<Rect> = rows.iter().filter(|(r, ..)| *r != id).map(|(_, rect, _)| *rect).collect();
        let slot =
            others.iter().filter(|r| r.center().y < pointer.y).count().max(v.first_free_index()).min(others.len());
        let y = match (slot.checked_sub(1).and_then(|s| others.get(s)), others.get(slot)) {
            (Some(above), Some(below)) => Some((above.bottom() + below.top()) / 2.0),
            (Some(above), None) => Some(above.bottom() + 1.0),
            (None, Some(below)) => Some(below.top() - 1.0),
            (None, None) => None,
        };
        if let Some(y) = y.filter(|_| slot != from) {
            let x = rows.first().map_or(ui.max_rect().x_range(), |(_, r, _)| r.x_range());
            let painter =
                ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("role-drop")));
            painter.hline(x, y, Stroke::new(2.0, p.accent));
            painter.circle_filled(pos2(x.min, y), 4.0, p.accent);
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some(r) = v.roles.get(id) {
            egui::Area::new(egui::Id::unique("role-lifted"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pointer - grab)
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                    ui.painter().rect_filled(
                        rect.expand(1.0),
                        CornerRadius::same(theme::R_SM),
                        Color32::from_black_alpha(60),
                    );
                    ui.painter().rect(
                        rect,
                        CornerRadius::same(theme::R_SM),
                        p.pop,
                        Stroke::new(1.0, p.accent),
                        egui::StrokeKind::Inside,
                    );
                    let lead = Rect::from_min_size(pos2(rect.left() + 4.0, rect.top()), vec2(16.0, rect.height()));
                    super::servers::drag_dots(ui, lead, true);
                    draw_row_body(ui, v, r, rect, false);
                });
        }
        if released {
            self.drag = None;
            if slot != from {
                send.push(ClientMsg::MoveRole { role_id: id, index: slot as u32 });
            }
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui, v: &View, rect: Rect, send: &mut Vec<ClientMsg>) {
        let t = v.t;
        let p = pal(ui);
        let Some(id) = self.selected else { return };
        let Some(server) = v.roles.get(id) else { return };
        let editable = v.can_edit(id);
        let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - FOOTER_H), rect.max);
        ui.painter().hline(footer.x_range(), footer.top() + 0.5, Stroke::new(1.0, p.border));
        let body = Rect::from_min_max(rect.min, pos2(rect.right(), footer.top()));
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(body.intersect(ui.clip_rect()));
        let error = self.draft.as_ref().and_then(|d| name_error(v, d));
        let mut submitted = false;
        egui::ScrollArea::vertical().id_salt("role-editor").auto_shrink([false, false]).show(&mut child, |ui| {
            egui::Frame::new().inner_margin(Margin::symmetric(20, 16)).show(ui, |ui| {
                ui.set_width(body.width() - 40.0);
                ui.spacing_mut().item_spacing.y = 18.0;
                if !editable {
                    banner(ui, &self.banner_text(v, server));
                }
                let Some(draft) = self.draft.as_mut() else { return };
                let base_perms = self.base.as_ref().map_or(draft.perms, |b| b.perms);
                ui.add_enabled_ui(editable, |ui| {
                    ui.spacing_mut().item_spacing.y = 18.0;
                    submitted = name_field(ui, v, server, draft, error.as_deref());
                    color_picker(ui, t, draft);
                    icon_picker(ui, t, draft);
                });
                perm_grid(ui, v, server, draft, base_perms, editable);
                preview(ui, v, draft);
            });
        });

        // Footer: Delete on the left; Cancel and Save while there are changes.
        if !editable {
            return;
        }
        let inner = footer.shrink2(vec2(20.0, 0.0));
        let mut left = ui
            .new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
        left.add_space(-10.0);
        let del = Button::ghost(t.delete_role).with_icon(icons::TRASH).icon_color(p.danger);
        if left.add(del).clicked() {
            self.delete = Some(id);
        }
        let mut right = ui
            .new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
        right.spacing_mut().item_spacing.x = 8.0;
        if self.dirty() {
            // Enter in the name field saves too.
            let enter = submitted && error.is_none() && !self.dialog_open();
            if right.add_enabled(error.is_none(), Button::primary(t.save)).clicked() || enter {
                self.save(v, send);
            }
            if right.add(Button::secondary(t.cancel)).clicked() {
                self.draft = self.base.clone();
            }
            right.add_space(4.0);
            right.label(theme::text(t.unsaved_changes, 12.0).color(p.t2));
            let (dot, _) = right.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
            right.painter().circle_filled(dot.center(), 3.0, p.warn);
        } else {
            right.label(theme::text(t.no_changes, 12.0).color(p.t3));
        }
    }

    /// Why the role cannot be changed.
    fn banner_text(&self, v: &View, role: &RoleInfo) -> String {
        let t = v.t;
        if role.id == shared::OWNER_ROLE {
            t.owner_role_banner.into()
        } else if v.my_role == Some(role.id) {
            t.own_role_banner.into()
        } else {
            let mine = roles::name_of(t, v.roles, v.my_role);
            fill(t.above_role_banner, &[("name", &roles::name(t, role)), ("mine", &mine)])
        }
    }

    /// "Delete role?" and "Save changes?". Returns `true` when the panel
    /// should close (the answer to a pending close).
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, v: &View, send: &mut Vec<ClientMsg>) -> bool {
        let t = v.t;
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let mut close = false;
        if let Some(id) = self.delete {
            let name = v.roles.get(id).map(|r| roles::name(t, r)).unwrap_or_default();
            let n = v.count(Some(id)).map_or("?".into(), |n| n.to_string());
            let mut keep = true;
            let r = Dialog::new("role-delete", fill(t.delete_role_title, &[("name", &name)]))
                .icon(icons::TRASH, p.danger)
                .show(
                    ctx,
                    |ui| {
                        ui.add(
                            egui::Label::new(theme::text(fill(t.delete_role_text, &[("n", &n)]), 13.0).color(p.t2))
                                .wrap(),
                        );
                    },
                    |ui| {
                        if ui.add(Button::danger(t.delete_role)).clicked() {
                            send.push(ClientMsg::DeleteRole { role_id: id });
                            keep = false;
                        }
                        if ui.add(Button::secondary(t.cancel)).clicked() {
                            keep = false;
                        }
                    },
                );
            if !keep || r.close_requested {
                self.delete = None;
            }
        }
        if let Some(pending) = self.pending {
            let name = self.base.as_ref().map(|r| r.name.clone()).unwrap_or_default();
            let can_save = self.draft.as_ref().is_some_and(|d| name_error(v, d).is_none());
            let mut answer = None;
            let r = Dialog::new("role-save", fill(t.save_role_title, &[("name", &name)])).show(
                ctx,
                |ui| {
                    ui.add(egui::Label::new(theme::text(t.save_role_text, 13.0).color(p.t2)).wrap());
                },
                |ui| {
                    if ui.add_enabled(can_save, Button::primary(t.save)).clicked() {
                        answer = Some(true);
                    }
                    if ui.add(Button::secondary(t.dont_save)).clicked() {
                        answer = Some(false);
                    }
                    if ui.add(Button::ghost(t.cancel)).clicked() {
                        self.pending = None;
                    }
                },
            );
            if r.close_requested {
                self.pending = None;
            }
            if let Some(save) = answer {
                self.pending = None;
                if save {
                    self.save(v, send);
                }
                // Either way the edits are done with: saved or dropped.
                self.draft = self.base.clone();
                close = self.perform(pending, v, send);
            }
        }
        close
    }
}

/// Icon, name and user count of a row (the lead column is drawn by the caller).
fn draw_row_body(ui: &egui::Ui, v: &View, r: &RoleInfo, rect: Rect, selected: bool) {
    let p = pal(ui);
    let color = roles::color(p, &r.color);
    let icon = Rect::from_min_size(pos2(rect.left() + 28.0, rect.top()), vec2(16.0, rect.height()));
    if let Some(g) = icon_galley(ui, &r.icon, 14.0, color) {
        ui.painter().galley(icon.center() - g.size() / 2.0, g, color);
    }
    let mut right = rect.right() - 8.0;
    if let Some(n) = v.count(Some(r.id)) {
        right = put_right(ui, rect, right, galley(ui, &n.to_string(), font(11.5, FontFamily::Monospace), p.t3)).left()
            - 8.0;
    }
    let family = if selected { theme::semibold() } else { theme::medium() };
    let x = icon.right() + 8.0;
    let name = roles::name(v.t, r);
    let g = galley_cut(ui, &name, font(13.0, family.clone()), color, right - x);
    let cut = g.size().x + 1.0 < galley(ui, &name, font(13.0, family), color).size().x;
    put(ui, rect, x, g);
    if cut {
        // The full name when it does not fit.
        ui.interact(rect, egui::Id::unique(("role-name", r.id)), Sense::hover()).on_hover_text(name);
    }
}

fn banner(ui: &mut egui::Ui, text: &str) {
    let p = pal(ui);
    egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(theme::R_MD))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new(icons::FILL_LOCK_SIMPLE).family(theme::icons_fill()).size(15.0).color(p.t3));
                ui.add(egui::Label::new(theme::text(text, 12.5).color(p.t2)).wrap());
            });
        });
}

/// A field title with an optional note on the right (counter, color name).
fn field_title(ui: &mut egui::Ui, title: &str, right: Option<&str>) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    put(ui, rect, rect.left(), galley(ui, title, font(12.0, FontFamily::Proportional), p.t2));
    if let Some(r) = right {
        put_right(ui, rect, rect.right(), galley(ui, r, font(11.0, FontFamily::Monospace), p.t3));
    }
}

/// Returns `true` when Enter was pressed in the field.
fn name_field(ui: &mut egui::Ui, v: &View, server: &RoleInfo, draft: &mut RoleInfo, error: Option<&str>) -> bool {
    let t = v.t;
    let p = pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let counter = format!("{} / {}", draft.name.chars().count(), shared::MAX_ROLE_NAME_CHARS);
        field_title(ui, t.role_name, Some(&counter));
        let field =
            theme::text_field(&mut draft.name).char_limit(shared::MAX_ROLE_NAME_CHARS).desired_width(f32::INFINITY);
        let resp = ui.add(field);
        let submitted = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if error.is_some() {
            ui.painter().rect_stroke(
                resp.rect,
                CornerRadius::same(theme::R_SM),
                Stroke::new(1.0, p.danger),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(e) = error {
            ui.label(theme::text(format!("{}  {e}", icons::WARNING_CIRCLE), 12.0).color(p.danger));
        } else {
            let untouched = Roles::defaults().get(server.id).is_some_and(|d| d.name == server.name);
            if untouched && draft.name == roles::name(t, server) && roles::name(t, server) != server.name {
                ui.label(theme::text(t.default_role_note, 12.0).color(p.t3));
            }
        }
        submitted
    })
    .inner
}

fn color_picker(ui: &mut egui::Ui, t: &Tr, draft: &mut RoleInfo) {
    let p = pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        // An unknown name shows as gray, the way it is drawn.
        let shown = if roles::COLORS.contains(&draft.color.as_str()) { draft.color.clone() } else { "gray".into() };
        field_title(ui, t.role_color, Some(&shown));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.add_space(2.0);
            for (i, name) in roles::COLORS.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                let c = p.roles[i];
                if *name == shown {
                    ui.painter().circle_stroke(rect.center(), 16.5, Stroke::new(2.0, p.t1));
                }
                ui.painter().circle_filled(rect.center(), 13.0, c);
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.on_hover_text(*name).clicked() {
                    draft.color = (*name).into();
                }
            }
        });
    });
}

fn icon_picker(ui: &mut egui::Ui, t: &Tr, draft: &mut RoleInfo) {
    let p = pal(ui);
    let color = roles::color(p, &draft.color);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        field_title(ui, t.role_icon, None);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
            for name in roles::ICONS {
                let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                let on = draft.icon == name;
                let (bg, border) = if on {
                    (p.sel, p.accent)
                } else if resp.hovered() {
                    (p.hover, p.border)
                } else {
                    (Color32::TRANSPARENT, p.border)
                };
                ui.painter().rect(
                    rect,
                    CornerRadius::same(theme::R_SM),
                    bg,
                    Stroke::new(1.0, border),
                    egui::StrokeKind::Inside,
                );
                let c = if on { color } else { p.t2 };
                let g = icon_galley(ui, name, 16.0, c)
                    .unwrap_or_else(|| galley(ui, icons::CIRCLE_DASHED, font(16.0, FontFamily::Proportional), c));
                ui.painter().galley(rect.center() - g.size() / 2.0, g, c);
                let tip = if name.is_empty() { t.role_no_icon } else { name };
                if resp.on_hover_text(tip).clicked() {
                    draft.icon = name.into();
                }
            }
        });
    });
}

/// The rights as checkboxes in two columns. A right the viewer lacks is
/// locked (the server would refuse to switch it); a changed one is tinted
/// until saved.
fn perm_grid(ui: &mut egui::Ui, v: &View, server: &RoleInfo, draft: &mut RoleInfo, base: Perms, editable: bool) {
    let t = v.t;
    // The owner has every right whatever the list says.
    let shown = if server.id == shared::OWNER_ROLE { Perms::ALL } else { draft.perms };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        field_title(ui, t.role_perms, None);
        let col_w = (ui.available_width() - 12.0) / 2.0;
        for pair in perm_rows(t).chunks(2) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for &(perm, title, desc) in pair {
                    let mine = v.perms.contains(perm);
                    let enabled = editable && mine;
                    let changed = Perms::from_bits(base.bits() ^ draft.perms.bits()).contains(perm) && editable;
                    let on = shown.contains(perm);
                    let resp = ui
                        .allocate_ui(vec2(col_w, 0.0), |ui| {
                            ui.add_enabled_ui(enabled, |ui| perm_item(ui, col_w, on, title, desc, !enabled, changed))
                                .inner
                        })
                        .inner;
                    if enabled && resp.clicked() {
                        draft.perms = Perms::from_bits(draft.perms.bits() ^ perm.bits());
                    }
                    if editable && !mine {
                        resp.on_disabled_hover_text(t.perm_locked_tip);
                    }
                }
            });
        }
    });
}

fn perm_item(
    ui: &mut egui::Ui,
    width: f32,
    on: bool,
    title: &str,
    desc: &str,
    locked: bool,
    changed: bool,
) -> egui::Response {
    let p = pal(ui);
    let desc_g = ui.painter().layout(desc.to_owned(), font(12.0, FontFamily::Proportional), p.t3, width - 44.0);
    let height = 16.0 + 4.0 + desc_g.size().y + 16.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let bg = if changed {
        Color32::from_rgba_unmultiplied(p.warn.r(), p.warn.g(), p.warn.b(), 20)
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(theme::R_SM), bg);
    let bx = Rect::from_min_size(rect.min + vec2(8.0, 9.0), vec2(18.0, 18.0));
    let radius = CornerRadius::same(theme::R_XS);
    if on {
        ui.painter().rect_filled(bx, radius, p.accent);
        let g = galley(ui, icons::CHECK, font(13.0, theme::bold()), p.on_accent);
        ui.painter().galley(bx.center() - g.size() / 2.0, g, p.on_accent);
    } else {
        let stroke = if resp.hovered() { p.t2 } else { p.t3 };
        ui.painter().rect_stroke(bx, radius, Stroke::new(1.5, stroke), egui::StrokeKind::Inside);
    }
    let x = bx.right() + 10.0;
    let title_color = if locked { p.t3 } else { p.t1 };
    let tg = galley(ui, title, font(13.0, theme::medium()), title_color);
    let ty = rect.top() + 8.0;
    let tw = tg.size().x;
    ui.painter().galley(pos2(x, ty), tg, title_color);
    if locked {
        let lg = galley(ui, icons::FILL_LOCK_SIMPLE, font(12.0, theme::icons_fill()), p.t3);
        ui.painter().galley(pos2(x + tw + 6.0, ty + 2.0), lg, p.t3);
    }
    ui.painter().galley(pos2(x, ty + 20.0), desc_g, p.t3);
    resp
}

/// The role as others will see it: a nick in the channel list and the chip.
fn preview(ui: &mut egui::Ui, v: &View, draft: &RoleInfo) {
    let t = v.t;
    let p = pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        field_title(ui, t.role_preview, None);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover());
        ui.painter().rect(
            rect,
            CornerRadius::same(theme::R_MD),
            p.bg2,
            Stroke::new(1.0, p.border),
            egui::StrokeKind::Inside,
        );
        let color = roles::color(p, &draft.color);
        // Someone who has the role, else the viewer.
        let nick = v
            .users
            .and_then(|u| u.iter().find(|u| u.role == Some(draft.id)))
            .map_or(v.my_nick, |u| u.nickname.as_str());
        let mut x = rect.left() + 14.0;
        ui.painter().circle_filled(pos2(x + 4.0, rect.center().y), 4.0, p.dot_idle);
        x = put(ui, rect, x + 16.0, galley(ui, nick, font(13.0, theme::medium()), color)) + 6.0;
        if let Some(g) = icon_galley(ui, &draft.icon, 12.0, color) {
            x = put(ui, rect, x, g);
        }
        x += 16.0;
        ui.painter().vline(x, (rect.center().y - 10.0)..=(rect.center().y + 10.0), Stroke::new(1.0, p.border));
        let shown = if draft.name.is_empty() { roles::name(t, draft) } else { draft.name.clone() };
        chip(ui, rect, x + 16.0, draft, &shown);
    });
}

/// A role chip at `x` in `row`: 22 px high, at most 220 px wide.
fn chip(ui: &egui::Ui, row: Rect, x: f32, role: &RoleInfo, text: &str) {
    let p = pal(ui);
    let color = roles::color(p, &role.color);
    let ig = icon_galley(ui, &role.icon, 12.0, color);
    let icon_w = ig.as_ref().map_or(0.0, |g| g.size().x + 5.0);
    let tg = galley_cut(ui, text, font(12.0, theme::semibold()), color, 220.0 - 16.0 - icon_w);
    let rect = Rect::from_min_size(pos2(x, row.center().y - 11.0), vec2(icon_w + tg.size().x + 16.0, 22.0));
    ui.painter().rect_filled(rect, CornerRadius::same(5), roles::chip_bg(p, color));
    let mut x = rect.left() + 8.0;
    if let Some(g) = ig {
        x = put(ui, rect, x, g) + 5.0;
    }
    put(ui, rect, x, tg);
}
