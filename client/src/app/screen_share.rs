//! Screen share state: our share and its preview, the stream we watch, and
//! their frames. The windows themselves are in `viewer_window.rs` and
//! `share_dialog.rs`.

use super::viewers::ViewerList;
use super::yuv_video::YuvRenderer;
use super::*;

mod share_dialog;
mod viewer_window;

pub(super) use share_dialog::ShareDialog;
use viewer_window::{Content, WindowCell, WindowRequest, WindowState};

/// The latest frame of our own share, shared by the thumbnail in the "me"
/// block and the "Your screen" window, whichever takes the frame first.
pub(super) type PreviewTexture = Arc<Mutex<Option<egui::TextureHandle>>>;

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
    /// Frames of our own share, straight from the capture (see `screen::Preview`).
    pub(super) preview: Arc<Preview>,
    /// The latest of them, for the thumbnail and the "Your screen" window.
    pub(super) preview_texture: PreviewTexture,
    /// Draws watched streams from their YUV planes on the GPU (one for the app).
    pub(super) video: YuvRenderer,
    /// The "Your screen" window, while open.
    pub(super) own_window: Option<OwnWindow>,
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
            preview: Arc::new(Preview::new(own_viewport())),
            preview_texture: Default::default(),
            video: YuvRenderer::default(),
            own_window: None,
        }
    }

    /// The latest frame of our own share, if there is one.
    pub(super) fn preview_frame(&self) -> Option<egui::TextureHandle> {
        self.preview_texture.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The "Your screen" window: our own share, large.
pub(super) struct OwnWindow {
    /// Fixed while the window is open (see `Watching::builder`).
    pub(super) builder: egui::ViewportBuilder,
    cell: WindowCell,
}

/// Someone else's screen share being watched.
pub(super) struct Watching {
    pub(super) user_id: UserId,
    pub(super) name: String,
    /// The viewer window is created with this size. The viewport description
    /// must not change while it is open, otherwise egui would move it every frame.
    pub(super) builder: egui::ViewportBuilder,
    /// The window's state, with the decoder and the frames.
    cell: WindowCell,
}

impl Watching {
    /// Stops the decoder now; the window lets go of its state a pass later.
    pub(super) fn stop(&self) {
        if let Content::Watch(c) = &mut viewer_window::lock(&self.cell).content {
            c.viewer = None;
        }
    }
}

impl OwnWindow {
    pub(super) fn new(
        builder: egui::ViewportBuilder,
        t: &'static Tr,
        custom_frame: bool,
        screen: &ScreenState,
    ) -> Self {
        let counters = screen.sharer.as_ref().map(|s| s.counters().clone()).unwrap_or_default();
        let preview = screen.preview.clone();
        let cell = WindowState::own(t, custom_frame, preview, screen.preview_texture.clone(), counters);
        Self { builder, cell }
    }
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

/// Takes the newest preview frame, if any, into `slot`.
fn refresh_preview(ctx: &egui::Context, preview: &Preview, slot: &PreviewTexture) {
    let Some(image) = preview.take_frame() else { return };
    let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
    match &mut *slot {
        Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
        None => *slot = Some(ctx.load_texture("screen-preview", image, egui::TextureOptions::LINEAR)),
    }
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
            *self.screen.preview_texture.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return;
        }
        refresh_preview(&self.ctx, &self.screen.preview, &self.screen.preview_texture);
    }

    /// Opens the viewer window on `viewer`'s stream.
    pub(super) fn start_watching(
        &mut self,
        user_id: UserId,
        name: String,
        viewer: Viewer,
        builder: egui::ViewportBuilder,
    ) {
        let volume = self.cfg.stream_volume(&name);
        let cell = WindowState::watch(
            self.t(),
            !self.cfg.system_frame,
            name.clone(),
            viewer,
            self.screen.video.clone(),
            volume,
            self.audio.mixer.clone(),
        );
        self.screen.watching = Some(Watching { user_id, name, builder, cell });
    }

    /// Someone else's screen share in a separate OS window: it can be moved
    /// outside the main window and onto another monitor. Always dark.
    pub(super) fn draw_viewer(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let t = self.t();
        let Some(w) = &self.screen.watching else { return };
        let requests = {
            let mut st = viewer_window::lock(&w.cell);
            st.t = t;
            st.custom_frame = !self.cfg.system_frame;
            st.viewers = self.session.as_ref().map(|s| ViewerList::of(s, w.user_id)).unwrap_or_default();
            std::mem::take(&mut st.requests)
        };
        viewer_window::show(ctx, viewer_viewport(), w.builder.clone(), &w.cell);
        for r in requests {
            match r {
                WindowRequest::Close => actions.push(Action::StopWatch),
                WindowRequest::SaveVolume(v) => actions.push(Action::SetStreamVolume(w.name.clone(), v)),
                WindowRequest::StopShare => {}
            }
        }
    }

    /// Our own share large, in the same kind of window as a viewer's: no
    /// sound, a Stop sharing button. Closing it only closes the preview.
    pub(super) fn draw_own_window(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let t = self.t();
        let (Some(own), Some(sharer), Some(s)) = (&self.screen.own_window, &self.screen.sharer, &self.session) else {
            return;
        };
        let requests = {
            let mut st = viewer_window::lock(&own.cell);
            st.t = t;
            st.custom_frame = !self.cfg.system_frame;
            st.viewers = ViewerList::of(s, s.my_id);
            if let Content::Own(c) = &mut st.content {
                c.counters = sharer.counters().clone();
                c.minimized = sharer.source_minimized();
            }
            std::mem::take(&mut st.requests)
        };
        viewer_window::show(ctx, own_viewport(), own.builder.clone(), &own.cell);
        for r in requests {
            match r {
                WindowRequest::Close => actions.push(Action::ClosePreview),
                WindowRequest::StopShare => actions.push(Action::StopShare),
                WindowRequest::SaveVolume(_) => {}
            }
        }
    }
}
