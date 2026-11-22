//! The share's audio: one capture backend per platform (`win.rs`: WASAPI
//! process loopback; `linux.rs`: PipeWire), the same Opus packing for all.

use std::collections::VecDeque;

use shared::media::{MediaKind, VOICE_FRAME_SAMPLES, VOICE_SAMPLE_RATE};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod win;

#[cfg(target_os = "linux")]
pub(super) use linux::StreamAudio;
#[cfg(windows)]
pub(super) use win::StreamAudio;

#[cfg(not(any(windows, target_os = "linux")))]
pub(super) use stub::StreamAudio;

/// What to capture.
#[derive(Clone, Copy, Debug)]
pub(super) enum AudioTarget {
    /// Audio of the process and all its children.
    Only(u32),
    /// All audio except the process and its children.
    AllExcept(u32),
}

/// Screen share audio bitrate: music and games need more than voice.
const BITRATE: i32 = 128_000;
/// Interleaved stereo.
const CHANNELS: usize = 2;
/// Samples quieter than this are treated as silence and not sent.
const SILENCE: f32 = 1.0e-4;

/// Packs interleaved stereo f32 samples at 48 kHz into 20 ms Opus packets
/// and sends them as share audio. Silent chunks are not sent: viewers do
/// not need the extra traffic.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(super) struct Packer {
    encoder: opus::Encoder,
    queue: VecDeque<f32>,
    pcm: Vec<f32>,
    packet: Vec<u8>,
    out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>,
}

#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
impl Packer {
    pub(super) fn new(out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>) -> Result<Self, String> {
        let err = |e: opus::Error| e.to_string();
        let mut encoder =
            opus::Encoder::new(VOICE_SAMPLE_RATE, opus::Channels::Stereo, opus::Application::Audio).map_err(err)?;
        encoder.set_bitrate(opus::Bitrate::Bits(BITRATE)).map_err(err)?;
        let frame = VOICE_FRAME_SAMPLES * CHANNELS;
        Ok(Self { encoder, queue: VecDeque::new(), pcm: vec![0.0; frame], packet: vec![0; 4000], out })
    }

    /// Adds captured samples and sends every complete 20 ms frame.
    pub(super) fn push(&mut self, samples: impl IntoIterator<Item = f32>) -> Result<(), String> {
        self.queue.extend(samples);
        let frame = self.pcm.len();
        while self.queue.len() >= frame {
            for (dst, s) in self.pcm.iter_mut().zip(self.queue.drain(..frame)) {
                *dst = s;
            }
            if self.pcm.iter().all(|s| s.abs() < SILENCE) {
                continue;
            }
            let len = self.encoder.encode_float(&self.pcm, &mut self.packet).map_err(|e| e.to_string())?;
            // If the network cannot keep up, drop this chunk rather than build up latency.
            let _ = self.out.try_send((MediaKind::ScreenAudio, self.packet[..len].to_vec()));
        }
        Ok(())
    }
}

/// No share audio on this platform.
#[cfg(not(any(windows, target_os = "linux")))]
mod stub {
    use shared::media::MediaKind;

    use super::AudioTarget;

    pub struct StreamAudio;

    impl StreamAudio {
        pub fn start(
            _target: AudioTarget,
            _out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>,
        ) -> Result<Self, String> {
            Err("sharing audio is not available on this platform".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packer_sends_full_frames_and_skips_silence() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let mut p = Packer::new(tx).unwrap();
        let frame = VOICE_FRAME_SAMPLES * CHANNELS;
        // Half a frame of sound: nothing yet.
        p.push((0..frame / 2).map(|i| (i as f32 * 0.05).sin() * 0.3)).unwrap();
        assert!(rx.try_recv().is_err());
        // The other half completes one frame.
        p.push((frame / 2..frame).map(|i| (i as f32 * 0.05).sin() * 0.3)).unwrap();
        let (kind, packet) = rx.try_recv().expect("one packet");
        assert_eq!(kind, MediaKind::ScreenAudio);
        assert!(!packet.is_empty());
        // A whole silent frame is not sent.
        p.push(std::iter::repeat_n(0.0, frame)).unwrap();
        assert!(rx.try_recv().is_err());
    }
}
