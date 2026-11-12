//! Scan plan: the channel list, its grouping into windows (one device centre each) and the
//! power measurement on a spectrum frame.
//!
//! Everything here is pure: the machine decides, the runtime executes.

use crate::vfo::VfoMode;
use crate::window::USABLE_FRACTION;

/// Hz between the carrier side of an SSB channel and the start of its passband.
const SSB_LOW_CUT_HZ: f64 = 300.0;
/// Percentile of the bins that estimates the noise floor.
const NOISE_PERCENTILE: f64 = 0.30;

/// One channel to watch.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    /// Tuning frequency of the VFO, Hz.
    pub freq_hz: u64,
    /// Demodulation mode the VFO takes when the scanner stops here.
    pub mode: VfoMode,
    /// Explicit bandwidth (VFO bandwidth when stopped); `None` = the mode's default.
    pub bandwidth_hz: Option<u32>,
    /// Width used for the measurement when it differs from the VFO bandwidth (range scans cap
    /// it at the step so neighbours do not bleed into each other).
    pub detect_hz: Option<u32>,
    /// Display name.
    pub label: Option<String>,
    /// The priority channel (checked at an interval and wins over a normal hit).
    pub priority: bool,
}

/// Default bandwidth of a mode, Hz (same table as the VFOs, `superhet-dsp`).
#[must_use]
pub fn mode_default_bandwidth(mode: VfoMode) -> u32 {
    match mode {
        VfoMode::Wfm => 180_000,
        VfoMode::Nfm => 12_500,
        VfoMode::Am => 10_000,
        VfoMode::Usb | VfoMode::Lsb => 2_400,
        VfoMode::Cw => 500,
    }
}

impl Channel {
    /// Effective bandwidth of the VFO.
    #[must_use]
    pub fn vfo_bandwidth_hz(&self) -> u32 {
        self.bandwidth_hz
            .unwrap_or_else(|| mode_default_bandwidth(self.mode))
    }

    /// Lower and upper edge of the part of the spectrum to measure, Hz.
    #[must_use]
    pub fn span_hz(&self) -> (f64, f64) {
        let f = self.freq_hz as f64;
        let width = f64::from(self.detect_hz.unwrap_or_else(|| self.vfo_bandwidth_hz()));
        match self.mode {
            VfoMode::Usb => (f + SSB_LOW_CUT_HZ, f + SSB_LOW_CUT_HZ + width),
            VfoMode::Lsb => (f - SSB_LOW_CUT_HZ - width, f - SSB_LOW_CUT_HZ),
            _ => (f - width / 2.0, f + width / 2.0),
        }
    }
}

/// Channels that share one device centre; scanned in one go.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Device centre for this window.
    pub center_hz: u64,
    /// Indices into the channel list, ascending frequency.
    pub channels: Vec<usize>,
    /// Lowest edge of the member channels, Hz.
    pub low_hz: u64,
    /// Highest edge of the member channels, Hz.
    pub high_hz: u64,
}

/// A plan that cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// A channel is wider than the usable window of the sample rate.
    #[error("channel {freq_hz} Hz is wider than the usable window ({usable_hz} Hz)")]
    TooWide {
        /// The channel's frequency.
        freq_hz: u64,
        /// Width of the usable window.
        usable_hz: u64,
    },
}

