//! Server list (before connecting) and the bookmark editor.

use egui::{CornerRadius, Margin, Stroke, vec2};

use super::*;
use crate::icons;
use crate::theme::{self, Button, Dialog, Kind, pal};

/// How often the start screen asks the servers for their status.
const PROBE_INTERVAL: Duration = Duration::from_secs(15);

/// State of the server list (the bookmarks themselves are in `cfg`).
pub(super) struct ServerList {
    /// Status of each bookmark's server by address, and when it was asked.
    pub(super) probes: HashMap<String, (net::ServerProbe, Instant)>,
    /// Addresses being asked right now.
    pub(super) probing: HashSet<String>,
    probe_tx: std::sync::mpsc::Sender<(String, net::ServerProbe)>,
    probe_rx: std::sync::mpsc::Receiver<(String, net::ServerProbe)>,
    /// When the last status answer arrived, for "12 s ago".
    pub(super) probes_updated: Option<Instant>,
    /// Ask all servers again now (the refresh button, a failed connect).
    pub(super) force_probe: bool,
    /// A server card being dragged to a new place.
    pub(super) card_drag: Option<CardDrag>,
    /// Search text (the field shows from `SEARCH_FROM` servers).
    pub(super) search: String,
    /// Collapsed groups: favorites, others.
    pub(super) collapsed: [bool; 2],
    /// The bookmark editor dialog.
    pub(super) editor: Option<BookmarkEditor>,
}

impl ServerList {
    pub(super) fn new() -> Self {
        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        Self {
            probes: HashMap::new(),
            probing: HashSet::new(),
            probe_tx,
            probe_rx,
            probes_updated: None,
            force_probe: false,
            card_drag: None,
            search: String::new(),
            collapsed: [false, false],
            editor: None,
        }
    }

    /// Takes the status answers that arrived.
    pub(super) fn receive_probes(&mut self) {
        while let Ok((address, probe)) = self.probe_rx.try_recv() {
            self.probing.remove(&address);
            self.probes.insert(address, (probe, Instant::now()));
            self.probes_updated = Some(Instant::now());
        }
    }

    /// Asks every bookmark's server for its status, at most every
    /// `PROBE_INTERVAL`, while the server list is on screen.
    pub(super) fn refresh(&mut self, servers: &[ServerBookmark], rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        let force = std::mem::take(&mut self.force_probe);
        for bm in servers {
            let address = bm.address.trim().to_string();
            if address.is_empty() || self.probing.contains(&address) {
                continue;
            }
            if !force && self.probes.get(&address).is_some_and(|(_, at)| at.elapsed() < PROBE_INTERVAL) {
                continue;
            }
            self.probing.insert(address.clone());
            let (tx, ctx, fp) = (self.probe_tx.clone(), ctx.clone(), bm.cert_fingerprint.clone());
            rt.spawn(async move {
                let probe = net::probe(address.clone(), fp).await;
                let _ = tx.send((address, probe));
                ctx.request_repaint();
            });
        }
        ctx.request_repaint_after(PROBE_INTERVAL);
    }
}

pub(super) struct BookmarkEditor {
    pub(super) index: Option<usize>,
    pub(super) data: ServerBookmark,
    /// Validation error and the field it belongs to.
    pub(super) error: Option<(EditorField, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorField {
    Address,
    Nickname,
    Password,
}

/// Whether two bookmark addresses name the same host and port: case does
/// not matter, and a missing port means the default one.
pub(super) fn same_address(a: &str, b: &str) -> bool {
    fn norm(addr: &str) -> String {
        let addr = addr.trim().to_lowercase();
        if addr.starts_with('[') || addr.matches(':').count() == 1 {
            addr
        } else if addr.contains(':') {
            format!("[{addr}]:{}", shared::DEFAULT_PORT)
        } else {
            format!("{addr}:{}", shared::DEFAULT_PORT)
        }
    }
    norm(a) == norm(b)
}

impl App {
    pub(super) fn draw_server_list(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let t = self.t();
        let logo = self.logo(root.ctx());
        let busy = self.conn != ConnState::Idle;
        let p = pal(root);

        let custom_frame = !self.cfg.system_frame;
        let right = if custom_frame { 0 } else { 14 };
        let top = egui::Frame::new().fill(p.bg2).inner_margin(Margin { left: 18, right, top: 0, bottom: 0 });
        egui::Panel::top("start-top").exact_size(52.0).frame(top).show(root, |ui| {
            window_frame::title_bar(ui, custom_frame, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if let Some(logo) = &logo {
                        ui.add(egui::Image::new((logo.id(), vec2(26.0, 26.0))).corner_radius(CornerRadius::same(7)));
                    }
                    ui.label(theme::text_bold(shared::APP_NAME, 16.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if custom_frame {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            window_frame::window_buttons(ui, t);
                            ui.add_space(8.0);
                        }
                        let resp = ui.add(Button::ghost(t.settings).with_icon(icons::GEAR_SIX).height(30.0));
                        let resp = match self.update_tooltip() {
                            Some(tip) => {
                                super::updates_ui::icon_dot(
                                    ui,
                                    resp.rect,
                                    if resp.hovered() { p.hover } else { p.bg2 },
                                );
                                resp.on_hover_text(tip)
                            }
                            None => resp,
                        };
                        if resp.clicked() {
                            actions.push(Action::OpenSettings);
                        }
                    });
                });
            });
        });

