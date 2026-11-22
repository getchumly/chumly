//! A running screen share, the same on every platform: a capture backend
//! feeds the encoder thread, the share's audio runs next to them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use shared::media::MediaKind;

use super::audio::{AudioTarget, StreamAudio};
use super::capture::{self, Capture, FrameSink};
use super::encode::EncoderThread;
use super::{Counters, Preview, Quality, SourceId};

/// A running screen share. Dropping it stops capture and the encoder.
pub struct Sharer {
    capture: Option<Box<dyn Capture>>,
    keyframe: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    counters: Arc<Counters>,
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

        let sink = FrameSink::new(tx, pending, ended.clone(), ctx, counters.clone(), quality);
        let capture = match capture::start(source, sink) {
            Ok(c) => c,
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                return Err(e);
            }
        };
        // Where share audio is not available the dialog does not offer it.
        let (audio, audio_error) = if with_audio && super::CAN_SHARE_AUDIO {
            let target = match capture.audio_process() {
                Some(pid) => AudioTarget::Only(pid),
                // Everything but Chumly itself: viewers must not hear their own voices.
                None => AudioTarget::AllExcept(std::process::id()),
            };
            match StreamAudio::start(target, audio_out) {
                Ok(a) => (Some(a), None),
                Err(e) => (None, Some(e)),
            }
        } else {
            (None, None)
        };
        Ok(Self { capture: Some(capture), keyframe, ended, stop, counters, _audio: audio, audio_error })
    }

    /// The shared window is minimized: no new frames come, viewers see the
    /// last one. Always `false` for a monitor.
    pub fn source_minimized(&self) -> bool {
        self.capture.as_ref().is_some_and(|c| c.source_minimized())
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
    pub fn counters(&self) -> &Arc<Counters> {
        &self.counters
    }

    /// Capture ended on its own: the window was closed or an error occurred.
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed) || self.capture.as_ref().is_none_or(|c| c.finished())
    }
}

impl Drop for Sharer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Stops capturing.
        self.capture.take();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::time::{Duration, Instant};

    use openh264::decoder::Decoder;
    use openh264::formats::YUVSource;
    use shared::media::ScreenFragment;

    use super::super::packet::Assembler;
    use super::*;

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
        // CHUMLY_TEST_NODECODE=1: only the sharer runs, so the process CPU is its cost.
        let decode = std::env::var("CHUMLY_TEST_NODECODE").is_err();
        let mut asm = Assembler::default();
        let mut dec = Decoder::new().unwrap();
        let (mut decoded, mut dec_us) = (0u64, 0u64);
        // Skip the first second: capture start-up.
        std::thread::sleep(Duration::from_secs(1));
        while rx.try_recv().is_ok() {}
        let cpu_start = process_cpu();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            let Ok((_, p)) = rx.try_recv() else {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            };
            if decode && let Some(f) = asm.push(ScreenFragment::decode(&p).unwrap()) {
                let t = Instant::now();
                if let Ok(Some(_)) = dec.decode(&f.data) {
                    dec_us += t.elapsed().as_micros() as u64;
                    decoded += 1;
                }
            }
        }
        let el = start.elapsed().as_secs_f64();
        let cpu = (process_cpu() - cpu_start).as_secs_f64();
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        let c = sharer.counters();
        let tm = &c.timing;
        let ld = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
        let captured = ld(&tm.captured);
        let busy = ld(&tm.busy);
        let taken = (captured - busy).max(1);
        let enc = ld(&tm.encoded).max(1);
        println!("{} in {el:.1} s, source {}", quality.label(), monitor_size(1));
        println!("  process CPU {:.1}% of all cores ({:.0}% of one)", cpu / el / cores * 100.0, cpu / el * 100.0);
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

    /// CPU time this process has used, all threads.
    fn process_cpu() -> Duration {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let (mut c, mut e, mut k, mut u) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: plain out-parameters for the current process.
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) }.unwrap();
        let t = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
        Duration::from_nanos((t(k) + t(u)) * 100)
    }

    fn monitor_size(index: usize) -> String {
        let m = windows_capture::monitor::Monitor::from_index(index).unwrap();
        format!("{}×{}", m.width().unwrap_or(0), m.height().unwrap_or(0))
    }
}
