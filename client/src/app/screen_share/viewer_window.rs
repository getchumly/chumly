//! The stream windows: someone else's stream or our own ("Your screen"), with
//! the strip on top (fullscreen, viewers, stats, volume or Stop sharing) and
//! the video below.
//!
//! Each window is a deferred viewport: it draws on its own, not as a part of
//! the main window. On Wayland the compositor stops asking a covered or
//! minimized window to draw, and a fullscreen stream covers the main window;
//! drawn as a part of it, the stream froze and stopped taking clicks. The
//! window's state is shared with the app ([`WindowCell`]): the window takes
//! its frames itself and leaves what only the app can do in `requests`.

use egui::{CornerRadius, FontFamily, FontId, Margin, Rect, Sense, Stroke, pos2, vec2};

use super::super::popover::HoverPopover;
use super::super::viewers::ViewerList;
use super::*;
use crate::icons;
use crate::screen::Counters;
use crate::theme::{self, Button, pal};

/// A stream window's state, shared by the app and the window.
pub(super) type WindowCell = Arc<Mutex<WindowState>>;

pub(super) fn lock(cell: &WindowCell) -> std::sync::MutexGuard<'_, WindowState> {
    cell.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) struct WindowState {
    /// Refreshed by the app whenever it runs.
    pub(super) t: &'static Tr,
    pub(super) custom_frame: bool,
    pub(super) viewers: ViewerList,
    /// When fullscreen was entered; `None` if not fullscreen.
    fullscreen: Option<Instant>,
    popover: HoverPopover,
    /// What only the app can do, done when it runs next.
    pub(super) requests: Vec<WindowRequest>,
    pub(super) content: Content,
}

/// What a stream window asks of the app.
pub(super) enum WindowRequest {
    Close,
    StopShare,
    /// The stream volume changed (already applied); remember it.
    SaveVolume(f32),
}

pub(super) enum Content {
    Watch(WatchContent),
    Own(OwnContent),
}

/// Someone else's stream: its decoder's frames and the volume.
pub(super) struct WatchContent {
    name: String,
    /// `None` once watching stopped: the decoder must stop right then, not
    /// when the window lets go of its state (see `App::stop_watch`).
    pub(super) viewer: Option<Viewer>,
    /// The app's GPU renderer (see `ScreenState::video`).
    video: YuvRenderer,
    /// Size of the latest frame; `None` until the first one.
    frame_size: Option<egui::Vec2>,
    /// The latest frame converted on the CPU, when the GPU path failed.
    fallback: Option<egui::TextureHandle>,
    /// Per-frame timings for the diagnostics tooltip.
    stages: StageMeter,
    fps: FpsMeter,
    volume: f32,
    mixer: Arc<Mutex<Mixer>>,
    /// The share stopped and the window asked the app to close it.
    ended: bool,
}

/// Our own share: the preview frames.
pub(super) struct OwnContent {
    preview: Arc<Preview>,
    /// Shared with the thumbnail in the "me" block.
    texture: PreviewTexture,
    /// Refreshed by the app: the running share's counters.
    pub(super) counters: Arc<Counters>,
    fps: FpsMeter,
    /// Refreshed by the app: the shared window is minimized.
    pub(super) minimized: bool,
}

impl WindowState {
    fn new(t: &'static Tr, custom_frame: bool, content: Content) -> WindowCell {
        Arc::new(Mutex::new(Self {
            t,
            custom_frame,
            viewers: ViewerList::default(),
            fullscreen: None,
            popover: HoverPopover::default(),
            requests: Vec::new(),
            content,
        }))
    }

    pub(super) fn watch(
        t: &'static Tr,
        custom_frame: bool,
        name: String,
        viewer: Viewer,
        video: YuvRenderer,
        volume: f32,
        mixer: Arc<Mutex<Mixer>>,
    ) -> WindowCell {
        let content = WatchContent {
            name,
            viewer: Some(viewer),
            video,
            frame_size: None,
            fallback: None,
            stages: StageMeter::default(),
            fps: FpsMeter::new(),
            volume,
            mixer,
            ended: false,
        };
        Self::new(t, custom_frame, Content::Watch(content))
    }

