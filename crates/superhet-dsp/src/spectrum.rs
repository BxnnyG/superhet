//! Streaming spectrum estimator.
//!
//! Complex samples go in; frames of power in dBFS come out at (at most) the configured
//! frame rate. Per frame: Blackman-Harris windowed FFTs with 50 % overlap are power-averaged
//! over the frame interval, `fftshift`ed (bin 0 = `-fs/2`, bin `N/2` = DC) and reduced to the
//! requested bin count by max-decimation (keeps narrow peaks visible).
//!
//! Scaling: a complex tone of amplitude 1.0 reads 0 dBFS in its bin.

use std::sync::Arc;

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

/// Power floor (linear) to keep `log10` finite.
const POWER_FLOOR: f32 = 1e-20;

/// Spectrum parameters.
#[derive(Debug, Clone)]
pub struct SpectrumConfig {
    /// FFT length (any size >= 16; powers of two are fastest).
    pub fft_size: usize,
    /// Output bins per frame (`<= fft_size`).
    pub bins: usize,
    /// Target frame rate in frames per second.
    pub fps: f32,
    /// Input sample rate in Hz.
    pub sample_rate: u32,
}

/// Invalid [`SpectrumConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpectrumError {
    /// FFT size is too small.
    #[error("fft_size must be >= 16")]
    FftSize,
    /// Bin count is zero or larger than the FFT size.
    #[error("bins must be in 1..=fft_size")]
    Bins,
    /// Frame rate is not a positive finite number.
    #[error("fps must be positive and finite")]
    Fps,
    /// Sample rate is zero.
    #[error("sample_rate must be non-zero")]
    SampleRate,
}

/// Streaming spectrum processor; see the module docs.
pub struct SpectrumProcessor {
    cfg: SpectrumConfig,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// `1 / (sum(window))^2`, so a full-scale tone reads 0 dBFS.
    norm: f32,
    hop: usize,
    pending: Vec<Complex32>,
    scratch: Vec<Complex32>,
    fft_scratch: Vec<Complex32>,
    acc: Vec<f32>,
    acc_count: u32,
    /// Samples consumed since the last emitted frame.
    since_frame: u64,
}

impl std::fmt::Debug for SpectrumProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectrumProcessor")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

/// 4-term Blackman-Harris window (periodic form not needed for spectrum display).
fn blackman_harris(n: usize) -> Vec<f32> {
    const A: [f64; 4] = [0.35875, 0.48829, 0.14128, 0.01168];
    let denom = (n - 1) as f64;
    (0..n)
        .map(|i| {
            let x = std::f64::consts::TAU * i as f64 / denom;
            (A[0] - A[1] * x.cos() + A[2] * (2.0 * x).cos() - A[3] * (3.0 * x).cos()) as f32
        })
        .collect()
}

impl SpectrumProcessor {
    /// Create a processor.
    ///
    /// # Errors
    /// Fails if the configuration is invalid.
    pub fn new(cfg: SpectrumConfig) -> Result<Self, SpectrumError> {
        if cfg.fft_size < 16 {
            return Err(SpectrumError::FftSize);
        }
        if cfg.bins == 0 || cfg.bins > cfg.fft_size {
            return Err(SpectrumError::Bins);
        }
        if !(cfg.fps.is_finite() && cfg.fps > 0.0) {
            return Err(SpectrumError::Fps);
        }
        if cfg.sample_rate == 0 {
            return Err(SpectrumError::SampleRate);
        }
        let fft = FftPlanner::new().plan_fft_forward(cfg.fft_size);
        let window = blackman_harris(cfg.fft_size);
        let sum: f32 = window.iter().sum();
        let fft_scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        Ok(Self {
            norm: 1.0 / (sum * sum),
            hop: cfg.fft_size / 2,
            pending: Vec::with_capacity(cfg.fft_size * 2),
            scratch: vec![Complex32::default(); cfg.fft_size],
            fft_scratch,
            acc: vec![0.0; cfg.fft_size],
            acc_count: 0,
            since_frame: 0,
            fft,
            window,
            cfg,
        })
    }

    /// Current configuration.
    pub fn config(&self) -> &SpectrumConfig {
        &self.cfg
    }

    /// Change the input sample rate (e.g. after a device reconfiguration) and restart averaging.
    ///
    /// # Errors
    /// Fails if `hz` is zero.
    pub fn set_sample_rate(&mut self, hz: u32) -> Result<(), SpectrumError> {
        if hz == 0 {
            return Err(SpectrumError::SampleRate);
        }
        self.cfg.sample_rate = hz;
        self.reset();
        Ok(())
    }

    /// Discard buffered samples and accumulated power (call after retuning).
    pub fn reset(&mut self) {
        self.pending.clear();
        self.acc.fill(0.0);
        self.acc_count = 0;
        self.since_frame = 0;
    }

    /// Feed samples; returns zero or more finished frames (`bins` dBFS values each).
    pub fn push(&mut self, samples: &[Complex32]) -> Vec<Vec<f32>> {
        let n = self.cfg.fft_size;
        let interval = (f64::from(self.cfg.sample_rate) / f64::from(self.cfg.fps)).max(1.0) as u64;
        let mut frames = Vec::new();
        self.pending.extend_from_slice(samples);
        let mut start = 0;
        while self.pending.len() - start >= n {
            self.accumulate(start);
            start += self.hop;
            self.since_frame += self.hop as u64;
            if self.since_frame >= interval {
                frames.push(self.finish_frame());
            }
        }
        self.pending.drain(..start);
        frames
    }

