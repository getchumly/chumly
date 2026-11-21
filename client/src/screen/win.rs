//! Screen sharing on Windows: Windows Graphics Capture for capture,
//! software OpenH264 for encoding and decoding.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use openh264::OpenH264API;
use openh264::decoder::Decoder;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, RateControlMode, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer, YUVSource};
use shared::ClientMsg;
use shared::media::{MediaKind, SCREEN_FRAGMENT_DATA, ScreenFragment};
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, GraphicsCaptureItemType,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use super::audio::{AudioTarget, StreamAudio};
use super::{
    Counters, Preview, PreviewMode, Quality, ScreenShared, Source, SourceId, THUMBNAIL_INTERVAL, Timing, preview_image,
    scale_bgra, target_size,
};
use crate::net::NetCommand;

/// Keyframes no more often than this, however many viewers ask.
const MIN_KEYFRAME_INTERVAL: Duration = Duration::from_millis(300);
/// Largest H.264 slice. Frames are split into slices so the encoder
/// can compress them on several threads.
const MAX_SLICE_BYTES: u32 = 10_000;
/// A viewer does not request keyframes more often than this.
const KEYFRAME_RETRY: Duration = Duration::from_millis(500);

/// Monitors and windows available for sharing.
pub fn sources() -> Vec<Source> {
    let mut out = Vec::new();
    if let Ok(monitors) = Monitor::enumerate() {
        for (i, m) in monitors.iter().enumerate() {
            let meta = match (m.width(), m.height()) {
                (Ok(w), Ok(h)) => format!("{w}×{h}"),
                _ => String::new(),
            };
            out.push(Source { id: SourceId::Monitor(i + 1), label: (i + 1).to_string(), meta });
        }
    }
    let me = std::process::id();
    if let Ok(windows) = Window::enumerate() {
        for w in windows {
            let Ok(title) = w.title() else { continue };
            if title.trim().is_empty() || w.process_id().is_ok_and(|p| p == me) {
                continue;
            }
            let meta = w.process_name().unwrap_or_default();
            out.push(Source { id: SourceId::Window(w.as_raw_hwnd() as isize), label: title, meta });
        }
    }
    out
}

struct Captured {
    width: usize,
    height: usize,
    bgra: Vec<u8>,
}

#[derive(Clone)]
struct Flags {
    out: SyncSender<Captured>,
    /// A frame is already waiting for the encoder: skip the next captured
    /// one instead of wasting time downscaling it.
    pending: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
    interval: Duration,
    max_height: u32,
    ctx: egui::Context,
    counters: Arc<Counters>,
}

struct Handler {
    flags: Flags,
    last: Option<Instant>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = Flags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { flags: ctx.flags, last: None })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, _control: InternalCaptureControl) -> Result<(), Self::Error> {
        let now = Instant::now();
        // Windows deliver frames at the monitor refresh rate; skip the extras.
        if self.last.is_some_and(|l| now - l < self.flags.interval.mul_f32(0.9)) {
            return Ok(());
        }
        let timing = &self.flags.counters.timing;
        timing.captured.fetch_add(1, Ordering::Relaxed);
        if self.flags.pending.load(Ordering::Relaxed) {
            timing.busy.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        self.last = Some(now);
        let t = Instant::now();
        let mut buf = frame.buffer().map_err(|e| e.to_string())?;
        let (sw, sh, pitch) = (buf.width(), buf.height(), buf.row_pitch() as usize);
        let (dw, dh) = target_size(sw, sh, self.flags.max_height);
        let raw = buf.as_raw_buffer();
        Timing::add(&timing.copy_us, t);
        let t = Instant::now();
        let bgra = scale_bgra(raw, sw as usize, sh as usize, pitch, dw as usize, dh as usize);
        Timing::add(&timing.scale_us, t);
        self.flags.pending.store(true, Ordering::Relaxed);
        let captured = Captured { width: dw as usize, height: dh as usize, bgra };
        match self.flags.out.try_send(captured) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err("encoder stopped".into()),
        }
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.flags.ended.store(true, Ordering::Relaxed);
        self.flags.ctx.request_repaint();
        Ok(())
    }
}

