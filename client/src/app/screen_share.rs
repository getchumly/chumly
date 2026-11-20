//! Screen share: the share dialog, the viewer window and video frames.

use egui::{CornerRadius, FontFamily, FontId, Margin, Rect, Sense, Stroke, pos2, vec2};

use super::*;
use crate::icons;
use crate::theme::{self, Button, Dialog, pal};

/// Our screen share and the share we watch.
pub(super) struct ScreenState {
    /// State shared with the capture and decoder threads.
    pub(super) shared: Arc<ScreenShared>,
    /// Encoded fragments of our share to the network.
    pub(super) tx: mpsc::Sender<(MediaKind, Vec<u8>)>,
    pub(super) sharer: Option<Sharer>,
    pub(super) fps: FpsMeter,
    /// The server confirmed we are sharing. Until it does, a UserUpdated
    /// without the share flag does not mean the share was stopped.
    pub(super) announced: bool,
    pub(super) dialog: Option<ShareDialog>,
    pub(super) watching: Option<Watching>,
    /// The streamer's viewer list (the eye in the "me" block).
    pub(super) viewers_popover: super::popover::HoverPopover,
    /// The viewer list in the stream window.
    pub(super) watch_popover: super::popover::HoverPopover,
}

impl ScreenState {
    pub(super) fn new(shared: Arc<ScreenShared>, tx: mpsc::Sender<(MediaKind, Vec<u8>)>) -> Self {
        Self {
            shared,
            tx,
            sharer: None,
            fps: FpsMeter::new(),
            announced: false,
            dialog: None,
            watching: None,
            viewers_popover: Default::default(),
            watch_popover: Default::default(),
        }
    }
}

pub(super) struct ShareDialog {
    pub(super) sources: Vec<Source>,
    pub(super) selected: Option<SourceId>,
    pub(super) quality: Quality,
    pub(super) audio: bool,
}

/// Someone else's screen share being watched.
pub(super) struct Watching {
    pub(super) user_id: UserId,
    pub(super) name: String,
    pub(super) viewer: Viewer,
    pub(super) texture: Option<egui::TextureHandle>,
    /// When fullscreen was entered; `None` if not fullscreen.
    pub(super) fullscreen: Option<Instant>,
    pub(super) fps: FpsMeter,
    /// The viewer window is created with this size. The viewport description
    /// must not change while it is open, otherwise egui would move it every frame.
    pub(super) builder: egui::ViewportBuilder,
}

/// Frames per second from a counter, updated once per second.
pub(super) struct FpsMeter {
    pub(super) since: Instant,
    pub(super) base: u32,
    pub(super) fps: f32,
}

impl FpsMeter {
    pub(super) fn new() -> Self {
        Self { since: Instant::now(), base: 0, fps: 0.0 }
    }

    pub(super) fn update(&mut self, count: u32) {
        let elapsed = self.since.elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.fps = count.wrapping_sub(self.base) as f32 / elapsed.as_secs_f32();
            self.base = count;
            self.since = Instant::now();
        }
    }
}

pub(super) fn viewer_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("screen-viewer")
}

/// Draws a screen share frame centered in the available space,
/// preserving aspect ratio, on a black background.
pub(super) fn paint_video(ui: &mut egui::Ui, texture: Option<&egui::TextureHandle>, waiting: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    match texture {
        Some(tex) => {
            let size = tex.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let fitted = Rect::from_center_size(rect.center(), size * scale);
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            painter.image(tex.id(), fitted, uv, Color32::WHITE);
        }
        None => {
            let p = pal(ui);
            let spinner = Rect::from_center_size(rect.center() - vec2(0.0, 16.0), vec2(18.0, 18.0));
            egui::Spinner::new().size(18.0).color(p.accent).paint_at(ui, spinner);
            painter.text(
                rect.center() + vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                waiting,
                FontId::new(13.0, FontFamily::Proportional),
                p.t2,
            );
        }
    }
    resp
}

/// One selectable source row in the share dialog.
fn source_row(ui: &mut egui::Ui, icon: &str, name: &str, meta: &str, selected: bool) -> egui::Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let bg = if selected {
        p.sel
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(theme::R_SM), bg);
    let c = pos2(rect.left() + 16.0, rect.center().y);
    if selected {
        ui.painter().circle_stroke(c, 5.5, Stroke::new(5.0, p.accent));
    } else {
        ui.painter().circle_stroke(c, 7.25, Stroke::new(1.5, p.t3));
    }
    let g = ui.painter().layout_no_wrap(icon.to_owned(), FontId::new(15.0, FontFamily::Proportional), p.t3);
    let mut x = rect.left() + 34.0;
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
    x += 25.0;
    let mg = ui.painter().layout_no_wrap(meta.to_owned(), FontId::new(12.0, FontFamily::Monospace), p.t3);
    let meta_w = mg.size().x;
    ui.painter().galley(
        pos2(rect.right() - 8.0 - meta_w, rect.center().y - mg.size().y / 2.0),
        mg,
        Color32::PLACEHOLDER,
    );
    let max = (rect.right() - 20.0 - meta_w - x).max(20.0);
    let mut job =
        egui::text::LayoutJob::simple_singleline(name.to_owned(), FontId::new(13.0, FontFamily::Proportional), p.t1);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max);
    let ng = ui.painter().layout_job(job);
    ui.painter().galley(pos2(x, rect.center().y - ng.size().y / 2.0), ng, Color32::PLACEHOLDER);
    resp
}

