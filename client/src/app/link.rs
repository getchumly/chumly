//! Connection quality while connected: smooths the once-a-second samples
//! from the network (`NetEvent::Link`) and decides the quality level.
//! Thresholds and timing are from the design (top bar, sections 19–23).

use std::collections::VecDeque;

use crate::net::LinkStats;
use crate::theme::Quality;

/// Samples averaged for display: 5 seconds.
const WINDOW: usize = 5;
/// The level changes only after this many samples in a row agree.
const SAMPLES_TO_CHANGE: u8 = 2;

/// Level of a ping alone (ms): the server list has no loss figures.
pub(super) fn ping_quality(rtt_ms: f32) -> Quality {
    if rtt_ms < 80.0 {
        Quality::Good
    } else if rtt_ms <= 200.0 {
        Quality::Fair
    } else {
        Quality::Poor
    }
}

/// Level of a loss share (0..=1).
pub(super) fn loss_quality(loss: f32) -> Quality {
    if loss < 0.01 {
        Quality::Good
    } else if loss <= 0.05 {
        Quality::Fair
    } else {
        Quality::Poor
    }
}

/// The worse of ping and packet loss.
pub(super) fn quality(s: &LinkStats) -> Quality {
    ping_quality(s.rtt_ms).max(loss_quality(s.loss))
}

#[derive(Default)]
pub(super) struct LinkMonitor {
    samples: VecDeque<LinkStats>,
    level: Option<Quality>,
    /// A different level seen in the last samples, and how many times in a row.
    pending: Option<(Quality, u8)>,
}

impl LinkMonitor {
    pub(super) fn push(&mut self, sample: LinkStats) {
        self.samples.push_back(sample);
        while self.samples.len() > WINDOW {
            self.samples.pop_front();
        }
        let now = quality(&self.average().expect("just pushed"));
        match self.level {
            None => self.level = Some(now),
            Some(level) if level == now => self.pending = None,
            Some(_) => {
                let count = match self.pending {
                    Some((q, n)) if q == now => n + 1,
                    _ => 1,
                };
                if count >= SAMPLES_TO_CHANGE {
                    self.level = Some(now);
                    self.pending = None;
                } else {
                    self.pending = Some((now, count));
                }
            }
        }
    }

    /// Average over the window; `None` before the first sample.
    pub(super) fn average(&self) -> Option<LinkStats> {
        let n = self.samples.len() as f32;
        if n == 0.0 {
            return None;
        }
        let mut a = LinkStats::default();
        for s in &self.samples {
            a.rtt_ms += s.rtt_ms / n;
            a.loss += s.loss / n;
            a.voice_lost += s.voice_lost / n;
            a.voice_late += s.voice_late / n;
            a.rx_kbps += s.rx_kbps / n;
            a.tx_kbps += s.tx_kbps / n;
        }
        Some(a)
    }

    /// The shown level: changes only when the new one holds.
    pub(super) fn level(&self) -> Option<Quality> {
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(rtt_ms: f32, loss: f32) -> LinkStats {
        LinkStats { rtt_ms, loss, ..Default::default() }
    }

    #[test]
    fn thresholds() {
        assert_eq!(quality(&s(23.0, 0.0)), Quality::Good);
        assert_eq!(quality(&s(142.0, 0.0)), Quality::Fair);
        assert_eq!(quality(&s(23.0, 0.03)), Quality::Fair, "loss alone makes it fair");
        assert_eq!(quality(&s(348.0, 0.0)), Quality::Poor);
        assert_eq!(quality(&s(23.0, 0.07)), Quality::Poor);
    }

    #[test]
    fn one_spike_does_not_flip_the_level() {
        let mut m = LinkMonitor::default();
        for _ in 0..5 {
            m.push(s(20.0, 0.0));
        }
        assert_eq!(m.level(), Some(Quality::Good));
        // One bad second: the 5-second average stays good.
        m.push(s(400.0, 0.0));
        assert_eq!(m.level(), Some(Quality::Good));
        // It stays bad: the average crosses, and after two samples the level follows.
        for _ in 0..3 {
            m.push(s(400.0, 0.0));
        }
        assert_eq!(m.level(), Some(Quality::Poor));
        assert!(m.average().unwrap().rtt_ms > 200.0);
    }
}