        if !busy {
            self.server_list.refresh(&self.cfg.servers, &self._rt, &self.ctx);
            // "12 s ago" ticks.
            root.ctx().request_repaint_after(Duration::from_secs(1));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.bg1)).show(root, |ui| {
            if self.cfg.servers.is_empty() && !busy {
                empty_state(ui, t, logo.as_ref(), actions);
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let width = (ui.available_width() - 32.0).min(580.0);
                let side = ((ui.available_width() - width) / 2.0).max(16.0);
                ui.add_space(48.0);
                ui.horizontal(|ui| {
                    ui.add_space(side);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 10.0;
                        self.list_header(ui, busy, actions);
                        // The crash banner comes before the update banner.
                        self.crash_banner(ui);
                        self.update_banner(ui);
                        if busy {
                            self.connect_banner(ui, actions);
                        }
                        self.server_cards(ui, busy, actions);
                    });
                });
                ui.add_space(24.0);
            });
        });
    }

    /// "Servers", search (from 8 servers), "↻ 12 s ago", "Add server".
    fn list_header(&mut self, ui: &mut egui::Ui, busy: bool, actions: &mut Vec<Action>) {
        let t = self.t();
        let p = pal(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(theme::text_semibold(t.servers, 20.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add = Button::secondary(t.add_server).with_icon(icons::PLUS);
                if ui.add_enabled(!busy, add).clicked() {
                    actions.push(Action::EditBookmark(None));
                }
                let ago = match (self.server_list.probing.is_empty(), self.server_list.probes_updated) {
                    (false, _) => "…".to_string(),
                    (true, Some(at)) => fill(t.updated_ago, &[("s", &at.elapsed().as_secs().to_string())]),
                    (true, None) => String::new(),
                };
                let refresh = Button::ghost(ago).with_icon(icons::ARROWS_CLOCKWISE).small();
                if ui.add_enabled(!busy, refresh).on_hover_text(t.refresh).clicked() {
                    actions.push(Action::RefreshStatus);
                }
                if self.cfg.servers.len() >= SEARCH_FROM {
                    let field = egui::Frame::new()
                        .fill(p.field)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(theme::R_SM))
                        .inner_margin(Margin::symmetric(10, 6));
                    let hint = theme::text(theme::with_icon(icons::MAGNIFYING_GLASS, t.search), 13.0).color(p.t3);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.server_list.search)
                            .frame(field)
                            .hint_text(hint)
                            .desired_width(ui.available_width() - 20.0),
                    );
                }
            });
        });
    }

    /// The cards in their groups, with search, collapsing and drag and drop.
    fn server_cards(&mut self, ui: &mut egui::Ui, busy: bool, actions: &mut Vec<Action>) {
        let t = self.t();
        let query = self.server_list.search.trim().to_lowercase();
        let matches = |b: &ServerBookmark| {
            query.is_empty() || [&b.name, &b.address, &b.nickname].iter().any(|f| f.to_lowercase().contains(&query))
        };
        let any_favorite = self.cfg.servers.iter().any(|b| b.favorite);
        // Reordering only when every card is visible: otherwise the drop place is unclear.
        let can_drag = !busy && query.is_empty() && !self.server_list.collapsed.iter().any(|c| *c);
        let mut cards: Vec<(usize, CardOut)> = Vec::new();
        let mut shown = 0;
        for (i, bm) in self.cfg.servers.iter().enumerate() {
            let group = usize::from(!bm.favorite);
            if any_favorite && (i == 0 || self.cfg.servers[i - 1].favorite != bm.favorite) {
                let title = if bm.favorite { t.favorites } else { t.other_servers };
                if group_caption(ui, title, bm.favorite, self.server_list.collapsed[group]).clicked() {
                    self.server_list.collapsed[group] = !self.server_list.collapsed[group];
                }
            }
            if (any_favorite && self.server_list.collapsed[group]) || !matches(bm) {
                continue;
            }
            shown += 1;
            let probe = self.server_list.probes.get(bm.address.trim()).map(|(p, _)| p);
            if let Some(d) = self.server_list.card_drag.as_ref().filter(|d| d.index == i) {
                // The lifted card follows the cursor; its place is a dashed slot.
                let (slot, _) = ui.allocate_exact_size(d.size, egui::Sense::hover());
                dashed_rect(ui, slot, pal(ui).border_strong);
                cards.push((i, CardOut { rect: slot, handle: None }));
                continue;
            }
            cards.push((i, server_card(ui, t, i, bm, probe, busy, can_drag, actions)));
        }
        if shown == 0 && !query.is_empty() {
            ui.label(theme::text(t.nothing_found, 13.0).color(pal(ui).t3));
        }
        if can_drag {
            self.drag_cards(ui, &cards, actions);
        } else {
            self.server_list.card_drag = None;
        }
    }

    /// Drag and drop of server cards by their handles. The lifted card follows
    /// the cursor, its old place is a dashed slot, and an accent line with a
    /// dot marks where it goes; on release it moves there (joining that group).
    fn drag_cards(&mut self, ui: &mut egui::Ui, cards: &[(usize, CardOut)], actions: &mut Vec<Action>) {
        for (i, c) in cards {
            if let Some(h) = &c.handle
                && h.drag_started()
                && let Some(pos) = h.interact_pointer_pos()
            {
                self.server_list.card_drag = Some(CardDrag { index: *i, grab: pos - c.rect.min, size: c.rect.size() });
            }
        }
        let Some(drag) = self.server_list.card_drag.clone() else { return };
        let from_slot = cards.iter().position(|(i, _)| *i == drag.index).unwrap_or(drag.index);
        // Every early exit below must still end the drag on release, or the
        // card stays a dashed slot (e.g. a single card has nowhere to go).
        let released = ui.input(|i| i.pointer.any_released() || !i.pointer.any_down());
        let Some(pointer) = ui.ctx().pointer_interact_pos() else {
            if released {
                self.server_list.card_drag = None;
            }
            return;
        };
        let p = pal(ui);
        // Slot among the other cards: how many of them are above the pointer.
        let others: Vec<egui::Rect> = cards.iter().filter(|(i, _)| *i != drag.index).map(|(_, c)| c.rect).collect();
        let slot = others.iter().filter(|r| r.center().y < pointer.y).count();
        let y = match (slot.checked_sub(1).and_then(|s| others.get(s)), others.get(slot)) {
            (Some(above), Some(below)) => Some((above.bottom() + below.top()) / 2.0),
            (Some(above), None) => Some(above.bottom() + 5.0),
            (None, Some(below)) => Some(below.top() - 5.0),
            (None, None) => None,
        };
        if let Some(y) = y.filter(|_| slot != from_slot) {
            let x = ui.max_rect().x_range();
            let painter =
                ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("card-drop")));
            painter.hline(x, y, Stroke::new(2.0, p.accent));
            painter.circle_filled(egui::pos2(x.min, y), 4.0, p.accent);
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);

        // The lifted card, drawn at the cursor.
        if let Some(bm) = self.cfg.servers.get(drag.index) {
            let t = self.t();
            let probe = self.server_list.probes.get(bm.address.trim()).map(|(p, _)| p);
            egui::Area::new(egui::Id::unique("card-lifted"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pointer - drag.grab)
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    ui.set_width(drag.size.x);
                    let mut ignored = Vec::new();
                    lifted_card(ui, t, drag.index, bm, probe, &mut ignored);
                });
        }
        if released {
            self.server_list.card_drag = None;
            if slot != from_slot {
                actions.push(Action::MoveBookmark { from: drag.index, slot });
            }
        }
    }

    /// "Connecting…" or "Reconnecting (attempt N)…" with Cancel.
    fn connect_banner(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let t = self.t();
        let p = pal(ui);
        let (text, _) = self.status_text();
        let reconnecting = matches!(self.conn, ConnState::Reconnecting(_));
        let (fill, stroke, spin) = if reconnecting {
            (p.warn.gamma_multiply(0.10), p.warn.gamma_multiply(0.30), p.warn)
        } else {
            (p.field, p.border, p.t2)
        };
        egui::Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(theme::R_MD))
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    ui.add(egui::Spinner::new().size(16.0).color(spin));
                    ui.label(theme::text(text, 13.0).color(p.t1));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(Button::secondary(t.cancel).height(30.0)).clicked() {
                            actions.push(Action::Disconnect);
                        }
                    });
                });
            });
    }

    pub(super) fn draw_editor(&mut self, ctx: &egui::Context) {
        let Some(mut ed) = self.server_list.editor.take() else { return };
        let t = self.t();
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let mut save = false;
        // "Save and connect", the form's main action (also Enter).
        let mut connect = false;
        let mut cancel = false;
        let can_connect = self.conn == ConnState::Idle;
        let title = if ed.index.is_some() { t.edit_server } else { t.new_server };
        let response = Dialog::new("bookmark-editor", title).width(520.0).closable().show(
            ctx,
            |ui| {
                // Label column 116 px, gap 14 px, the field takes the rest.
                let w = ui.available_width() - 116.0 - 14.0;
                egui::Grid::new("bookmark")
                    .num_columns(2)
                    .spacing([14.0, 12.0])
                    .min_col_width(116.0)
                    .striped(false)
                    .show(ui, |ui| {
                        theme::field_label(ui, t.name);
                        ui.add(theme::text_field(&mut ed.data.name).hint_text(t.new_server).desired_width(w));
                        ui.end_row();
                        theme::field_label(ui, t.address);
                        ui.add(theme::text_field(&mut ed.data.address).hint_text(t.address_hint).desired_width(w));
                        ui.end_row();
                        if let Some((kind, e)) = &ed.error
                            && *kind == EditorField::Address
                        {
                            ui.label("");
                            theme::note(ui, theme::Note::Warn, e);
                            ui.end_row();
                        }
                        theme::field_label(ui, t.nickname);
                        ui.add(
                            theme::text_field(&mut ed.data.nickname)
                                .char_limit(shared::MAX_NICKNAME_CHARS)
                                .desired_width(w),
                        );
                        ui.end_row();
                        if let Some((kind, e)) = &ed.error
                            && *kind == EditorField::Nickname
                        {
                            ui.label("");
                            theme::note(ui, theme::Note::Error, e);
                            ui.end_row();
                        }
                        theme::field_label(ui, t.server_password);
                        ui.add(
                            theme::text_field(&mut ed.data.password)
                                .password(true)
                                .hint_text(t.password_hint)
                                .desired_width(w),
                        );
                        ui.end_row();
                        if let Some((kind, e)) = &ed.error
                            && *kind == EditorField::Password
                        {
                            ui.label("");
                            theme::note(ui, theme::Note::Error, e);
                            ui.end_row();
                        }
                        // No "forget" button: a changed certificate goes through the
                        // "certificate changed" prompt, which shows both fingerprints.
                        theme::field_label(ui, t.fingerprint);
                        let text = ed.data.cert_fingerprint.clone().unwrap_or_else(|| t.fingerprint_none.to_string());
                        egui::Frame::new()
                            .stroke(Stroke::new(1.0, p.border))
                            .corner_radius(CornerRadius::same(theme::R_SM))
                            .inner_margin(Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(w - 20.0);
                                ui.add(egui::Label::new(theme::mono(text, 11.5).color(p.t2)).truncate())
                                    .on_hover_text(ed.data.cert_fingerprint.as_deref().unwrap_or(""));
                            });
                        ui.end_row();
                    });
            },
            |ui| {
                connect |= ui.add_enabled(can_connect, Button::primary(t.save_and_connect)).clicked();
                save |= ui.add(Button::secondary(t.save)).clicked();
                cancel |= ui.add(Button::secondary(t.cancel)).clicked();
            },
        );

        // Enter does the main action, as in the other dialogs: save and
        // connect, or only save while another connection is in progress. A
        // single-line field gives up its focus on Enter, so the key is still
        // there to read.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            if can_connect {
                connect = true;
            } else {
                save = true;
            }
        }
        if save || connect {
            ed.data.address = ed.data.address.trim().to_string();
            ed.data.nickname = ed.data.nickname.trim().to_string();
            if ed.data.address.is_empty() {
                ed.error = Some((EditorField::Address, t.need_address.into()));
            } else if !shared::validate_nickname(&ed.data.nickname) {
                ed.error = Some((EditorField::Nickname, t.reject_nick_invalid.into()));
            } else if let Some(other) = self
                .cfg
                .servers
                .iter()
                .enumerate()
                .find(|(i, b)| Some(*i) != ed.index && same_address(&b.address, &ed.data.address))
            {
                // One server, one identity: a second bookmark would be the same
                // person, and different nicknames in them would keep renaming
                // the registration back and forth.
                ed.error = Some((EditorField::Address, fill(t.duplicate_server, &[("name", &other.1.name)])));
            } else {
                if ed.data.name.trim().is_empty() {
                    ed.data.name = ed.data.address.clone();
                }
                let index = match ed.index {
                    Some(i) if i < self.cfg.servers.len() => {
                        self.cfg.servers[i] = ed.data;
                        i
                    }
                    // New bookmarks go to the top, right under the favorites.
                    _ => {
                        let at = self.cfg.new_bookmark_index();
                        self.cfg.servers.insert(at, ed.data);
                        at
                    }
                };
                self.save_config();
                if connect {
                    self.connect(index);
                }
                return;
            }
        }
        if !cancel && !response.close_requested {
            self.server_list.editor = Some(ed);
        }
    }
}