    pub(super) fn own(
        t: &'static Tr,
        custom_frame: bool,
        preview: Arc<Preview>,
        texture: PreviewTexture,
        counters: Arc<Counters>,
    ) -> WindowCell {
        let content = OwnContent { preview, texture, counters, fps: FpsMeter::new(), minimized: false };
        Self::new(t, custom_frame, Content::Own(content))
    }
}

impl WatchContent {
    /// Takes the latest frame: hands it to the GPU renderer, or converts it
    /// on the CPU if the GPU path is not available.
    fn update(&mut self, ctx: &egui::Context) {
        let Some(viewer) = &self.viewer else { return };
        let c = viewer.counters();
        let frames = c.frames.load(Ordering::Relaxed);
        self.fps.update(frames);
        let video = &self.video;
        let (display_us, displayed) =
            if self.fallback.is_some() || video.failed() { self.stages.cpu_display } else { video.upload_stats() };
        self.stages.update([
            u64::from(frames),
            c.timing.decode_us.load(Ordering::Relaxed),
            c.timing.convert_us.load(Ordering::Relaxed),
            display_us,
            displayed,
        ]);
        if video.failed() {
            // Colors on the CPU, on the decoder thread, as before the GPU path.
            viewer.convert_on_cpu();
        }
        let Some(frame) = viewer.take_frame() else { return };
        let [fw, fh] = frame.size();
        self.frame_size = Some(vec2(fw as f32, fh as f32));
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
        match &mut self.fallback {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => self.fallback = Some(ctx.load_texture("screen-share", image, egui::TextureOptions::LINEAR)),
        }
        self.stages.cpu_display.0 += t.elapsed().as_micros() as u64;
        self.stages.cpu_display.1 += 1;
    }
}

/// The video in the viewer window: an egui texture (our own preview, or
/// the CPU fallback) or YUV planes drawn by the GPU renderer.
#[derive(Clone, Copy)]
enum Video<'a> {
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

/// What differs between someone else's stream and our own in the window.
enum WindowKind {
    /// Someone else's stream, with its volume.
    Watch { volume: f32 },
    /// Our own share: no sound; `minimized`: the shared window is minimized.
    Own { minimized: bool },
}

/// What the window was asked to do this frame.
#[derive(Default)]
struct WindowOut {
    close: bool,
    fullscreen: Option<bool>,
    volume: Option<f32>,
    stop_share: bool,
}

/// Shows the window for this pass of the app; it then draws whenever it
/// needs to, until the app stops calling this.
pub(super) fn show(ctx: &egui::Context, id: egui::ViewportId, builder: egui::ViewportBuilder, cell: &WindowCell) {
    let cell = cell.clone();
    ctx.show_viewport_deferred(id, builder, move |ui, _class| draw(ui, &mut lock(&cell)));
}

