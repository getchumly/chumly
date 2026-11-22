//! Screen share: the share dialog, the viewer window and video frames.

use egui::{CornerRadius, FontFamily, FontId, Margin, Rect, Sense, Stroke, pos2, vec2};

use super::yuv_video::YuvRenderer;
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
    /// Frames of our own share, straight from the capture (see `screen::Preview`).
    pub(super) preview: Arc<Preview>,
    /// The latest of them, for the thumbnail and the "Your screen" window.
    pub(super) preview_texture: Option<egui::TextureHandle>,
    /// Draws watched streams from their YUV planes on the GPU (one for the app).
    pub(super) video: YuvRenderer,
    /// The "Your screen" window, while open.
    pub(super) own_window: Option<OwnWindow>,
    /// The viewer list in the "Your screen" window.
    pub(super) own_popover: super::popover::HoverPopover,
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
            preview: Arc::new(Preview::default()),
            preview_texture: None,
            video: YuvRenderer::default(),
            own_window: None,
            own_popover: Default::default(),
        }
    }
}

/// The "Your screen" window: our own share, large.
pub(super) struct OwnWindow {
    /// When fullscreen was entered; `None` if not fullscreen.
    pub(super) fullscreen: Option<Instant>,
    /// Fixed while the window is open (see `Watching::builder`).
    pub(super) builder: egui::ViewportBuilder,
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
    /// Size of the latest frame; `None` until the first one. The frame
    /// itself is in `ScreenState::video` (GPU path).
    pub(super) frame_size: Option<egui::Vec2>,
    /// The latest frame converted on the CPU, when the GPU path failed.
    pub(super) fallback: Option<egui::TextureHandle>,
    /// Per-frame timings for the diagnostics tooltip.
    pub(super) stages: StageMeter,
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

/// Average time per frame of each viewing stage, over the last second:
/// decoding, copying the planes out of the decoder, and getting the frame
/// to the screen (GPU upload, or CPU conversion and upload in the fallback).
#[derive(Default)]
pub(super) struct StageMeter {
    since: Option<Instant>,
    /// Counter values at `since`: frames, decode µs, copy µs, display µs, displayed.
    base: [u64; 5],
    /// Milliseconds per frame: decode, copy, display.
    pub(super) ms: Option<[f32; 3]>,
    /// Time spent on the display stage in the CPU fallback, µs and frames.
    pub(super) cpu_display: (u64, u64),
}

impl StageMeter {
    /// `now`: frames decoded, decode µs, copy µs, display µs, frames displayed.
    fn update(&mut self, now: [u64; 5]) {
        let Some(since) = self.since else {
            self.since = Some(Instant::now());
            self.base = now;
            return;
        };
        if since.elapsed() < Duration::from_secs(1) {
            return;
        }
        let d: Vec<u64> = now.iter().zip(self.base).map(|(n, b)| n.saturating_sub(b)).collect();
        let per = |us: u64, n: u64| if n == 0 { 0.0 } else { us as f32 / n as f32 / 1000.0 };
        self.ms = Some([per(d[1], d[0]), per(d[2], d[0]), per(d[3], d[4])]);
        self.base = now;
        self.since = Some(Instant::now());
    }
}

/// The video in the viewer window: an egui texture (our own preview, or
/// the CPU fallback) or YUV planes drawn by the GPU renderer.
#[derive(Clone, Copy)]
pub(super) enum Video<'a> {
    Texture(&'a egui::TextureHandle),
    Yuv(&'a YuvRenderer, egui::Vec2),
}

impl Video<'_> {
    fn size(&self) -> egui::Vec2 {
        match self {
            Video::Texture(tex) => tex.size_vec2(),
            Video::Yuv(_, size) => *size,
        }
    }
}

pub(super) fn viewer_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("screen-viewer")
}

pub(super) fn own_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("screen-preview")
}

/// A frozen frame (the shared window is minimized) is drawn this bright.
pub(super) const FROZEN_DIM: f32 = 0.45;

