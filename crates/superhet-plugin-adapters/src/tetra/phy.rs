//! TETRA physical layer receive side: pi/4-DQPSK at 18 ksym/s (EN 300 392-2 clause 5) from complex
//! baseband to the unpacked bit stream `tetra-rx` reads (one byte per bit, 0 or 1).
//!
//! ```text
//!  cf32 @ 144 kHz --> root-raised-cosine (alpha 0.35) --> symbol timing (Oerder & Meyr) -->
//!  differential phase --> frequency offset removal (4th power of the phase steps) --> 2 bits/symbol
//! ```
//!
//! Eight samples per symbol. The timing is a feed-forward estimate (the symbol-rate line of
//! `|x|^2`), the carrier offset is taken out of the phase steps, so no carrier loop can lose
//! lock: the capture range is the plus/minus 2.25 kHz at which a phase step becomes ambiguous.
//! Burst and frame synchronisation are not done here; `tetra-rx` finds the training sequences in
//! the bit stream.

use std::f32::consts::{FRAC_PI_4, TAU};

use num_complex::Complex32;

/// Sample rate the plugin asks the host for (`plugins/tetra/plugin.toml`).
pub const SAMPLE_RATE: u32 = 144_000;
/// Symbols per second.
pub const SYMBOL_RATE: u32 = 18_000;
/// Samples per symbol.
pub const SPS: usize = 8;

/// Roll-off of the root-raised-cosine pulse shaping filter (EN 300 392-2 clause 5.5).
const ROLL_OFF: f64 = 0.35;
/// The filter spans this many symbols.
const SPAN_SYMBOLS: usize = 12;
/// Samples per timing estimate; the estimates are smoothed over several of them.
const TIMING_BLOCK: usize = SPS * 64;
/// Weight of the history in the smoothed timing estimate.
const TIMING_KEEP: f32 = 0.9;
/// How far one block's estimate moves the sampling instant (in samples per sample of error).
const TIMING_GAIN: f64 = 0.1;
/// Weight of the history in the smoothed frequency-offset estimate (per symbol).
const FREQ_KEEP: f32 = 0.995;

/// Taps of the root-raised-cosine filter at `SPS` samples per symbol, normalised to unit energy.
#[must_use]
pub fn rrc_taps() -> Vec<f32> {
    let n = SPAN_SYMBOLS * SPS + 1;
    let centre = (n / 2) as f64;
    let a = ROLL_OFF;
    let pi = std::f64::consts::PI;
    let mut taps: Vec<f64> = (0..n)
        .map(|i| {
            // Time in symbols.
            let t = (i as f64 - centre) / SPS as f64;
            if t.abs() < 1e-9 {
                1.0 - a + 4.0 * a / pi
            } else if (t.abs() - 1.0 / (4.0 * a)).abs() < 1e-9 {
                a / 2f64.sqrt()
                    * ((1.0 + 2.0 / pi) * (pi / (4.0 * a)).sin()
                        + (1.0 - 2.0 / pi) * (pi / (4.0 * a)).cos())
            } else {
                let x = pi * t;
                ((x * (1.0 - a)).sin() + 4.0 * a * t * (x * (1.0 + a)).cos())
                    / (x * (1.0 - (4.0 * a * t).powi(2)))
            }
        })
        .collect();
    let energy: f64 = taps.iter().map(|v| v * v).sum::<f64>().sqrt();
    for tap in &mut taps {
        *tap /= energy;
    }
    taps.into_iter().map(|v| v as f32).collect()
}

/// The two bits of a pi/4-DQPSK phase step (EN 300 392-2 table 5.1): 00 = +pi/4, 01 = +3pi/4,
/// 11 = -3pi/4, 10 = -pi/4. `step` is the step in units of pi/4: 1, 3, -1 or -3.
#[must_use]
pub fn step_bits(step: i32) -> [u8; 2] {
    match step {
        1 => [0, 0],
        3 => [0, 1],
        -3 => [1, 1],
        _ => [1, 0],
    }
}

/// The phase step in units of pi/4 for two bits (inverse of `step_bits`).
#[must_use]
pub fn bits_step(bits: [u8; 2]) -> i32 {
    match bits {
        [0, 0] => 1,
        [0, 1] => 3,
        [1, 1] => -3,
        _ => -1,
    }
}

