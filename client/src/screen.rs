//! Screen sharing.
//!
//! Sharer ([`Sharer`], `share.rs`): a capture backend (`capture/`, one per
//! platform) delivers frames, they are downscaled (`scale.rs`), encoded with
//! H.264 on the encoder thread (`encode.rs`) and split into datagrams
//! (`packet.rs`); our own preview gets the frames too (`preview.rs`), the
//! share's audio runs next to them (`audio/`).
//! Viewer ([`Viewer`], `view.rs`): reassembling frames from fragments,
//! decoding, handing the picture to the UI. The server only forwards
//! fragments to subscribed viewers.
//!
//! Watching works on every platform. Sharing works on Windows and on Linux
//! under X11 ([`can_share`]); the share's audio only on Windows so far
//! ([`CAN_SHARE_AUDIO`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

mod audio;
mod capture;
mod encode;
mod packet;
mod preview;
mod scale;
mod share;
mod view;

pub use capture::sources;
pub use preview::{Preview, PreviewMode};
pub use share::Sharer;
pub use view::{Frame, Viewer, YuvFrame};

/// Whether this session can share its screen: Windows, or Linux under X11.
/// Watching is always available.
pub fn can_share() -> bool {
    capture::supported()
}

/// Whether the share's audio can go with the video (Windows only so far).
pub const CAN_SHARE_AUDIO: bool = cfg!(windows);

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

    pub fn max_height(self) -> u32 {
        match self {
            Quality::P720F30 => 720,
            Quality::P1080F30 | Quality::P1080F60 => 1080,
        }
    }

    pub fn fps(self) -> u32 {
        match self {
            Quality::P720F30 | Quality::P1080F30 => 30,
            Quality::P1080F60 => 60,
        }
    }

    /// Encoder bitrate, bits per second. Fast motion (games, scene cuts)
    /// turned to mush at half these rates. The server passes up to 16 Mbit/s
    /// per share, audio included (`SCREEN_BYTES_PER_SEC` in server/src/conn.rs).
    pub fn bitrate(self) -> u32 {
        match self {
            Quality::P720F30 => 5_000_000,
            Quality::P1080F30 => 8_000_000,
            Quality::P1080F60 => 12_000_000,
        }
    }
}

/// What to share: a monitor by index or a window by its handle. Only
/// platforms that can share list sources; elsewhere none is ever made.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceId {
    Monitor(usize),
    Window(isize),
    /// Whatever the user picks in the system dialog (Wayland's desktop portal).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Portal,
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
    pub timing: Timing,
}

/// Filled by the capture, encoder and decoder; read by the viewer window's
/// diagnostics and the capture benchmark test (some fields only there).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
pub struct Timing {
    /// How many frames the system delivered.
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

    pub(crate) fn attach(&self, from: shared::UserId, tx: std::sync::mpsc::Sender<Vec<u8>>) {
        *self.incoming.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        self.watching.store(from, Ordering::Relaxed);
    }

    pub(crate) fn detach(&self) {
        self.watching.store(0, Ordering::Relaxed);
        *self.incoming.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
