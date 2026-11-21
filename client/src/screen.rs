//! Screen sharing.
//!
//! Sharer: screen or window capture, downscaling to the target
//! resolution, H.264 encoding, splitting frames into datagrams.
//! Viewer: reassembling frames from fragments, decoding, handing the
//! picture to the UI. The server only forwards fragments to subscribed viewers.
//!
//! Only implemented on Windows so far. On other platforms
//! [`SUPPORTED`] is `false`: the client does not tell the server it can
//! show video, and the UI buttons are disabled.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::{Sharer, Viewer, sources};

#[cfg(not(windows))]
pub use stub::{Sharer, Viewer, sources};

/// Whether screen sharing is available on this platform.
pub const SUPPORTED: bool = cfg!(windows);

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    P720F30,
    P1080F30,
    P1080F60,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::P720F30, Quality::P1080F30, Quality::P1080F60];

    pub fn label(self) -> &'static str {
        match self {
            Quality::P720F30 => "720p 30",
            Quality::P1080F30 => "1080p 30",
            Quality::P1080F60 => "1080p 60",
        }
    }

    // Used by the capture and encoder, so far only on Windows.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn max_height(self) -> u32 {
        match self {
            Quality::P720F30 => 720,
            Quality::P1080F30 | Quality::P1080F60 => 1080,
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn fps(self) -> u32 {
        match self {
            Quality::P720F30 | Quality::P1080F30 => 30,
            Quality::P1080F60 => 60,
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn bitrate(self) -> u32 {
        match self {
            Quality::P720F30 => 2_500_000,
            Quality::P1080F30 => 4_000_000,
            Quality::P1080F60 => 6_000_000,
        }
    }
}

/// What to share: a monitor by index or a window by its HWND. Only the
/// Windows capture lists sources, elsewhere none is ever made.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceId {
    Monitor(usize),
    Window(isize),
}

#[derive(Clone, Debug)]
pub struct Source {
    pub id: SourceId,
    /// Window title; for a monitor, its number.
    pub label: String,
    /// Monitor resolution or the window's executable.
    pub meta: String,
}

/// Counters for the status line under the video.
#[derive(Default)]
pub struct Counters {
    pub frames: AtomicU32,
    pub lost: AtomicU32,
    pub waiting_key: AtomicBool,
    /// Per-stage timings in microseconds, for diagnostics.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub timing: Timing,
}

/// Filled and read by the Windows capture, encoder and decoder.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
pub struct Timing {
    /// How many frames Windows delivered.
    pub captured: AtomicU64,
    /// Of those, skipped because the encoder was still busy with the previous one.
    pub busy: AtomicU64,
    /// Copying the frame from the GPU.
    pub copy_us: AtomicU64,
    pub scale_us: AtomicU64,
    pub encoded: AtomicU64,
    pub encode_us: AtomicU64,
    pub encode_max_us: AtomicU64,
    pub bytes: AtomicU64,
    pub decode_us: AtomicU64,
    pub convert_us: AtomicU64,
}

impl Timing {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn add(counter: &AtomicU64, since: std::time::Instant) -> u64 {
        let us = since.elapsed().as_micros() as u64;
        counter.fetch_add(us, Ordering::Relaxed);
        us
    }
}

/// Shared with the network module: whose share is being watched and where
/// to deliver incoming frame fragments.
#[derive(Default)]
pub struct ScreenShared {
    /// Sharer id; 0 if nothing is being watched.
    watching: AtomicU32,
    incoming: Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>,
}

impl ScreenShared {
    /// Whether `from`'s share is currently being watched.
    pub fn is_watching(&self, from: shared::UserId) -> bool {
        from != 0 && self.watching.load(Ordering::Relaxed) == from
    }

    /// Called by the network module for every screen share datagram.
    pub fn push(&self, from: shared::UserId, payload: &[u8]) {
        if from == 0 || self.watching.load(Ordering::Relaxed) != from {
            return;
        }
        if let Some(tx) = self.incoming.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.send(payload.to_vec());
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn attach(&self, from: shared::UserId, tx: std::sync::mpsc::Sender<Vec<u8>>) {
        *self.incoming.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        self.watching.store(from, Ordering::Relaxed);
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn detach(&self) {
        self.watching.store(0, Ordering::Relaxed);
        *self.incoming.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// What the UI wants to see of our own share.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreviewMode {
    /// Nothing: no share, or the preview is hidden.
    Off,
    /// The thumbnail in the "me" block: small and a few times a second.
    Thumbnail,
    /// The "Your screen" window: every frame at the stream's resolution.
    Full,
}

/// Thumbnail frames are downscaled to at most this height: the thumbnail is
/// under 260 px tall even in the widest panel, 360 leaves room for 1.5x screens.
pub const THUMBNAIL_MAX_HEIGHT: u32 = 360;
/// The thumbnail is updated at most this often.
#[cfg_attr(not(windows), allow(dead_code))]
pub const THUMBNAIL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// The picture of our own share for the UI. The encoder thread puts the
/// frames it captured, before encoding: nothing goes over the network, there
/// is no delay and no sound.
pub struct Preview {
    mode: std::sync::atomic::AtomicU8,
    frame: Mutex<Option<eframe::egui::ColorImage>>,
}

impl Default for Preview {
    fn default() -> Self {
        Self { mode: std::sync::atomic::AtomicU8::new(PreviewMode::Off as u8), frame: Mutex::new(None) }
    }
}

impl Preview {
    pub fn set_mode(&self, mode: PreviewMode) {
        self.mode.store(mode as u8, Ordering::Relaxed);
        if mode == PreviewMode::Off {
            self.frame.lock().unwrap_or_else(|e| e.into_inner()).take();
        }
    }

    pub fn mode(&self) -> PreviewMode {
        match self.mode.load(Ordering::Relaxed) {
            m if m == PreviewMode::Thumbnail as u8 => PreviewMode::Thumbnail,
            m if m == PreviewMode::Full as u8 => PreviewMode::Full,
            _ => PreviewMode::Off,
        }
    }

    /// The newest frame since the last call.
    pub fn take_frame(&self) -> Option<eframe::egui::ColorImage> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn put(&self, image: eframe::egui::ColorImage) {
        let mut frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
        // Turned off meanwhile (the share stopped): keep nothing for the next one.
        if self.mode() != PreviewMode::Off {
            *frame = Some(image);
        }
    }
}

/// A captured BGRA frame (`w` × `h`, tightly packed) as a preview image:
/// downscaled for the thumbnail, opaque (capture alpha is not meaningful).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn preview_image(bgra: &[u8], w: usize, h: usize, mode: PreviewMode) -> eframe::egui::ColorImage {
    use eframe::egui::{Color32, ColorImage};
    let scaled;
    let (src, w, h) = if mode == PreviewMode::Thumbnail && h > THUMBNAIL_MAX_HEIGHT as usize {
        let (tw, th) = target_size(w as u32, h as u32, THUMBNAIL_MAX_HEIGHT);
        let (tw, th) = (tw as usize, th as usize);
        scaled = scale_bgra(bgra, w, h, w * 4, tw, th);
        (scaled.as_slice(), tw, th)
    } else {
        (bgra, w, h)
    };
    let pixels = src.as_chunks::<4>().0.iter().map(|px| Color32::from_rgb(px[2], px[1], px[0])).collect();
    ColorImage::new([w, h], pixels)
}

/// Frame size after downscaling: at most `max_h` tall, same aspect
/// ratio, even dimensions (required by H.264 4:2:0).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn target_size(w: u32, h: u32, max_h: u32) -> (u32, u32) {
    let (mut tw, mut th) = if h > max_h {
        let tw = (w as u64 * max_h as u64 / h as u64) as u32;
        (tw, max_h)
    } else {
        (w, h)
    };
    // OpenH264 cannot encode wider than 3840.
    if tw > 3840 {
        th = (th as u64 * 3840 / tw as u64) as u32;
        tw = 3840;
    }
    ((tw & !1).max(2), (th & !1).max(2))
}

/// Downscales a BGRA image with bilinear interpolation. `pitch` is the
/// source row length in bytes: for captured frames it can exceed
/// width × 4. Rows are split across several threads.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn scale_bgra(src: &[u8], sw: usize, sh: usize, pitch: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut dst = vec![0u8; dw * dh * 4];
    if sw == dw && sh == dh {
        for (y, row) in dst.chunks_exact_mut(dw * 4).enumerate() {
            row.copy_from_slice(&src[y * pitch..y * pitch + dw * 4]);
        }
        return dst;
    }
    // Neighbor pixel weight in 1/256 units.
    let pos = |d: usize, dn: usize, sn: usize| -> (usize, u32) {
        let f = ((d as f32 + 0.5) * sn as f32 / dn as f32 - 0.5).max(0.0);
        let i = (f as usize).min(sn - 1);
        (i, ((f - i as f32) * 256.0) as u32)
    };
    let xs: Vec<(usize, u32)> = (0..dw).map(|x| pos(x, dw, sw)).collect();
    let xs = &xs;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 4);
    let rows_per = dh.div_ceil(threads);
    std::thread::scope(|s| {
        for (ci, chunk) in dst.chunks_mut(rows_per * dw * 4).enumerate() {
            s.spawn(move || {
                for (ry, row) in chunk.chunks_exact_mut(dw * 4).enumerate() {
                    let (y0, wy) = pos(ci * rows_per + ry, dh, sh);
                    let y1 = (y0 + 1).min(sh - 1);
                    let r0 = &src[y0 * pitch..];
                    let r1 = &src[y1 * pitch..];
                    for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let (x0, wx) = xs[x];
                        let x1 = (x0 + 1).min(sw - 1);
                        for c in 0..4 {
                            let top = r0[x0 * 4 + c] as u32 * (256 - wx) + r0[x1 * 4 + c] as u32 * wx;
                            let bot = r1[x0 * 4 + c] as u32 * (256 - wx) + r1[x1 * 4 + c] as u32 * wx;
                            px[c] = ((top * (256 - wy) + bot * wy) >> 16) as u8;
                        }
                    }
                }
            });
        }
    });
    dst
}

