//! Microphone capture and voice preparation for sending.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use anyhow::{Context, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use nnnoiseless::DenoiseState;
use shared::media::{VOICE_FRAME_SAMPLES, VOICE_SAMPLE_RATE};
use tokio::sync::mpsc;

use super::dsp::{Resampler, level_db};
use super::{AudioShared, find_device};
use crate::net::VoiceOut;

/// After the voice drops below the threshold, transmission continues for
/// another 300 ms so word endings are not cut off.
const VAD_HANGOVER_FRAMES: u32 = 15;
const OPUS_BITRATE: i32 = 32_000;

pub fn start(
    name: Option<&str>,
    shared: Arc<AudioShared>,
    voice_tx: mpsc::Sender<VoiceOut>,
) -> anyhow::Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = find_device(host.input_devices()?, host.default_input_device(), name)
        .ok_or_else(|| anyhow!("no microphone"))?;
    let supported = device.default_input_config().context("microphone config")?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let rate = config.sample_rate.0;
    tracing::info!("microphone: {} ({rate} Hz, {} ch, {format})", device.name().unwrap_or_default(), config.channels);

    let (tx, rx) = sync_channel::<Vec<f32>>(64);
    let stream = match format {
        cpal::SampleFormat::F32 => build_input::<f32>(&device, &config, tx),
        cpal::SampleFormat::I16 => build_input::<i16>(&device, &config, tx),
        cpal::SampleFormat::U16 => build_input::<u16>(&device, &config, tx),
        cpal::SampleFormat::I32 => build_input::<i32>(&device, &config, tx),
        other => bail!("unsupported microphone sample format {other}"),
    }?;

    let mut processor = Processor::new(rate, shared, voice_tx)?;
    std::thread::Builder::new().name("audio-capture".into()).spawn(move || processor.run(rx))?;
    stream.play().context("starting microphone")?;
    Ok(stream)
}

fn build_input<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: SyncSender<Vec<f32>>,
) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            // Downmix to mono. If the processing thread falls behind, the block is dropped,
            // but the sound card callback never blocks.
            let mono: Vec<f32> = data
                .chunks(channels)
                .map(|f| f.iter().map(|&s| f32::from_sample_(s)).sum::<f32>() / channels as f32)
                .collect();
            let _ = tx.try_send(mono);
        },
        |e| tracing::warn!("microphone error: {e}"),
        None,
    )?;
    Ok(stream)
}

struct Processor {
    shared: Arc<AudioShared>,
    voice_tx: mpsc::Sender<VoiceOut>,
    resampler: Resampler,
    denoise: Box<DenoiseState<'static>>,
    encoder: opus::Encoder,
    pending: Vec<f32>,
    frame: Vec<f32>,
    seq: u32,
    hangover: u32,
}

impl Processor {
    fn new(rate: u32, shared: Arc<AudioShared>, voice_tx: mpsc::Sender<VoiceOut>) -> anyhow::Result<Self> {
        let mut encoder = opus::Encoder::new(VOICE_SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)?;
        encoder.set_bitrate(opus::Bitrate::Bits(OPUS_BITRATE))?;
        // In-band redundancy helps survive the loss of an adjacent packet.
        encoder.set_inband_fec(true)?;
        encoder.set_packet_loss_perc(10)?;
        Ok(Self {
            shared,
            voice_tx,
            resampler: Resampler::new(rate, VOICE_SAMPLE_RATE),
            denoise: DenoiseState::new(),
            encoder,
            pending: Vec::new(),
            frame: Vec::with_capacity(VOICE_FRAME_SAMPLES),
            seq: 0,
            hangover: 0,
        })
    }

    /// Runs while the cpal stream is alive: dropping it closes the channel.
    fn run(&mut self, rx: Receiver<Vec<f32>>) {
        let mut resampled = Vec::new();
        while let Ok(chunk) = rx.recv() {
            resampled.clear();
            self.resampler.process(&chunk, &mut resampled);
            self.pending.extend_from_slice(&resampled);
            // nnnoiseless works on 10 ms frames (480 samples).
            let n = DenoiseState::FRAME_SIZE;
            while self.pending.len() >= n {
                let mut block = [0.0f32; DenoiseState::FRAME_SIZE];
                block.copy_from_slice(&self.pending[..n]);
                self.pending.drain(..n);
                self.process_block(&mut block);
            }
        }
        self.shared.transmitting.store(false, Ordering::Relaxed);
        self.shared.mic_level_db.store(-100.0);
    }

    fn process_block(&mut self, block: &mut [f32; DenoiseState::FRAME_SIZE]) {
        let sh = &self.shared;
        // Push-to-talk is not pressed and the level meter is closed: the audio
        // goes nowhere and is shown nowhere, so skip the denoiser and codec.
        if sh.ptt_mode.load(Ordering::Relaxed)
            && !sh.ptt_pressed.load(Ordering::Relaxed)
            && !sh.metering.load(Ordering::Relaxed)
        {
            self.frame.clear();
            sh.transmitting.store(false, Ordering::Relaxed);
            sh.mic_level_db.store(-100.0);
            return;
        }
        let gain = self.shared.input_gain.load();
        for s in block.iter_mut() {
            *s *= gain;
        }
        if self.shared.noise_suppression.load(Ordering::Relaxed) {
            // nnnoiseless expects samples in the i16 range.
            let input: [f32; DenoiseState::FRAME_SIZE] = std::array::from_fn(|i| block[i] * 32767.0);
            let mut out = [0.0f32; DenoiseState::FRAME_SIZE];
            self.denoise.process_frame(&mut out, &input);
            for (s, o) in block.iter_mut().zip(out) {
                *s = o / 32767.0;
            }
        }
        self.frame.extend_from_slice(block);
        if self.frame.len() >= VOICE_FRAME_SAMPLES {
            self.process_frame();
            self.frame.clear();
        }
    }

    fn process_frame(&mut self) {
        let sh = &self.shared;
        let db = level_db(&self.frame);
        sh.mic_level_db.store(db);

        let active = if sh.ptt_mode.load(Ordering::Relaxed) {
            sh.ptt_pressed.load(Ordering::Relaxed)
        } else if db >= sh.vad_threshold_db.load() {
            self.hangover = VAD_HANGOVER_FRAMES;
            true
        } else if self.hangover > 0 {
            self.hangover -= 1;
            true
        } else {
            false
        };
        let transmit = active
            && sh.connected.load(Ordering::Relaxed)
            && !sh.muted.load(Ordering::Relaxed)
            && !sh.deafened.load(Ordering::Relaxed);
        sh.transmitting.store(transmit, Ordering::Relaxed);
        if !transmit {
            return;
        }
        for s in self.frame.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        let mut buf = [0u8; 1500];
        match self.encoder.encode_float(&self.frame, &mut buf) {
            Ok(len) => {
                let _ = self.voice_tx.try_send(VoiceOut { seq: self.seq, data: buf[..len].to_vec() });
                self.seq = self.seq.wrapping_add(1);
            }
            Err(e) => tracing::warn!("Opus encoding error: {e}"),
        }
    }
}
