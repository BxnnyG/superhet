//! HF listening comfort: noise blanker, manual and automatic notch, noise reduction.
//!
//! All blocks are optional, off by default and run per VFO inside a [`crate::Channel`]:
//!
//! - [`NoiseBlanker`] works on the complex channel-rate baseband *before* the channel filter
//!   (impulse noise is still short there; the channel filter would smear it). It zeroes samples
//!   whose power exceeds a multiple of the slowly tracked mean power, plus a guard window on both
//!   sides (a small delay line gives the look-behind).
//! - [`ManualNotch`] is a second-order IIR notch at an audio frequency with an adjustable width.
//! - [`AutoNotch`] is an NLMS adaptive predictor: a steady carrier is predictable from the delayed
//!   past, speech (correlated only over a short time) is not, so `x - prediction` removes the
//!   whistle without a frequency setting. The strength sets the adaptation speed.
//! - [`NoiseReducer`] is spectral subtraction on 50 %-overlapping sqrt-Hann frames with a
//!   minimum-statistics noise estimate. Chosen over an LMS line enhancer because it also reduces
//!   broadband noise on speech; the price is a latency of one frame (about 8 ms), which is
//!   inaudible for voice and far below the squelch / AGC time constants.
//!
//! The audio blocks run on the demodulated signal at the channel rate (before the AGC for SSB, so a
//! whistle cannot pump the gain). They are meaningless for WFM and are skipped there.

use std::collections::VecDeque;
use std::f64::consts::TAU;
use std::sync::Arc;

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

/// Frequency and width of the manual notch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotchSetting {
    /// Centre in Hz of the audio (demodulated) signal.
    pub freq_hz: f32,
    /// -3 dB width in Hz.
    pub width_hz: f32,
}

/// Which comfort blocks run; `None` = off, strengths are `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ComfortConfig {
    /// Noise blanker strength (higher = blanks smaller peaks).
    pub noise_blanker: Option<f32>,
    /// Manual notch.
    pub notch: Option<NotchSetting>,
    /// Automatic notch adaptation speed.
    pub auto_notch: Option<f32>,
    /// Noise reduction strength (over-subtraction and maximum attenuation).
    pub noise_reduction: Option<f32>,
}

impl ComfortConfig {
    /// Clamps strengths to `0..=1` and the notch to a usable range.
    pub fn sanitized(self) -> Self {
        let s = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.5
            }
        };
        Self {
            noise_blanker: self.noise_blanker.map(s),
            notch: self.notch.map(|n| NotchSetting {
                freq_hz: if n.freq_hz.is_finite() {
                    n.freq_hz.clamp(NOTCH_MIN_HZ, NOTCH_MAX_HZ)
                } else {
                    1_000.0
                },
                width_hz: if n.width_hz.is_finite() {
                    n.width_hz.clamp(NOTCH_MIN_WIDTH_HZ, NOTCH_MAX_WIDTH_HZ)
                } else {
                    50.0
                },
            }),
            auto_notch: self.auto_notch.map(s),
            noise_reduction: self.noise_reduction.map(s),
        }
    }

    /// True if no block is on.
    pub fn is_off(&self) -> bool {
        self.noise_blanker.is_none()
            && self.notch.is_none()
            && self.auto_notch.is_none()
            && self.noise_reduction.is_none()
    }
}

/// Lowest / highest manual notch frequency in Hz.
pub const NOTCH_MIN_HZ: f32 = 50.0;
/// Highest manual notch frequency in Hz.
pub const NOTCH_MAX_HZ: f32 = 12_000.0;
/// Narrowest manual notch in Hz.
pub const NOTCH_MIN_WIDTH_HZ: f32 = 10.0;
/// Widest manual notch in Hz.
pub const NOTCH_MAX_WIDTH_HZ: f32 = 1_000.0;

// ---------------------------------------------------------------------------------------------
// Noise blanker

/// Time constant of the mean power the blanker compares against.
const NB_AVG_S: f64 = 0.02;
/// Guard time blanked before and after a detected peak.
const NB_GUARD_S: f64 = 0.0003;
/// Start-up time in which the mean is learned and nothing is blanked.
const NB_WARMUP_S: f64 = 0.005;