#[cfg(not(windows))]
mod stub {
    use std::sync::Arc;

    use super::{Counters, Preview, Quality, ScreenShared, Source, SourceId};
    use crate::net::NetCommand;

    pub fn sources() -> Vec<Source> {
        Vec::new()
    }

    pub struct Sharer;

    impl Sharer {
        pub fn start(
            _source: SourceId,
            _quality: Quality,
            _with_audio: bool,
            _out: tokio::sync::mpsc::Sender<(shared::media::MediaKind, Vec<u8>)>,
            _preview: Arc<Preview>,
            _ctx: eframe::egui::Context,
        ) -> Result<Self, String> {
            Err("screen sharing is currently only available on Windows".into())
        }
        pub fn source_minimized(&self) -> bool {
            false
        }
        pub fn request_keyframe(&self) {}
        pub fn audio_error(&self) -> Option<&str> {
            None
        }
        pub fn ended(&self) -> bool {
            true
        }
        pub fn counters(&self) -> &Counters {
            unreachable!("Sharer is never created on this platform")
        }
    }

    pub struct Viewer;

    impl Viewer {
        pub fn start(
            _user_id: shared::UserId,
            _shared: Arc<ScreenShared>,
            _net: tokio::sync::mpsc::UnboundedSender<NetCommand>,
            _ctx: eframe::egui::Context,
        ) -> Result<Self, String> {
            Err("screen sharing is currently only available on Windows".into())
        }
        pub fn take_frame(&self) -> Option<eframe::egui::ColorImage> {
            None
        }
        pub fn counters(&self) -> &Counters {
            unreachable!("Viewer is never created on this platform")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_size_keeps_aspect_and_parity() {
        assert_eq!(target_size(3840, 2160, 1080), (1920, 1080));
        assert_eq!(target_size(1280, 720, 1080), (1280, 720));
        assert_eq!(target_size(2560, 1440, 720), (1280, 720));
        assert_eq!(target_size(801, 601, 1080), (800, 600));
        assert_eq!(target_size(7680, 1080, 1080), (3840, 540));
    }

    #[test]
    fn scale_identity_strips_padding() {
        // 2x2 with a 12-byte row pitch instead of 8.
        let src = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 9, 10, 11, 12, 13, 14, 15, 16, 0, 0, 0, 0];
        assert_eq!(scale_bgra(&src, 2, 2, 12, 2, 2), (1..=16).collect::<Vec<u8>>());
    }

