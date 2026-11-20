//! Event sounds, synthesized at startup; no files needed.
//!
//! Soft mallet / glass tones: a sine with overtones that fade faster than
//! it, a short attack and an exponential decay. All notes are from one key,
//! so the sounds go together. Up means on / in, down means off / out; the
//! number of notes and the register tell the events apart:
//! message 1 note, mic and channel 2 (a fifth / a quick fourth), sound and
//! server 3 (low / middle), stream 4 (high), viewers 2 quiet high ones.

use std::f32::consts::TAU;

use shared::media::VOICE_SAMPLE_RATE;

pub struct Sounds {
    pub join: Vec<f32>,
    pub leave: Vec<f32>,
    pub message: Vec<f32>,
    pub mute: Vec<f32>,
    pub unmute: Vec<f32>,
    pub deafen: Vec<f32>,
    pub undeafen: Vec<f32>,
    pub connect: Vec<f32>,
    pub disconnect: Vec<f32>,
    pub stream_start: Vec<f32>,
    pub stream_stop: Vec<f32>,
    pub viewer_join: Vec<f32>,
    pub viewer_leave: Vec<f32>,
}

const D4: f32 = 293.66;
const FS4: f32 = 369.99;
const A4: f32 = 440.0;
const C5: f32 = 523.25;
const E5: f32 = 659.25;
const G5: f32 = 783.99;
const A5: f32 = 880.0;
const B5: f32 = 987.77;
const C6: f32 = 1046.5;
const D6: f32 = 1174.7;
const G6: f32 = 1568.0;

/// Peak of the loudest sounds before the notification volume; the old
/// sine beeps peaked at 0.2 and this is about as loud at the default 70 %.
const LOUD: f32 = 0.25;
/// Viewer sounds are about half as loud: the streamer may be in a game.
const QUIET: f32 = 0.12;

impl Sounds {
    pub fn new() -> Self {
        let t = |notes: &[f32], step: f32, dur: f32| Tone { notes: notes.to_vec(), step, dur, ..Tone::default() };
        Self {
            // Channel: two quick bubbly notes, each sliding a little.
            join: Tone { glide: 1.06, bright: 0.15, ..t(&[E5, A5], 0.045, 0.13) }.render(0.9 * LOUD),
            leave: Tone { glide: 0.94, bright: 0.15, ..t(&[A5, E5], 0.045, 0.13) }.render(0.9 * LOUD),
            message: Tone { bright: 0.25, ..t(&[C6], 0.0, 0.2) }.render(0.8 * LOUD),
            // Mic: two notes a fifth apart.
            mute: Tone { bright: 0.25, ..t(&[G5, C5], 0.07, 0.16) }.render(0.84 * LOUD),
            unmute: Tone { bright: 0.25, ..t(&[C5, G5], 0.07, 0.16) }.render(0.84 * LOUD),
            // Sound: three low notes.
            deafen: Tone { bright: 0.3, ..t(&[A4, FS4, D4], 0.06, 0.2) }.render(0.96 * LOUD),
            undeafen: Tone { bright: 0.3, ..t(&[D4, FS4, A4], 0.06, 0.2) }.render(0.96 * LOUD),
            // Server: a warm three-note arpeggio.
            connect: t(&[C5, E5, G5], 0.075, 0.32).render(LOUD),
            disconnect: t(&[G5, E5, C5], 0.075, 0.32).render(LOUD),
            // Stream: a quick bright run of four.
            stream_start: Tone { bright: 0.3, ..t(&[G5, B5, D6, G6], 0.05, 0.3) }.render(0.9 * LOUD),
            stream_stop: Tone { bright: 0.3, ..t(&[G6, D6, B5, G5], 0.05, 0.3) }.render(0.84 * LOUD),
            // Viewers: two quiet notes a minor third apart.
            viewer_join: Tone { bright: 0.1, ..t(&[A5, C6], 0.045, 0.11) }.render(QUIET),
            viewer_leave: Tone { bright: 0.1, ..t(&[C6, A5], 0.045, 0.11) }.render(QUIET),
        }
    }
}

/// Notes started `step` seconds apart, each ringing for `dur`.
struct Tone {
    notes: Vec<f32>,
    step: f32,
    dur: f32,
    /// End frequency / start frequency of each note (a slide).
    glide: f32,
    /// Loudness of the overtones: higher is more glassy.
    bright: f32,
}

impl Default for Tone {
    fn default() -> Self {
        Self { notes: Vec::new(), step: 0.0, dur: 0.2, glide: 1.0, bright: 0.35 }
    }
}

impl Tone {
    /// The samples, scaled to `peak`, with a short fade at the end.
    fn render(&self, peak: f32) -> Vec<f32> {
        let rate = VOICE_SAMPLE_RATE as f32;
        let mut out: Vec<f32> = Vec::new();
        for (i, &freq) in self.notes.iter().enumerate() {
            let start = (rate * self.step * i as f32) as usize;
            let note = self.note(freq);
            if out.len() < start + note.len() {
                out.resize(start + note.len(), 0.0);
            }
            for (o, s) in out[start..].iter_mut().zip(note) {
                *o += s;
            }
        }
        let max = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        if max > 0.0 {
            out.iter_mut().for_each(|s| *s *= peak / max);
        }
        let fade = ((rate * 0.015) as usize).min(out.len());
        let len = out.len();
        for i in 0..fade {
            out[len - 1 - i] *= i as f32 / fade as f32;
        }
        out
    }

    /// One note: the fundamental and two overtones that die out faster.
    fn note(&self, freq: f32) -> Vec<f32> {
        const ATTACK: f32 = 0.004;
        let rate = VOICE_SAMPLE_RATE as f32;
        let n = (rate * self.dur) as usize;
        // (frequency multiple, loudness, how much faster it fades)
        let partials = [(1.0, 1.0, 1.0), (2.0, self.bright, 2.5), (3.01, self.bright * 0.4, 4.0)];
        let mut phase = [0.0f32; 3];
        (0..n)
            .map(|i| {
                let t = i as f32 / rate;
                let f = freq * self.glide.powf(t / self.dur);
                let env = (t / ATTACK).min(1.0) * (-t * 5.0 / self.dur).exp();
                let mut s = 0.0;
                for (k, &(mult, amp, fast)) in partials.iter().enumerate() {
                    phase[k] = (phase[k] + TAU * f * mult / rate) % TAU;
                    s += amp * phase[k].sin() * (-t * fast * 3.0 / self.dur).exp();
                }
                s * env
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_and_within_range() {
        let s = Sounds::new();
        let all = [
            &s.join,
            &s.leave,
            &s.message,
            &s.mute,
            &s.unmute,
            &s.deafen,
            &s.undeafen,
            &s.connect,
            &s.disconnect,
            &s.stream_start,
            &s.stream_stop,
            &s.viewer_join,
            &s.viewer_leave,
        ];
        for v in all {
            assert!(!v.is_empty() && v.len() < VOICE_SAMPLE_RATE as usize / 2, "under half a second");
            let peak = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.05 && peak <= LOUD + 1e-6, "peak {peak}");
            assert!(v.last().unwrap().abs() < 1e-3, "fades out, no click at the end");
        }
        let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak(&s.viewer_join) < peak(&s.join) * 0.6, "viewer sounds are quiet");
    }
}