/// Streaming pi/4-DQPSK demodulator.
pub struct Demod {
    taps: Vec<f32>,
    invert: bool,
    /// Input samples not yet filtered (the filter's memory plus the new chunk).
    pending: Vec<Complex32>,
    /// Filter output not yet consumed by the symbol sampler; `base` is the absolute index of its
    /// first element.
    filtered: Vec<Complex32>,
    base: u64,
    /// Absolute index of the next symbol instant, `None` until the timing is known.
    next: Option<f64>,
    /// `sum |x|^2 e^(-j 2 pi n / SPS)` of the block being collected, and its smoothed form.
    block: Complex32,
    block_len: usize,
    timing: Complex32,
    /// Absolute index of the next filter output.
    index: u64,
    previous: Option<Complex32>,
    /// Smoothed `e^(j 4 d)` of the phase steps `d`.
    freq: Complex32,
    /// Smoothed carrier offset in radians per symbol.
    offset: f32,
}

impl Demod {
    /// `invert` conjugates the input (a receiver chain that mirrors the spectrum).
    #[must_use]
    pub fn new(invert: bool) -> Self {
        Self {
            taps: rrc_taps(),
            invert,
            pending: Vec::new(),
            filtered: Vec::new(),
            base: 0,
            next: None,
            block: Complex32::new(0.0, 0.0),
            block_len: 0,
            timing: Complex32::new(0.0, 0.0),
            index: 0,
            previous: None,
            freq: Complex32::new(0.0, 0.0),
            offset: 0.0,
        }
    }

    /// Forget everything (a gap in the input).
    pub fn reset(&mut self) {
        *self = Self::new(self.invert);
    }

    /// Estimated carrier offset in Hz.
    #[must_use]
    pub fn offset_hz(&self) -> f32 {
        self.offset * SYMBOL_RATE as f32 / TAU
    }

    /// Demodulate `iq`; appends the recovered bits (0 or 1 per element) to `bits`.
    pub fn process(&mut self, iq: &[Complex32], bits: &mut Vec<u8>) {
        if self.invert {
            self.pending.extend(iq.iter().map(Complex32::conj));
        } else {
            self.pending.extend_from_slice(iq);
        }
        self.filter();
        self.sample(bits);
    }

    /// Run the matched filter over everything that has enough history.
    fn filter(&mut self) {
        let n = self.taps.len();
        if self.pending.len() < n {
            return;
        }
        let outputs = self.pending.len() - n + 1;
        for start in 0..outputs {
            let window = &self.pending[start..start + n];
            let mut re = 0.0f32;
            let mut im = 0.0f32;
            for (tap, sample) in self.taps.iter().zip(window) {
                re += tap * sample.re;
                im += tap * sample.im;
            }
            let value = Complex32::new(re, im);
            self.filtered.push(value);
            // Symbol-rate component of the envelope.
            let phase = TAU * (self.index % SPS as u64) as f32 / SPS as f32;
            self.block += Complex32::from_polar(value.norm_sqr(), -phase);
            self.block_len += 1;
            self.index += 1;
            if self.block_len == TIMING_BLOCK {
                self.timing_block();
            }
        }
        self.pending.drain(..outputs);
    }

    /// One block of the envelope line is complete: smooth it and steer the sampling instant.
    fn timing_block(&mut self) {
        self.timing = self.timing * TIMING_KEEP + self.block;
        self.block = Complex32::new(0.0, 0.0);
        self.block_len = 0;
        if self.timing.norm_sqr() <= 0.0 {
            return;
        }
        // The envelope peaks at the symbol instants; the line's phase is minus that position.
        let target = f64::from(-self.timing.arg()) / f64::from(TAU) * SPS as f64;
        match &mut self.next {
            None => {
                // First estimate: the next instant on the grid at or after the data we hold.
                let first = self.base as f64;
                let phase = (target - first).rem_euclid(SPS as f64);
                self.next = Some(first + phase);
            }
            Some(next) => {
                let mut error = (target - *next).rem_euclid(SPS as f64);
                if error >= SPS as f64 / 2.0 {
                    error -= SPS as f64;
                }
                *next += TIMING_GAIN * error;
            }
        }
    }

