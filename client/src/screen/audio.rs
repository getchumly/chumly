//! Screen share audio on Windows: per-process audio capture
//! (process loopback, Windows 10 2004 and later) and Opus encoding.
//!
//! Window share: audio of that window's program and its child
//! processes only. Screen share: all system audio except Chumly itself,
//! so viewers do not hear their own voices in the stream.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use shared::media::{MediaKind, VOICE_FRAME_SAMPLES, VOICE_SAMPLE_RATE};
use wasapi::{AudioClient, Direction, SampleType, StreamMode, WaveFormat};

/// What to capture.
#[derive(Clone, Copy, Debug)]
pub enum AudioTarget {
    /// Audio of the process and all its children.
    Only(u32),
    /// All audio except the process and its children.
    AllExcept(u32),
}

/// Screen share audio bitrate: music and games need more than voice.
const BITRATE: i32 = 128_000;
const CHANNELS: usize = 2;
/// Samples quieter than this are treated as silence and not sent.
const SILENCE: f32 = 1.0e-4;

pub struct StreamAudio {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl StreamAudio {
    pub fn start(target: AudioTarget, out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("screen-audio".into())
            .spawn(move || {
                if let Err(e) = capture_loop(target, &out, &flag, &ready_tx) {
                    // If startup already finished, nobody is waiting for this message.
                    let _ = ready_tx.send(Err(e.clone()));
                    tracing::warn!("screen share audio: {e}");
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { stop, thread: Some(thread) }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("audio capture thread exited".into()),
        }
    }
}

impl Drop for StreamAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn capture_loop(
    target: AudioTarget,
    out: &tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>,
    stop: &AtomicBool,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    // Dedicated thread with no UI on it, so MTA is fine.
    let _ = wasapi::initialize_mta();
    let (pid, include) = match target {
        AudioTarget::Only(pid) => (pid, true),
        AudioTarget::AllExcept(pid) => (pid, false),
    };
    let format = WaveFormat::new(32, 32, SampleType::Float, VOICE_SAMPLE_RATE as usize, CHANNELS, None);
    let block_align = format.get_blockalign() as usize;
    let mut client = AudioClient::new_application_loopback_client(pid, include).map_err(|e| err(&e))?;
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 };
    client.initialize_client(&format, Direction::Capture, mode, None).map_err(|e| err(&e))?;
    let event = client.set_get_eventhandle().map_err(|e| err(&e))?;
    let capture = client.get_audiocaptureclient().map_err(|e| err(&e))?;

    let mut encoder =
        opus::Encoder::new(VOICE_SAMPLE_RATE, opus::Channels::Stereo, opus::Application::Audio).map_err(|e| err(&e))?;
    encoder.set_bitrate(opus::Bitrate::Bits(BITRATE)).map_err(|e| err(&e))?;

    client.start_stream().map_err(|e| err(&e))?;
    let _ = ready.send(Ok(()));

    let frame_bytes = VOICE_FRAME_SAMPLES * block_align;
    let mut queue: VecDeque<u8> = VecDeque::new();
    let mut pcm = vec![0.0f32; VOICE_FRAME_SAMPLES * CHANNELS];
    let mut packet = vec![0u8; 4000];
    let mut raw = Vec::with_capacity(frame_bytes);
    while !stop.load(Ordering::Relaxed) {
        // With nothing playing there may be no events: just keep waiting.
        if event.wait_for_event(200).is_err() {
            continue;
        }
        loop {
            match capture.get_next_packet_size() {
                Ok(Some(n)) if n > 0 => {
                    capture.read_from_device_to_deque(&mut queue).map_err(|e| err(&e))?;
                }
                Ok(_) => break,
                Err(e) => return Err(err(&e)),
            }
        }
        while queue.len() >= frame_bytes {
            raw.clear();
            raw.extend(queue.drain(..frame_bytes));
            for (s, bytes) in pcm.iter_mut().zip(raw.as_chunks::<4>().0) {
                *s = f32::from_le_bytes(*bytes);
            }
            // Silence is not sent: viewers do not need the extra traffic.
            if pcm.iter().all(|s| s.abs() < SILENCE) {
                continue;
            }
            let len = encoder.encode_float(&pcm, &mut packet).map_err(|e| err(&e))?;
            // If the network cannot keep up, drop this chunk rather than build up latency.
            let _ = out.try_send((MediaKind::ScreenAudio, packet[..len].to_vec()));
        }
    }
    let _ = client.stop_stream();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Starts capturing all system audio except this process.
    /// Only checks that Windows allowed opening it.
    #[test]
    #[ignore = "opens real system audio"]
    fn starts_capture() {
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        let a = StreamAudio::start(AudioTarget::AllExcept(std::process::id()), tx).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(a);
    }

    /// A separate process plays a system sound; check that its audio
    /// shows up in both capture modes.
    #[test]
    #[ignore = "plays sound through the speakers"]
    fn captures_other_process() {
        let play = || {
            std::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    "(New-Object Media.SoundPlayer 'C:\\Windows\\Media\\Ring01.wav').PlaySync()",
                ])
                .spawn()
                .unwrap()
        };
        for only in [true, false] {
            // The exclusion also applies to child processes, and the player is
            // spawned by the test, so exclude an unrelated process instead.
            let mut decoy = std::process::Command::new("ping")
                .args(["-n", "15", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let mut child = play();
            let target = if only { AudioTarget::Only(child.id()) } else { AudioTarget::AllExcept(decoy.id()) };
            let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
            let a = StreamAudio::start(target, tx).unwrap();
            let _ = child.wait();
            drop(a);
            let _ = decoy.kill();
            let _ = decoy.wait();
            let mut packets = 0;
            while let Ok((kind, p)) = rx.try_recv() {
                assert_eq!(kind, MediaKind::ScreenAudio);
                assert!(!p.is_empty());
                packets += 1;
            }
            println!("{target:?}: {packets} packets (20 ms each)");
            assert!(packets > 10, "no audio captured");
        }
    }
}