impl App {
    /// Takes the latest screen share frame and updates the texture.
    pub(super) fn update_video(&mut self) {
        let Some(w) = &mut self.screen.watching else { return };
        w.fps.update(w.viewer.counters().frames.load(Ordering::Relaxed));
        // Keep the stats under the video updating even if the picture is frozen.
        self.ctx.request_repaint_after(Duration::from_millis(500));
        let Some(image) = w.viewer.take_frame() else { return };
        match &mut w.texture {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => w.texture = Some(self.ctx.load_texture("screen-share", image, egui::TextureOptions::LINEAR)),
        }
    }

    /// Screen share viewer in a separate OS window: it can be moved
    /// outside the main window and onto another monitor. Always dark.
    pub(super) fn draw_viewer(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let t = self.t();
        let Some(w) = &self.screen.watching else { return };
        let session = self.session.as_ref();
        let cfg = &self.cfg;
        let pop = &mut self.screen.watch_popover;
        let volume = self.cfg.stream_volume(&w.name);
        let custom_frame = !self.cfg.system_frame;
        ctx.show_viewport_immediate(viewer_viewport(), w.builder.clone(), |ui, _class| {
            ui.set_style(ui.ctx().style_of(egui::Theme::Dark));
            let p = &theme::DARK;
            if ui.input(|i| i.viewport().close_requested()) {
                actions.push(Action::StopWatch);
            }
            if w.fullscreen.is_none() {
                let right = if custom_frame { 0 } else { 14 };
                let strip = egui::Frame::new().fill(p.bg1).inner_margin(Margin { left: 14, right, top: 0, bottom: 0 });
                egui::Panel::top("viewer-strip").exact_size(40.0).frame(strip).show(ui, |ui| {
                    window_frame::title_bar(ui, custom_frame, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.spacing_mut().item_spacing.x = 14.0;
                            let full = Button::secondary(t.fullscreen).with_icon(icons::CORNERS_OUT).small();
                            if ui.add(full).clicked() {
                                actions.push(Action::SetFullscreen(true));
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                theme::badge(ui, t.live, Color32::WHITE, p.danger);
                                ui.label(theme::text_semibold(&w.name, 13.0));
                            });
                            // Who else is watching; the list on hover (no watch times here).
                            if let Some(s) = session {
                                let n = s.viewers_of(w.user_id).len();
                                let pill = super::viewers::eye_pill(ui, n, false, 24.0);
                                super::viewers::popover(ui, t, s, cfg, w.user_id, false, pop, &pill, false, actions);
                            }
                            let c = w.viewer.counters();
                            let stats = fill(
                                t.stream_stats,
                                &[
                                    ("fps", &format!("{:.0}", w.fps.fps)),
                                    ("lost", &c.lost.load(Ordering::Relaxed).to_string()),
                                ],
                            );
                            ui.label(theme::mono(stats, 11.0).color(p.t3));
                            if c.waiting_key.load(Ordering::Relaxed) {
                                ui.label(theme::mono(t.waiting_keyframe, 11.0).color(p.warn));
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if custom_frame {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    window_frame::window_buttons(ui, t);
                                    ui.add_space(8.0);
                                }
                                ui.spacing_mut().item_spacing.x = 10.0;
                                ui.add_sized(
                                    [36.0, 16.0],
                                    egui::Label::new(theme::mono(format!("{:.0}%", volume * 100.0), 11.0).color(p.t2)),
                                );
                                let mut v = volume;
                                if theme::slider(ui, &mut v, 0.0..=2.0, 120.0, Some(1.0))
                                    .on_hover_text(t.stream_volume)
                                    .changed()
                                {
                                    actions.push(Action::SetStreamVolume(w.name.clone(), v));
                                }
                                ui.label(theme::text(icons::SPEAKER_HIGH, 16.0).color(p.t2))
                                    .on_hover_text(t.stream_volume);
                            });
                        });
                    });
                });
            }
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
                let resp = paint_video(ui, w.texture.as_ref(), t.waiting_video);
                if resp.double_clicked() {
                    actions.push(Action::SetFullscreen(w.fullscreen.is_none()));
                }
                if let Some(since) = w.fullscreen {
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        actions.push(Action::SetFullscreen(false));
                    }
                    let shown = since.elapsed().as_secs_f32();
                    let total = FULLSCREEN_HINT_TIME.as_secs_f32();
                    if shown < total {
                        // Full for most of the time, then fades out over the last half second.
                        let alpha = ((total - shown) / 0.5).clamp(0.0, 1.0);
                        let g = ui.painter().layout_no_wrap(
                            t.exit_fullscreen_hint.to_owned(),
                            FontId::new(12.0, FontFamily::Proportional),
                            p.t1.gamma_multiply(alpha),
                        );
                        let pill = Rect::from_center_size(
                            pos2(resp.rect.center().x, resp.rect.top() + 16.0 + 15.0),
                            g.size() + vec2(28.0, 16.0),
                        );
                        ui.painter().rect_filled(
                            pill,
                            CornerRadius::same(theme::R_MD),
                            p.bg1.gamma_multiply(0.85 * alpha),
                        );
                        ui.painter().rect_stroke(
                            pill,
                            CornerRadius::same(theme::R_MD),
                            Stroke::new(1.0, p.border.gamma_multiply(alpha)),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().galley(pill.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
                        ui.ctx().request_repaint();
                    }
                }
            });
            if custom_frame && w.fullscreen.is_none() {
                window_frame::edges(ui.ctx());
            }
        });
    }

    pub(super) fn draw_share_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(mut d) = self.screen.dialog.take() else { return };
        let t = self.t();
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let mut cancel = false;
        let mut refresh = false;
        let mut start = None;
        let mut start_button = false;
        // The footer sees the selection as of the start of the frame.
        let selected = d.selected;
        let r = Dialog::new("share-screen", t.share_title).width(540.0).closable().show(
            ctx,
            |ui| {
                ui.spacing_mut().item_spacing.y = 14.0;
                ui.horizontal(|ui| {
                    ui.label(theme::text_semibold(t.share_pick, 13.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(Button::ghost(t.refresh).with_icon(icons::ARROWS_CLOCKWISE).small()).clicked() {
                            refresh = true;
                        }
                    });
                });
                egui::Frame::new()
                    .fill(p.field)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(theme::R_MD))
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(224.0)
                            .min_scrolled_height(224.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                let screens: Vec<&Source> =
                                    d.sources.iter().filter(|s| matches!(s.id, SourceId::Monitor(_))).collect();
                                let windows: Vec<&Source> =
                                    d.sources.iter().filter(|s| matches!(s.id, SourceId::Window(_))).collect();
                                for (caption, group, icon) in [
                                    (t.share_screens, screens, icons::MONITOR),
                                    (t.share_windows, windows, icons::APP_WINDOW),
                                ] {
                                    if group.is_empty() {
                                        continue;
                                    }
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(8.0);
                                        ui.label(theme::caption(ui, caption));
                                    });
                                    for src in group {
                                        let name = match src.id {
                                            SourceId::Monitor(_) => fill(t.share_screen_n, &[("n", &src.label)]),
                                            SourceId::Window(_) => src.label.clone(),
                                        };
                                        let row = source_row(ui, icon, &name, &src.meta, d.selected == Some(src.id));
                                        if row.clicked() {
                                            d.selected = Some(src.id);
                                        }
                                        if row.double_clicked() {
                                            start = Some(src.id);
                                        }
                                    }
                                }
                            });
                    });
                ui.horizontal(|ui| {
                    ui.label(theme::text(t.share_quality, 13.0).color(p.t2));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let options: Vec<(Quality, Option<&str>, &str)> =
                            Quality::ALL.iter().map(|q| (*q, None, q.label())).collect();
                        theme::segmented(ui, &mut d.quality, &options);
                    });
                });
                theme::checkbox(ui, &mut d.audio, t.share_audio, Some(t.share_audio_tip));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(theme::text(icons::CPU, 14.0).color(p.t3));
                    ui.add(egui::Label::new(theme::text(t.share_note, 12.0).color(p.t3)).wrap());
                });
            },
            |ui| {
                let go = Button::primary(t.share_start).with_icon(icons::SCREENCAST);
                start_button |= ui.add_enabled(selected.is_some(), go).clicked();
                cancel |= ui.add(Button::secondary(t.cancel)).clicked();
            },
        );
        if refresh {
            d.sources = screen::sources();
            if !d.sources.iter().any(|s| Some(s.id) == d.selected) {
                d.selected = None;
            }
        }
        if start_button {
            start = selected;
        }
        if let Some(source) = start {
            actions.push(Action::StartShare(source, d.quality, d.audio));
        } else if !cancel && !r.close_requested {
            self.screen.dialog = Some(d);
        }
    }
}