/// Warning colors (section 44): a dark orange strip, calm, not red.
pub(super) const WARN_BG: Color32 = Color32::from_rgb(0x2A, 0x1E, 0x14);
pub(super) const WARN_TEXT: Color32 = Color32::from_rgb(0xF5, 0xD2, 0xB8);
pub(super) const WARN_ICON: Color32 = Color32::from_rgb(0xF6, 0x7F, 0x2F);
/// The strip's border: the icon color at 55%.
pub(super) const WARN_BORDER: Color32 = Color32::from_rgba_unmultiplied_const(0xF6, 0x7F, 0x2F, 140);

/// "30 fps · frames lost: 0".
pub(super) fn stream_stats(t: &Tr, fps: f32, lost: u32) -> String {
    fill(t.stream_stats, &[("fps", &format!("{fps:.0}")), ("lost", &lost.to_string())])
}

/// What differs between someone else's stream and our own in the viewer window.
enum WindowKind {
    /// Someone else's stream, with its volume.
    Watch { volume: f32 },
    /// Our own share: no sound; `minimized`: the shared window is minimized.
    Own { minimized: bool },
}

struct ViewerWindow<'a> {
    id: egui::ViewportId,
    builder: &'a egui::ViewportBuilder,
    /// The streamer's name, or "Your screen".
    title: &'a str,
    /// Whose viewers the eye pill lists.
    streamer: UserId,
    video: Option<Video<'a>>,
    fullscreen: Option<Instant>,
    stats: String,
    /// Diagnostics on hovering the stats: time per frame of each stage.
    stats_hover: Option<String>,
    waiting_key: bool,
    kind: WindowKind,
}

/// What the viewer window was asked to do this frame.
#[derive(Default)]
struct WindowOut {
    close: bool,
    fullscreen: Option<bool>,
    volume: Option<f32>,
    stop_share: bool,
}

/// The viewer window: a strip with Fullscreen, LIVE and the title, the
/// viewers, the stats and, on the right, the volume (someone else's stream)
/// or the no-sound note and Stop sharing (ours); the video below.
#[allow(clippy::too_many_arguments)]
fn viewer_window(
    ctx: &egui::Context,
    w: &ViewerWindow,
    t: &Tr,
    session: Option<&Session>,
    cfg: &ClientConfig,
    pop: &mut super::popover::HoverPopover,
    custom_frame: bool,
    actions: &mut Vec<Action>,
) -> WindowOut {
    let mut out = WindowOut::default();
    ctx.show_viewport_immediate(w.id, w.builder.clone(), |ui, _class| {
        ui.set_style(ui.ctx().style_of(egui::Theme::Dark));
        let p = &theme::DARK;
        if ui.input(|i| i.viewport().close_requested()) {
            out.close = true;
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
                            out.fullscreen = Some(true);
                        }
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            theme::badge(ui, t.live, Color32::WHITE, p.danger);
                            ui.label(theme::text_semibold(w.title, 13.0));
                        });
                        // Who is watching; the list on hover (no watch times here).
                        if let Some(s) = session {
                            let n = s.viewers_of(w.streamer).len();
                            let pill = super::viewers::eye_pill(ui, n, false, 24.0);
                            super::viewers::popover(ui, t, s, cfg, w.streamer, false, pop, &pill, false, actions);
                        }
                        let stats = ui.label(theme::mono(&w.stats, 11.0).color(p.t3));
                        if let Some(hover) = &w.stats_hover {
                            stats.on_hover_text(hover);
                        }
                        if w.waiting_key {
                            ui.label(theme::mono(t.waiting_keyframe, 11.0).color(p.warn));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if custom_frame {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                window_frame::window_buttons(ui, t);
                                ui.add_space(8.0);
                            }
                            ui.spacing_mut().item_spacing.x = 10.0;
                            match w.kind {
                                WindowKind::Watch { volume } => volume_control(ui, t, volume, &mut out),
                                WindowKind::Own { .. } => {
                                    let stop = Button::danger(t.stop_share).with_fill_icon(icons::FILL_STOP).small();
                                    if ui.add(stop).clicked() {
                                        out.stop_share = true;
                                    }
                                    no_sound_note(ui, t);
                                }
                            }
                        });
                    });
                });
            });
        }
        let frozen = matches!(w.kind, WindowKind::Own { minimized: true });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
            let tint = if frozen { Color32::WHITE.gamma_multiply(FROZEN_DIM) } else { Color32::WHITE };
            let resp = paint_video(ui, w.video, t.waiting_video, tint);
            if resp.double_clicked() {
                out.fullscreen = Some(w.fullscreen.is_none());
            }
            if frozen {
                frozen_banner(ui, resp.rect, t.preview_minimized);
            }
            if let Some(since) = w.fullscreen {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out.fullscreen = Some(false);
                }
                fullscreen_hint(ui, resp.rect, since, t.exit_fullscreen_hint);
            }
        });
        if custom_frame && w.fullscreen.is_none() {
            window_frame::edges(ui.ctx());
        }
    });
    out
}

