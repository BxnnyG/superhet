//! The usable window of a tuning and the free-tuning rule (`docs/ARCHITECTURE.md` section 3.1).
//!
//! Pure functions: given a VFO that was just created or moved and the current tuning, decide
//! whether and where the centre has to follow. Applying the result (and the tuning lock) is the
//! controller's business.

use crate::control::MAX_CENTER_HZ;
use crate::vfo::{Tuning, Vfo};

/// Share of the sample rate that is usable; the rest is the anti-alias roll-off at the edges.
pub const USABLE_FRACTION: f64 = 0.85;

/// Width of the usable window in Hz.
#[must_use]
pub fn usable_hz(sample_rate: u32) -> f64 {
    f64::from(sample_rate) * USABLE_FRACTION
}

/// Edges of the usable window in Hz (the lower edge may be negative for very low centres).
#[must_use]
pub fn bounds(tuning: Tuning) -> (i64, i64) {
    let half = usable_hz(tuning.sample_rate) / 2.0;
    let center = tuning.center_hz as f64;
    (
        (center - half).round() as i64,
        (center + half).round() as i64,
    )
}

/// Whether the whole channel of `vfo` lies inside the usable window.
#[must_use]
pub fn fits(vfo: &Vfo, tuning: Tuning) -> bool {
    vfo.offset_hz(tuning.center_hz).abs() + f64::from(vfo.bandwidth()) / 2.0
        <= usable_hz(tuning.sample_rate) / 2.0
}

/// The centre the device should move to so that `active` (the VFO the user just created or
/// moved) is inside the window again, or `None` if it already is or cannot fit at all.
///
/// The target puts `active` a quarter of the window away from the centre on the side it came
/// from (off DC, with room to continue in the same direction). "Fit" mode: the other VFOs that
/// are inside the window now are kept inside if all of them plus `active` fit into the window
/// together; the target centre is then moved just far enough. Otherwise `active` wins.
#[must_use]
pub fn retune_center(active: &Vfo, others: &[Vfo], tuning: Tuning) -> Option<u64> {
    if fits(active, tuning) {
        return None;
    }
    let usable = usable_hz(tuning.sample_rate);
    let half_bw = f64::from(active.bandwidth()) / 2.0;
    if half_bw * 2.0 > usable {
        return None;
    }
    let freq = active.freq_hz as f64;
    let offset = (usable / 4.0).min(usable / 2.0 - half_bw);
    let mut desired = if active.freq_hz > tuning.center_hz {
        freq - offset
    } else {
        freq + offset
    };
    let (mut low, mut high) = (freq - half_bw, freq + half_bw);
    for other in others.iter().filter(|o| fits(o, tuning)) {
        let half = f64::from(other.bandwidth()) / 2.0;
        low = low.min(other.freq_hz as f64 - half);
        high = high.max(other.freq_hz as f64 + half);
    }
    if high - low <= usable {
        desired = desired.clamp(high - usable / 2.0, low + usable / 2.0);
    }
    let center = (desired.round().max(1.0) as u64).min(MAX_CENTER_HZ);
    (center != tuning.center_hz).then_some(center)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfo::VfoMode;

    const RATE: u32 = 1_000_000; // usable 850 kHz, half 425 kHz
    const CENTER: u64 = 100_000_000;
    const T: Tuning = Tuning {
        center_hz: CENTER,
        sample_rate: RATE,
    };

    fn vfo(id: u32, freq_hz: u64) -> Vfo {
        Vfo {
            id,
            freq_hz,
            mode: VfoMode::Nfm,
            bandwidth_hz: Some(10_000),
            squelch_dbfs: None,
            deemphasis: None,
        }
    }

    #[test]
    fn inside_the_window_nothing_moves() {
        assert_eq!(retune_center(&vfo(1, CENTER + 400_000), &[], T), None);
        assert_eq!(retune_center(&vfo(1, CENTER - 420_000), &[], T), None);
        assert_eq!(bounds(T), (99_575_000, 100_425_000));
    }

    #[test]
    fn the_margin_counts_as_outside() {
        // 430 kHz is inside the sample rate (500 kHz) but outside the usable window.
        let v = vfo(1, CENTER + 430_000);
        assert!(!fits(&v, T));
        assert_eq!(retune_center(&v, &[], T), Some(CENTER + 430_000 - 212_500));
    }

    #[test]
    fn follows_in_the_direction_of_the_move() {
        // Above: the VFO ends up a quarter window (212.5 kHz) above the new centre.
        assert_eq!(
            retune_center(&vfo(1, 101_000_000), &[], T),
            Some(101_000_000 - 212_500)
        );
        // Below: a quarter window below.
        assert_eq!(
            retune_center(&vfo(1, 99_000_000), &[], T),
            Some(99_000_000 + 212_500)
        );
        // The result really contains the VFO.
        let new = Tuning {
            center_hz: 101_000_000 - 212_500,
            ..T
        };
        assert!(fits(&vfo(1, 101_000_000), new));
    }

    #[test]
    fn wide_channels_sit_as_far_in_as_they_must() {
        let mut v = vfo(1, 101_000_000);
        v.bandwidth_hz = Some(700_000); // half 350 kHz: only 75 kHz of offset left
        let c = retune_center(&v, &[], T).expect("retune");
        assert_eq!(c, 101_000_000 - 75_000);
        // Wider than the window: cannot fit anywhere.
        v.bandwidth_hz = Some(900_000);
        assert_eq!(retune_center(&v, &[], T), None);
    }

    #[test]
    fn fit_mode_keeps_the_other_vfos_inside() {
        // A jumps to +600 kHz while B sits at -200 kHz. Together they span 810 kHz, which
        // fits, so the plain target (A at a quarter window, centre +387.5 kHz) is pulled back
        // to +220 kHz: B stays at the lower edge of the new window.
        let b = vfo(2, CENTER - 200_000);
        let a = vfo(1, CENTER + 600_000);
        let c = retune_center(&a, std::slice::from_ref(&b), T).expect("retune");
        assert_eq!(c, CENTER + 220_000);
        let new = Tuning { center_hz: c, ..T };
        assert!(fits(&a, new) && fits(&b, new));
        // Too far apart to fit together: the active VFO wins.
        let far = vfo(3, CENTER - 400_000);
        let wide = vfo(4, CENTER + 700_000);
        let c = retune_center(&wide, std::slice::from_ref(&far), T).expect("retune");
        let new = Tuning { center_hz: c, ..T };
        assert!(fits(&wide, new) && !fits(&far, new), "center {c}");
        assert_eq!(c, CENTER + 700_000 - 212_500);
    }

    #[test]
    fn others_outside_the_window_are_ignored() {
        let parked = vfo(2, CENTER + 5_000_000);
        let a = vfo(1, CENTER + 700_000);
        assert_eq!(
            retune_center(&a, &[parked], T),
            Some(CENTER + 700_000 - 212_500)
        );
    }

    #[test]
    fn centre_stays_in_range() {
        let low = Tuning {
            center_hz: 1_000_000,
            sample_rate: RATE,
        };
        let v = vfo(1, 100);
        assert_eq!(retune_center(&v, &[], low), Some(100 + 212_500));
        let v = vfo(1, 1);
        assert!(retune_center(&v, &[], low).is_some());
    }
}