fn empty_state(ui: &mut egui::Ui, t: &Tr, logo: Option<&egui::TextureHandle>, actions: &mut Vec<Action>) {
    let p = pal(ui);
    ui.vertical_centered(|ui| {
        ui.add_space(((ui.available_height() - 200.0) / 2.0).max(24.0));
        ui.spacing_mut().item_spacing.y = 14.0;
        if let Some(logo) = logo {
            ui.add(
                egui::Image::new((logo.id(), vec2(56.0, 56.0)))
                    .corner_radius(CornerRadius::same(14))
                    .tint(Color32::from_white_alpha(230)),
            );
        }
        ui.label(theme::text_semibold(t.no_servers_title, 16.0));
        ui.label(theme::text(t.no_servers, 13.0).color(p.t2));
        ui.add_space(4.0);
        if ui.add(Button::primary(t.add_server).with_icon(icons::PLUS).height(34.0)).clicked() {
            actions.push(Action::EditBookmark(None));
        }
    });
}

/// "today at 14:05", "yesterday at 09:12" or a date, for the last connect.
fn last_connected_text(t: &Tr, ts: i64) -> String {
    let now = shared::unix_now();
    if (0..60).contains(&(now - ts)) {
        return t.last_just_now.to_string();
    }
    let tz = jiff::tz::TimeZone::system();
    let day = |s: i64| jiff::Timestamp::from_second(s).ok().map(|x| x.to_zoned(tz.clone()).date());
    let (Some(then), Some(today)) = (day(ts), day(now)) else { return String::new() };
    let time = local_time(ts, "%H:%M");
    if then == today {
        fill(t.last_today, &[("time", &time)])
    } else if today.yesterday().ok() == Some(then) {
        fill(t.last_yesterday, &[("time", &time)])
    } else {
        local_time(ts, "%d.%m.%Y")
    }
}

