//! Zoom spectrum on the DSP thread: a second FFT over a narrow slice of the band.
//!
//! The gateway leases a slice with `ctl.spectrum.zoom` (see `superhet_bus::zoom`); this module
//! keeps the request in a shared slot, plans the slice (decimation exponent and centre) and runs
//! a [`ZoomSpectrum`] on every IQ block while the lease is alive. Frames go out on
//! `sh.v1.dev.<dev>.spectrum.zoom` in the regular spectrum wire format; each header describes
//! exactly the (trimmed) slice its bins cover.
//!
//! Panning inside the current slice does not touch the processor: the slice is only replaced when
//! the requested view leaves it or needs a different decimation, so a drag keeps the frame rate.
//! There is one slice per device (the latest request wins); other clients fall back to the
//! full-band spectrum, see `docs/adr/0009-zoom-fft.md`.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use superhet_bus::spectrum::SpectrumFrame;
use superhet_bus::zoom::{LEASE, ZoomCommand};
use superhet_device::Complex32;
use superhet_dsp::zoom::{ZoomConfig, ZoomSpectrum};

use crate::config::SpectrumSettings;

/// Shared slot between the bus loop (writer) and the DSP thread (reader).
#[derive(Debug, Clone, Default)]
pub struct ZoomHandle {
    slot: Arc<Mutex<Option<(ZoomCommand, Instant)>>>,
}

impl ZoomHandle {
    /// Store a request; [`ZoomCommand::Off`] clears the slot.
    pub fn set(&self, command: ZoomCommand) {
        self.set_at(command, Instant::now());
    }

    fn set_at(&self, command: ZoomCommand, now: Instant) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        *slot = match command {
            ZoomCommand::Off => None,
            view @ ZoomCommand::View { .. } => Some((view, now)),
        };
    }

    /// The live request: `(center_hz, span_hz)`, unless cleared or older than the lease.
    fn current(&self, now: Instant) -> Option<(u64, u32)> {
        let slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        match *slot {
            Some((ZoomCommand::View { center_hz, span_hz }, at))
                if now.saturating_duration_since(at) <= LEASE =>
            {
                Some((center_hz, span_hz))
            }
            _ => None,
        }
    }
}

/// The slice that is being computed.
struct Slice {
    zoom: ZoomSpectrum,
    shift: u32,
    /// Centre of the slice in Hz (absolute).
    center_hz: f64,
    /// Lowest and highest centre that keeps the slice inside the sampled band.
    reach: (f64, f64),
    /// Block centre and rate the slice was planned for.
    block_center: u64,
    block_rate: u32,
}

impl Slice {
    /// True when the slice already serves the view `center ± span / 2`: it lies within the
    /// trimmed band, or the slice sits at the band limit nearest to it (it cannot move closer).
    fn holds(&self, center_hz: f64, span_hz: f64) -> bool {
        let target = center_hz.clamp(self.reach.0, self.reach.1);
        if (target - self.center_hz).abs() < 0.5 {
            return true;
        }
        let half = (self.zoom.usable_hz() - span_hz) / 2.0;
        half >= 0.0 && (center_hz - self.center_hz).abs() <= half
    }
}

/// Runs the zoom spectrum for the DSP thread.
pub struct ZoomEngine {
    handle: ZoomHandle,
    settings: SpectrumSettings,
    slice: Option<Slice>,
    sink: Box<dyn FnMut(Vec<u8>) + Send>,
}

impl ZoomEngine {
    /// Create an engine that follows `handle` and hands encoded frames to `sink`.
    pub fn new(
        handle: ZoomHandle,
        settings: SpectrumSettings,
        sink: impl FnMut(Vec<u8>) + Send + 'static,
    ) -> Self {
        Self {
            handle,
            settings,
            slice: None,
            sink: Box::new(sink),
        }
    }

    /// Forget the slice (call after a retune: the old filter and FFT state is meaningless).
    pub fn reset(&mut self) {
        self.slice = None;
    }

    /// True while a slice is being computed.
    #[cfg(test)]
    pub fn active(&self) -> bool {
        self.slice.is_some()
    }

