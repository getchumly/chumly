//! The sharer's encoder thread, the same on every platform: takes captured
//! frames, encodes them with OpenH264, sends the fragments and hands frames
//! to our own preview. On Windows, frames prepared on the GPU go to the
//! graphics card's own encoder instead while it works (`encode/mf.rs`,
//! compiled only there).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use eframe::egui;
use openh264::OpenH264API;
use openh264::encoder::{Encoder, EncoderConfig, FrameType};
use openh264::formats::{BgraSliceU8, YUVBuffer, YUVSource};
use shared::media::MediaKind;

use super::capture::{Captured, Pixels};
use super::preview::{Preview, PreviewMode, THUMBNAIL_INTERVAL, preview_image};
use super::{Counters, Quality, Timing, packet};

#[cfg(windows)]
mod mf;

/// Keyframes no more often than this, however many viewers ask.
const MIN_KEYFRAME_INTERVAL: Duration = Duration::from_millis(300);
/// Largest H.264 slice. Frames are split into slices so the encoder
/// can compress them on several threads.
const MAX_SLICE_BYTES: u32 = 10_000;

/// The sharer's encoder settings. Shared with the tests: they check that
/// viewers decode exactly this stream.
pub(super) fn encoder_config(quality: Quality) -> EncoderConfig {
    use openh264::encoder::{BitRate, Complexity, FrameRate, IntraFramePeriod, RateControlMode, UsageType};
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

/// What the encoder thread last gave the preview.
struct PreviewState {
    mode: PreviewMode,
    at: Option<Instant>,
    /// A frame arrived that the preview has not shown yet.
    stale: bool,
}

pub(super) struct EncoderThread {
    pub(super) preview: Arc<Preview>,
    pub(super) ctx: egui::Context,
    pub(super) rx: Receiver<Captured>,
    /// Set by the capture when it hands over a frame, cleared here on taking it.
    pub(super) pending: Arc<AtomicBool>,
    pub(super) keyframe: Arc<AtomicBool>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) ended: Arc<AtomicBool>,
    pub(super) quality: Quality,
    pub(super) out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>,
    pub(super) counters: Arc<Counters>,
    /// Windows: encode on the graphics card when it can ("Hardware
    /// acceleration" in the share dialog).
    #[cfg(windows)]
    pub(super) hardware: bool,
}

impl EncoderThread {
    pub(super) fn run(self) {
        if let Err(e) = self.encode_loop() {
            tracing::error!("screen share encoder: {e}");
            self.ended.store(true, Ordering::Relaxed);
        }
    }

    fn encode_loop(&self) -> Result<(), String> {
        let config = encoder_config(self.quality);
        let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| e.to_string())?;
        let mut yuv: Option<YUVBuffer> = None;
        let mut last: Option<Captured> = None;
        let mut last_keyframe: Option<Instant> = None;
        let mut frame_id: u32 = 0;
        let mut bytes = Vec::new();
        let mut preview = PreviewState { mode: PreviewMode::Off, at: None, stale: false };
        #[cfg(windows)]
        let mut hardware = mf::Hardware::new(self.hardware, self.quality);
        #[cfg(windows)]
        let mut i420 = Vec::new();