/// Group `channels` into windows for a device at `home_center_hz` with `sample_rate`.
///
/// Channels that fit the current window form the first segment, which keeps the current centre
/// (a scan inside the visible span never retunes); the rest are grouped greedily in ascending
/// frequency, each group centred on its extent. Locked-out channels are still planned; the
/// machine skips them.
///
/// # Errors
/// [`PlanError::TooWide`] for a channel the window cannot hold.
pub fn plan(
    channels: &[Channel],
    home_center_hz: u64,
    sample_rate: u32,
) -> Result<Vec<Segment>, PlanError> {
    let usable = f64::from(sample_rate) * USABLE_FRACTION;
    let spans: Vec<(f64, f64)> = channels.iter().map(Channel::span_hz).collect();
    for (channel, (lo, hi)) in channels.iter().zip(&spans) {
        if hi - lo > usable {
            return Err(PlanError::TooWide {
                freq_hz: channel.freq_hz,
                usable_hz: usable as u64,
            });
        }
    }
    let mut order: Vec<usize> = (0..channels.len()).collect();
    order.sort_by_key(|&i| channels[i].freq_hz);

    let home_lo = home_center_hz as f64 - usable / 2.0;
    let home_hi = home_center_hz as f64 + usable / 2.0;
    let (home, rest): (Vec<usize>, Vec<usize>) = order
        .into_iter()
        .partition(|&i| spans[i].0 >= home_lo && spans[i].1 <= home_hi);

    let mut segments = Vec::new();
    if !home.is_empty() {
        segments.push(segment(&home, home_center_hz, &spans));
    }
    let mut group: Vec<usize> = Vec::new();
    let (mut lo, mut hi) = (0.0_f64, 0.0_f64);
    for i in rest {
        let (clo, chi) = spans[i];
        if !group.is_empty() && chi.max(hi) - clo.min(lo) > usable {
            segments.push(grouped(&group, &spans));
            group.clear();
        }
        if group.is_empty() {
            (lo, hi) = (clo, chi);
        } else {
            lo = lo.min(clo);
            hi = hi.max(chi);
        }
        group.push(i);
    }
    if !group.is_empty() {
        segments.push(grouped(&group, &spans));
    }
    Ok(segments)
}

fn grouped(group: &[usize], spans: &[(f64, f64)]) -> Segment {
    let lo = group.iter().map(|&i| spans[i].0).fold(f64::MAX, f64::min);
    let hi = group.iter().map(|&i| spans[i].1).fold(f64::MIN, f64::max);
    let center = ((lo + hi) / 2.0).round().max(1.0) as u64;
    segment(group, center, spans)
}

fn segment(group: &[usize], center_hz: u64, spans: &[(f64, f64)]) -> Segment {
    let low = group.iter().map(|&i| spans[i].0).fold(f64::MAX, f64::min);
    let high = group.iter().map(|&i| spans[i].1).fold(f64::MIN, f64::max);
    Segment {
        center_hz,
        channels: group.to_vec(),
        low_hz: low.max(0.0) as u64,
        high_hz: high.max(0.0) as u64,
    }
}

/// A spectrum frame as the scanner sees it: dBFS per bin, low to high frequency.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Increases with every frame the probe took.
    pub seq: u64,
    /// Device centre the frame was captured at, Hz.
    pub center_hz: u64,
    /// Sample rate, Hz.
    pub sample_rate: u32,
    /// Power per bin, dBFS.
    pub db: Vec<f32>,
}

/// Power of one channel relative to the noise floor of a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    /// Mean power over the channel, dBFS.
    pub dbfs: f32,
    /// `dbfs` minus the frame's noise floor, dB.
    pub margin_db: f32,
}

/// Measures channels on one frame; the noise floor is computed once.
pub struct Measure<'a> {
    frame: &'a Frame,
    floor_db: f32,
}

impl<'a> Measure<'a> {
    /// Prepare `frame` (noise floor: low percentile of the bins in the usable window).
    #[must_use]
    pub fn new(frame: &'a Frame) -> Self {
        let n = frame.db.len();
        let (a, b) = usable_bins(n);
        let mut copy: Vec<f32> = frame.db[a..b]
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .collect();
        let floor_db = if copy.is_empty() {
            -200.0
        } else {
            let k = ((copy.len() - 1) as f64 * NOISE_PERCENTILE) as usize;
            let (_, v, _) = copy.select_nth_unstable_by(k, f32::total_cmp);
            *v
        };
        Self { frame, floor_db }
    }

    /// Noise floor estimate, dBFS.
    #[cfg(test)]
    #[must_use]
    pub fn floor_db(&self) -> f32 {
        self.floor_db
    }

