/// Streaming resampler with linear interpolation. Good enough for
/// voice, and state is kept between calls so block boundaries
/// do not click.
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    pos: f64,
    last: f32,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        Self { step: in_rate as f64 / out_rate as f64, pos: 0.0, last: 0.0 }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        // Index 0 is the last sample of the previous block,
        // index k is input[k - 1].
        let len = input.len() as f64;
        while self.pos < len {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = input[i];
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= len;
        self.last = input[input.len() - 1];
    }
}

/// Block loudness in dBFS.
pub fn level_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -100.0;
    }
    let mean_sq = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    20.0 * mean_sq.sqrt().max(1e-5).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_keeps_length() {
        let mut r = Resampler::new(48_000, 48_000);
        let mut out = Vec::new();
        for _ in 0..10 {
            r.process(&[0.5; 480], &mut out);
        }
        assert_eq!(out.len(), 4800);
    }

    #[test]
    fn upsample_ratio() {
        let mut r = Resampler::new(44_100, 48_000);
        let mut out = Vec::new();
        for _ in 0..100 {
            r.process(&[0.1; 441], &mut out);
        }
        // 44100 input samples yield 48000 output samples, give or take one.
        assert!((out.len() as i64 - 48_000).abs() <= 1, "{}", out.len());
    }

    #[test]
    fn interpolates_between_blocks() {
        let mut r = Resampler::new(1, 2);
        let mut out = Vec::new();
        r.process(&[1.0], &mut out);
        r.process(&[3.0], &mut out);
        assert_eq!(out, vec![0.0, 0.5, 1.0, 2.0]);
    }

    #[test]
    fn level() {
        assert!(level_db(&[0.0; 10]) < -90.0);
        assert!((level_db(&[1.0; 10])).abs() < 0.01);
    }
}