        while !self.stop.load(Ordering::Relaxed) {
            let fresh = match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(c) => {
                    self.pending.store(false, Ordering::Relaxed);
                    Some(c)
                }
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let want_key = self.keyframe.load(Ordering::Relaxed)
                && last_keyframe.is_none_or(|t| t.elapsed() >= MIN_KEYFRAME_INTERVAL);
            if let Some(c) = fresh {
                last = Some(c);
                preview.stale = true;
            } else if !want_key {
                // The screen did not change and nobody needs a keyframe.
                // The preview may still want the last frame: it was just
                // opened, or the thumbnail skipped the newest frame.
                self.update_preview(last.as_ref(), &mut preview);
                continue;
            }
            // A static screen produces no frames; re-encode the last one
            // for a new viewer.
            let Some(frame) = last.as_ref() else { continue };
            if want_key {
                self.keyframe.store(false, Ordering::Relaxed);
                last_keyframe = Some(Instant::now());
                encoder.force_intra_frame();
            }

            let t = Instant::now();
            let (width, height) = (frame.width, frame.height);
            // Windows: frames prepared on the GPU go to the hardware encoder
            // while it works; otherwise on to OpenH264 below.
            #[cfg(windows)]
            if let Pixels::Nv12(nv12) = &frame.pixels
                && let Some(frames) = hardware.encode(nv12, width, height, want_key, &self.counters)
            {
                self.count_encoded(t);
                for f in frames {
                    self.emit(&mut frame_id, f.keyframe, &f.data);
                }
                self.update_preview(Some(frame), &mut preview);
                continue;
            }
            let stream = match &frame.pixels {
                Pixels::Bgra(bgra) => {
                    let buf = match &mut yuv {
                        Some(b) if b.dimensions() == (width, height) => b,
                        _ => yuv.insert(YUVBuffer::new(width, height)),
                    };
                    buf.read_bgra8(BgraSliceU8::new(bgra, (width, height)));
                    encoder.encode(buf)
                }
                // Windows only: prepared on the GPU, but the hardware encoder
                // is off or failed.
                #[cfg(windows)]
                Pixels::Nv12(nv12) => {
                    super::capture::nv12_to_i420(nv12, width, height, &mut i420);
                    encoder.encode(&super::capture::I420Frame { data: &i420, width, height })
                }
            }
            .map_err(|e| e.to_string())?;
            self.count_encoded(t);
            let kind = stream.frame_type();
            if matches!(kind, FrameType::Skip | FrameType::Invalid) {
                continue;
            }
            let keyframe = matches!(kind, FrameType::IDR | FrameType::I);
            bytes.clear();
            stream.write_vec(&mut bytes);
            self.emit(&mut frame_id, keyframe, &bytes);
            // After sending, so the preview does not delay the stream.
            self.update_preview(Some(frame), &mut preview);
        }
        Ok(())
    }

    fn count_encoded(&self, started: Instant) {
        let timing = &self.counters.timing;
        let us = Timing::add(&timing.encode_us, started);
        timing.encode_max_us.fetch_max(us, Ordering::Relaxed);
        timing.encoded.fetch_add(1, Ordering::Relaxed);
    }

    /// Sends one encoded frame.
    fn emit(&self, frame_id: &mut u32, keyframe: bool, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.counters.timing.bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        self.send_frame(*frame_id, keyframe, data);
        *frame_id = frame_id.wrapping_add(1);
    }

    /// Hands `frame` to the UI if the preview wants it: right after the mode
    /// changed, then every new frame (full) or a few times a second (thumbnail).
    fn update_preview(&self, frame: Option<&Captured>, st: &mut PreviewState) {
        let mode = self.preview.mode();
        let changed = mode != st.mode;
        st.mode = mode;
        if mode == PreviewMode::Off {
            return;
        }
        let Some(frame) = frame else { return };
        let due = mode == PreviewMode::Full || st.at.is_none_or(|t| t.elapsed() >= THUMBNAIL_INTERVAL);
        if !changed && !(st.stale && due) {
            return;
        }
        st.at = Some(Instant::now());
        st.stale = false;
        let (w, h) = (frame.width, frame.height);
        let image = match &frame.pixels {
            Pixels::Bgra(bgra) => preview_image(bgra, w, h, mode),
            #[cfg(windows)]
            Pixels::Nv12(nv12) => preview_image(&super::capture::nv12_to_bgra(nv12, w, h), w, h, mode),
        };
        self.preview.put(image);
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        self.ctx.request_repaint_of(self.preview.window());
    }

    fn send_frame(&self, frame_id: u32, keyframe: bool, data: &[u8]) {
        let Some(fragments) = packet::split(frame_id, keyframe, data) else {
            tracing::warn!("screen share frame too large: {} bytes", data.len());
            return;
        };
        for fragment in fragments {
            // Queue is full: the network cannot keep up. The frame is broken
            // anyway, viewers will request a keyframe.
            if self.out.try_send((MediaKind::Screen, fragment)).is_err() {
                self.counters.lost.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        self.counters.frames.fetch_add(1, Ordering::Relaxed);
    }
}
