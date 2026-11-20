//! `AudioState`: the audio engine (started only while needed), the mixer
//! and the event sounds.

use super::*;

/// How long the audio output stays on after a sound beyond its length:
/// the device needs time to start and buffers some audio.
const SOUND_MARGIN: Duration = Duration::from_millis(600);

pub(super) struct AudioState {
    /// State shared with the audio threads: levels, transmitting, settings.
    pub(super) shared: Arc<AudioShared>,
    pub(super) mixer: Arc<Mutex<Mixer>>,
    /// Encoded voice from the capture thread to the network.
    voice_tx: mpsc::Sender<VoiceOut>,
    /// The running microphone and output, `None` while audio is not needed.
    pub(super) engine: Option<AudioEngine>,
    /// Restart the engine on the next frame (a device or setting changed).
    pub(super) restart: bool,
    /// Device errors from the last start, shown in the settings.
    pub(super) errors: Vec<String>,
    sounds: Sounds,
    /// The output stays on until then so a sound can finish, e.g. the
    /// disconnect sound after leaving the server.
    sound_until: Option<Instant>,
}

impl AudioState {
    pub(super) fn new(shared: Arc<AudioShared>, mixer: Arc<Mutex<Mixer>>, voice_tx: mpsc::Sender<VoiceOut>) -> Self {
        Self {
            shared,
            mixer,
            voice_tx,
            engine: None,
            restart: false,
            errors: Vec::new(),
            sounds: Sounds::new(),
            sound_until: None,
        }
    }

    /// Plays `sound` at `volume` (0..=1) and keeps the output on until it ends.
    pub(super) fn play(&mut self, sound: Sound, volume: f32) {
        let s = &self.sounds;
        let samples = match sound {
            Sound::Join => &s.join,
            Sound::Leave => &s.leave,
            Sound::Message => &s.message,
            Sound::Mute => &s.mute,
            Sound::Unmute => &s.unmute,
            Sound::Deafen => &s.deafen,
            Sound::Undeafen => &s.undeafen,
            Sound::Connect => &s.connect,
            Sound::Disconnect => &s.disconnect,
            Sound::StreamStart => &s.stream_start,
            Sound::StreamStop => &s.stream_stop,
            Sound::ViewerJoin => &s.viewer_join,
            Sound::ViewerLeave => &s.viewer_leave,
        };
        let scaled: Vec<f32> = samples.iter().map(|s| s * volume).collect();
        // Its length plus a margin for the output's buffering and startup.
        let tail =
            Duration::from_secs_f32(scaled.len() as f32 / shared::media::VOICE_SAMPLE_RATE as f32) + SOUND_MARGIN;
        let until = Instant::now() + tail;
        self.sound_until = Some(self.sound_until.map_or(until, |u| u.max(until)));
        lock_mixer(&self.mixer).play_effect(&scaled);
    }

    /// Starts or stops the engine: it runs while `needed` or a sound is
    /// still playing, and restarts when asked to.
    pub(super) fn manage(&mut self, needed: bool, cfg: &crate::config::AudioConfig, ctx: &egui::Context) {
        let now = Instant::now();
        let sounding = self.sound_until.filter(|u| now < *u);
        if let Some(u) = sounding {
            ctx.request_repaint_after(u.saturating_duration_since(now));
        }
        let want = needed || sounding.is_some();
        if self.restart {
            self.engine = None;
            self.restart = false;
        }
        if want && self.engine.is_none() {
            let engine = AudioEngine::start(cfg, self.shared.clone(), self.mixer.clone(), self.voice_tx.clone());
            self.errors = engine.errors.clone();
            self.engine = Some(engine);
        } else if !want && self.engine.is_some() {
            self.engine = None;
            lock_mixer(&self.mixer).clear_effects();
        }
    }
}
