//! ADC overload detection.
//!
//! An RTL-SDR has an 8-bit ADC. In a crowded band (FM broadcast) the tuner's automatic gain
//! often leaves the sum of all stations at or above full scale; the clipping then smears
//! intermodulation products over every channel, which costs stereo separation and wipes out RDS
//! (needs ~15 dB carrier-to-noise at 57 kHz). The monitor counts samples at full scale, logs a
//! sustained overload and publishes the result in [`OverloadStatus`], which goes into the
//! device state document so the UI can tell the user to lower the gain.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use superhet_device::Complex32;

/// Report interval.
const INTERVAL: Duration = Duration::from_secs(5);
/// A component at or beyond this magnitude counts as clipped (the 8-bit rails map to +-1.0).
const RAIL: f32 = 0.992;
/// Fraction of clipped components above which the front end counts as overloaded.
const OVERLOAD_FRACTION: f64 = 0.01;

/// Latest result of the monitor, shared with the control service.
#[derive(Debug, Default)]
pub struct OverloadStatus {
    overloaded: AtomicBool,
    /// Clipped share of the last interval in 1/1000.
    clipped_permille: AtomicU32,
}

impl OverloadStatus {
    /// Stores the result of an interval.
    pub fn set(&self, fraction: f64, overloaded: bool) {
        self.clipped_permille.store(
            (fraction * 1000.0).round().clamp(0.0, 1000.0) as u32,
            Ordering::Release,
        );
        self.overloaded.store(overloaded, Ordering::Release);
    }

    /// `(clipped share in percent with one decimal, overloaded)` of the last interval.
    #[must_use]
    pub fn get(&self) -> (f64, bool) {
        (
            f64::from(self.clipped_permille.load(Ordering::Acquire)) / 10.0,
            self.overloaded.load(Ordering::Acquire),
        )
    }
}

/// Counts samples at the ADC rails.
#[derive(Debug)]
pub struct OverloadMonitor {
    clipped: u64,
    total: u64,
    since: Instant,
    overloaded: bool,
}

impl Default for OverloadMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl OverloadMonitor {
    /// Creates a monitor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            clipped: 0,
            total: 0,
            since: Instant::now(),
            overloaded: false,
        }
    }

    /// Fraction of the components of `samples` at the rails.
    #[must_use]
    pub fn clipped_fraction(samples: &[Complex32]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let clipped: usize = samples
            .iter()
            .map(|s| usize::from(s.re.abs() >= RAIL) + usize::from(s.im.abs() >= RAIL))
            .sum();
        clipped as f64 / (2 * samples.len()) as f64
    }

    /// Accounts for a block; returns `(fraction, overloaded, was_overloaded)` when an interval
    /// ended.
    pub fn push(&mut self, samples: &[Complex32]) -> Option<(f64, bool, bool)> {
        let clipped = Self::clipped_fraction(samples) * (2 * samples.len()) as f64;
        self.clipped += clipped.round() as u64;
        self.total += 2 * samples.len() as u64;
        if self.since.elapsed() < INTERVAL || self.total == 0 {
            return None;
        }
        let fraction = self.clipped as f64 / self.total as f64;
        self.clipped = 0;
        self.total = 0;
        self.since = Instant::now();
        let was = self.overloaded;
        self.overloaded = fraction >= OVERLOAD_FRACTION;
        Some((fraction, self.overloaded, was))
    }

    /// Accounts for a block, publishes the result of a finished interval to `status` and logs
    /// when the overload starts or ends.
    pub fn observe(&mut self, samples: &[Complex32], status: &OverloadStatus) {
        let Some((fraction, overloaded, was)) = self.push(samples) else {
            return;
        };
        status.set(fraction, overloaded);
        let clipped_percent = format!("{:.1}", fraction * 100.0);
        match (overloaded, was) {
            (true, false) => tracing::warn!(
                clipped_percent,
                "ADC overload: the front end clips; lower the tuner gain (RDS and stereo suffer)"
            ),
            (false, true) => tracing::info!(clipped_percent, "ADC overload ended"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(clip_every: usize) -> Vec<Complex32> {
        (0..1000)
            .map(|i| {
                if i % clip_every == 0 {
                    Complex32::new(1.0, -1.0)
                } else {
                    Complex32::new(0.1, 0.2)
                }
            })
            .collect()
    }

    #[test]
    fn counts_components_at_the_rails() {
        assert_eq!(OverloadMonitor::clipped_fraction(&[]), 0.0);
        assert!((OverloadMonitor::clipped_fraction(&block(10)) - 0.1).abs() < 1e-9);
        assert!(OverloadMonitor::clipped_fraction(&block(100_000)) > 0.0);
        assert_eq!(
            OverloadMonitor::clipped_fraction(&[Complex32::new(0.99, -0.5)]),
            0.0
        );
    }

    #[test]
    fn reports_every_interval_and_tracks_the_state() {
        let mut m = OverloadMonitor::new();
        assert_eq!(m.push(&block(10)), None, "interval not over yet");
        m.since -= INTERVAL;
        let (fraction, overloaded, was) = m.push(&block(10)).expect("report");
        assert!(overloaded && !was && (fraction - 0.1).abs() < 1e-9);
        assert_eq!(m.push(&block(10)), None, "next interval still running");
        (m.clipped, m.total) = (0, 0);
        m.since -= INTERVAL;
        let (_, overloaded, was) = m.push(&block(100_000)).expect("report");
        assert!(!overloaded && was);
        m.since -= INTERVAL;
        let (_, overloaded, was) = m.push(&block(100_000)).expect("quiet intervals report too");
        assert!(!overloaded && !was);
    }

    #[test]
    fn status_keeps_the_last_interval() {
        let status = OverloadStatus::default();
        assert_eq!(status.get(), (0.0, false));
        status.set(0.0734, true);
        assert_eq!(status.get(), (7.3, true));
        status.set(2.0, true);
        assert_eq!(status.get(), (100.0, true));
    }
}