    /// Take symbols at the instants the timing gives, as long as the filter output reaches them.
    fn sample(&mut self, bits: &mut Vec<u8>) {
        let Some(mut next) = self.next else {
            return;
        };
        loop {
            let at = next.floor();
            let lower = at as u64;
            if lower < self.base {
                next += SPS as f64;
                continue;
            }
            let i = (lower - self.base) as usize;
            if i + 1 >= self.filtered.len() {
                break;
            }
            let fraction = (next - at) as f32;
            let value = self.filtered[i] * (1.0 - fraction) + self.filtered[i + 1] * fraction;
            next += SPS as f64;
            self.symbol(value, bits);
        }
        self.next = Some(next);
        // Keep one sample before the next instant, drop the rest.
        let keep_from = (next.floor() as u64).saturating_sub(SPS as u64);
        if keep_from > self.base {
            let drop = ((keep_from - self.base) as usize).min(self.filtered.len());
            self.filtered.drain(..drop);
            self.base += drop as u64;
        }
    }

    fn symbol(&mut self, value: Complex32, bits: &mut Vec<u8>) {
        let Some(previous) = self.previous.replace(value) else {
            return;
        };
        let step = (value * previous.conj()).arg();
        // For clean symbols every step is an odd multiple of pi/4 plus the carrier offset;
        // times four that is pi plus four offsets, whatever the data.
        self.freq =
            self.freq * FREQ_KEEP + Complex32::from_polar(1.0, 4.0 * step) * (1.0 - FREQ_KEEP);
        self.offset = (-self.freq).arg() / 4.0;
        let corrected = step - self.offset;
        // In units of pi/4, folded to [-4, 4).
        let units = ((corrected / FRAC_PI_4) + 4.0).rem_euclid(8.0) - 4.0;
        let decided = ((units / 2.0).floor() as i32 * 2 + 1).clamp(-3, 3);
        bits.extend_from_slice(&step_bits(decided));
    }
}

/// Complex samples from little-endian `cf32` bytes; a trailing partial sample stays in `carry`.
pub fn parse_cf32(carry: &mut Vec<u8>, bytes: &[u8], out: &mut Vec<Complex32>) {
    carry.extend_from_slice(bytes);
    let whole = carry.len() - carry.len() % 8;
    for chunk in carry[..whole].as_chunks::<8>().0 {
        let re = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let im = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        out.push(Complex32::new(re, im));
    }
    carry.drain(..whole);
}

/// Deterministic noise source (xorshift), uniform in [-1, 1).
struct Noise(u64);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
    }
}

