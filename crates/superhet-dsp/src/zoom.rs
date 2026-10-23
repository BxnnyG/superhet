//! Zoom spectrum: an FFT of a narrow slice of the band with the full bin count.
//!
//! The IQ stream is frequency-shifted so the slice centre sits at DC, low-pass filtered and
//! decimated by `2^shift` (a cascade of stages that decimate by up to 8, each with a short filter
//! because the transition band is wide for all but the last one), and then fed to a regular
//! [`SpectrumProcessor`]. The FFT length is chosen for the decimated rate, so the bin width
//! shrinks by about `2^shift` against the full-band FFT: this is real resolution, not
//! stretched pixels.
//!
//! Only the central [`USABLE_FRACTION`] of the decimated band is returned; the outer parts
//! hold the filter roll-off and are never shown.
//!
//! Limits (see `docs/adr/0009-zoom-fft.md`): the decimated rate stays above [`MIN_ZOOM_RATE`]
//! and the FFT window never exceeds [`MAX_WINDOW_SECS`], so a zoom frame arrives at least
//! about every 60 ms and the bin width does not fall below roughly 9 Hz.

use num_complex::Complex32;

use crate::fir::{DecimFir, design_lowpass};
use crate::nco::Nco;
use crate::spectrum::{SpectrumConfig, SpectrumError, SpectrumProcessor};

/// Share of the decimated band that is returned (centred); the rest is filter roll-off.
pub const USABLE_FRACTION: f64 = 0.8;
/// The usable band must exceed the requested span by this factor, which leaves room for
/// panning without recomputing the slice.
pub const PAN_MARGIN: f64 = 1.15;
/// Lowest decimated sample rate in Hz.
pub const MIN_ZOOM_RATE: f64 = 12_500.0;
/// Longest FFT window in seconds (bounds the latency and the temporal resolution limit).
pub const MAX_WINDOW_SECS: f64 = 0.125;
/// Smallest FFT length used for a slice.
pub const MIN_FFT: usize = 512;
/// Largest supported decimation exponent.
pub const MAX_SHIFT: u32 = 10;
/// Largest decimation exponent of one filter stage (decimation by 8): the first stages see the
/// full rate, so fewer, wider steps are much cheaper than a cascade of halvings.
const MAX_STAGE_SHIFT: u32 = 3;
/// Stopband attenuation of the decimation filters in dB.
const STOP_ATTEN_DB: f64 = 90.0;

/// Invalid zoom configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ZoomError {
    /// The decimation exponent is outside `1..=MAX_SHIFT`.
    #[error("shift must be in 1..={MAX_SHIFT}")]
    Shift,
    /// The decimated rate is below [`MIN_ZOOM_RATE`].
    #[error("decimated rate is too low")]
    Rate,
    /// The spectrum settings are invalid.
    #[error(transparent)]
    Spectrum(#[from] SpectrumError),
}

/// Decimation exponent `k` (decimation by `2^k`) that shows `span_hz` of a band sampled at
/// `sample_rate`, or `None` when the full-band spectrum is good enough (`k` would be 0) or the
/// input is unusable. The largest `k` is used whose usable band still holds the span with
/// [`PAN_MARGIN`] to spare and whose rate stays above [`MIN_ZOOM_RATE`].
#[must_use]
pub fn shift_for_span(sample_rate: f64, span_hz: f64) -> Option<u32> {
    if !(sample_rate.is_finite() && sample_rate > 0.0 && span_hz.is_finite() && span_hz > 0.0) {
        return None;
    }
    let mut k = 0;
    while k < MAX_SHIFT {
        let rate = sample_rate / f64::from(1u32 << (k + 1));
        if rate < MIN_ZOOM_RATE || USABLE_FRACTION * rate < PAN_MARGIN * span_hz {
            break;
        }
        k += 1;
    }
    (k > 0).then_some(k)
}

/// FFT length for a decimated rate: the largest power of two whose window fits
/// [`MAX_WINDOW_SECS`], within `MIN_FFT..=max_fft`.
#[must_use]
pub fn zoom_fft_size(rate: f64, max_fft: usize) -> usize {
    let max_fft = max_fft.max(MIN_FFT);
    let mut n = MIN_FFT;
    while n * 2 <= max_fft && (n * 2) as f64 <= rate * MAX_WINDOW_SECS {
        n *= 2;
    }
    n
}