type Control = CaptureControl<Handler, String>;

fn start_capture<T>(item: T, flags: Flags) -> Result<Control, String>
where
    T: TryInto<GraphicsCaptureItemType> + Send + Copy + 'static,
{
    // No yellow border and a frame rate cap. Only available on
    // newer Windows builds; older ones start with default settings.
    let tuned = Settings::new(
        item,
        CursorCaptureSettings::WithCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Custom(flags.interval),
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags.clone(),
    );
    match Handler::start_free_threaded(tuned) {
        Ok(c) => Ok(c),
        Err(first) => {
            tracing::debug!("capture with settings failed ({first}), retrying with defaults");
            let plain = Settings::new(
                item,
                CursorCaptureSettings::Default,
                DrawBorderSettings::Default,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Default,
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                flags,
            );
            Handler::start_free_threaded(plain).map_err(|e| e.to_string())
        }
    }
}

/// A running screen share. Dropping it stops capture and the encoder.
pub struct Sharer {
    control: Option<Control>,
    keyframe: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    counters: Arc<Counters>,
    /// The shared window; `None` for a monitor.
    hwnd: Option<isize>,
    // Declared last so it stops after the video.
    _audio: Option<StreamAudio>,
    audio_error: Option<String>,
}

impl Sharer {
    /// `with_audio`: also capture audio. If audio fails to start, video
    /// still runs and the reason is available via [`Sharer::audio_error`].
    /// Captured frames also go to `preview` while the UI asks for them.
    pub fn start(
        source: SourceId,
        quality: Quality,
        with_audio: bool,
        out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>,
        preview: Arc<Preview>,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let pending = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());

        let audio_out = out.clone();
        let enc = EncoderThread {
            preview,
            ctx: ctx.clone(),
            counters: counters.clone(),
            rx,
            pending: pending.clone(),
            keyframe: keyframe.clone(),
            stop: stop.clone(),
            ended: ended.clone(),
            quality,
            out,
        };
        std::thread::Builder::new()
            .name("screen-encoder".into())
            .spawn(move || enc.run())
            .map_err(|e| e.to_string())?;

        let flags = Flags {
            out: tx,
            pending,
            ended: ended.clone(),
            interval: Duration::from_secs_f64(1.0 / quality.fps() as f64),
            max_height: quality.max_height(),
            ctx,
            counters: counters.clone(),
        };
        let mut audio_target = AudioTarget::AllExcept(std::process::id());
        let hwnd = match source {
            SourceId::Window(h) => Some(h),
            SourceId::Monitor(_) => None,
        };
        let started = match source {
            SourceId::Monitor(index) => {
                let m = Monitor::from_index(index).map_err(|e| e.to_string())?;
                start_capture(m, flags)
            }
            SourceId::Window(hwnd) => {
                let w = Window::from_raw_hwnd(hwnd as *mut std::ffi::c_void);
                if !w.is_valid() {
                    stop.store(true, Ordering::Relaxed);
                    return Err("window already closed".into());
                }
                match w.process_id() {
                    Ok(pid) => audio_target = AudioTarget::Only(pid),
                    Err(e) => tracing::warn!("failed to get window process: {e}"),
                }
                start_capture(w, flags)
            }
        };
        let control = match started {
            Ok(c) => c,
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                return Err(e);
            }
        };
        let (audio, audio_error) = if with_audio {
            match StreamAudio::start(audio_target, audio_out) {
                Ok(a) => (Some(a), None),
                Err(e) => (None, Some(e)),
            }
        } else {
            (None, None)
        };
        Ok(Self { control: Some(control), keyframe, ended, stop, counters, hwnd, _audio: audio, audio_error })
    }

    /// The shared window is minimized: Windows sends no new frames, viewers
    /// see the last one. Always `false` for a monitor.
    pub fn source_minimized(&self) -> bool {
        // SAFETY: IsIconic only reads the window state; a stale handle gives 0.
        self.hwnd.is_some_and(|h| unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(h as _) } != 0)
    }

    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }

    /// Why audio failed to start, if audio was requested.
    pub fn audio_error(&self) -> Option<&str> {
        self.audio_error.as_deref()
    }

    /// frames: sent frames, lost: dropped because the network
    /// could not keep up.
    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// Capture ended on its own: the window was closed or an error occurred.
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed) || self.control.as_ref().is_none_or(|c| c.is_finished())
    }
}