/// The stream volume on the right of the strip (laid out right to left).
fn volume_control(ui: &mut egui::Ui, t: &Tr, volume: f32, out: &mut WindowOut) {
    let p = &theme::DARK;
    ui.add_sized([36.0, 16.0], egui::Label::new(theme::mono(format!("{:.0}%", volume * 100.0), 11.0).color(p.t2)));
    let mut v = volume;
    if theme::slider(ui, &mut v, 0.0..=2.0, 120.0, Some(1.0)).on_hover_text(t.stream_volume).changed() {
        out.volume = Some(v);
    }
    ui.label(theme::text(icons::SPEAKER_HIGH, 16.0).color(p.t2)).on_hover_text(t.stream_volume);
}

/// "Preview: no sound, only you see this" with a muted speaker, in the
/// volume slider's place (laid out right to left); truncated first when narrow.
fn no_sound_note(ui: &mut egui::Ui, t: &Tr) {
    let p = &theme::DARK;
    let icon_w = 14.0 + 6.0;
    let mut job = egui::text::LayoutJob::simple_singleline(
        t.preview_note.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        p.t3,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((ui.available_width() - icon_w).max(0.0));
    let g = ui.painter().layout_job(job);
    if g.size().x < 24.0 {
        return;
    }
    ui.spacing_mut().item_spacing.x = 6.0;
    ui.label(g);
    ui.label(theme::text(icons::SPEAKER_SIMPLE_SLASH, 14.0).color(p.t3));
}

/// The minimized-window warning over the large preview (section 44).
fn frozen_banner(ui: &egui::Ui, rect: Rect, text: &str) {
    let painter = ui.painter_at(rect);
    let icon =
        painter.layout_no_wrap(icons::FILL_PAUSE_CIRCLE.to_owned(), FontId::new(16.0, theme::icons_fill()), WARN_ICON);
    let label = painter.layout_no_wrap(text.to_owned(), FontId::new(13.0, FontFamily::Proportional), WARN_TEXT);
    let size = vec2(14.0 + icon.size().x + 8.0 + label.size().x + 14.0, 8.0 + label.size().y.max(icon.size().y) + 8.0);
    let pill = Rect::from_min_size(pos2(rect.center().x - size.x / 2.0, rect.top() + 16.0), size);
    painter.rect_filled(pill, CornerRadius::same(theme::R_MD), WARN_BG);
    painter.rect_stroke(pill, CornerRadius::same(theme::R_MD), Stroke::new(1.0, WARN_BORDER), egui::StrokeKind::Inside);
    let x = pill.left() + 14.0;
    painter.galley(pos2(x, pill.center().y - icon.size().y / 2.0), icon.clone(), Color32::PLACEHOLDER);
    painter.galley(pos2(x + icon.size().x + 8.0, pill.center().y - label.size().y / 2.0), label, Color32::PLACEHOLDER);
}

/// "Esc or double-click to exit fullscreen" at the top, fading out.
fn fullscreen_hint(ui: &egui::Ui, rect: Rect, since: Instant, text: &str) {
    let p = &theme::DARK;
    let shown = since.elapsed().as_secs_f32();
    let total = FULLSCREEN_HINT_TIME.as_secs_f32();
    if shown >= total {
        return;
    }
    // Full for most of the time, then fades out over the last half second.
    let alpha = ((total - shown) / 0.5).clamp(0.0, 1.0);
    let g = ui.painter().layout_no_wrap(
        text.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        p.t1.gamma_multiply(alpha),
    );
    let pill = Rect::from_center_size(pos2(rect.center().x, rect.top() + 16.0 + 15.0), g.size() + vec2(28.0, 16.0));
    ui.painter().rect_filled(pill, CornerRadius::same(theme::R_MD), p.bg1.gamma_multiply(0.85 * alpha));
    ui.painter().rect_stroke(
        pill,
        CornerRadius::same(theme::R_MD),
        Stroke::new(1.0, p.border.gamma_multiply(alpha)),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(pill.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
    ui.ctx().request_repaint();
}

/// Draws a screen share frame centered in the available space,
/// preserving aspect ratio, on a black background. `tint` dims a frozen frame.
pub(super) fn paint_video(ui: &mut egui::Ui, video: Option<Video>, waiting: &str, tint: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    match video {
        Some(video) => {
            let size = video.size();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let fitted = Rect::from_center_size(rect.center(), size * scale);
            match video {
                Video::Texture(tex) => {
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    painter.image(tex.id(), fitted, uv, tint);
                }
                Video::Yuv(renderer, _) => renderer.paint(&painter, fitted, tint),
            }
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
    /// Tells the capture what our own preview needs (nothing, the thumbnail
    /// or full frames for the "Your screen" window) and takes the newest frame.
    pub(super) fn update_preview(&mut self) {
        let sharing = self.screen.sharer.is_some() && self.session.is_some();
        let mode = if !sharing {
            PreviewMode::Off
        } else if self.screen.own_window.is_some() {
            PreviewMode::Full
        } else if self.cfg.share.preview {
            PreviewMode::Thumbnail
        } else {
            PreviewMode::Off
        };
        if self.screen.preview.mode() != mode {
            self.screen.preview.set_mode(mode);
        }
        if sharing {
            // The stats and the minimized-window warning change without new frames.
            self.ctx.request_repaint_after(Duration::from_millis(500));
        }
        if mode == PreviewMode::Off {
            self.screen.preview_texture = None;
            return;
        }
        let Some(image) = self.screen.preview.take_frame() else { return };
        match &mut self.screen.preview_texture {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => {
                let tex = self.ctx.load_texture("screen-preview", image, egui::TextureOptions::LINEAR);
                self.screen.preview_texture = Some(tex);
            }
        }
    }

    /// Takes the latest screen share frame: hands it to the GPU renderer,
    /// or converts it on the CPU if the GPU path is not available.
    pub(super) fn update_video(&mut self) {
        let Some(w) = &mut self.screen.watching else { return };
        let c = w.viewer.counters();
        let frames = c.frames.load(Ordering::Relaxed);
        w.fps.update(frames);
        let video = &self.screen.video;
        let (display_us, displayed) =
            if w.fallback.is_some() || video.failed() { w.stages.cpu_display } else { video.upload_stats() };
        w.stages.update([
            u64::from(frames),
            c.timing.decode_us.load(Ordering::Relaxed),
            c.timing.convert_us.load(Ordering::Relaxed),
            display_us,
            displayed,
        ]);
        // Keep the stats under the video updating even if the picture is frozen.
        self.ctx.request_repaint_after(Duration::from_millis(500));
        if video.failed() {
            // Colors on the CPU, on the decoder thread, as before the GPU path.
            w.viewer.convert_on_cpu();
        }
        let Some(frame) = w.viewer.take_frame() else { return };
        let [fw, fh] = frame.size();
        w.frame_size = Some(vec2(fw as f32, fh as f32));
        let image = match frame {
            Frame::Yuv(f) if !video.failed() => {
                video.set_frame(f);
                return;
            }
            // Decoded before the decoder thread switched to RGBA.
            Frame::Yuv(f) => f.to_color_image(),
            Frame::Rgba(image) => image,
        };
        let t = Instant::now();
        match &mut w.fallback {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => w.fallback = Some(self.ctx.load_texture("screen-share", image, egui::TextureOptions::LINEAR)),
        }
        w.stages.cpu_display.0 += t.elapsed().as_micros() as u64;
        w.stages.cpu_display.1 += 1;
    }

    /// Someone else's screen share in a separate OS window: it can be moved
    /// outside the main window and onto another monitor. Always dark.
    pub(super) fn draw_viewer(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let t = self.t();
        let Some(w) = &self.screen.watching else { return };
        let c = w.viewer.counters();
        let video = match (&w.fallback, w.frame_size) {
            (Some(tex), _) => Some(Video::Texture(tex)),
            (None, Some(size)) if !self.screen.video.failed() => Some(Video::Yuv(&self.screen.video, size)),
            _ => None,
        };
        let stats_hover = w.stages.ms.map(|[dec, copy, show]| {
            let mode = if w.fallback.is_some() { t.video_cpu } else { t.video_gpu };
            let ms = |v: f32| format!("{v:.1}");
            fill(t.stream_timing, &[("dec", &ms(dec)), ("copy", &ms(copy)), ("show", &ms(show)), ("mode", mode)])
        });
        let window = ViewerWindow {
            id: viewer_viewport(),
            builder: &w.builder,
            title: &w.name,
            streamer: w.user_id,
            video,
            fullscreen: w.fullscreen,
            stats: stream_stats(t, w.fps.fps, c.lost.load(Ordering::Relaxed)),
            stats_hover,
            waiting_key: c.waiting_key.load(Ordering::Relaxed),
            kind: WindowKind::Watch { volume: self.cfg.stream_volume(&w.name) },
        };
        let custom_frame = !self.cfg.system_frame;
        let pop = &mut self.screen.watch_popover;
        let out = viewer_window(ctx, &window, t, self.session.as_ref(), &self.cfg, pop, custom_frame, actions);
        if out.close {
            actions.push(Action::StopWatch);
        }
        if let Some(on) = out.fullscreen {
            actions.push(Action::SetFullscreen(on));
        }
        if let Some(v) = out.volume {
            actions.push(Action::SetStreamVolume(w.name.clone(), v));
        }
    }

    /// Our own share large, in the same kind of window as a viewer's: no
    /// sound, a Stop sharing button. Closing it only closes the preview.
    pub(super) fn draw_own_window(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let t = self.t();
        let (Some(own), Some(sharer), Some(s)) = (&self.screen.own_window, &self.screen.sharer, &self.session) else {
            return;
        };
        let window = ViewerWindow {
            id: own_viewport(),
            builder: &own.builder,
            title: t.preview_title,
            streamer: s.my_id,
            video: self.screen.preview_texture.as_ref().map(Video::Texture),
            fullscreen: own.fullscreen,
            stats: stream_stats(t, self.screen.fps.fps, sharer.counters().lost.load(Ordering::Relaxed)),
            stats_hover: None,
            waiting_key: false,
            kind: WindowKind::Own { minimized: sharer.source_minimized() },
        };
        let custom_frame = !self.cfg.system_frame;
        let pop = &mut self.screen.own_popover;
        let out = viewer_window(ctx, &window, t, Some(s), &self.cfg, pop, custom_frame, actions);
        if out.close {
            actions.push(Action::ClosePreview);
        }
        if let Some(on) = out.fullscreen {
            actions.push(Action::SetPreviewFullscreen(on));
        }
        if out.stop_share {
            actions.push(Action::StopShare);
        }
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
