//! Screen share state: our share and its preview, the stream we watch, and
//! their frames. The windows themselves are in `viewer_window.rs` and
//! `share_dialog.rs`.

use egui::vec2;

use super::yuv_video::YuvRenderer;
use super::*;

mod share_dialog;
mod viewer_window;

pub(super) use share_dialog::ShareDialog;
use viewer_window::{Video, ViewerWindow, WindowKind, viewer_window};

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
}