/// Search field appears from this many servers.
const SEARCH_FROM: usize = 8;

/// A card being dragged: which one, where it was grabbed, its size.
#[derive(Clone)]
pub(super) struct CardDrag {
    index: usize,
    grab: egui::Vec2,
    size: egui::Vec2,
}

pub(super) fn dashed_rect(ui: &egui::Ui, r: egui::Rect, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in pts.windows(2) {
        ui.painter().extend(egui::Shape::dashed_line(&[w[0], w[1]], stroke, 5.0, 4.0));
    }
}

/// A group caption ("Favorites" / "Other servers"); click collapses it.
fn group_caption(ui: &mut egui::Ui, title: &str, favorite: bool, collapsed: bool) -> egui::Response {
    let p = pal(ui);
    let caret = if collapsed { icons::CARET_RIGHT } else { icons::CARET_DOWN };
    let icon = if favorite {
        RichText::new(icons::FILL_STAR).family(theme::icons_fill()).size(11.0).color(p.gold)
    } else {
        RichText::new(icons::LIST).size(11.0).color(p.t3)
    };
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add(egui::Label::new(icon).selectable(false));
            ui.add(egui::Label::new(theme::caption(ui, title)).selectable(false));
            ui.add(egui::Label::new(RichText::new(caret).size(11.0).color(p.t3)).selectable(false));
        })
        .response
        .interact(egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// The status line of a card: signal bars, online count and ping, or why
/// there is none. The ping is coloured only when fair or poor.
fn probe_line(ui: &mut egui::Ui, t: &Tr, probe: Option<&net::ServerProbe>) {
    let p = pal(ui);
    let ping = |ms: u32| {
        let q = super::link::ping_quality(ms as f32);
        let color = if q == theme::Quality::Good { p.t3 } else { q.color(p) };
        (fill(t.probe_ms, &[("ms", &ms.to_string())]), color, q)
    };
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            match probe {
                None => {
                    theme::signal_bars(ui, None);
                    ui.add(egui::Label::new(theme::text("…", 12.0).color(p.t3)).selectable(false));
                }
                Some(net::ServerProbe::Online { ping_ms, status }) => {
                    let (ms, color, q) = ping(*ping_ms);
                    theme::signal_bars(ui, Some(q));
                    let count = fill(
                        t.probe_online,
                        &[("n", &status.users_online.to_string()), ("max", &status.max_users.to_string())],
                    );
                    ui.add(egui::Label::new(theme::text(count, 12.0).color(p.t2)).selectable(false));
                    ui.add(egui::Label::new(theme::text("·", 12.0).color(p.t3)).selectable(false));
                    ui.add(egui::Label::new(theme::mono(ms, 11.5).color(color)).selectable(false));
                }
                Some(net::ServerProbe::Up { ping_ms }) => {
                    let (ms, color, q) = ping(*ping_ms);
                    theme::signal_bars(ui, Some(q));
                    ui.add(egui::Label::new(theme::text(t.probe_up, 12.0).color(p.t2)).selectable(false));
                    ui.add(egui::Label::new(theme::text("·", 12.0).color(p.t3)).selectable(false));
                    ui.add(egui::Label::new(theme::mono(ms, 11.5).color(color)).selectable(false));
                }
                Some(net::ServerProbe::CertChanged { .. }) => {
                    ui.add(
                        egui::Label::new(
                            RichText::new(icons::FILL_SHIELD_WARNING)
                                .family(theme::icons_fill())
                                .size(12.0)
                                .color(p.warn),
                        )
                        .selectable(false),
                    );
                    ui.add(egui::Label::new(theme::text_semibold(t.probe_cert, 12.0).color(p.warn)).selectable(false));
                }
                Some(net::ServerProbe::Offline) => {
                    ui.add(egui::Label::new(theme::text(icons::PLUGS, 12.0).color(p.t3)).selectable(false));
                    ui.add(egui::Label::new(theme::text(t.probe_offline, 12.0).color(p.t3)).selectable(false));
                }
            }
        })
        .response;
    if let Some(net::ServerProbe::Online { status, .. }) = probe {
        resp.on_hover_text(fill(t.probe_tip, &[("name", &status.name), ("version", &status.version)]));
    }
}