    #[test]
    fn preview_swaps_channels_and_ignores_alpha() {
        // One blue-ish pixel in BGRA with zero alpha.
        let img = preview_image(&[10, 20, 30, 0], 1, 1, PreviewMode::Full);
        assert_eq!(img.size, [1, 1]);
        assert_eq!(img.pixels[0], eframe::egui::Color32::from_rgb(30, 20, 10));
    }

    #[test]
    fn preview_thumbnail_is_downscaled_full_is_not() {
        let (w, h) = (1920, 1080);
        let src = vec![128u8; w * h * 4];
        let thumb = preview_image(&src, w, h, PreviewMode::Thumbnail);
        assert_eq!(thumb.size, [640, THUMBNAIL_MAX_HEIGHT as usize]);
        let full = preview_image(&src, w, h, PreviewMode::Full);
        assert_eq!(full.size, [w, h]);
        // A small frame is not upscaled for the thumbnail.
        let small = preview_image(&vec![0u8; 320 * 200 * 4], 320, 200, PreviewMode::Thumbnail);
        assert_eq!(small.size, [320, 200]);
    }

    #[test]
    fn preview_off_drops_the_frame() {
        let p = Preview::default();
        p.set_mode(PreviewMode::Thumbnail);
        p.put(eframe::egui::ColorImage::filled([1, 1], eframe::egui::Color32::RED));
        p.set_mode(PreviewMode::Off);
        assert!(p.take_frame().is_none());
        assert_eq!(p.mode(), PreviewMode::Off);
    }

    #[test]
    fn scale_down_averages() {
        // A solid 4x4 downscales to a 2x2 of the same color.
        let src: Vec<u8> = [10u8, 20, 30, 255].repeat(16);
        let out = scale_bgra(&src, 4, 4, 16, 2, 2);
        for px in out.chunks(4) {
            assert!(px.iter().zip([10, 20, 30, 255]).all(|(a, b)| a.abs_diff(b) <= 1), "{px:?}");
        }
    }
}