impl Drop for Sharer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(c) = self.control.take()
            && let Err(e) = c.stop()
        {
            tracing::debug!("stopping capture: {e}");
        }
    }
}

/// What the encoder thread last gave the preview.
struct PreviewState {
    mode: PreviewMode,
    at: Option<Instant>,
    /// A frame arrived that the preview has not shown yet.
    stale: bool,
}

struct EncoderThread {
    preview: Arc<Preview>,
    ctx: egui::Context,
    rx: Receiver<Captured>,
    pending: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
    quality: Quality,
    out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>,
    counters: Arc<Counters>,
}

impl EncoderThread {
    fn run(self) {
        if let Err(e) = self.encode_loop() {
            tracing::error!("screen share encoder: {e}");
            self.ended.store(true, Ordering::Relaxed);
        }
    }

    fn encode_loop(&self) -> Result<(), String> {
        let fps = self.quality.fps();
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4) as u16;
        let config = EncoderConfig::new()
            // OpenH264 screen content mode spent 200 ms per 1080p frame on moving
            // content, sometimes over a second. Camera mode handles it in 45 ms,
            // and in 17 ms with multiple threads.
            .usage_type(UsageType::CameraVideoRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(self.quality.bitrate()))
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
            .skip_frames(false);
        let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| e.to_string())?;
        let mut yuv: Option<YUVBuffer> = None;
        let mut last: Option<Captured> = None;
        let mut last_keyframe: Option<Instant> = None;
        let mut frame_id: u32 = 0;
        let mut bytes = Vec::new();
        let mut preview = PreviewState { mode: PreviewMode::Off, at: None, stale: false };

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

            let buf = match &mut yuv {
                Some(b) if b.dimensions() == (frame.width, frame.height) => b,
                _ => yuv.insert(YUVBuffer::new(frame.width, frame.height)),
            };
            let t = Instant::now();
            buf.read_bgra8(BgraSliceU8::new(&frame.bgra, (frame.width, frame.height)));
            let stream = encoder.encode(buf).map_err(|e| e.to_string())?;
            let timing = &self.counters.timing;
            let us = Timing::add(&timing.encode_us, t);
            timing.encode_max_us.fetch_max(us, Ordering::Relaxed);
            timing.encoded.fetch_add(1, Ordering::Relaxed);
            let kind = stream.frame_type();
            if matches!(kind, FrameType::Skip | FrameType::Invalid) {
                continue;
            }
            let keyframe = matches!(kind, FrameType::IDR | FrameType::I);
            bytes.clear();
            stream.write_vec(&mut bytes);
            if bytes.is_empty() {
                continue;
            }
            self.counters.timing.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            self.send_frame(frame_id, keyframe, &bytes);
            frame_id = frame_id.wrapping_add(1);
            // After sending, so the preview does not delay the stream.
            self.update_preview(Some(frame), &mut preview);
        }
        Ok(())
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
        self.preview.put(preview_image(&frame.bgra, frame.width, frame.height, mode));
        self.ctx.request_repaint();
    }

    fn send_frame(&self, frame_id: u32, keyframe: bool, data: &[u8]) {
        let count = data.len().div_ceil(SCREEN_FRAGMENT_DATA);
        let Ok(count) = u16::try_from(count) else {
            tracing::warn!("screen share frame too large: {} bytes", data.len());
            return;
        };
        for (index, chunk) in data.chunks(SCREEN_FRAGMENT_DATA).enumerate() {
            let fragment = ScreenFragment { frame_id, index: index as u16, count, keyframe, data: chunk };
            // Queue is full: the network cannot keep up. The frame is broken
            // anyway, viewers will request a keyframe.
            if self.out.try_send((MediaKind::Screen, fragment.encode())).is_err() {
                self.counters.lost.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        self.counters.frames.fetch_add(1, Ordering::Relaxed);
    }
}

/// Watching someone's screen share. Dropping it stops decoding.
pub struct Viewer {
    shared: Arc<ScreenShared>,
    frame: Arc<Mutex<Option<egui::ColorImage>>>,
    counters: Arc<Counters>,
}

impl Viewer {
    pub fn start(
        user_id: shared::UserId,
        shared: Arc<ScreenShared>,
        net: tokio::sync::mpsc::UnboundedSender<NetCommand>,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        let decoder = Decoder::new().map_err(|e| e.to_string())?;
        let (tx, rx) = std::sync::mpsc::channel();
        let frame = Arc::new(Mutex::new(None));
        let slot = frame.clone();
        let counters = Arc::new(Counters::default());
        let c = counters.clone();
        std::thread::Builder::new()
            .name("screen-decoder".into())
            .spawn(move || decode_loop(user_id, decoder, rx, slot, c, net, ctx))
            .map_err(|e| e.to_string())?;
        shared.attach(user_id, tx);
        Ok(Self { shared, frame, counters })
    }

    /// A new frame, if one arrived since the last call.
    pub fn take_frame(&self) -> Option<egui::ColorImage> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// frames: displayed frames, lost: lost in the network,
    /// waiting_key: picture is frozen until a keyframe arrives.
    pub fn counters(&self) -> &Counters {
        &self.counters
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        // The channel closes and the decoder thread exits on its own.
        self.shared.detach();
    }
}

/// Reassembles frames from fragments. Fragments may arrive out of order
/// or get lost: then the frame never completes and is discarded.
#[derive(Default)]
struct Assembler {
    partial: HashMap<u32, Partial>,
    /// Last completed frame: anything older is no longer needed.
    newest: Option<u32>,
}

struct Partial {
    keyframe: bool,
    parts: Vec<Option<Vec<u8>>>,
    received: usize,
}

struct Assembled {
    frame_id: u32,
    keyframe: bool,
    data: Vec<u8>,
}

/// `a` is newer than `b`, accounting for counter wraparound.
fn newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < u32::MAX / 2
}

