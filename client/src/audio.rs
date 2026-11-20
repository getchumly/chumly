//! Audio engine: microphone capture and speaker output via cpal.
//!
//! Data flow:
//! microphone -> [cpal callback] -> channel -> [processing thread: resampling,
//! denoising, voice activation, Opus] -> network
//!
//! network -> Mixer (decoding, jitter buffer) -> [cpal output callback]

pub mod capture;
pub mod dsp;
pub mod mixer;
pub mod sounds;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use tokio::sync::mpsc;

use crate::config::{AudioConfig, VoiceMode};
use crate::net::VoiceOut;
use dsp::Resampler;
use mixer::Mixer;
use shared::media::VOICE_SAMPLE_RATE;

pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

/// State read by the audio threads and written by the UI.
pub struct AudioShared {
    pub connected: AtomicBool,
    pub muted: AtomicBool,
    pub deafened: AtomicBool,
    pub ptt_mode: AtomicBool,
    pub ptt_pressed: AtomicBool,
    pub noise_suppression: AtomicBool,
    pub vad_threshold_db: AtomicF32,
    pub input_gain: AtomicF32,
    /// Last measured microphone level, for the meter.
    pub mic_level_db: AtomicF32,
    /// Whether voice is being transmitted right now.
    pub transmitting: AtomicBool,
    /// The level meter in settings is open: the microphone must be measured
    /// even if the voice is not sent anywhere.
    pub metering: AtomicBool,
}

impl AudioShared {
    pub fn new(cfg: &AudioConfig) -> Self {
        let s = Self {
            connected: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            ptt_mode: AtomicBool::new(false),
            ptt_pressed: AtomicBool::new(false),
            noise_suppression: AtomicBool::new(true),
            vad_threshold_db: AtomicF32::new(-45.0),
            input_gain: AtomicF32::new(1.0),
            mic_level_db: AtomicF32::new(-100.0),
            transmitting: AtomicBool::new(false),
            metering: AtomicBool::new(false),
        };
        s.apply(cfg);
        s
    }

    pub fn apply(&self, cfg: &AudioConfig) {
        self.ptt_mode.store(cfg.mode == VoiceMode::PushToTalk, Ordering::Relaxed);
        self.noise_suppression.store(cfg.noise_suppression, Ordering::Relaxed);
        self.vad_threshold_db.store(cfg.vad_threshold_db);
        self.input_gain.store(cfg.input_gain);
    }
}

/// Open input and output streams. Audio stops when dropped.
pub struct AudioEngine {
    _input: Option<cpal::Stream>,
    _output: Option<cpal::Stream>,
    pub errors: Vec<String>,
}

impl AudioEngine {
    pub fn start(
        cfg: &AudioConfig,
        shared: Arc<AudioShared>,
        mixer: Arc<Mutex<Mixer>>,
        voice_tx: mpsc::Sender<VoiceOut>,
    ) -> Self {
        let mut errors = Vec::new();
        let input = capture::start(cfg.input_device.as_deref(), shared, voice_tx)
            .map_err(|e| errors.push(format!("{e:#}")))
            .ok();
        let output = start_output(cfg.output_device.as_deref(), mixer).map_err(|e| errors.push(format!("{e:#}"))).ok();
        for e in &errors {
            tracing::error!("audio: {e}");
        }
        Self { _input: input, _output: output, errors }
    }
}

pub fn lock_mixer(m: &Mutex<Mixer>) -> std::sync::MutexGuard<'_, Mixer> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn input_device_names() -> Vec<String> {
    cpal::default_host().input_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
}

pub fn output_device_names() -> Vec<String> {
    cpal::default_host().output_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
}

fn find_device(
    devices: impl Iterator<Item = cpal::Device>,
    default: Option<cpal::Device>,
    name: Option<&str>,
) -> Option<cpal::Device> {
    if let Some(name) = name {
        let mut devices = devices;
        if let Some(d) = devices.find(|d| d.name().is_ok_and(|n| n == name)) {
            return Some(d);
        }
        tracing::warn!("device '{name}' not found, using the default device");
    }
    default
}

fn start_output(name: Option<&str>, mixer: Arc<Mutex<Mixer>>) -> anyhow::Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = find_device(host.output_devices()?, host.default_output_device(), name)
        .ok_or_else(|| anyhow!("no output device"))?;
    let supported = device.default_output_config().context("output device config")?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    tracing::info!(
        "output: {} ({} Hz, {} ch, {format})",
        device.name().unwrap_or_default(),
        config.sample_rate.0,
        config.channels
    );
    let stream = match format {
        cpal::SampleFormat::F32 => build_output::<f32>(&device, &config, mixer),
        cpal::SampleFormat::I16 => build_output::<i16>(&device, &config, mixer),
        cpal::SampleFormat::U16 => build_output::<u16>(&device, &config, mixer),
        cpal::SampleFormat::I32 => build_output::<i32>(&device, &config, mixer),
        other => bail!("unsupported output sample format {other}"),
    }?;
    stream.play().context("starting output")?;
    Ok(stream)
}

fn build_output<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mixer: Arc<Mutex<Mixer>>,
) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut resample = [
        Resampler::new(VOICE_SAMPLE_RATE, config.sample_rate.0),
        Resampler::new(VOICE_SAMPLE_RATE, config.sample_rate.0),
    ];
    // Sample pairs: left and right.
    let mut fifo: VecDeque<(f32, f32)> = VecDeque::new();
    // 10 ms blocks keep the intermediate buffer latency minimal.
    let block_frames = VOICE_SAMPLE_RATE as usize / 100;
    let mut block = vec![0.0f32; block_frames * 2];
    let mut side = [vec![0.0f32; block_frames], vec![0.0f32; block_frames]];
    let mut resampled = [Vec::new(), Vec::new()];
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / channels;
            while fifo.len() < frames {
                lock_mixer(&mixer).pull(&mut block);
                for (i, f) in block.as_chunks::<2>().0.iter().enumerate() {
                    side[0][i] = f[0];
                    side[1][i] = f[1];
                }
                for ch in 0..2 {
                    resampled[ch].clear();
                    resample[ch].process(&side[ch], &mut resampled[ch]);
                }
                fifo.extend(resampled[0].iter().copied().zip(resampled[1].iter().copied()));
            }
            for frame in data.chunks_mut(channels) {
                let (l, r) = fifo.pop_front().unwrap_or((0.0, 0.0));
                match frame {
                    [mono] => *mono = T::from_sample_((l + r) * 0.5),
                    // 5.1 and larger layouts: audio goes to front left and right only.
                    [left, right, rest @ ..] => {
                        *left = T::from_sample_(l);
                        *right = T::from_sample_(r);
                        for s in rest {
                            *s = T::from_sample_(0.0f32);
                        }
                    }
                    [] => {}
                }
            }
        },
        |e| tracing::warn!("audio output error: {e}"),
        None,
    )?;
    Ok(stream)
}