/// Drag handle: six dots. Returns its response (drag sense).
fn drag_handle(ui: &mut egui::Ui, t: &Tr, enabled: bool) -> egui::Response {
    let p = pal(ui);
    let sense = if enabled { egui::Sense::drag() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 40.0), sense);
    let color = if enabled && (resp.hovered() || resp.dragged()) { p.t2 } else { p.dot_idle };
    paint_dots(ui, rect, color);
    if enabled && (resp.hovered() || resp.dragged()) {
        ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    }
    if enabled { resp.on_hover_text(t.drag_to_reorder) } else { resp }
}

fn paint_dots(ui: &egui::Ui, rect: egui::Rect, color: Color32) {
    for row in 0..3 {
        for col in 0..2 {
            let c = rect.center() + vec2(-3.0 + col as f32 * 6.0, -6.0 + row as f32 * 6.0);
            ui.painter().circle_filled(c, 1.6, color);
        }
    }
}

/// The six dots of a drag handle in `rect`, brighter when `active`.
pub(super) fn drag_dots(ui: &egui::Ui, rect: egui::Rect, active: bool) {
    let p = pal(ui);
    paint_dots(ui, rect, if active { p.t2 } else { p.dot_idle });
}

struct CardOut {
    rect: egui::Rect,
    /// `None` for the empty slot of a dragged card.
    handle: Option<egui::Response>,
}