    /// Process one IQ block captured at `block_center` / `block_rate`.
    pub fn process(&mut self, samples: &[Complex32], block_center: u64, block_rate: u32) {
        let Some((view_center, span)) = self.handle.current(Instant::now()) else {
            self.slice = None;
            return;
        };
        if self
            .slice
            .as_ref()
            .is_some_and(|s| s.block_center != block_center || s.block_rate != block_rate)
        {
            self.slice = None;
        }
        let keep = self.slice.as_ref().is_some_and(|s| {
            superhet_dsp::zoom::shift_for_span(f64::from(block_rate), f64::from(span))
                == Some(s.shift)
                && s.holds(view_center as f64, f64::from(span))
        });
        if !keep {
            self.slice = self.plan(view_center, span, block_center, block_rate);
        }
        let Some(slice) = &mut self.slice else { return };
        let frames = slice.zoom.push(samples);
        if frames.is_empty() {
            return;
        }
        let width = slice.zoom.usable_hz().round().max(1.0);
        let center = slice.center_hz.round().max(0.0);
        for db in frames {
            let frame = SpectrumFrame::quantize(
                center as u64,
                width as u32,
                now_us(),
                self.settings.min_db,
                self.settings.max_db,
                &db,
            );
            (self.sink)(frame.encode());
        }
    }

    /// Plan a slice for the view; `None` when the full-band spectrum is good enough, the view
    /// is outside the band, or the processor cannot be built.
    fn plan(
        &self,
        view_center: u64,
        span: u32,
        block_center: u64,
        block_rate: u32,
    ) -> Option<Slice> {
        let rate = f64::from(block_rate);
        let shift = superhet_dsp::zoom::shift_for_span(rate, f64::from(span))?;
        let mut zoom = ZoomSpectrum::new(&ZoomConfig {
            sample_rate: block_rate,
            shift,
            max_fft: self.settings.fft_size,
            bins: self.settings.bins,
            fps: self.settings.fps,
        })
        .ok()?;
        // The slice must stay inside the sampled band, or it would show the far edge wrapped.
        let reach = (rate - zoom.usable_hz()) / 2.0;
        let band_center = block_center as f64;
        let wanted = view_center as f64;
        if (wanted - band_center).abs() - f64::from(span) / 2.0 > rate / 2.0 {
            return None;
        }
        let center = wanted.clamp(band_center - reach, band_center + reach);
        zoom.set_offset(center - band_center);
        Some(Slice {
            zoom,
            shift,
            center_hz: center,
            reach: (band_center - reach, band_center + reach),
            block_center,
            block_rate,
        })
    }
}