impl Assembler {
    fn push(&mut self, f: ScreenFragment<'_>) -> Option<Assembled> {
        if self.newest.is_some_and(|n| !newer(f.frame_id, n)) {
            return None;
        }
        let p = self.partial.entry(f.frame_id).or_insert_with(|| Partial {
            keyframe: f.keyframe,
            parts: vec![None; f.count as usize],
            received: 0,
        });
        let slot = p.parts.get_mut(f.index as usize)?;
        if slot.is_none() {
            *slot = Some(f.data.to_vec());
            p.received += 1;
        }
        if p.received < p.parts.len() {
            // Do not accumulate incomplete frames: keep only the last 8.
            if self.partial.len() > 16 {
                let id = f.frame_id;
                self.partial.retain(|k, _| !newer(id, *k) || id.wrapping_sub(*k) < 8);
            }
            return None;
        }
        let p = self.partial.remove(&f.frame_id)?;
        self.newest = Some(f.frame_id);
        let id = f.frame_id;
        self.partial.retain(|k, _| newer(*k, id));
        let data = p.parts.into_iter().flatten().flatten().collect();
        Some(Assembled { frame_id: id, keyframe: p.keyframe, data })
    }
}

fn decode_loop(
    user_id: shared::UserId,
    mut decoder: Decoder,
    rx: Receiver<Vec<u8>>,
    slot: Arc<Mutex<Option<egui::ColorImage>>>,
    counters: Arc<Counters>,
    net: tokio::sync::mpsc::UnboundedSender<NetCommand>,
    ctx: egui::Context,
) {
    let mut asm = Assembler::default();
    let mut last_id: Option<u32> = None;
    let mut need_key = true;
    let mut last_request: Option<Instant> = None;
    let mut rgba = Vec::new();
    let request_key = |last: &mut Option<Instant>| {
        if last.is_none_or(|t| t.elapsed() >= KEYFRAME_RETRY) {
            *last = Some(Instant::now());
            let _ = net.send(NetCommand::Send(ClientMsg::RequestKeyframe { user_id }));
        }
    };

    while let Ok(payload) = rx.recv() {
        let Some(fragment) = ScreenFragment::decode(&payload) else { continue };
        let Some(frame) = asm.push(fragment) else { continue };
        // A frame was skipped: subsequent frames will decode with errors without it.
        if let Some(l) = last_id
            && frame.frame_id != l.wrapping_add(1)
        {
            counters.lost.fetch_add(frame.frame_id.wrapping_sub(l).wrapping_sub(1), Ordering::Relaxed);
            if !frame.keyframe {
                need_key = true;
            }
        }
        counters.waiting_key.store(need_key && !frame.keyframe, Ordering::Relaxed);
        last_id = Some(frame.frame_id);
        if need_key && !frame.keyframe {
            request_key(&mut last_request);
            continue;
        }
        let t = Instant::now();
        let image = match decoder.decode(&frame.data) {
            Ok(Some(yuv)) => {
                Timing::add(&counters.timing.decode_us, t);
                let t = Instant::now();
                let (w, h) = yuv.dimensions();
                rgba.resize(w * h * 4, 0);
                yuv.write_rgba8(&mut rgba);
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                Timing::add(&counters.timing.convert_us, t);
                image
            }
            Ok(None) => continue,
            Err(e) => {
                tracing::debug!("decoding screen share frame: {e}");
                need_key = true;
                request_key(&mut last_request);
                continue;
            }
        };
        need_key = false;
        counters.waiting_key.store(false, Ordering::Relaxed);
        counters.frames.fetch_add(1, Ordering::Relaxed);
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(image);
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frag(frame_id: u32, index: u16, count: u16, data: &[u8]) -> ScreenFragment<'_> {
        ScreenFragment { frame_id, index, count, keyframe: index == 0 && frame_id == 0, data }
    }

    #[test]
    fn assembles_out_of_order_and_drops_stale() {
        let mut a = Assembler::default();
        assert!(a.push(frag(0, 1, 2, b"cd")).is_none());
        let f = a.push(frag(0, 0, 2, b"ab")).unwrap();
        assert_eq!((f.frame_id, f.data.as_slice()), (0, &b"abcd"[..]));
        // Frame 1 lost a fragment, frame 2 completed: frame 1 is discarded.
        assert!(a.push(frag(1, 0, 2, b"x")).is_none());
        assert_eq!(a.push(frag(2, 0, 1, b"y")).unwrap().frame_id, 2);
        assert!(a.push(frag(1, 1, 2, b"z")).is_none());
        assert!(a.partial.is_empty());
    }

    /// Captures the first monitor and checks that frames are encoded and
    /// decoded. Needs a desktop, so it does not run by default.
    #[test]
    #[ignore = "captures the real screen"]
    fn capture_monitor() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
        let started = Instant::now();
        // For benchmarks: CHUMLY_TEST_QUALITY=1080 and CHUMLY_TEST_FRAMES=150.
        let quality = if std::env::var("CHUMLY_TEST_QUALITY").as_deref() == Ok("1080") {
            Quality::P1080F30
        } else {
            Quality::P720F30
        };
        let want: usize = std::env::var("CHUMLY_TEST_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
        let sharer =
            Sharer::start(SourceId::Monitor(1), quality, false, tx, Default::default(), egui::Context::default())
                .unwrap();
        let mut asm = Assembler::default();
        let mut dec = Decoder::new().unwrap();
        let mut frames = 0;
        let mut size = (0, 0);
        let deadline = Instant::now() + Duration::from_secs(3 + want as u64 / 10);
        while Instant::now() < deadline && frames < want {
            let Ok((_, p)) = rx.try_recv() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            if let Some(f) = asm.push(ScreenFragment::decode(&p).unwrap())
                && let Ok(Some(yuv)) = dec.decode(&f.data)
            {
                size = yuv.dimensions();
                frames += 1;
            }
        }
        assert!(!sharer.ended());
        drop(sharer);
        println!("frames: {frames}, size {size:?}, in {:?}", started.elapsed());
        assert!(frames >= 1, "no frames");
        assert!(size.1 <= quality.max_height() as usize && size.0 > 0);
    }

    /// Like capture_monitor, but prints per-stage timings. Something
    /// should be moving on screen when it runs.
    #[test]
    #[ignore = "captures the real screen"]
    fn capture_timing() {
        let quality = match std::env::var("CHUMLY_TEST_QUALITY").as_deref() {
            Ok("720") => Quality::P720F30,
            Ok("60") => Quality::P1080F60,
            _ => Quality::P1080F30,
        };
        let secs: u64 = std::env::var("CHUMLY_TEST_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8192);
        let sharer =
            Sharer::start(SourceId::Monitor(1), quality, false, tx, Default::default(), egui::Context::default())
                .unwrap();
        let mut asm = Assembler::default();
        let mut dec = Decoder::new().unwrap();
        let (mut decoded, mut dec_us) = (0u64, 0u64);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            let Ok((_, p)) = rx.try_recv() else {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            };
            if let Some(f) = asm.push(ScreenFragment::decode(&p).unwrap()) {
                let t = Instant::now();
                if let Ok(Some(_)) = dec.decode(&f.data) {
                    dec_us += t.elapsed().as_micros() as u64;
                    decoded += 1;
                }
            }
        }
        let el = start.elapsed().as_secs_f64();
        let c = sharer.counters();
        let tm = &c.timing;
        let ld = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
        let captured = ld(&tm.captured);
        let busy = ld(&tm.busy);
        let taken = (captured - busy).max(1);
        let enc = ld(&tm.encoded).max(1);
        println!("{} in {el:.1} s", quality.label());
        println!("  from Windows: {:.1} fps, skipped (encoder busy): {busy}", captured as f64 / el);
        println!(
            "  copy {:.1} ms, downscale {:.1} ms per frame",
            ld(&tm.copy_us) as f64 / taken as f64 / 1000.0,
            ld(&tm.scale_us) as f64 / taken as f64 / 1000.0
        );
        println!(
            "  encoded {:.1} fps, encode {:.1} ms avg, {:.1} ms max, {:.1} Mbit/s",
            enc as f64 / el,
            ld(&tm.encode_us) as f64 / enc as f64 / 1000.0,
            ld(&tm.encode_max_us) as f64 / 1000.0,
            ld(&tm.bytes) as f64 * 8.0 / el / 1e6
        );
        println!(
            "  sent {:.1} fps, dropped by network {}",
            c.frames.load(Ordering::Relaxed) as f64 / el,
            c.lost.load(Ordering::Relaxed)
        );
        println!(
            "  decoded {:.1} fps, {:.1} ms per frame",
            decoded as f64 / el,
            dec_us as f64 / decoded.max(1) as f64 / 1000.0
        );
    }

    #[test]
    fn encode_decode_roundtrip() {
        let (w, h) = (320usize, 240usize);
        let mut bgra = vec![0u8; w * h * 4];
        for (i, px) in bgra.chunks_mut(4).enumerate() {
            px.copy_from_slice(&[(i % w) as u8, (i / w) as u8, 128, 255]);
        }
        let config = EncoderConfig::new().usage_type(UsageType::ScreenContentRealTime);
        let mut enc = Encoder::with_api_config(OpenH264API::from_source(), config).unwrap();
        let mut yuv = YUVBuffer::new(w, h);
        yuv.read_bgra8(BgraSliceU8::new(&bgra, (w, h)));
        let data = enc.encode(&yuv).unwrap().to_vec();
        assert!(!data.is_empty());
        let mut dec = Decoder::new().unwrap();
        let out = dec.decode(&data).unwrap().expect("frame decoded");
        assert_eq!(out.dimensions(), (w, h));
    }
}