/// Impulse noise blanker for complex baseband; see the module docs.
#[derive(Debug, Clone)]
pub struct NoiseBlanker {
    /// Squared amplitude ratio above which a sample counts as an impulse.
    thr2: f32,
    avg: f32,
    /// Samples left of the warm-up in which the mean follows quickly and nothing is blanked.
    warmup: usize,
    warmup_len: usize,
    alpha: f32,
    guard: usize,
    blank_left: usize,
    interpolate: bool,
    hist: Vec<Complex32>,
    hist_mask: Vec<bool>,
    work: Vec<Complex32>,
    mask: Vec<bool>,
    last_good: Complex32,
}

impl NoiseBlanker {
    /// Creates a blanker for baseband at `rate_hz`; `strength` is `0.0..=1.0`.
    pub fn new(rate_hz: f64, strength: f32) -> Self {
        let guard = ((rate_hz * NB_GUARD_S).round() as usize).max(3);
        let mut nb = Self {
            thr2: 1.0,
            avg: 0.0,
            warmup: 0,
            warmup_len: ((rate_hz * NB_WARMUP_S) as usize).max(8),
            alpha: (1.0 - (-1.0 / (rate_hz * NB_AVG_S)).exp()) as f32,
            guard,
            blank_left: 0,
            interpolate: false,
            hist: vec![Complex32::new(0.0, 0.0); guard],
            hist_mask: vec![false; guard],
            work: Vec::new(),
            mask: Vec::new(),
            last_good: Complex32::new(0.0, 0.0),
        };
        nb.warmup = nb.warmup_len;
        nb.set_strength(strength);
        nb
    }

    /// Strength `0..=1` maps to an amplitude threshold of 6x (gentle) down to 2.5x (strong) the
    /// mean amplitude.
    pub fn set_strength(&mut self, strength: f32) {
        let k = 6.0 - 3.5 * strength.clamp(0.0, 1.0);
        self.thr2 = k * k;
    }

    /// Forgets the mean power and the delay line.
    pub fn reset(&mut self) {
        self.avg = 0.0;
        self.warmup = self.warmup_len;
        self.blank_left = 0;
        self.hist.fill(Complex32::new(0.0, 0.0));
        self.hist_mask.fill(false);
        self.last_good = Complex32::new(0.0, 0.0);
    }

    /// Fills blanked spans by linear interpolation instead of zeros. Right for AM (the carrier
    /// sits at DC, so the baseband barely moves over a span); for SSB / CW / FM the baseband
    /// rotates and zeros are the better replacement.
    pub fn set_interpolate(&mut self, interpolate: bool) {
        self.interpolate = interpolate;
    }

    /// Blanks impulses in `buf` in place. The output is delayed by [`Self::delay`] samples.
    pub fn process(&mut self, buf: &mut [Complex32]) {
        let g = self.guard;
        let n = buf.len();
        self.work.clear();
        self.work.extend_from_slice(&self.hist);
        self.work.extend_from_slice(buf);
        self.mask.clear();
        self.mask.extend_from_slice(&self.hist_mask);
        self.mask.resize(g + n, false);
        for i in 0..n {
            let j = g + i;
            let p = self.work[j].norm_sqr();
            if self.warmup > 0 {
                self.warmup -= 1;
                self.avg += 0.05 * (p - self.avg);
                continue;
            }
            let limit = self.thr2 * self.avg;
            if p > limit {
                self.mask[i..=j].fill(true);
                self.blank_left = g;
                // The mean still creeps up (clipped), so a level step cannot lock the blanker
                // into blanking everything.
                self.avg += self.alpha * (limit - self.avg);
            } else if self.blank_left > 0 {
                self.mask[j] = true;
                self.blank_left -= 1;
            } else {
                self.avg += self.alpha * (p - self.avg);
            }
        }
        self.fill_blanked();
        buf.copy_from_slice(&self.work[..n]);
        self.hist.copy_from_slice(&self.work[n..n + g]);
        self.hist_mask.copy_from_slice(&self.mask[n..n + g]);
        if let Some(v) = (0..n).rev().find(|&i| !self.mask[i]) {
            self.last_good = self.work[v];
        }
    }