/// Zoom spectrum parameters.
#[derive(Debug, Clone)]
pub struct ZoomConfig {
    /// Input sample rate in Hz.
    pub sample_rate: u32,
    /// Decimation exponent (decimation by `2^shift`).
    pub shift: u32,
    /// Largest FFT length allowed.
    pub max_fft: usize,
    /// Output bins per frame before trimming (`<=` the chosen FFT length).
    pub bins: usize,
    /// Target frame rate in frames per second.
    pub fps: f32,
}

/// Streaming zoom spectrum; see the module docs.
pub struct ZoomSpectrum {
    nco: Nco,
    stages: Vec<DecimFir<Complex32>>,
    processor: SpectrumProcessor,
    in_rate: f64,
    out_rate: f64,
    bins: usize,
    lo: usize,
    hi: usize,
    mixed: Vec<Complex32>,
    a: Vec<Complex32>,
    b: Vec<Complex32>,
}

impl std::fmt::Debug for ZoomSpectrum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZoomSpectrum")
            .field("out_rate", &self.out_rate)
            .field("bins", &self.bins)
            .finish_non_exhaustive()
    }
}

impl ZoomSpectrum {
    /// Create a zoom spectrum centred on DC of the input (use [`Self::set_offset`] to move it).
    ///
    /// # Errors
    /// Fails if the exponent or the resulting rate is out of range, or the spectrum settings
    /// are invalid.
    pub fn new(cfg: &ZoomConfig) -> Result<Self, ZoomError> {
        if cfg.shift == 0 || cfg.shift > MAX_SHIFT {
            return Err(ZoomError::Shift);
        }
        let in_rate = f64::from(cfg.sample_rate);
        let out_rate = in_rate / f64::from(1u32 << cfg.shift);
        if out_rate < MIN_ZOOM_RATE {
            return Err(ZoomError::Rate);
        }
        let fft_size = zoom_fft_size(out_rate, cfg.max_fft);
        let bins = cfg.bins.clamp(1, fft_size);
        let processor = SpectrumProcessor::new(SpectrumConfig {
            fft_size,
            bins,
            fps: cfg.fps,
            sample_rate: out_rate.round().max(1.0) as u32,
        })?;
        // Everything above `r_out - u` would alias into the half-band `u` that is displayed.
        let half_usable = USABLE_FRACTION / 2.0 * out_rate;
        let mut stages = Vec::new();
        let mut rate = in_rate;
        let mut remaining = cfg.shift;
        while remaining > 0 {
            let step = remaining.min(MAX_STAGE_SHIFT);
            let m = 1usize << step;
            let next = rate / m as f64;
            // The stopband runs from `next - half_usable` up to Nyquist, so every alias band
            // that would fold into the displayed part is covered, whatever `m` is.
            let taps = design_lowpass(rate, half_usable, next - half_usable, STOP_ATTEN_DB);
            stages.push(DecimFir::new(taps, m));
            rate = next;
            remaining -= step;
        }
        let lo = bins / 10;
        Ok(Self {
            nco: Nco::new(0.0, in_rate),
            stages,
            processor,
            in_rate,
            out_rate,
            bins,
            lo,
            hi: bins - lo,
            mixed: Vec::new(),
            a: Vec::new(),
            b: Vec::new(),
        })
    }

    /// Move the slice centre to `offset_hz` relative to the input centre (continuous: filters
    /// and phase are kept). Call [`Self::reset_averaging`] when the offset changed noticeably.
    pub fn set_offset(&mut self, offset_hz: f64) {
        self.nco.set_offset(offset_hz, self.in_rate);
    }

    /// Discard the FFT averaging state (the filters keep running).
    pub fn reset_averaging(&mut self) {
        self.processor.reset();
    }

    /// Decimated sample rate in Hz.
    #[must_use]
    pub fn out_rate(&self) -> f64 {
        self.out_rate
    }

    /// FFT length in use.
    #[must_use]
    pub fn fft_size(&self) -> usize {
        self.processor.config().fft_size
    }

    /// Bandwidth of the returned (trimmed) frames in Hz.
    #[must_use]
    pub fn usable_hz(&self) -> f64 {
        self.out_rate * (self.hi - self.lo) as f64 / self.bins as f64
    }