/// How a card looks: normal, offline (grey name, secondary Connect) or with a
/// changed certificate (orange, "Review…" instead of Connect).
fn card_state(probe: Option<&net::ServerProbe>) -> (bool, Option<&str>) {
    match probe {
        Some(net::ServerProbe::Offline) => (true, None),
        Some(net::ServerProbe::CertChanged { actual }) => (false, Some(actual.as_str())),
        _ => (false, None),
    }
}

#[allow(clippy::too_many_arguments)]
fn card_contents(
    ui: &mut egui::Ui,
    t: &Tr,
    i: usize,
    bm: &ServerBookmark,
    probe: Option<&net::ServerProbe>,
    can_drag: bool,
    hovered: bool,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let p = pal(ui);
    let (offline, cert_changed) = card_state(probe);
    let mut handle = None;
    ui.horizontal(|ui| {
        handle = Some(drag_handle(ui, t, can_drag));
        ui.add_space(2.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let title = if bm.name.is_empty() { &bm.address } else { &bm.name };
            let title_color = if offline { p.t2 } else { p.t1 };
            // Not selectable, so a double-click on the text reaches the card.
            ui.add(egui::Label::new(theme::text_semibold(title, 14.0).color(title_color)).selectable(false));
            let nick = if bm.nickname.is_empty() { "?" } else { &bm.nickname };
            let mut sub = format!("{} · {nick}", bm.address);
            if let Some(ts) = bm.last_connected {
                sub.push_str(" · ");
                sub.push_str(&last_connected_text(t, ts));
            }
            ui.add(egui::Label::new(theme::text(sub, 12.0).color(p.t3)).selectable(false));
            probe_line(ui, t, probe);
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            match cert_changed {
                Some(actual) => {
                    let review = Button::new(Kind::Secondary, t.review_cert)
                        .with_fill_icon(icons::FILL_SHIELD_WARNING)
                        .icon_color(p.warn);
                    if ui.add(review).clicked() {
                        actions.push(Action::ReviewCert(i, actual.to_string()));
                    }
                }
                None => {
                    let kind = if offline { Kind::Secondary } else { Kind::Primary };
                    if ui.add(Button::new(kind, t.connect)).clicked() {
                        actions.push(Action::Connect(i));
                    }
                }
            }
            ui.add_space(8.0);
            // Edit and delete only on hover, so a long list stays calm.
            if hovered {
                if ui.add(Button::icon(icons::TRASH).height(30.0)).on_hover_text(t.delete).clicked() {
                    actions.push(Action::DeleteBookmark(i));
                }
                if ui.add(Button::icon(icons::PENCIL_SIMPLE).height(30.0)).on_hover_text(t.edit).clicked() {
                    actions.push(Action::EditBookmark(Some(i)));
                }
            }
            if bm.favorite || hovered {
                let star = if bm.favorite {
                    Button::icon(icons::FILL_STAR).with_fill_icon(icons::FILL_STAR).icon_color(p.gold)
                } else {
                    Button::icon(icons::STAR)
                };
                let tip = if bm.favorite { t.favorite_remove } else { t.favorite_add };
                if ui.add(star.height(30.0)).on_hover_text(tip).clicked() {
                    actions.push(Action::ToggleFavorite(i));
                }
            }
        });
    });
    handle.expect("drawn above")
}