    fn accumulate(&mut self, start: usize) {
        let n = self.cfg.fft_size;
        for ((out, s), w) in self
            .scratch
            .iter_mut()
            .zip(&self.pending[start..start + n])
            .zip(&self.window)
        {
            *out = *s * *w;
        }
        self.fft
            .process_with_scratch(&mut self.scratch, &mut self.fft_scratch);
        for (a, x) in self.acc.iter_mut().zip(&self.scratch) {
            *a += x.norm_sqr();
        }
        self.acc_count += 1;
    }

    fn finish_frame(&mut self) -> Vec<f32> {
        let n = self.cfg.fft_size;
        let scale = self.norm / self.acc_count.max(1) as f32;
        // fftshift: shifted[k] = acc[(k + n/2) % n] for even n; general form below.
        let shift = n.div_ceil(2);
        let shifted = |k: usize| self.acc[(k + shift) % n];
        let bins = self.cfg.bins;
        let out = (0..bins)
            .map(|b| {
                let lo = b * n / bins;
                let hi = ((b + 1) * n / bins).max(lo + 1);
                let peak = (lo..hi).map(shifted).fold(0.0f32, f32::max);
                10.0 * (peak * scale).max(POWER_FLOOR).log10()
            })
            .collect();
        self.acc.fill(0.0);
        self.acc_count = 0;
        self.since_frame = 0;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq_hz: f64, rate: u32, len: usize, amp: f32) -> Vec<Complex32> {
        (0..len)
            .map(|i| {
                let ph = std::f64::consts::TAU * freq_hz * i as f64 / f64::from(rate);
                Complex32::new(ph.cos() as f32 * amp, ph.sin() as f32 * amp)
            })
            .collect()
    }

    fn cfg(bins: usize) -> SpectrumConfig {
        SpectrumConfig {
            fft_size: 4096,
            bins,
            fps: 25.0,
            sample_rate: 2_048_000,
        }
    }

    fn argmax(v: &[f32]) -> usize {
        v.iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .expect("non-empty")
    }

    #[test]
    fn tone_lands_in_expected_bin_with_full_scale_level() {
        // Bin spacing = 2_048_000 / 4096 = 500 Hz. +100 kHz → 200 bins right of DC.
        let mut p = SpectrumProcessor::new(cfg(4096)).expect("cfg");
        let frames = p.push(&tone(100_000.0, 2_048_000, 2_048_000 / 25 * 2, 1.0));
        assert!(!frames.is_empty());
        let f = &frames[0];
        assert_eq!(f.len(), 4096);
        assert_eq!(argmax(f), 2048 + 200);
        assert!((f[2048 + 200] - 0.0).abs() < 0.2, "{}", f[2048 + 200]);
        // Far from the tone the Blackman-Harris sidelobes are > 90 dB down.
        assert!(f[100] < -90.0, "{}", f[100]);
    }

    #[test]
    fn negative_offset_and_half_amplitude() {
        let mut p = SpectrumProcessor::new(cfg(4096)).expect("cfg");
        let frames = p.push(&tone(-250_000.0, 2_048_000, 2_048_000 / 25 + 4096, 0.5));
        let f = frames.last().expect("frame");
        assert_eq!(argmax(f), 2048 - 500);
        assert!((f[2048 - 500] + 6.02).abs() < 0.2, "{}", f[2048 - 500]);
    }

    #[test]
    fn decimates_bins_with_max() {
        let mut p = SpectrumProcessor::new(cfg(512)).expect("cfg");
        let frames = p.push(&tone(100_000.0, 2_048_000, 2_048_000 / 25 + 4096, 1.0));
        let f = frames.last().expect("frame");
        assert_eq!(f.len(), 512);
        assert_eq!(argmax(f), (2048 + 200) / 8);
        assert!((f[(2048 + 200) / 8]).abs() < 0.2);
    }

    #[test]
    fn frame_rate_follows_target_fps() {
        let mut p = SpectrumProcessor::new(cfg(1024)).expect("cfg");
        let one_second = tone(10_000.0, 2_048_000, 2_048_000, 1.0);
        let mut frames = 0;
        for chunk in one_second.chunks(16_384) {
            frames += p.push(chunk).len();
        }
        assert!((24..=26).contains(&frames), "{frames}");
    }

    #[test]
    fn noise_floor_of_silence_is_finite() {
        let mut p = SpectrumProcessor::new(cfg(256)).expect("cfg");
        let frames = p.push(&vec![Complex32::default(); 2_048_000 / 25 + 4096]);
        let f = frames.last().expect("frame");
        assert!(f.iter().all(|v| v.is_finite() && *v <= -190.0));
    }

    #[test]
    fn rejects_bad_config() {
        let ok = cfg(100);
        for bad in [
            SpectrumConfig {
                fft_size: 8,
                ..ok.clone()
            },
            SpectrumConfig {
                bins: 0,
                ..ok.clone()
            },
            SpectrumConfig {
                bins: 5000,
                ..ok.clone()
            },
            SpectrumConfig {
                fps: 0.0,
                ..ok.clone()
            },
            SpectrumConfig {
                fps: f32::NAN,
                ..ok.clone()
            },
            SpectrumConfig {
                sample_rate: 0,
                ..ok
            },
        ] {
            assert!(SpectrumProcessor::new(bad).is_err());
        }
    }
}
