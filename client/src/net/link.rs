//! Connection quality: ping, packet loss, voice losses and traffic, measured
//! once a second while connected.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::lock_mixer;
use crate::audio::mixer::{Mixer, VoiceStats};

/// Connection quality over the last second.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LinkStats {
    pub rtt_ms: f32,
    /// Lost QUIC packets / sent, 0..=1.
    pub loss: f32,
    /// Incoming voice: never arrived / expected, and dropped as late / received.
    pub voice_lost: f32,
    pub voice_late: f32,
    pub rx_kbps: f32,
    pub tx_kbps: f32,
}

/// Cumulative counters; `LinkStats` is the difference of two readings.
pub(super) struct LinkCounters {
    at: Instant,
    sent: u64,
    lost: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    voice: VoiceStats,
}

impl LinkCounters {
    pub(super) fn read(conn: &quinn::Connection, mixer: &Arc<Mutex<Mixer>>) -> Self {
        let s = conn.stats();
        Self {
            at: Instant::now(),
            sent: s.path.sent_packets,
            lost: s.path.lost_packets,
            rx_bytes: s.udp_rx.bytes,
            tx_bytes: s.udp_tx.bytes,
            voice: lock_mixer(mixer).voice_stats,
        }
    }
}

impl LinkStats {
    pub(super) fn between(a: &LinkCounters, b: &LinkCounters, rtt: Duration) -> Self {
        let ratio = |part: u64, whole: u64| if whole == 0 { 0.0 } else { part as f32 / whole as f32 };
        let secs = b.at.duration_since(a.at).as_secs_f32().max(0.001);
        let kbps = |bytes: u64| bytes as f32 * 8.0 / 1000.0 / secs;
        let v_lost = b.voice.lost.saturating_sub(a.voice.lost);
        let v_recv = b.voice.received.saturating_sub(a.voice.received);
        let v_late = b.voice.late.saturating_sub(a.voice.late);
        Self {
            rtt_ms: rtt.as_secs_f32() * 1000.0,
            loss: ratio(b.lost.saturating_sub(a.lost), b.sent.saturating_sub(a.sent)),
            voice_lost: ratio(v_lost, v_lost + v_recv.saturating_sub(v_late)),
            voice_late: ratio(v_late, v_recv),
            rx_kbps: kbps(b.rx_bytes.saturating_sub(a.rx_bytes)),
            tx_kbps: kbps(b.tx_bytes.saturating_sub(a.tx_bytes)),
        }
    }
}