fn card_frame(ui: &egui::Ui, probe: Option<&net::ServerProbe>, lifted: bool) -> egui::Frame {
    let p = pal(ui);
    let cert = matches!(probe, Some(net::ServerProbe::CertChanged { .. }));
    let (fill, stroke) = if cert {
        (p.warn.gamma_multiply(0.06).blend(p.field), p.warn.gamma_multiply(0.7))
    } else if lifted {
        (p.field, p.accent)
    } else {
        (p.field, p.border)
    };
    let mut frame = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(theme::R_MD))
        .inner_margin(Margin { left: 8, right: 14, top: 8, bottom: 8 });
    if lifted {
        frame =
            frame.shadow(egui::Shadow { offset: [0, 10], blur: 24, spread: 0, color: Color32::from_black_alpha(110) });
    }
    frame
}

/// The dragged card, drawn at the cursor (not interactive).
fn lifted_card(
    ui: &mut egui::Ui,
    t: &Tr,
    i: usize,
    bm: &ServerBookmark,
    probe: Option<&net::ServerProbe>,
    actions: &mut Vec<Action>,
) {
    card_frame(ui, probe, true).show(ui, |ui| {
        ui.set_width(ui.available_width());
        card_contents(ui, t, i, bm, probe, false, false, actions);
    });
}

