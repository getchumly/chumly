//! Screen sharing.
//!
//! Sharer: screen or window capture, downscaling to the target
//! resolution, H.264 encoding, splitting frames into datagrams.
//! Viewer: reassembling frames from fragments, decoding, handing the
//! picture to the UI. The server only forwards fragments to subscribed viewers.
//!
//! Watching works on every platform. Sharing is only implemented on
//! Windows so far: elsewhere [`CAN_SHARE`] is `false` and the share
//! buttons are disabled.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[cfg(windows)]
mod audio;
mod view;
#[cfg(windows)]
mod win;

pub use view::{Frame, Viewer, YuvFrame};
#[cfg(windows)]
pub use win::{Sharer, sources};

#[cfg(not(windows))]
pub use stub::{Sharer, sources};

/// Whether this platform can share its screen. Watching is always available.
pub const CAN_SHARE: bool = cfg!(windows);

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

    /// Encoder bitrate, bits per second. Fast motion (games, scene cuts)
    /// turned to mush at half these rates. The server passes up to 16 Mbit/s
    /// per share, audio included (`SCREEN_BYTES_PER_SEC` in server/src/conn.rs).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn bitrate(self) -> u32 {
        match self {
            Quality::P720F30 => 5_000_000,
            Quality::P1080F30 => 8_000_000,
            Quality::P1080F60 => 12_000_000,
        }
    }
}

/// Largest H.264 slice. Frames are split into slices so the encoder
/// can compress them on several threads.
const MAX_SLICE_BYTES: u32 = 10_000;

/// The sharer's encoder settings. Shared with the tests on every platform:
/// they check that viewers decode exactly this stream.
#[cfg_attr(not(windows), allow(dead_code))]
fn encoder_config(quality: Quality) -> openh264::encoder::EncoderConfig {
    use openh264::encoder::{
        BitRate, Complexity, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode, UsageType,
    };
    let fps = quality.fps();
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4) as u16;
    EncoderConfig::new()
        // OpenH264 screen content mode spent 200 ms per 1080p frame on moving
        // content, sometimes over a second. Camera mode handles it in 45 ms,
        // and in 17 ms with multiple threads.
        .usage_type(UsageType::CameraVideoRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .bitrate(BitRate::from_bps(quality.bitrate()))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        .complexity(Complexity::Low)
        // Threads only help when the frame is split into several slices.
        .max_slice_len(MAX_SLICE_BYTES)
        .num_threads(threads)
        // A keyframe every 10 seconds just in case; normally viewers
        // request them.
        .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 10))
        // No frame skipping: otherwise after every heavy keyframe the encoder
        // dropped the following frames and the video froze. Now quality
        // dips briefly instead.
        .skip_frames(false)
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

/// Downscales a BGRA image (bilinear filter, SIMD). `pitch` is the source
/// row length in bytes: for captured frames it can exceed width × 4.
///
/// Runs on the calling thread on purpose. It used to split rows across
/// threads spawned for every frame, and in the running client (not in
/// headless tests) the OpenGL driver kept memory for every thread created:
/// about 80 MB per 5-second share from a 2560×1440 monitor.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn scale_bgra(src: &[u8], sw: usize, sh: usize, pitch: usize, dw: usize, dh: usize) -> Vec<u8> {
    use fast_image_resize::images::{CroppedImage, Image, ImageRef};
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

    let mut dst = vec![0u8; dw * dh * 4];
    if sw == dw && sh == dh {
        for (y, row) in dst.chunks_exact_mut(dw * 4).enumerate() {
            row.copy_from_slice(&src[y * pitch..y * pitch + dw * 4]);
        }
        return dst;
    }
    thread_local! {
        // Keeps its scratch buffers between frames.
        static RESIZER: std::cell::RefCell<Resizer> = std::cell::RefCell::new(Resizer::new());
    }
    let mut resize = || -> Result<(), String> {
        // The source as an image as wide as a whole row (padding included),
        // then a view of its visible part. A view, not `ResizeOptions::crop`:
        // with crop the filter still reads pixels past the edge, and the
        // padding showed up as a stripe along the right side.
        let whole = ImageRef::new((pitch / 4) as u32, sh as u32, src, PixelType::U8x4).map_err(|e| e.to_string())?;
        let visible = CroppedImage::new(&whole, 0, 0, sw as u32, sh as u32).map_err(|e| e.to_string())?;
        let mut dst_img =
            Image::from_slice_u8(dw as u32, dh as u32, &mut dst, PixelType::U8x4).map_err(|e| e.to_string())?;
        let options = ResizeOptions::new()
            .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
            // Capture alpha is not meaningful: no premultiplication.
            .use_alpha(false);
        RESIZER.with_borrow_mut(|r| r.resize(&visible, &mut dst_img, &options)).map_err(|e| e.to_string())
    };
    if let Err(e) = resize() {
        tracing::warn!("downscaling frame: {e}");
    }
    dst
}

#[cfg(not(windows))]
mod stub {
    use std::sync::Arc;

    use super::{Counters, Preview, Quality, Source, SourceId};

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
    fn scale_down_ignores_row_padding() {
        // 8x4 visible pixels of one color, rows padded to 10 pixels with white.
        let (sw, sh, pitch) = (8, 4, 10 * 4);
        let mut src = Vec::new();
        for _ in 0..sh {
            src.extend([10u8, 20, 30, 255].repeat(sw));
            src.extend([255u8; 8]);
        }
        let out = scale_bgra(&src, sw, sh, pitch, 4, 2);
        assert_eq!(out.len(), 4 * 2 * 4);
        for px in out.chunks(4) {
            assert!(px[..3].iter().zip([10, 20, 30]).all(|(a, b)| a.abs_diff(b) <= 1), "padding leaked: {px:?}");
        }
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