/// A transmitter for tests, also those of the plugin's end-to-end suite: bits to pi/4-DQPSK, root
/// raised cosine shaped, at `SAMPLE_RATE`, with a carrier offset, a timing offset (samples dropped
/// from the start) and noise.
pub fn modulate(bits: &[u8], offset_hz: f32, drop: usize, noise_amplitude: f32) -> Vec<Complex32> {
    let mut phase = 0.0f32;
    let mut symbols = Vec::new();
    for pair in bits.as_chunks::<2>().0 {
        phase += bits_step([pair[0], pair[1]]) as f32 * FRAC_PI_4;
        symbols.push(Complex32::from_polar(1.0, phase));
    }
    let taps = rrc_taps();
    let mut signal = vec![Complex32::new(0.0, 0.0); symbols.len() * SPS + taps.len()];
    for (k, symbol) in symbols.iter().enumerate() {
        for (j, tap) in taps.iter().enumerate() {
            signal[k * SPS + j] += *symbol * *tap;
        }
    }
    let mut noise = Noise(0x9e37_79b9_7f4a_7c15);
    signal
        .into_iter()
        .skip(drop)
        .enumerate()
        .map(|(n, s)| {
            let rotation =
                Complex32::from_polar(1.0, TAU * offset_hz * n as f32 / SAMPLE_RATE as f32 + 0.7);
            s * rotation + Complex32::new(noise.next(), noise.next()) * noise_amplitude
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_burst() -> Vec<u8> {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tetra/sync-burst.bits"),
        )
        .expect("fixture");
        let bits: Vec<u8> = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .flat_map(str::bytes)
            .map(|b| b - b'0')
            .collect();
        assert_eq!(bits.len(), 510);
        bits
    }

    /// The demodulator's output contains the burst: the first 200 bits (the training sequence,
    /// the frequency correction field and the start of the sync block) at some alignment.
    fn recovered(bits: &[u8], burst: &[u8]) -> bool {
        let needle = &burst[..200];
        // Differential detection starts one symbol late, so alignment is to the bit pair.
        bits.windows(needle.len()).skip(100).any(|w| w == needle)
    }

    fn run(offset_hz: f32, drop: usize, noise_amplitude: f32, invert: bool) -> bool {
        let burst = fixture_burst();
        let stream: Vec<u8> = burst.iter().copied().cycle().take(510 * 30).collect();
        let mut iq = modulate(&stream, offset_hz, drop, noise_amplitude);
        if invert {
            for s in &mut iq {
                *s = s.conj();
            }
        }
        let mut demod = Demod::new(invert);
        let mut bits = Vec::new();
        // The host delivers about 20 ms at a time.
        for chunk in iq.chunks(2880) {
            demod.process(chunk, &mut bits);
        }
        recovered(&bits, &burst)
    }

    #[test]
    fn the_filter_has_unit_energy_and_is_symmetric() {
        let taps = rrc_taps();
        assert_eq!(taps.len(), SPAN_SYMBOLS * SPS + 1);
        let energy: f32 = taps.iter().map(|t| t * t).sum();
        assert!((energy - 1.0).abs() < 1e-4);
        for i in 0..taps.len() / 2 {
            assert!((taps[i] - taps[taps.len() - 1 - i]).abs() < 1e-6);
        }
    }

    #[test]
    fn the_phase_step_mapping_is_its_own_inverse() {
        for step in [1, 3, -1, -3] {
            assert_eq!(bits_step(step_bits(step)), step);
        }
        // EN 300 392-2 table 5.1.
        assert_eq!(step_bits(1), [0, 0]);
        assert_eq!(step_bits(3), [0, 1]);
        assert_eq!(step_bits(-3), [1, 1]);
        assert_eq!(step_bits(-1), [1, 0]);
    }

    #[test]
    fn a_clean_sync_burst_from_osmo_tetras_generator_comes_back_bit_exact() {
        assert!(run(0.0, 0, 0.0, false));
    }

    #[test]
    fn any_sampling_phase_works() {
        for drop in [1, 2, 3, 4, 5, 6, 7] {
            assert!(run(0.0, drop, 0.0, false), "drop {drop}");
        }
    }

    #[test]
    fn a_carrier_offset_of_a_kilohertz_is_taken_out() {
        for offset in [-2000.0, -900.0, 700.0, 1800.0] {
            assert!(run(offset, 3, 0.0, false), "{offset} Hz");
        }
    }

    #[test]
    fn noise_at_a_moderate_level_is_survived() {
        // Signal amplitude is about 1 per sample after filtering; 0.2 per axis is roughly 15 dB.
        assert!(run(400.0, 5, 0.2, false));
    }

    #[test]
    fn a_mirrored_spectrum_needs_invert() {
        let burst = fixture_burst();
        let stream: Vec<u8> = burst.iter().copied().cycle().take(510 * 30).collect();
        let mut iq = modulate(&stream, 0.0, 0, 0.0);
        for s in &mut iq {
            *s = s.conj();
        }
        let mut plain = Demod::new(false);
        let mut bits = Vec::new();
        plain.process(&iq, &mut bits);
        assert!(!recovered(&bits, &burst));
        assert!(run(0.0, 0, 0.0, true));
    }

    #[test]
    fn the_offset_estimate_is_reported() {
        let burst = fixture_burst();
        let stream: Vec<u8> = burst.iter().copied().cycle().take(510 * 30).collect();
        let iq = modulate(&stream, 1500.0, 0, 0.0);
        let mut demod = Demod::new(false);
        let mut bits = Vec::new();
        demod.process(&iq, &mut bits);
        assert!(
            (demod.offset_hz() - 1500.0).abs() < 100.0,
            "{}",
            demod.offset_hz()
        );
    }

    #[test]
    fn samples_are_parsed_across_chunk_boundaries() {
        let mut bytes = Vec::new();
        for v in [1.0f32, -2.0, 0.5, 4.0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let (mut carry, mut out) = (Vec::new(), Vec::new());
        parse_cf32(&mut carry, &bytes[..5], &mut out);
        assert!(out.is_empty());
        parse_cf32(&mut carry, &bytes[5..13], &mut out);
        assert_eq!(out, vec![Complex32::new(1.0, -2.0)]);
        parse_cf32(&mut carry, &bytes[13..], &mut out);
        assert_eq!(out.len(), 2);
        assert!(carry.is_empty());
    }
}