/// The window: a strip with Fullscreen, LIVE and the title, the viewers,
/// the stats and, on the right, the volume (someone else's stream) or the
/// no-sound note and Stop sharing (ours); the video below.
fn draw(ui: &mut egui::Ui, st: &mut WindowState) {
    let ctx = ui.ctx().clone();
    // The stats change without new frames, also while the picture is frozen.
    ctx.request_repaint_after(Duration::from_millis(500));
    let WindowState { t, custom_frame, viewers, fullscreen, popover, requests, content } = st;
    let (t, custom_frame) = (*t, *custom_frame);
    let own_texture;
    let (title, video, stats, stats_hover, waiting_key, kind) = match content {
        Content::Watch(c) => {
            c.update(&ctx);
            let lost = c.viewer.as_ref().map_or(0, |v| v.counters().lost.load(Ordering::Relaxed));
            let waiting_key = c.viewer.as_ref().is_some_and(|v| v.counters().waiting_key.load(Ordering::Relaxed));
            let video = match (&c.fallback, c.frame_size) {
                (Some(tex), _) => Some(Video::Texture(tex)),
                (None, Some(size)) if !c.video.failed() => Some(Video::Yuv(&c.video, size)),
                _ => None,
            };
            let hover = c.stages.ms.map(|[dec, copy, show]| {
                let mode = if c.fallback.is_some() { t.video_cpu } else { t.video_gpu };
                let ms = |v: f32| format!("{v:.1}");
                fill(t.stream_timing, &[("dec", &ms(dec)), ("copy", &ms(copy)), ("show", &ms(show)), ("mode", mode)])
            });
            let stats = stream_stats(t, c.fps.fps, lost);
            (c.name.as_str(), video, stats, hover, waiting_key, WindowKind::Watch { volume: c.volume })
        }
        Content::Own(c) => {
            c.fps.update(c.counters.frames.load(Ordering::Relaxed));
            refresh_preview(&ctx, &c.preview, &c.texture);
            own_texture = c.texture.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let stats = stream_stats(t, c.fps.fps, c.counters.lost.load(Ordering::Relaxed));
            let video = own_texture.as_ref().map(Video::Texture);
            // Windows: which encoder runs (the graphics card's or OpenH264).
            #[cfg(windows)]
            let hover =
                Some(c.counters.encoder()).filter(|e| !e.is_empty()).map(|e| fill(t.stream_encoder, &[("enc", &e)]));
            #[cfg(not(windows))]
            let hover = None;
            (t.preview_title, video, stats, hover, false, WindowKind::Own { minimized: c.minimized })
        }
    };

    let mut out = WindowOut::default();
    ui.set_style(ctx.style_of(egui::Theme::Dark));
    let p = &theme::DARK;
    if ui.input(|i| i.viewport().close_requested()) {
        out.close = true;
    }
    if fullscreen.is_none() {
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
                        ui.label(theme::text_semibold(title, 13.0));
                    });
                    // Who is watching; the list on hover (no watch times here).
                    let pill = super::super::viewers::eye_pill(ui, viewers.viewers.len(), false, 24.0);
                    super::super::viewers::window_popover(ui, t, viewers, popover, &pill);
                    let stats = ui.label(theme::mono(&stats, 11.0).color(p.t3));
                    if let Some(hover) = &stats_hover {
                        stats.on_hover_text(hover);
                    }
                    if waiting_key {
                        ui.label(theme::mono(t.waiting_keyframe, 11.0).color(p.warn));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if custom_frame {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            window_frame::window_buttons(ui, t);
                            ui.add_space(8.0);
                        }
                        ui.spacing_mut().item_spacing.x = 10.0;
                        match kind {
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
    let frozen = matches!(kind, WindowKind::Own { minimized: true });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
        let tint = if frozen { Color32::WHITE.gamma_multiply(FROZEN_DIM) } else { Color32::WHITE };
        let resp = paint_video(ui, video, t.waiting_video, tint);
        if resp.double_clicked() {
            out.fullscreen = Some(fullscreen.is_none());
        }
        if frozen {
            frozen_banner(ui, resp.rect, t.preview_minimized);
        }
        if let Some(since) = *fullscreen {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out.fullscreen = Some(false);
            }
            fullscreen_hint(ui, resp.rect, since, t.exit_fullscreen_hint);
        }
    });
    if custom_frame && fullscreen.is_none() {
        window_frame::edges(ui.ctx());
    }

    if let Some(on) = out.fullscreen {
        *fullscreen = on.then(Instant::now);
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }
    if let (Some(v), Content::Watch(c)) = (out.volume, &mut *content) {
        c.volume = v;
        lock_mixer(&c.mixer).stream_volume = v;
        requests.push(WindowRequest::SaveVolume(v));
    }
    // The share stopped: the app closes the window, noticed once.
    let ended = match &mut *content {
        Content::Watch(c) if !c.ended => {
            c.ended = c.viewer.as_ref().is_some_and(|v| v.ended());
            c.ended
        }
        _ => false,
    };
    if out.close || out.stop_share || ended {
        step_aside(&ctx, fullscreen);
    }
    if out.close || ended {
        requests.push(WindowRequest::Close);
    }
    if out.stop_share {
        requests.push(WindowRequest::StopShare);
    }
    if !requests.is_empty() {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

/// Gets the window out of the way before the app closes it. The app may not
/// be running: on Wayland the main window is not drawn while it is covered
/// (by this window, fullscreen or maximized) or minimized, and only the app
/// can close this window or stop the share. So the window leaves fullscreen
/// and, on Linux, minimizes itself: that uncovers the main window, which
/// then closes it. If the main window is minimized too, it does that once
/// it is opened again, and meanwhile no frozen window stays on the screen.
fn step_aside(ctx: &egui::Context, fullscreen: &mut Option<Instant>) {
    if fullscreen.take().is_some() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }
    if cfg!(target_os = "linux") {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
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
fn paint_video(ui: &mut egui::Ui, video: Option<Video>, waiting: &str, tint: Color32) -> egui::Response {
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