#[allow(clippy::too_many_arguments)]
fn server_card(
    ui: &mut egui::Ui,
    t: &Tr,
    i: usize,
    bm: &ServerBookmark,
    probe: Option<&net::ServerProbe>,
    busy: bool,
    can_drag: bool,
    actions: &mut Vec<Action>,
) -> CardOut {
    let p = pal(ui);
    // Hover from the previous frame: the buttons inside are drawn before the
    // card knows its rect.
    let hover_id = egui::Id::unique(("card-hover", i));
    let hovered = !busy && ui.data(|d| d.get_temp::<bool>(hover_id)).unwrap_or(false);
    // The card's own click sense is registered below its children (UiBuilder::sense),
    // so the buttons inside still get their clicks.
    let card_ui = egui::UiBuilder::new().id_salt(("server-card", i)).sense(egui::Sense::click());
    let mut handle = None;
    let card = ui
        .add_enabled_ui(!busy, |ui| {
            ui.scope_builder(card_ui, |ui| {
                card_frame(ui, probe, false).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    handle = Some(card_contents(ui, t, i, bm, probe, can_drag, hovered, actions));
                });
            })
            .response
        })
        .inner;
    let now_hovered = ui.rect_contains_pointer(card.rect);
    if now_hovered != hovered {
        ui.data_mut(|d| d.insert_temp(hover_id, now_hovered));
        ui.ctx().request_repaint();
    }
    let out = CardOut { rect: card.rect, handle };
    if busy {
        return out;
    }
    if card.double_clicked() {
        actions.push(Action::Connect(i));
    }
    card.context_menu(|ui| {
        if ui.button(theme::with_icon(icons::SIGN_IN, t.connect)).clicked() {
            actions.push(Action::Connect(i));
            ui.close();
        }
        if ui.button(theme::with_icon(icons::PENCIL_SIMPLE, t.edit)).clicked() {
            actions.push(Action::EditBookmark(Some(i)));
            ui.close();
        }
        let star = if bm.favorite { t.favorite_remove } else { t.favorite_add };
        if ui.button(theme::with_icon(icons::STAR, star)).clicked() {
            actions.push(Action::ToggleFavorite(i));
            ui.close();
        }
        if ui.button(RichText::new(theme::with_icon(icons::TRASH, t.delete)).color(p.danger)).clicked() {
            actions.push(Action::DeleteBookmark(i));
            ui.close();
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::same_address;

    #[test]
    fn same_address_normalizes() {
        assert!(same_address("Example.org", "example.org:7777"));
        assert!(same_address(" 1.2.3.4:7777 ", "1.2.3.4"));
        assert!(!same_address("1.2.3.4:7778", "1.2.3.4"));
        assert!(same_address("::1", "[::1]:7777"));
        assert!(!same_address("a.org", "b.org"));
    }
}
