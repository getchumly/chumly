//! Receiving voice and screen share audio: decoding, jitter buffer and
//! mixing everything into a single 48 kHz stereo stream. Voices are mono
//! and play equally in both channels; screen share audio is stereo.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use shared::UserId;
use shared::media::{VOICE_FRAME_SAMPLES, VOICE_SAMPLE_RATE};

/// Buffer 40 ms before playback starts to smooth out uneven packet arrival.
const PREBUFFER: usize = VOICE_FRAME_SAMPLES * 2;
/// Beyond 300 ms buffered the delay becomes noticeable: drop old audio.
const MAX_BUFFER: usize = VOICE_SAMPLE_RATE as usize * 3 / 10;
const TRIM_TO: usize = VOICE_FRAME_SAMPLES * 3;
/// Larger sequence gaps are not counted as lost (the sender restarted).
const MAX_COUNTED_GAP: u32 = 500;
/// Do not conceal more than this many consecutive lost packets.
const MAX_CONCEAL: u32 = 5;
const SPEAKING_HOLD: Duration = Duration::from_millis(300);
/// Maximum Opus frame size: 120 ms.
const MAX_OPUS_FRAME: usize = 5760;
/// Screen share audio: buffer 60 ms before starting, keep at most 400 ms.
/// Stereo samples come in pairs, hence everything is multiplied by 2.
const STREAM_PREBUFFER: usize = VOICE_FRAME_SAMPLES * 3 * 2;
const STREAM_MAX_BUFFER: usize = VOICE_SAMPLE_RATE as usize * 4 / 10 * 2;
const STREAM_TRIM_TO: usize = VOICE_FRAME_SAMPLES * 5 * 2;

/// Audio of the screen share being watched. There is only ever one.
struct StreamPeer {
    user: UserId,
    decoder: opus::Decoder,
    /// Interleaved samples: left, right, left...
    buf: VecDeque<f32>,
    last_seq: Option<u32>,
    playing: bool,
}

struct Peer {
    decoder: opus::Decoder,
    buf: VecDeque<f32>,
    last_seq: Option<u32>,
    playing: bool,
    last_packet: Instant,
}

/// Incoming voice packets since the start: for the connection quality.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceStats {
    pub received: u64,
    /// Never arrived (gaps in sequence numbers).
    pub lost: u64,
    /// Arrived after a newer one; dropped.
    pub late: u64,
}

pub struct Mixer {
    pub voice_stats: VoiceStats,
    peers: HashMap<UserId, Peer>,
    volumes: HashMap<UserId, f32>,
    effects: VecDeque<f32>,
    stream: Option<StreamPeer>,
    pub stream_volume: f32,
    pub master_volume: f32,
    pub deafened: bool,
    pcm: Vec<f32>,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            voice_stats: VoiceStats::default(),
            peers: HashMap::new(),
            volumes: HashMap::new(),
            effects: VecDeque::new(),
            stream: None,
            stream_volume: 1.0,
            master_volume: 1.0,
            deafened: false,
            pcm: vec![0.0; MAX_OPUS_FRAME * 2],
        }
    }
}

