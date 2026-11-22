//! Screen and window capture: one backend per platform, one contract.
//!
//! A backend lists what can be shared ([`sources`]) and starts capturing a
//! source ([`start`]). It delivers frames through a [`FrameSink`]: the sink
//! paces them to the stream's frame rate, skips frames while the encoder is
//! busy, downscales and hands them to the encoder thread. The running
//! capture is a [`Capture`]; dropping it stops capturing.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant};

use eframe::egui;

use super::scale::{scale_bgra, target_size};
use super::{Counters, Quality, Timing};

#[cfg(windows)]
mod gpu;
#[cfg(windows)]
pub(super) use gpu::{I420Frame, i420_to_bgra};
#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::sources;
#[cfg(windows)]
pub(super) use win::start;

#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod x11;

/// Linux: X11 lists monitors and windows itself; under Wayland the system
/// dialog of the desktop portal picks, so there is one entry for it.
#[cfg(target_os = "linux")]
pub fn sources() -> Vec<super::Source> {
    if x11::session_supported() { x11::sources() } else { portal::sources() }
}

#[cfg(target_os = "linux")]
pub(super) fn start(source: super::SourceId, sink: FrameSink) -> Result<Box<dyn Capture>, String> {
    match source {
        super::SourceId::Portal => portal::start(sink),
        _ => x11::start(source, sink),
    }
}

/// Whether this session can share its screen: Windows, Linux under X11 or
/// Wayland (through the desktop portal).
pub fn supported() -> bool {
    #[cfg(windows)]
    return true;
    #[cfg(target_os = "linux")]
    return x11::session_supported() || portal::session_supported();
    #[cfg(not(any(windows, target_os = "linux")))]
    return false;
}

/// No capture on this platform: nothing to share.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn sources() -> Vec<super::Source> {
    Vec::new()
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(super) fn start(_source: super::SourceId, _sink: FrameSink) -> Result<Box<dyn Capture>, String> {
    Err("screen sharing is not available on this platform".into())
}

/// A running capture. Dropping it stops capturing.
pub(super) trait Capture {
    /// Capture stopped on its own: the window was closed or an error occurred.
    fn finished(&self) -> bool;
    /// The shared window is minimized: no new frames come, viewers see the
    /// last one. Always `false` for a monitor.
    fn source_minimized(&self) -> bool;
    /// Whose audio goes with the video: the shared window's process, or
    /// `None` for all system audio (except our own).
    fn audio_process(&self) -> Option<u32>;
}

/// A captured frame at the stream's size.
pub(super) struct Captured {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) pixels: Pixels,
}

/// The pixels of a [`Captured`] frame, tightly packed.
pub(super) enum Pixels {
    /// Downscaled on the CPU.
    Bgra(Vec<u8>),
    /// Windows only: downscaled and converted on the GPU (`capture/gpu.rs`):
    /// the Y plane, then U, then V, the encoder's own format. Does not exist
    /// in other builds, so their path is untouched by it.
    #[cfg(windows)]
    I420(Vec<u8>),
}

/// Where a capture backend puts its frames.
#[derive(Clone)]
pub(super) struct FrameSink {
    out: SyncSender<Captured>,
    /// A frame is already waiting for the encoder: skip the next captured
    /// one instead of wasting time downscaling it.
    pending: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
    ctx: egui::Context,
    counters: Arc<Counters>,
    /// Time between frames at the stream's frame rate.
    interval: Duration,
    max_height: u32,
}

// Only used by capture backends.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
impl FrameSink {
    pub(super) fn new(
        out: SyncSender<Captured>,
        pending: Arc<AtomicBool>,
        ended: Arc<AtomicBool>,
        ctx: egui::Context,
        counters: Arc<Counters>,
        quality: Quality,
    ) -> Self {
        let interval = Duration::from_secs_f64(1.0 / quality.fps() as f64);
        Self { out, pending, ended, ctx, counters, interval, max_height: quality.max_height() }
    }

    /// Time between frames at the stream's frame rate.
    pub(super) fn interval(&self) -> Duration {
        self.interval
    }

    /// Whether to take the frame the system just delivered. Systems deliver
    /// frames at the monitor's refresh rate: the extras are skipped, and so
    /// are frames while the encoder is still busy. `last`: when the backend
    /// last took a frame, updated here.
    pub(super) fn take_now(&self, last: &mut Option<Instant>) -> bool {
        let now = Instant::now();
        if last.is_some_and(|l| now - l < self.interval.mul_f32(0.9)) {
            return false;
        }
        let timing = &self.counters.timing;
        timing.captured.fetch_add(1, Ordering::Relaxed);
        if self.pending.load(Ordering::Relaxed) {
            timing.busy.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        *last = Some(now);
        true
    }

    /// Downscales a BGRA frame (`sw` × `sh`, rows `pitch` bytes apart) to
    /// the stream's size and hands it to the encoder. `copy_started`: when
    /// the backend began reading the frame (for the timing counters).
    pub(super) fn push(&self, raw: &[u8], sw: u32, sh: u32, pitch: usize, copy_started: Instant) -> Result<(), String> {
        let timing = &self.counters.timing;
        Timing::add(&timing.copy_us, copy_started);
        let (dw, dh) = target_size(sw, sh, self.max_height);
        let t = Instant::now();
        let bgra = scale_bgra(raw, sw as usize, sh as usize, pitch, dw as usize, dh as usize);
        Timing::add(&timing.scale_us, t);
        self.pending.store(true, Ordering::Relaxed);
        self.send(Captured { width: dw as usize, height: dh as usize, pixels: Pixels::Bgra(bgra) })
    }

    /// The stream's frame size for a `sw` × `sh` source.
    #[cfg(windows)]
    pub(super) fn target_size(&self, sw: u32, sh: u32) -> (u32, u32) {
        target_size(sw, sh, self.max_height)
    }

    /// Hands the encoder a frame already prepared at the stream's size (on
    /// the GPU). `started`: when the backend began preparing it.
    #[cfg(windows)]
    pub(super) fn push_prepared(&self, frame: Captured, started: Instant) -> Result<(), String> {
        Timing::add(&self.counters.timing.copy_us, started);
        self.pending.store(true, Ordering::Relaxed);
        self.send(frame)
    }

    fn send(&self, frame: Captured) -> Result<(), String> {
        match self.out.try_send(frame) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err("encoder stopped".into()),
        }
    }

    /// The source went away (window closed): the share ends.
    pub(super) fn closed(&self) {
        self.ended.store(true, Ordering::Relaxed);
        self.ctx.request_repaint();
    }
}
