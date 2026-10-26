//! RDS reception quality: block error rate over a sliding window.
//!
//! The decoder counts valid and failed blocks while its block sync holds. The cumulative
//! counts say little about the present reception (a station that came in strongly ten minutes
//! ago would hide today's drop-outs), so the monitor keeps a short history of the counters and
//! reports the rate over roughly the last [`WINDOW`].

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Length of the sliding window.
const WINDOW: Duration = Duration::from_secs(10);
/// Spacing of the history samples.
const STEP: Duration = Duration::from_millis(500);
/// Blocks needed in the window before a rate is reported (about 0.4 s of data).
const MIN_BLOCKS: u64 = 10;

/// Quality of the last [`WINDOW`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    /// Failed blocks in percent of the blocks seen while in sync, 0..=100.
    pub bler: u8,
    /// 100 minus `bler`; 0 while the block sync is lost.
    pub quality: u8,
}

/// Sliding-window block error rate.
#[derive(Debug, Default)]
pub struct RdsQuality {
    /// `(time, good, bad)` of the cumulative counters, oldest first.
    history: VecDeque<(Instant, u64, u64)>,
}

impl RdsQuality {
    /// Forgets the history (new channel).
    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Records the cumulative `(good, bad)` block counts at `now` and returns the quality of
    /// the window, or `None` while too few blocks were seen in it.
    pub fn observe(
        &mut self,
        now: Instant,
        (good, bad): (u64, u64),
        synced: bool,
    ) -> Option<Quality> {
        // The decoder was reset (retune): the counters start over.
        if self
            .history
            .back()
            .is_some_and(|&(_, g, b)| good < g || bad < b)
        {
            self.history.clear();
        }
        if self
            .history
            .back()
            .is_none_or(|&(t, ..)| now.saturating_duration_since(t) >= STEP)
        {
            self.history.push_back((now, good, bad));
        }
        while self
            .history
            .front()
            .is_some_and(|&(t, ..)| now.saturating_duration_since(t) > WINDOW)
        {
            self.history.pop_front();
        }
        let &(_, g0, b0) = self.history.front()?;
        let (dg, db) = (good - g0, bad - b0);
        if dg + db < MIN_BLOCKS {
            return None;
        }
        let bler = ((db * 100 + (dg + db) / 2) / (dg + db)).min(100) as u8;
        Some(Quality {
            bler,
            quality: if synced { 100 - bler } else { 0 },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn needs_a_few_blocks_first() {
        let base = Instant::now();
        let mut q = RdsQuality::default();
        assert_eq!(q.observe(base, (0, 0), true), None);
        assert_eq!(q.observe(at(base, 1000), (5, 0), true), None);
    }

    #[test]
    fn reports_the_rate_of_the_window_not_of_all_time() {
        let base = Instant::now();
        let mut q = RdsQuality::default();
        let (mut good, mut bad) = (0, 0);
        // A minute of a clean signal (~47 blocks per second)...
        for step in 0..=120 {
            good += 23;
            q.observe(at(base, step * 500), (good, bad), true);
        }
        // ... then 12 s in which every second block fails.
        let mut last = None;
        for step in 121..=145 {
            good += 12;
            bad += 11;
            last = q.observe(at(base, step * 500), (good, bad), true);
        }
        let last = last.expect("quality");
        assert!((45..=55).contains(&last.bler), "{last:?}");
        assert_eq!(u32::from(last.bler) + u32::from(last.quality), 100);
    }

    #[test]
    fn quality_is_zero_without_sync_and_the_window_runs_dry() {
        let base = Instant::now();
        let mut q = RdsQuality::default();
        q.observe(base, (0, 0), true);
        let seen = q.observe(at(base, 2000), (40, 10), false).expect("quality");
        assert_eq!((seen.bler, seen.quality), (20, 0));
        // No more blocks (sync lost): after the window nothing is left to report.
        let mut last = Some(seen);
        for step in 5..=40 {
            last = q.observe(at(base, step * 500), (40, 10), false);
        }
        assert_eq!(last, None);
    }

    #[test]
    fn a_reset_starts_over() {
        let base = Instant::now();
        let mut q = RdsQuality::default();
        q.observe(base, (0, 0), true);
        assert!(q.observe(at(base, 1000), (100, 0), true).is_some());
        assert_eq!(q.observe(at(base, 1500), (3, 1), true), None);
    }
}