impl Mixer {
    pub fn push_voice(&mut self, user: UserId, seq: u32, payload: &[u8]) {
        if self.deafened {
            return;
        }
        let peer = match self.peers.entry(user) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let Ok(decoder) = opus::Decoder::new(VOICE_SAMPLE_RATE, opus::Channels::Mono) else { return };
                e.insert(Peer {
                    decoder,
                    buf: VecDeque::new(),
                    last_seq: None,
                    playing: false,
                    last_packet: Instant::now(),
                })
            }
        };
        let pcm = &mut self.pcm;
        self.voice_stats.received += 1;

        if let Some(last) = peer.last_seq {
            let diff = seq.wrapping_sub(last) as i32;
            if diff <= 0 {
                self.voice_stats.late += 1;
                return; // duplicate or late packet
            }
            let lost = diff as u32 - 1;
            // A huge gap is a restarted sender, not lost packets.
            if lost <= MAX_COUNTED_GAP {
                self.voice_stats.lost += lost as u64;
            }
            if lost > 0 && lost <= MAX_CONCEAL && peer.playing {
                for i in 0..lost {
                    // Recover the last lost frame from the current packet's FEC data,
                    // conceal the rest.
                    let frame = &mut pcm[..VOICE_FRAME_SAMPLES];
                    let res = if i == lost - 1 {
                        peer.decoder.decode_float(payload, frame, true)
                    } else {
                        peer.decoder.decode_float(&[], frame, false)
                    };
                    if let Ok(n) = res {
                        peer.buf.extend(&frame[..n]);
                    }
                }
            }
        }
        if let Ok(n) = peer.decoder.decode_float(payload, pcm, false) {
            peer.buf.extend(&pcm[..n]);
        }
        peer.last_seq = Some(seq);
        peer.last_packet = Instant::now();
        if peer.buf.len() > MAX_BUFFER {
            let excess = peer.buf.len() - TRIM_TO;
            peer.buf.drain(..excess);
        }
    }

    /// Screen share audio from user `user`. A new share
    /// replaces the previous one.
    pub fn push_stream(&mut self, user: UserId, seq: u32, payload: &[u8]) {
        if self.deafened {
            return;
        }
        if self.stream.as_ref().is_none_or(|s| s.user != user) {
            let Ok(decoder) = opus::Decoder::new(VOICE_SAMPLE_RATE, opus::Channels::Stereo) else { return };
            self.stream = Some(StreamPeer { user, decoder, buf: VecDeque::new(), last_seq: None, playing: false });
        }
        let Some(st) = self.stream.as_mut() else { return };
        let pcm = &mut self.pcm;
        if let Some(last) = st.last_seq {
            let diff = seq.wrapping_sub(last) as i32;
            if diff <= 0 {
                return;
            }
            let lost = diff as u32 - 1;
            if lost > 0 && lost <= MAX_CONCEAL && st.playing {
                for _ in 0..lost {
                    if let Ok(n) = st.decoder.decode_float(&[], &mut pcm[..VOICE_FRAME_SAMPLES * 2], false) {
                        st.buf.extend(&pcm[..n * 2]);
                    }
                }
            }
        }
        // decode_float returns the number of samples per channel.
        if let Ok(n) = st.decoder.decode_float(payload, pcm, false) {
            st.buf.extend(&pcm[..n * 2]);
        }
        st.last_seq = Some(seq);
        if st.buf.len() > STREAM_MAX_BUFFER {
            let excess = st.buf.len() - STREAM_TRIM_TO;
            st.buf.drain(..excess);
        }
    }

    pub fn clear_stream(&mut self) {
        self.stream = None;
    }

    /// Fills `out` with mixed 48 kHz stereo audio: samples come
    /// in left/right pairs.
    pub fn pull(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        for (id, p) in self.peers.iter_mut() {
            if !p.playing {
                if p.buf.len() < PREBUFFER {
                    continue;
                }
                p.playing = true;
            }
            let vol = if self.deafened { 0.0 } else { self.volumes.get(id).copied().unwrap_or(1.0) };
            for frame in out.as_chunks_mut::<2>().0.iter_mut() {
                match p.buf.pop_front() {
                    Some(x) => {
                        frame[0] += x * vol;
                        frame[1] += x * vol;
                    }
                    None => {
                        p.playing = false;
                        break;
                    }
                }
            }
        }
        if let Some(st) = self.stream.as_mut() {
            if !st.playing && st.buf.len() >= STREAM_PREBUFFER {
                st.playing = true;
            }
            if st.playing {
                let vol = if self.deafened { 0.0 } else { self.stream_volume };
                for s in out.iter_mut() {
                    match st.buf.pop_front() {
                        Some(x) => *s += x * vol,
                        None => {
                            st.playing = false;
                            break;
                        }
                    }
                }
            }
        }
        for frame in out.as_chunks_mut::<2>().0.iter_mut() {
            if let Some(e) = self.effects.pop_front() {
                frame[0] += e;
                frame[1] += e;
            }
            for s in frame {
                *s = (*s * self.master_volume).clamp(-1.0, 1.0);
            }
        }
    }

    pub fn is_speaking(&self, user: UserId) -> bool {
        self.peers.get(&user).is_some_and(|p| p.last_packet.elapsed() < SPEAKING_HOLD)
    }

    /// Who is speaking right now, sorted by id.
    pub fn speaking_users(&self) -> Vec<UserId> {
        let mut v: Vec<UserId> = self.peers.keys().copied().filter(|id| self.is_speaking(*id)).collect();
        v.sort_unstable();
        v
    }

    pub fn set_volume(&mut self, user: UserId, volume: f32) {
        self.volumes.insert(user, volume);
    }

    pub fn remove_peer(&mut self, user: UserId) {
        self.peers.remove(&user);
        self.volumes.remove(&user);
    }

    pub fn clear(&mut self) {
        self.peers.clear();
        self.volumes.clear();
        self.stream = None;
    }

    /// Drops event sounds that have not played yet (the output is stopping:
    /// they must not come out the next time it starts).
    pub fn clear_effects(&mut self) {
        self.effects.clear();
    }

    pub fn play_effect(&mut self, samples: &[f32]) {
        // Do not accumulate sounds forever if output is not working.
        if self.effects.len() < VOICE_SAMPLE_RATE as usize * 2 {
            self.effects.extend(samples);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_frames(n: usize) -> Vec<Vec<u8>> {
        let mut enc = opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut out = Vec::new();
        for f in 0..n {
            let pcm: Vec<f32> =
                (0..VOICE_FRAME_SAMPLES).map(|i| ((f * VOICE_FRAME_SAMPLES + i) as f32 * 0.05).sin() * 0.3).collect();
            let mut buf = [0u8; 1500];
            let len = enc.encode_float(&pcm, &mut buf).unwrap();
            out.push(buf[..len].to_vec());
        }
        out
    }

    #[test]
    fn prebuffers_then_plays() {
        let packets = encode_frames(4);
        let mut m = Mixer::default();
        let mut out = vec![0.0; VOICE_FRAME_SAMPLES * 2];
        m.push_voice(1, 0, &packets[0]);
        m.pull(&mut out);
        assert!(out.iter().all(|&s| s == 0.0), "one frame is below prebuffer");
        m.push_voice(1, 1, &packets[1]);
        m.pull(&mut out);
        assert!(out.iter().any(|&s| s != 0.0));
        assert!(m.is_speaking(1));
        assert!(!m.is_speaking(2));
    }

    #[test]
    fn drops_duplicates_and_conceals_loss() {
        let packets = encode_frames(5);
        let mut m = Mixer::default();
        m.push_voice(1, 0, &packets[0]);
        m.push_voice(1, 1, &packets[1]);
        let mut out = vec![0.0; 10];
        m.pull(&mut out); // starts playback
        let before = m.peers[&1].buf.len();
        m.push_voice(1, 1, &packets[1]);
        assert_eq!(m.peers[&1].buf.len(), before, "duplicate ignored");
        m.push_voice(1, 4, &packets[4]); // 2 and 3 are lost
        assert_eq!(m.peers[&1].buf.len(), before + 3 * VOICE_FRAME_SAMPLES);
        assert_eq!(m.voice_stats.lost, 2, "2 and 3 counted as lost");
        assert_eq!(m.voice_stats.late, 1, "the duplicate counted as late");
    }

    #[test]
    fn deafened_is_silent() {
        let packets = encode_frames(3);
        let mut m = Mixer { deafened: true, ..Default::default() };
        for (i, p) in packets.iter().enumerate() {
            m.push_voice(1, i as u32, p);
        }
        let mut out = vec![0.0; VOICE_FRAME_SAMPLES * 2];
        m.pull(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn voice_is_centered_stream_is_stereo() {
        let packets = encode_frames(3);
        let mut m = Mixer::default();
        for (i, p) in packets.iter().enumerate() {
            m.push_voice(1, i as u32, p);
        }
        let mut out = vec![0.0; VOICE_FRAME_SAMPLES * 2];
        m.pull(&mut out);
        assert!(out.chunks(2).all(|f| f[0] == f[1]), "voice is identical in both channels");
        assert!(out.iter().any(|&s| s != 0.0));

        // Left-only audio must stay on the left.
        let mut enc = opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut m = Mixer::default();
        for f in 0..6u32 {
            let pcm: Vec<f32> = (0..VOICE_FRAME_SAMPLES)
                .flat_map(|i| [((f as usize * VOICE_FRAME_SAMPLES + i) as f32 * 0.05).sin() * 0.5, 0.0])
                .collect();
            let mut buf = [0u8; 4000];
            let len = enc.encode_float(&pcm, &mut buf).unwrap();
            m.push_stream(7, f, &buf[..len]);
        }
        let mut out = vec![0.0; VOICE_FRAME_SAMPLES * 2 * 2];
        m.pull(&mut out);
        let left: f32 = out.chunks(2).map(|f| f[0].abs()).sum();
        let right: f32 = out.chunks(2).map(|f| f[1].abs()).sum();
        assert!(left > 10.0 * right.max(0.001), "left {left}, right {right}");
        m.stream_volume = 0.0;
        m.pull(&mut out);
        assert!(out.iter().all(|&s| s == 0.0), "volume 0 mutes the screen share");
    }
}