    /// Number of bins in a returned frame.
    #[must_use]
    pub fn frame_bins(&self) -> usize {
        self.hi - self.lo
    }

    /// Feed samples; returns finished frames of [`Self::frame_bins`] dBFS values each, lowest
    /// frequency first, centred on the slice centre.
    pub fn push(&mut self, samples: &[Complex32]) -> Vec<Vec<f32>> {
        self.nco.mix(samples, &mut self.mixed);
        // The result of the cascade ends up in `mixed` (no stages) or in `a`.
        let mut in_mixed = true;
        for stage in &mut self.stages {
            if in_mixed {
                self.a.clear();
                stage.process(&self.mixed, &mut self.a);
                in_mixed = false;
            } else {
                self.b.clear();
                stage.process(&self.a, &mut self.b);
                std::mem::swap(&mut self.a, &mut self.b);
            }
        }
        let decimated = if in_mixed { &self.mixed } else { &self.a };
        self.processor
            .push(decimated)
            .into_iter()
            .map(|f| f[self.lo..self.hi].to_vec())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: u32 = 2_048_000;

    fn tones(parts: &[(f64, f32)], len: usize) -> Vec<Complex32> {
        (0..len)
            .map(|i| {
                parts.iter().fold(Complex32::default(), |acc, &(f, amp)| {
                    let ph = std::f64::consts::TAU * f * i as f64 / f64::from(FS);
                    acc + Complex32::new(ph.cos() as f32 * amp, ph.sin() as f32 * amp)
                })
            })
            .collect()
    }

    fn zoom(shift: u32) -> ZoomSpectrum {
        ZoomSpectrum::new(&ZoomConfig {
            sample_rate: FS,
            shift,
            max_fft: 4096,
            bins: 2048,
            fps: 25.0,
        })
        .expect("zoom")
    }

    fn argmax(v: &[f32]) -> usize {
        v.iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .expect("non-empty")
    }

    fn last_frame(z: &mut ZoomSpectrum, input: &[Complex32]) -> Vec<f32> {
        let mut last = None;
        for chunk in input.chunks(8192) {
            if let Some(f) = z.push(chunk).pop() {
                last = Some(f);
            }
        }
        last.expect("a frame")
    }

    #[test]
    fn shift_picks_the_deepest_zoom_that_holds_the_span() {
        // 2.4 MS/s: k=1 usable 0.96 MHz, k=7 usable 15 kHz.
        assert_eq!(shift_for_span(2_400_000.0, 2_400_000.0), None);
        assert_eq!(shift_for_span(2_400_000.0, 1_000_000.0), None);
        assert_eq!(shift_for_span(2_400_000.0, 800_000.0), Some(1));
        assert_eq!(shift_for_span(2_400_000.0, 100_000.0), Some(4));
        assert_eq!(shift_for_span(2_400_000.0, 10_000.0), Some(7));
        // The rate floor limits narrower spans.
        assert_eq!(shift_for_span(2_400_000.0, 100.0), Some(7));
        assert_eq!(shift_for_span(f64::NAN, 100.0), None);
        assert_eq!(shift_for_span(2_400_000.0, 0.0), None);
        // Always holds the span with margin.
        for span in [5e3, 2e4, 7e4, 3e5, 7e5] {
            if let Some(k) = shift_for_span(2_400_000.0, span) {
                assert!(USABLE_FRACTION * 2.4e6 / f64::from(1u32 << k) >= PAN_MARGIN * span);
            }
        }
    }

    #[test]
    fn fft_size_follows_the_window_limit() {
        assert_eq!(zoom_fft_size(300_000.0, 4096), 4096);
        assert_eq!(zoom_fft_size(32_000.0, 4096), 2048);
        assert_eq!(zoom_fft_size(18_750.0, 4096), 2048);
        assert_eq!(zoom_fft_size(12_500.0, 4096), 1024);
        assert_eq!(zoom_fft_size(1_000.0, 4096), MIN_FFT);
        assert_eq!(zoom_fft_size(300_000.0, 1024), 1024);
    }

    #[test]
    fn rejects_bad_shift_and_rate() {
        let mut cfg = ZoomConfig {
            sample_rate: FS,
            shift: 0,
            max_fft: 4096,
            bins: 2048,
            fps: 25.0,
        };
        assert_eq!(ZoomSpectrum::new(&cfg).expect_err("0"), ZoomError::Shift);
        cfg.shift = MAX_SHIFT + 1;
        assert_eq!(ZoomSpectrum::new(&cfg).expect_err("big"), ZoomError::Shift);
        cfg.shift = 8; // 8 kHz < 12.5 kHz
        assert_eq!(ZoomSpectrum::new(&cfg).expect_err("rate"), ZoomError::Rate);
    }

    #[test]
    fn tone_lands_in_the_expected_bin_at_full_scale() {
        // Slice centred 100 kHz above the input centre, tone 1 kHz above the slice centre.
        let mut z = zoom(6);
        assert_eq!(z.out_rate(), 32_000.0);
        z.set_offset(100_000.0);
        let frame = last_frame(&mut z, &tones(&[(101_000.0, 1.0)], 200_000));
        assert_eq!(frame.len(), z.frame_bins());
        // Bin width = 32 kHz / 2048 = 15.625 Hz; DC bin sits at 1024 before trimming.
        let lo = 2048 / 10;
        let peak = argmax(&frame);
        assert_eq!(peak + lo, 1024 + 64);
        assert!(frame[peak] > -1.0, "{}", frame[peak]);
    }

    #[test]
    fn resolves_tones_the_full_band_fft_cannot_separate() {
        // 300 Hz apart: closer than the 500 Hz bins of the full-band FFT (2.048 MS/s / 4096).
        let parts = [(100_000.0 + 2_000.0, 1.0f32), (100_000.0 + 2_300.0, 1.0)];
        let input = tones(&parts, 300_000);

        let mut full = SpectrumProcessor::new(SpectrumConfig {
            fft_size: 4096,
            bins: 4096,
            fps: 25.0,
            sample_rate: FS,
        })
        .expect("full");
        let f = full.push(&input).pop().expect("frame");
        // +102.0 kHz and +102.3 kHz fall into the neighbouring bins 204 and 205 around DC.
        let (b1, b2) = (2048 + 204, 2048 + 205);
        let peak_full = f.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            f[b1].min(f[b2]) > peak_full - 3.0,
            "one merged lobe, no valley"
        );

        let mut z = zoom(6);
        z.set_offset(100_000.0);
        let frame = last_frame(&mut z, &input);
        // 15.625 Hz bins, DC at 1024 before the 204-bin trim: +2000 Hz = bin 948, +2300 Hz = 967.
        let (t1, t2) = (1024 + 128 - 204, 1024 + 147 - 204);
        let peak = frame.iter().copied().fold(f32::MIN, f32::max);
        assert!(peak > -1.0, "{peak}");
        assert!(
            frame[t1 - 1..=t1 + 1]
                .iter()
                .copied()
                .fold(f32::MIN, f32::max)
                > peak - 1.0
        );
        assert!(
            frame[t2 - 1..=t2 + 1]
                .iter()
                .copied()
                .fold(f32::MIN, f32::max)
                > peak - 1.0
        );
        let valley = frame[t1 + 6..=t2 - 6]
            .iter()
            .copied()
            .fold(f32::MIN, f32::max);
        assert!(valley < peak - 40.0, "valley {valley} vs peak {peak}");
    }

    #[test]
    fn strong_signal_outside_the_slice_is_rejected() {
        // 16 kHz output Nyquist at shift 7; a full-scale tone 60 kHz away must not alias in.
        let mut z = zoom(7);
        z.set_offset(0.0);
        let frame = last_frame(&mut z, &tones(&[(60_000.0, 1.0)], 400_000));
        let worst = frame.iter().copied().fold(f32::MIN, f32::max);
        assert!(worst < -80.0, "{worst}");
    }

    #[test]
    fn frames_describe_the_trimmed_band() {
        let z = zoom(5);
        assert_eq!(z.frame_bins(), 2048 - 2 * (2048 / 10));
        let expect = z.out_rate() * z.frame_bins() as f64 / 2048.0;
        assert!((z.usable_hz() - expect).abs() < 1e-9);
        assert!((z.usable_hz() / z.out_rate() - USABLE_FRACTION).abs() < 0.01);
    }
}
