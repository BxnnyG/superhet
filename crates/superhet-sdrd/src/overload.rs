//! ADC overload detection.
//!
//! An RTL-SDR has an 8-bit ADC. In a crowded band (FM broadcast) the tuner's automatic gain
//! often leaves the sum of all stations at or above full scale; the clipping then smears
//! intermodulation products over every channel, which costs stereo separation and wipes out RDS
//! (needs ~15 dB carrier-to-noise at 57 kHz). The monitor counts samples at full scale and
//! reports a sustained overload once per interval, with a hint at the cause.

use std::time::{Duration, Instant};

use superhet_device::Complex32;

/// Report interval.
const INTERVAL: Duration = Duration::from_secs(10);
/// A component at or beyond this magnitude counts as clipped (the 8-bit rails map to +-1.0).
const RAIL: f32 = 0.992;
/// Fraction of clipped components above which the front end counts as overloaded.
const OVERLOAD_FRACTION: f64 = 0.01;

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

    /// Accounts for a block; returns the clipped fraction when an interval ended and the
    /// overload state changed or persists (`Some((fraction, overloaded))`).
    pub fn push(&mut self, samples: &[Complex32]) -> Option<(f64, bool)> {
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
        let overloaded = fraction >= OVERLOAD_FRACTION;
        let report = overloaded || self.overloaded;
        self.overloaded = overloaded;
        report.then_some((fraction, overloaded))
    }

    /// Accounts for a block and logs a sustained overload (and its end).
    pub fn observe(&mut self, samples: &[Complex32]) {
        match self.push(samples) {
            Some((fraction, true)) => tracing::warn!(
                clipped_percent = format!("{:.1}", fraction * 100.0),
                "ADC overload: the front end clips; lower the tuner gain (RDS and stereo suffer)"
            ),
            Some((fraction, false)) => tracing::info!(
                clipped_percent = format!("{:.1}", fraction * 100.0),
                "ADC overload ended"
            ),
            None => {}
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
    fn reports_a_sustained_overload_once_per_interval_and_its_end() {
        let mut m = OverloadMonitor::new();
        assert_eq!(m.push(&block(10)), None, "interval not over yet");
        m.since -= INTERVAL;
        let (fraction, overloaded) = m.push(&block(10)).expect("report");
        assert!(overloaded && (fraction - 0.1).abs() < 1e-9);
        assert_eq!(m.push(&block(10)), None, "next interval still running");
        (m.clipped, m.total) = (0, 0);
        m.since -= INTERVAL;
        assert_eq!(m.push(&block(100_000)).map(|r| r.1), Some(false));
        m.since -= INTERVAL;
        assert_eq!(m.push(&block(100_000)), None, "quiet again, nothing to say");
    }
}