    /// Replaces the masked samples of `work` (zeros, or a line between the neighbours).
    fn fill_blanked(&mut self) {
        let total = self.work.len();
        let zero = Complex32::new(0.0, 0.0);
        let mut left = self.last_good;
        let mut i = 0;
        while i < total {
            if !self.mask[i] {
                left = self.work[i];
                i += 1;
                continue;
            }
            let start = i;
            while i < total && self.mask[i] {
                i += 1;
            }
            if !self.interpolate {
                self.work[start..i].fill(zero);
            } else if i < total {
                // Linear between the neighbours; `right` is the first sample after the span.
                let right = self.work[i];
                let len = (i - start + 1) as f32;
                for (k, w) in self.work[start..i].iter_mut().enumerate() {
                    *w = left + (right - left) * ((k + 1) as f32 / len);
                }
            } else {
                // Runs into the delay line: hold for now, redone when the next block arrives.
                self.work[start..i].fill(left);
            }
        }
    }

    /// Delay of the output in samples.
    pub fn delay(&self) -> usize {
        self.guard
    }
}

// ---------------------------------------------------------------------------------------------
// Manual notch

/// Second-order notch (RBJ biquad) with f64 state.
#[derive(Debug, Clone)]
pub struct ManualNotch {
    rate: f64,
    b0: f64,
    b1: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl ManualNotch {
    /// Creates a notch for a signal at `rate_hz`.
    pub fn new(rate_hz: f64, setting: NotchSetting) -> Self {
        let mut n = Self {
            rate: rate_hz,
            b0: 1.0,
            b1: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        };
        n.set(setting);
        n
    }

    /// Moves the notch without clearing the filter state (dragging is click-free).
    pub fn set(&mut self, setting: NotchSetting) {
        let f = f64::from(setting.freq_hz).clamp(20.0, 0.45 * self.rate);
        let width = f64::from(setting.width_hz).clamp(1.0, f);
        let w0 = TAU * f / self.rate;
        let q = f / width;
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        let c = w0.cos();
        self.b0 = 1.0 / a0;
        self.b1 = -2.0 * c / a0;
        self.a1 = -2.0 * c / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Filters `buf` in place (transposed direct form II).
    pub fn process(&mut self, buf: &mut [f32]) {
        for v in buf {
            let x = f64::from(*v);
            let y = self.b0 * x + self.z1;
            self.z1 = self.b1 * x - self.a1 * y + self.z2;
            self.z2 = self.b0 * x - self.a2 * y;
            *v = y as f32;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Automatic notch

/// Taps of the adaptive predictor.
const ANF_TAPS: usize = 32;
/// Decorrelation delay (speech is not predictable beyond this, a steady carrier is).
const ANF_DELAY_S: f64 = 0.0006;
const ANF_EPS: f32 = 1e-9;

/// NLMS adaptive notch; see the module docs.
#[derive(Debug, Clone)]
pub struct AutoNotch {
    mu: f32,
    delay: usize,
    w: [f32; ANF_TAPS],
    /// Circular history, length `delay + ANF_TAPS`, doubled for contiguous slices.
    hist: Vec<f32>,
    pos: usize,
    energy: f32,
}

impl AutoNotch {
    /// Creates a notch for a signal at `rate_hz`; `strength` is `0.0..=1.0`.
    pub fn new(rate_hz: f64, strength: f32) -> Self {
        let delay = ((rate_hz * ANF_DELAY_S).round() as usize).max(4);
        let len = delay + ANF_TAPS;
        let mut n = Self {
            mu: 0.0,
            delay,
            w: [0.0; ANF_TAPS],
            hist: vec![0.0; 2 * len],
            pos: 0,
            energy: 0.0,
        };
        n.set_strength(strength);
        n
    }

    /// Strength `0..=1` maps to a step size of 0.002 (careful with speech) to 0.05 (fast).
    pub fn set_strength(&mut self, strength: f32) {
        self.mu = 0.002 * 25.0_f32.powf(strength.clamp(0.0, 1.0));
    }

    /// Forgets the adapted weights and the history.
    pub fn reset(&mut self) {
        self.w = [0.0; ANF_TAPS];
        self.hist.fill(0.0);
        self.pos = 0;
        self.energy = 0.0;
    }

    /// Filters `buf` in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        let len = self.delay + ANF_TAPS;
        for v in buf {
            let x = *v;
            // hist[pos..pos+len] holds the last `len` inputs, oldest first; the reference vector
            // is the part older than `delay`.
            let r = &self.hist[self.pos..self.pos + ANF_TAPS];
            let y: f32 = self.w.iter().zip(r).map(|(w, u)| w * u).sum();
            let e = x - y;
            let g = self.mu * e / (self.energy + ANF_EPS);
            for (w, u) in self.w.iter_mut().zip(r) {
                *w += g * u;
            }
            *v = e;
            self.push(x, len);
        }
    }

    fn push(&mut self, x: f32, len: usize) {
        self.hist[self.pos] = x;
        self.hist[self.pos + len] = x;
        self.pos = (self.pos + 1) % len;
        // Recompute the reference energy (taps are few; exact, no drift).
        self.energy = self.hist[self.pos..self.pos + ANF_TAPS]
            .iter()
            .map(|u| u * u)
            .sum();
    }
}

// ---------------------------------------------------------------------------------------------
// Noise reduction

/// Frame length target in seconds (rounded up to a power of two).
const NR_FRAME_S: f64 = 0.008;
/// Smoothing of the periodogram (per hop).
const NR_SMOOTH: f32 = 0.7;
/// Length of one noise-minimum sub-window and number of them.
const NR_SUB_S: f64 = 0.25;
const NR_SUBS: usize = 6;
/// Bias of the minimum of the smoothed periodogram versus the mean noise power.
const NR_MIN_BIAS: f32 = 2.2;
/// Temporal smoothing of the gain.
const NR_GAIN_SMOOTH: f32 = 0.5;
/// Frames between noise floor refreshes and half width (bins) of its frequency median.
const NR_NOISE_EVERY: usize = 4;
const NR_MEDIAN_HALF: usize = 8;

/// Spectral-subtraction noise reducer; see the module docs.
pub struct NoiseReducer {
    n: usize,
    hop: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    frame: Vec<f32>,
    fill: usize,
    ola: Vec<f32>,
    outq: VecDeque<f32>,
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
    smooth: Vec<f32>,
    cur_min: Vec<f32>,
    subs: Vec<Vec<f32>>,
    sub_idx: usize,
    sub_frames: usize,
    sub_len: usize,
    noise: Vec<f32>,
    floor_min: Vec<f32>,
    median_buf: Vec<f32>,
    noise_age: usize,
    raw: Vec<f32>,
    gain: Vec<f32>,
    alpha: f32,
    floor: f32,
    primed: bool,
}

impl NoiseReducer {
    /// Creates a reducer for a signal at `rate_hz`; `strength` is `0.0..=1.0`.
    pub fn new(rate_hz: f64, strength: f32) -> Self {
        let n = ((rate_hz * NR_FRAME_S).ceil() as usize)
            .next_power_of_two()
            .clamp(64, 1024);
        let hop = n / 2;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        let window = (0..n)
            .map(|i| (std::f64::consts::PI * (i as f64 + 0.5) / n as f64).sin() as f32)
            .collect();
        let bins = n / 2 + 1;
        let scratch_len = fft
            .get_inplace_scratch_len()
            .max(ifft.get_inplace_scratch_len());
        let mut nr = Self {
            n,
            hop,
            fft,
            ifft,
            window,
            frame: vec![0.0; n],
            fill: 0,
            ola: vec![0.0; n],
            outq: std::iter::repeat_n(0.0, n).collect(),
            spec: vec![Complex32::new(0.0, 0.0); n],
            scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            smooth: vec![0.0; bins],
            cur_min: vec![f32::MAX; bins],
            subs: vec![vec![f32::MAX; bins]; NR_SUBS],
            sub_idx: 0,
            sub_frames: 0,
            sub_len: ((rate_hz * NR_SUB_S) / hop as f64).ceil().max(1.0) as usize,
            noise: vec![0.0; bins],
            floor_min: vec![0.0; bins],
            median_buf: Vec::with_capacity(2 * NR_MEDIAN_HALF + 1),
            noise_age: NR_NOISE_EVERY,
            raw: vec![1.0; bins],
            gain: vec![1.0; bins],
            alpha: 1.0,
            floor: 0.5,
            primed: false,
        };
        nr.set_strength(strength);
        nr
    }

    /// Strength `0..=1`: over-subtraction 1.0..=2.5 and maximum attenuation 6..=24 dB.
    pub fn set_strength(&mut self, strength: f32) {
        let s = strength.clamp(0.0, 1.0);
        self.alpha = 1.0 + 1.5 * s;
        self.floor = 10.0_f32.powf(-(6.0 + 18.0 * s) / 20.0);
    }

    /// Delay of the output in samples.
    pub fn delay(&self) -> usize {
        self.n
    }

    /// Forgets the noise estimate and the sample history.
    pub fn reset(&mut self) {
        self.frame.fill(0.0);
        self.fill = 0;
        self.ola.fill(0.0);
        self.outq = std::iter::repeat_n(0.0, self.n).collect();
        self.smooth.fill(0.0);
        self.cur_min.fill(f32::MAX);
        for s in &mut self.subs {
            s.fill(f32::MAX);
        }
        self.sub_idx = 0;
        self.sub_frames = 0;
        self.noise.fill(0.0);
        self.noise_age = NR_NOISE_EVERY;
        self.gain.fill(1.0);
        self.primed = false;
    }

    /// Filters `buf` in place (delayed by [`Self::delay`] samples).
    pub fn process(&mut self, buf: &mut [f32]) {
        for v in buf {
            self.frame[self.fill] = *v;
            self.fill += 1;
            if self.fill == self.n {
                self.run_frame();
                self.frame.copy_within(self.hop.., 0);
                self.fill = self.hop;
            }
            *v = self.outq.pop_front().unwrap_or(0.0);
        }
    }

    fn run_frame(&mut self) {
        let (n, bins) = (self.n, self.n / 2 + 1);
        for i in 0..n {
            self.spec[i] = Complex32::new(self.frame[i] * self.window[i], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.spec, &mut self.scratch);

        // Smoothed periodogram and the noise floor from the minimum over the last sub-windows.
        for k in 0..bins {
            let p = self.spec[k].norm_sqr();
            self.smooth[k] = if self.primed {
                NR_SMOOTH * self.smooth[k] + (1.0 - NR_SMOOTH) * p
            } else {
                p
            };
            self.cur_min[k] = self.cur_min[k].min(self.smooth[k]);
        }
        if !self.primed {
            self.primed = true;
            for s in &mut self.subs {
                s.copy_from_slice(&self.smooth);
            }
        }
        self.sub_frames += 1;
        if self.sub_frames >= self.sub_len {
            self.sub_frames = 0;
            self.subs[self.sub_idx].copy_from_slice(&self.cur_min);
            self.sub_idx = (self.sub_idx + 1) % NR_SUBS;
            self.cur_min.copy_from_slice(&self.smooth);
        }
        // The noise floor changes slowly: refresh it every few frames only. The per-bin minimum
        // is median-filtered over frequency, so narrow steady lines (a wanted tone, voice
        // harmonics) do not count as noise while the broadband floor does.
        self.noise_age += 1;
        if self.noise_age >= NR_NOISE_EVERY {
            self.noise_age = 0;
            for k in 0..bins {
                self.floor_min[k] = self
                    .subs
                    .iter()
                    .map(|s| s[k])
                    .fold(self.cur_min[k], f32::min);
            }
            for k in 0..bins {
                let lo = k.saturating_sub(NR_MEDIAN_HALF);
                let hi = (k + NR_MEDIAN_HALF + 1).min(bins);
                let w = &mut self.median_buf;
                w.clear();
                w.extend_from_slice(&self.floor_min[lo..hi]);
                let mid = w.len() / 2;
                let (_, m, _) = w.select_nth_unstable_by(mid, f32::total_cmp);
                self.noise[k] = *m * NR_MIN_BIAS;
            }
        }

        // Gain: power subtraction on the smoothed spectrum, floored, smoothed over time and over
        // three neighbouring bins against musical noise.
        let raw = &mut self.raw;
        for (k, r) in raw.iter_mut().enumerate() {
            let p = self.smooth[k].max(1e-20);
            *r = (1.0 - self.alpha * self.noise[k] / p)
                .max(self.floor * self.floor)
                .sqrt();
        }
        for k in 0..bins {
            let lo = raw[k.saturating_sub(1)];
            let hi = raw[(k + 1).min(bins - 1)];
            let g = 0.25 * lo + 0.5 * raw[k] + 0.25 * hi;
            self.gain[k] = NR_GAIN_SMOOTH * self.gain[k] + (1.0 - NR_GAIN_SMOOTH) * g;
        }
        for k in 0..bins {
            self.spec[k] *= self.gain[k];
            if k > 0 && k < n - k {
                self.spec[n - k] = self.spec[k].conj();
            }
        }
        self.ifft
            .process_with_scratch(&mut self.spec, &mut self.scratch);
        let norm = 1.0 / n as f32;
        for i in 0..n {
            self.ola[i] += self.spec[i].re * norm * self.window[i];
        }
        for i in 0..self.hop {
            self.outq.push_back(self.ola[i]);
        }
        self.ola.copy_within(self.hop.., 0);
        let tail = n - self.hop;
        self.ola[tail..].fill(0.0);
    }
}

// ---------------------------------------------------------------------------------------------
// Chain

/// The audio-domain blocks of one channel in processing order: manual notch, auto notch, NR.
pub struct AudioComfort {
    notch: Option<ManualNotch>,
    auto: Option<AutoNotch>,
    nr: Option<NoiseReducer>,
}

impl AudioComfort {
    /// Builds the blocks that `cfg` enables for a signal at `rate_hz`.
    pub fn new(rate_hz: f64, cfg: &ComfortConfig) -> Self {
        Self {
            notch: cfg.notch.map(|n| ManualNotch::new(rate_hz, n)),
            auto: cfg.auto_notch.map(|s| AutoNotch::new(rate_hz, s)),
            nr: cfg.noise_reduction.map(|s| NoiseReducer::new(rate_hz, s)),
        }
    }

    /// Applies a changed configuration: blocks are created / dropped as needed, parameters of
    /// running blocks change without clearing their state (so dragging the notch is click-free).
    pub fn update(&mut self, rate_hz: f64, cfg: &ComfortConfig) {
        match (cfg.notch, &mut self.notch) {
            (Some(s), Some(n)) => n.set(s),
            (Some(s), None) => self.notch = Some(ManualNotch::new(rate_hz, s)),
            (None, _) => self.notch = None,
        }
        match (cfg.auto_notch, &mut self.auto) {
            (Some(s), Some(a)) => a.set_strength(s),
            (Some(s), None) => self.auto = Some(AutoNotch::new(rate_hz, s)),
            (None, _) => self.auto = None,
        }
        match (cfg.noise_reduction, &mut self.nr) {
            (Some(s), Some(n)) => n.set_strength(s),
            (Some(s), None) => self.nr = Some(NoiseReducer::new(rate_hz, s)),
            (None, _) => self.nr = None,
        }
    }

    /// Clears all state.
    pub fn reset(&mut self) {
        if let Some(n) = &mut self.notch {
            n.reset();
        }
        if let Some(a) = &mut self.auto {
            a.reset();
        }
        if let Some(n) = &mut self.nr {
            n.reset();
        }
    }

    /// Processes `buf` in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        if let Some(n) = &mut self.notch {
            n.process(buf);
        }
        if let Some(a) = &mut self.auto {
            a.process(buf);
        }
        if let Some(n) = &mut self.nr {
            n.process(buf);
        }
    }

    /// Delay of the enabled blocks in samples.
    pub fn delay(&self) -> usize {
        self.nr.as_ref().map_or(0, NoiseReducer::delay)
    }

    /// True if no block is active.
    pub fn is_empty(&self) -> bool {
        self.notch.is_none() && self.auto.is_none() && self.nr.is_none()
    }
}
