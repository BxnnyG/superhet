//! Numerically controlled oscillator used to shift a channel to baseband.

use num_complex::Complex32;

/// Samples per block over which the phasor is rotated recursively; the phasor is
/// re-derived from the exact `f64` phase at every block boundary, so no error accumulates.
const BLOCK: usize = 128;

/// Frequency shifter: multiplies the input by `exp(-j * 2 * pi * offset * n / rate)`, so a tone
/// at `+offset` lands at DC. Changing the offset only changes the phase increment.
#[derive(Debug, Clone)]
pub struct Nco {
    /// Current phase in radians, kept in `[-pi, pi)`.
    phase: f64,
    /// Phase increment per sample in radians.
    inc: f64,
    step: Complex32,
}

impl Nco {
    /// Creates a shifter for `offset_hz` at `rate_hz`.
    pub fn new(offset_hz: f64, rate_hz: f64) -> Self {
        let mut n = Self {
            phase: 0.0,
            inc: 0.0,
            step: Complex32::new(1.0, 0.0),
        };
        n.set_offset(offset_hz, rate_hz);
        n
    }

    /// Changes the shift frequency without disturbing the phase (continuous retune).
    pub fn set_offset(&mut self, offset_hz: f64, rate_hz: f64) {
        self.inc = -std::f64::consts::TAU * offset_hz / rate_hz;
        let (s, c) = self.inc.sin_cos();
        self.step = Complex32::new(c as f32, s as f32);
    }

    /// Resets the phase to zero.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Shifts `input` into `out` (cleared first).
    pub fn mix(&mut self, input: &[Complex32], out: &mut Vec<Complex32>) {
        out.clear();
        out.reserve(input.len());
        for block in input.chunks(BLOCK) {
            let (s, c) = self.phase.sin_cos();
            let mut p = Complex32::new(c as f32, s as f32);
            out.extend(block.iter().map(|&x| {
                let y = x * p;
                p *= self.step;
                y
            }));
            self.phase += self.inc * block.len() as f64;
            self.phase = (self.phase + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifts_tone_to_dc_across_blocks() {
        let rate = 2_400_000.0;
        let f = 123_456.7;
        let x: Vec<Complex32> = (0..100_000)
            .map(|i| {
                let ph = std::f64::consts::TAU * f * f64::from(i) / rate;
                Complex32::new(ph.cos() as f32, ph.sin() as f32)
            })
            .collect();
        let mut nco = Nco::new(f, rate);
        let mut out = Vec::new();
        let mut all = Vec::new();
        for c in x.chunks(999) {
            nco.mix(c, &mut out);
            all.extend_from_slice(&out);
        }
        for y in &all {
            assert!((y.re - 1.0).abs() < 1e-3 && y.im.abs() < 1e-3);
        }
    }
}
