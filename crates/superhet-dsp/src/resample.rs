//! Arbitrary-ratio streaming resampler for real audio.
//!
//! Kaiser-windowed sinc interpolator with a precomputed polyphase table
//! (`PHASES` fractional positions, linearly interpolated between neighbouring phases). The
//! read position is an `f64` accumulator, so the long-term output rate is exact.

use crate::fir::{bessel_i0, kaiser, kaiser_beta};

/// Interpolator length in input samples.
const TAPS: usize = 24;
/// Table resolution (fractional positions per input sample).
const PHASES: usize = 256;

/// Streaming rational-or-irrational ratio resampler (`f32`, mono).
#[derive(Debug, Clone)]
pub struct Resampler {
    /// Input samples advanced per output sample.
    step: f64,
    /// `(PHASES + 1) * TAPS` coefficients, row-major by phase.
    table: Vec<f32>,
    pending: Vec<f32>,
    /// Read position (in `pending` indices) of the next output.
    pos: f64,
}

impl Resampler {
    /// Creates a resampler from `rate_in` to `rate_out` (both in Hz, positive).
    ///
    /// The cutoff is 0.45 of the lower of the two rates, so the input should be band-limited
    /// accordingly.
    pub fn new(rate_in: f64, rate_out: f64) -> Self {
        let fc = 0.45 * (rate_out / rate_in).min(1.0);
        let beta = kaiser_beta(70.0);
        let norm = bessel_i0(beta);
        let half = TAPS / 2;
        let mut table = vec![0.0f32; (PHASES + 1) * TAPS];
        for (j, row) in table.as_chunks_mut::<TAPS>().0.iter_mut().enumerate() {
            let frac = j as f64 / PHASES as f64;
            let mut h: Vec<f64> = (0..TAPS)
                .map(|k| {
                    let d = k as f64 - (half - 1) as f64 - frac;
                    let x = std::f64::consts::TAU * fc * d;
                    let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                    2.0 * fc * sinc * kaiser(d / half as f64, beta, norm)
                })
                .collect();
            let sum: f64 = h.iter().sum();
            for v in &mut h {
                *v /= sum;
            }
            for (dst, v) in row.iter_mut().zip(h) {
                *dst = v as f32;
            }
        }
        Self {
            step: rate_in / rate_out,
            table,
            pending: vec![0.0; half - 1],
            pos: (half - 1) as f64,
        }
    }

    /// Clears the delay line and the fractional position.
    pub fn reset(&mut self) {
        let half = TAPS / 2;
        self.pending.clear();
        self.pending.resize(half - 1, 0.0);
        self.pos = (half - 1) as f64;
    }

    /// Resamples `input`, appending to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.pending.extend_from_slice(input);
        let half = TAPS / 2;
        loop {
            let i = self.pos as usize;
            if i + half >= self.pending.len() {
                break;
            }
            let frac = self.pos - i as f64;
            let ph = frac * PHASES as f64;
            let j = (ph as usize).min(PHASES - 1);
            let a = (ph - j as f64) as f32;
            let r0 = &self.table[j * TAPS..(j + 1) * TAPS];
            let r1 = &self.table[(j + 1) * TAPS..(j + 2) * TAPS];
            let x = &self.pending[i + 1 - half..i + 1 + half];
            let (mut s0, mut s1) = (0.0f32, 0.0f32);
            for ((&xv, &c0), &c1) in x.iter().zip(r0).zip(r1) {
                s0 += xv * c0;
                s1 += xv * c1;
            }
            out.push(s0 + a * (s1 - s0));
            self.pos += self.step;
        }
        let drop = ((self.pos as usize) + 1 - half).min(self.pending.len());
        self.pending.drain(..drop);
        self.pos -= drop as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f64, rate: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (std::f64::consts::TAU * f * i as f64 / rate).sin() as f32)
            .collect()
    }

    #[test]
    fn count_is_exact_over_long_streams() {
        for (rin, rout) in [
            (51_200.0, 48_000.0),
            (40_000.0, 48_000.0),
            (56_250.0, 48_000.0),
        ] {
            let mut r = Resampler::new(rin, rout);
            let mut out = Vec::new();
            let n_in = (rin * 20.0) as usize;
            let x = vec![0.0f32; n_in];
            for c in x.chunks(1013) {
                r.process(c, &mut out);
            }
            let expect = 20.0 * rout;
            assert!(
                (out.len() as f64 - expect).abs() <= 16.0,
                "{rin}: {}",
                out.len()
            );
        }
    }

    #[test]
    fn preserves_tone_frequency_and_amplitude() {
        let (rin, rout) = (40_000.0, 48_000.0);
        let mut r = Resampler::new(rin, rout);
        let mut out = Vec::new();
        r.process(&tone(3000.0, rin, 40_000), &mut out);
        // Fit sin/cos at 3 kHz over an integer number of cycles, skipping the start.
        let seg = &out[1000..1000 + 480];
        let (mut a, mut b) = (0.0f64, 0.0f64);
        for (i, &v) in seg.iter().enumerate() {
            let ph = std::f64::consts::TAU * 3000.0 * (i as f64) / rout;
            a += f64::from(v) * ph.cos();
            b += f64::from(v) * ph.sin();
        }
        let amp = 2.0 * a.hypot(b) / seg.len() as f64;
        assert!((amp - 1.0).abs() < 0.01, "amp {amp}");
        let p: f64 = seg.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / seg.len() as f64;
        assert!((p - 0.5).abs() < 0.01, "power {p}");
    }
}