    /// Level of `channel`, or `None` if it is not inside the usable window of this frame.
    #[must_use]
    pub fn level(&self, channel: &Channel) -> Option<Level> {
        let n = self.frame.db.len();
        if n == 0 || self.frame.sample_rate == 0 {
            return None;
        }
        let rate = f64::from(self.frame.sample_rate);
        let bin_hz = rate / n as f64;
        let start = self.frame.center_hz as f64 - rate / 2.0;
        let (lo, hi) = channel.span_hz();
        let usable = rate * USABLE_FRACTION;
        let center = self.frame.center_hz as f64;
        if lo < center - usable / 2.0 - 1.0 || hi > center + usable / 2.0 + 1.0 {
            return None;
        }
        let first = (((lo - start) / bin_hz).floor().max(0.0) as usize).min(n - 1);
        let last = (((hi - start) / bin_hz).ceil() as usize)
            .saturating_sub(1)
            .clamp(first, n - 1);
        let mut sum = 0.0_f64;
        for v in &self.frame.db[first..=last] {
            sum += 10.0_f64.powf(f64::from(*v) / 10.0);
        }
        let dbfs = (10.0 * (sum / (last - first + 1) as f64).log10()) as f32;
        Some(Level {
            dbfs,
            margin_db: dbfs - self.floor_db,
        })
    }
}

/// Bin range `a..b` of the usable 85 % in the middle of a frame of `n` bins.
fn usable_bins(n: usize) -> (usize, usize) {
    let margin = (((1.0 - USABLE_FRACTION) / 2.0 * n as f64) as usize).min(n / 2);
    (margin, n - margin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nfm(freq_hz: u64) -> Channel {
        Channel {
            freq_hz,
            mode: VfoMode::Nfm,
            bandwidth_hz: None,
            detect_hz: None,
            label: None,
            priority: false,
        }
    }

    #[test]
    fn channels_inside_the_current_window_keep_the_centre() {
        // 2.4 MS/s: usable 2.04 MHz, so 144.1..145.9 around 145 MHz fits.
        let channels = [nfm(145_500_000), nfm(144_100_000), nfm(145_900_000)];
        let plan = plan(&channels, 145_000_000, 2_400_000).expect("plan");
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].center_hz, 145_000_000);
        assert_eq!(plan[0].channels, vec![1, 0, 2]);
    }

    #[test]
    fn far_channels_get_their_own_windows() {
        let channels = [
            nfm(145_500_000),
            nfm(433_500_000),
            nfm(433_600_000),
            nfm(446_006_250),
        ];
        let plan = plan(&channels, 145_000_000, 2_400_000).expect("plan");
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].center_hz, 145_000_000);
        assert_eq!(plan[0].channels, vec![0]);
        // 433.5 and 433.6 share a window centred on their extent, 446 MHz needs another one.
        assert_eq!(plan[1].channels, vec![1, 2]);
        assert!((433_500_000..=433_700_000).contains(&plan[1].center_hz));
        assert_eq!(plan[2].channels, vec![3]);
    }

    #[test]
    fn a_channel_wider_than_the_window_is_refused() {
        let wide = Channel {
            mode: VfoMode::Wfm,
            ..nfm(100_000_000)
        };
        let err = plan(&[wide], 100_000_000, 200_000).expect_err("too wide");
        assert!(matches!(err, PlanError::TooWide { .. }));
    }

    #[test]
    fn ssb_spans_start_above_or_below_the_carrier() {
        let usb = Channel {
            mode: VfoMode::Usb,
            ..nfm(14_200_000)
        };
        let lsb = Channel {
            mode: VfoMode::Lsb,
            ..nfm(7_100_000)
        };
        assert_eq!(usb.span_hz(), (14_200_300.0, 14_202_700.0));
        assert_eq!(lsb.span_hz(), (7_097_300.0, 7_099_700.0));
    }

    #[test]
    fn level_is_relative_to_the_noise_floor() {
        let mut frame = Frame {
            seq: 1,
            center_hz: 100_000_000,
            sample_rate: 1_000_000,
            db: vec![-80.0; 1000],
        };
        // 1 kHz per bin starting at 99.5 MHz; a 12.5 kHz channel at +100 kHz is 594..=606.
        frame.db[594..=606].fill(-60.0);
        let measure = Measure::new(&frame);
        assert!((measure.floor_db() + 80.0).abs() < 0.01);
        let on = measure.level(&nfm(100_100_000)).expect("inside");
        assert!((on.margin_db - 20.0).abs() < 1.5, "{on:?}");
        let off = measure.level(&nfm(100_200_000)).expect("inside");
        assert!(off.margin_db.abs() < 0.01);
        // Outside the usable 85 %: no measurement.
        assert!(measure.level(&nfm(100_460_000)).is_none());
        assert!(measure.level(&nfm(99_000_000)).is_none());
    }
}
