//! Windowed-sinc FIR design and streaming polyphase-style decimation.
//!
//! Filters are designed with a Kaiser window from a transition-band specification. The
//! decimator only evaluates the output samples that are kept (`1/M` of the input rate), which
//! is the polyphase saving for a single-stage FIR decimator.

use std::ops::{Add, Mul};

use num_complex::Complex32;

/// Upper bound on a designed filter length; keeps pathological specs from allocating wildly.
const MAX_TAPS: usize = 4001;

/// Sample types the decimator can filter (`f32` and `Complex32`).
pub trait Sample: Copy + Default + Add<Output = Self> + Mul<f32, Output = Self> {}
impl Sample for f32 {}
impl Sample for Complex32 {}

/// Zeroth-order modified Bessel function of the first kind (power series).
pub(crate) fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..60 {
        term *= q / (f64::from(k) * f64::from(k));
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Kaiser window shape parameter for a stopband attenuation in dB.
pub(crate) fn kaiser_beta(atten_db: f64) -> f64 {
    if atten_db > 50.0 {
        0.1102 * (atten_db - 8.7)
    } else if atten_db >= 21.0 {
        0.5842 * (atten_db - 21.0).powf(0.4) + 0.07886 * (atten_db - 21.0)
    } else {
        0.0
    }
}

/// Kaiser window value at normalised position `x` in `[-1, 1]`.
pub(crate) fn kaiser(x: f64, beta: f64, i0_beta: f64) -> f64 {
    let r = (1.0 - x * x).max(0.0).sqrt();
    bessel_i0(beta * r) / i0_beta
}

/// Designs a linear-phase (odd length, symmetric) low-pass with unity DC gain.
///
/// `fpass`/`fstop` are the band edges in Hz at sample rate `fs`; the stopband attenuation
/// is `atten_db`. The cutoff sits in the middle of the transition band.
pub fn design_lowpass(fs: f64, fpass: f64, fstop: f64, atten_db: f64) -> Vec<f32> {
    let delta = ((fstop - fpass) / fs).max(1e-5);
    let fc = 0.5 * (fpass + fstop) / fs;
    let n = ((atten_db - 7.95) / (2.285 * std::f64::consts::TAU * delta)).ceil() as usize + 1;
    let n = (n | 1).clamp(3, MAX_TAPS);
    let beta = kaiser_beta(atten_db);
    let i0b = bessel_i0(beta);
    let mid = (n - 1) as f64 / 2.0;
    let mut h: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 - mid;
            let x = std::f64::consts::TAU * fc * t;
            let sinc = if t == 0.0 { 1.0 } else { x.sin() / x };
            2.0 * fc * sinc * kaiser(t / (mid + 1.0), beta, i0b)
        })
        .collect();
    let sum: f64 = h.iter().sum();
    for v in &mut h {
        *v /= sum;
    }
    h.into_iter().map(|v| v as f32).collect()
}

/// Dot product with four independent accumulators (lets the compiler vectorise).
fn dot<S: Sample>(x: &[S], h: &[f32]) -> S {
    let mut acc = [S::default(); 4];
    let (xc, xr) = x.as_chunks::<4>();
    let (hc, hr) = h.as_chunks::<4>();
    for (xs, hs) in xc.iter().zip(hc) {
        for ((a, &xv), &hv) in acc.iter_mut().zip(xs).zip(hs) {
            *a = *a + xv * hv;
        }
    }
    let mut tail = S::default();
    for (&xv, &hv) in xr.iter().zip(hr) {
        tail = tail + xv * hv;
    }
    (acc[0] + acc[1]) + (acc[2] + acc[3]) + tail
}

/// Streaming FIR filter with integer decimation (`m == 1` is a plain FIR).
///
/// Produces exactly `ceil(total_input / m)` outputs over a stream, independent of how the
/// input is split into blocks.
#[derive(Debug, Clone)]
pub struct DecimFir<S: Sample> {
    taps: Vec<f32>,
    m: usize,
    pending: Vec<S>,
    /// Input samples still to be discarded (only when `m` exceeds the filter length).
    skip: usize,
}

impl<S: Sample> DecimFir<S> {
    /// Creates a decimator with the given taps and decimation factor (`m >= 1`).
    pub fn new(taps: Vec<f32>, m: usize) -> Self {
        let n = taps.len().max(1);
        Self {
            pending: vec![S::default(); n - 1],
            taps,
            m: m.max(1),
            skip: 0,
        }
    }

    /// Filter length.
    pub fn len(&self) -> usize {
        self.taps.len()
    }

    /// True if the filter has no taps (never the case for designed filters).
    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }

    /// Clears the delay line.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.pending
            .resize(self.taps.len().max(1) - 1, S::default());
        self.skip = 0;
    }

    /// Filters `input`, appending decimated samples to `out`.
    pub fn process(&mut self, input: &[S], out: &mut Vec<S>) {
        let s = self.skip.min(input.len());
        self.skip -= s;
        self.pending.extend_from_slice(&input[s..]);
        let n = self.taps.len();
        if n == 0 {
            return;
        }
        let mut p = 0;
        while p + n <= self.pending.len() {
            out.push(dot(&self.pending[p..p + n], &self.taps));
            p += self.m;
        }
        let d = p.min(self.pending.len());
        self.skip += p - d;
        self.pending.drain(..d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gain of real taps at normalised frequency `f` (cycles/sample).
    fn gain(h: &[f32], f: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in h.iter().enumerate() {
            let a = std::f64::consts::TAU * f * i as f64;
            re += f64::from(v) * a.cos();
            im -= f64::from(v) * a.sin();
        }
        re.hypot(im)
    }

    #[test]
    fn lowpass_meets_spec() {
        let h = design_lowpass(1000.0, 100.0, 150.0, 70.0);
        assert!((gain(&h, 0.0) - 1.0).abs() < 1e-6);
        assert!(gain(&h, 0.08) > 0.999 && gain(&h, 0.08) < 1.001);
        for k in 0..40 {
            let f = 0.15 + 0.35 * f64::from(k) / 39.0;
            assert!(20.0 * gain(&h, f).log10() < -68.0, "f={f}");
        }
    }

    #[test]
    fn decimation_count_is_block_independent() {
        let h = design_lowpass(1000.0, 50.0, 150.0, 60.0);
        let x: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        for m in [1usize, 3, 7] {
            let mut whole = DecimFir::<f32>::new(h.clone(), m);
            let mut a = Vec::new();
            whole.process(&x, &mut a);
            let mut split = DecimFir::<f32>::new(h.clone(), m);
            let mut b = Vec::new();
            for c in x.chunks(37) {
                split.process(c, &mut b);
            }
            assert_eq!(a.len(), 1000usize.div_ceil(m));
            assert_eq!(a, b);
        }
    }

    #[test]
    fn short_filter_with_large_factor_skips() {
        let mut f = DecimFir::<f32>::new(vec![1.0], 5);
        let mut out = Vec::new();
        for c in [3usize, 4, 8, 1].map(|n| vec![1.0f32; n]) {
            f.process(&c, &mut out);
        }
        assert_eq!(out.len(), 16usize.div_ceil(5));
    }
}