fn now_us() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use std::sync::mpsc::{Receiver, channel};

    const RATE: u32 = 2_048_000;
    const CENTER: u64 = 100_000_000;

    fn settings() -> SpectrumSettings {
        SpectrumSettings {
            fft_size: 4096,
            bins: 2048,
            fps: 25.0,
            min_db: -120.0,
            max_db: 0.0,
        }
    }

    fn engine() -> (ZoomHandle, ZoomEngine, Receiver<Vec<u8>>) {
        let handle = ZoomHandle::default();
        let (tx, rx) = channel();
        let engine = ZoomEngine::new(handle.clone(), settings(), move |f| {
            let _ = tx.send(f);
        });
        (handle, engine, rx)
    }

    /// Complex tone at `offset_hz` from the block centre, `len` samples starting at sample `at`.
    fn tone(offset_hz: f64, at: usize, len: usize) -> Vec<Complex32> {
        (at..at + len)
            .map(|i| {
                let ph = std::f64::consts::TAU * offset_hz * i as f64 / f64::from(RATE);
                Complex32::new(ph.cos() as f32, ph.sin() as f32)
            })
            .collect()
    }

    /// Feed `blocks` blocks of 16384 samples and return the frames produced.
    fn run(
        engine: &mut ZoomEngine,
        rx: &Receiver<Vec<u8>>,
        offset_hz: f64,
        blocks: usize,
    ) -> Vec<SpectrumFrame> {
        for b in 0..blocks {
            engine.process(&tone(offset_hz, b * 16384, 16384), CENTER, RATE);
        }
        rx.try_iter()
            .map(|f| SpectrumFrame::decode(&f).expect("frame"))
            .collect()
    }

    fn view(center_hz: u64, span_hz: u32) -> ZoomCommand {
        ZoomCommand::View { center_hz, span_hz }
    }

    #[test]
    fn idle_without_request_or_when_full_band_is_enough() {
        let (handle, mut engine, rx) = engine();
        assert!(run(&mut engine, &rx, 5_000.0, 4).is_empty());
        assert!(!engine.active());
        // A span of 1.5 MHz of 2.048 MS/s needs no zoom.
        handle.set(view(CENTER, 1_500_000));
        assert!(run(&mut engine, &rx, 5_000.0, 4).is_empty());
        assert!(!engine.active());
    }

    #[test]
    fn frames_describe_the_slice_and_put_the_tone_in_the_right_place() {
        let (handle, mut engine, rx) = engine();
        // View 20 kHz wide around centre + 50 kHz; the tone sits 3 kHz above the view centre.
        handle.set(view(CENTER + 50_000, 20_000));
        let frames = run(&mut engine, &rx, 53_000.0, 40);
        let frame = frames.last().expect("zoom frame");
        assert_eq!(frame.center_hz, CENTER + 50_000);
        // Usable band is about 80 % of 2.048 MS/s / 2^6 = 32 kHz and holds the 20 kHz view.
        assert!(
            frame.sample_rate >= 23_000 && frame.sample_rate <= 26_000,
            "{}",
            frame.sample_rate
        );
        let n = frame.bins.len();
        let peak = frame
            .bins
            .iter()
            .enumerate()
            .max_by_key(|(_, q)| **q)
            .map(|(i, _)| i)
            .expect("bins");
        let hz_per_bin = f64::from(frame.sample_rate) / n as f64;
        let peak_hz = (peak as f64 + 0.5) * hz_per_bin - f64::from(frame.sample_rate) / 2.0;
        assert!(
            (peak_hz - 3_000.0).abs() < 2.0 * hz_per_bin,
            "peak at {peak_hz} Hz"
        );
        assert!(hz_per_bin < 20.0, "resolution {hz_per_bin} Hz per bin");
        assert!(frame.db(peak).expect("db") > -3.0);
    }

    #[test]
    fn panning_inside_the_slice_keeps_the_processor() {
        let (handle, mut engine, rx) = engine();
        handle.set(view(CENTER, 10_000));
        let first = run(&mut engine, &rx, 0.0, 40);
        assert!(!first.is_empty());
        let before = first.last().expect("frame").center_hz;
        // A small pan stays within the slice: same centre in the headers.
        handle.set(view(CENTER + 1_000, 10_000));
        let moved = run(&mut engine, &rx, 0.0, 40);
        assert!(moved.iter().all(|f| f.center_hz == before));
        // A large pan or a different span re-plans the slice.
        handle.set(view(CENTER + 200_000, 10_000));
        let far = run(&mut engine, &rx, 0.0, 40);
        assert_eq!(far.last().expect("frame").center_hz, CENTER + 200_000);
        handle.set(view(CENTER + 200_000, 100_000));
        let wide = run(&mut engine, &rx, 0.0, 40);
        assert!(wide.last().expect("frame").sample_rate > 50_000);
    }

    #[test]
    fn slice_is_kept_inside_the_sampled_band() {
        let (handle, mut engine, rx) = engine();
        // View centre at the band edge: the slice shifts inwards so that it never wraps.
        handle.set(view(CENTER + 1_020_000, 10_000));
        let frames = run(&mut engine, &rx, 1_020_000.0, 40);
        let frame = frames.last().expect("frame");
        let upper = frame.center_hz as f64 + f64::from(frame.sample_rate) / 2.0;
        assert!(
            upper <= (CENTER + u64::from(RATE) / 2) as f64 + 1.0,
            "{upper}"
        );
        // The view is no longer inside the slice: the gateway will fall back to the full band.
        assert!(frame.center_hz < CENTER + 1_020_000);
    }

    #[test]
    fn lease_expiry_off_and_retune_stop_the_slice() {
        let (handle, mut engine, rx) = engine();
        let t0 = Instant::now();
        handle.set_at(view(CENTER, 10_000), t0);
        assert_eq!(
            handle.current(t0 + Duration::from_secs(5)),
            Some((CENTER, 10_000))
        );
        assert_eq!(handle.current(t0 + LEASE + Duration::from_secs(1)), None);

        handle.set(view(CENTER, 10_000));
        assert!(!run(&mut engine, &rx, 0.0, 40).is_empty());
        handle.set(ZoomCommand::Off);
        assert!(run(&mut engine, &rx, 0.0, 40).is_empty());
        assert!(!engine.active());

        handle.set(view(CENTER, 10_000));
        assert!(!run(&mut engine, &rx, 0.0, 40).is_empty());
        // A different block centre (retune) discards the slice; it is re-planned for the new
        // band, where the requested view is outside and nothing is produced.
        engine.process(&tone(0.0, 0, 16384), CENTER + 5_000_000, RATE);
        assert!(!engine.active());
    }
}
