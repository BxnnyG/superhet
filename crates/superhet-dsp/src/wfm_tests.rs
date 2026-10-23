//! End-to-end tests of stereo and RDS: a synthetic broadcast signal (stereo multiplex with
//! pilot and RDS, FM modulated) through [`Channel`].

use std::f64::consts::TAU;

use num_complex::Complex32;

use crate::channel::{Channel, ChannelConfig, Deemphasis, Mode, OUTPUT_RATE};
use crate::rds::RdsInfo;
use crate::testsig::{Content, Station};

fn channel(rate: u32, offset: f64, deemphasis: Deemphasis, stereo: bool) -> Channel {
    let mut ch = Channel::new(ChannelConfig {
        input_rate: rate,
        offset_hz: offset,
        mode: Mode::Wfm { deemphasis },
        bandwidth_hz: None,
        squelch_dbfs: None,
    })
    .expect("channel");
    ch.set_stereo(stereo);
    ch
}

fn feed(ch: &mut Channel, st: &mut Station, c: Content, seconds: f64, out: &mut Vec<f32>) {
    let mut left = (st.rate * seconds) as usize;
    while left > 0 {
        let len = left.min(4099);
        let block: Vec<Complex32> = (0..len).map(|_| st.next(c)).collect();
        ch.process(&block, out);
        left -= len;
    }
}

/// Amplitude of the tone at `f_hz` in channel `which` of interleaved stereo `audio`.
fn amp(audio: &[f32], which: usize, f_hz: f64) -> f64 {
    let (mut a, mut b) = (0.0, 0.0);
    let mut n = 0usize;
    for (i, frame) in audio.as_chunks::<2>().0.iter().enumerate() {
        let ph = TAU * f_hz * i as f64 / f64::from(OUTPUT_RATE);
        a += f64::from(frame[which]) * ph.cos();
        b += f64::from(frame[which]) * ph.sin();
        n += 1;
    }
    2.0 * a.hypot(b) / n as f64
}

/// Last half second of interleaved stereo audio.
fn tail(out: &[f32]) -> &[f32] {
    &out[out.len() - 2 * 24_000..]
}

#[test]
fn stereo_separation_is_better_than_20_db() {
    let rate = 1_200_000;
    for (offset, deemph) in [
        (100_000.0, Deemphasis::None),
        (-150_000.0, Deemphasis::Us50),
    ] {
        let mut ch = channel(rate, offset, deemph, true);
        assert_eq!(ch.output_channels(), 2);
        let mut st = Station::new(rate, offset, "SUPERHET", "Hello");
        let c = Content {
            left: 0.5,
            right: 0.0,
            pilot: 0.1,
            rds: 0.0,
        };
        let mut out = Vec::new();
        feed(&mut ch, &mut st, c, 1.5, &mut out);
        assert!(ch.stereo_locked(), "pilot not locked (offset {offset})");
        let left = amp(tail(&out), 0, 1_000.0);
        let right = amp(tail(&out), 1, 1_000.0);
        assert!(left > 0.35 && left < 0.5, "left amplitude {left}");
        let sep = 20.0 * (left / right.max(1e-9)).log10();
        assert!(sep > 20.0, "separation {sep} dB (offset {offset})");
    }
}

#[test]
fn both_channels_carry_their_own_tone() {
    let rate = 1_024_000;
    let mut ch = channel(rate, 0.0, Deemphasis::None, true);
    let mut st = Station::new(rate, 0.0, "SUPERHET", "Hello");
    let c = Content {
        left: 0.6,
        right: 0.4,
        pilot: 0.1,
        rds: 0.0,
    };
    let mut out = Vec::new();
    feed(&mut ch, &mut st, c, 1.5, &mut out);
    // Left: 1 kHz at 0.6, right: 3 kHz at 0.4 (times the 0.9 modulation index).
    assert!((amp(tail(&out), 0, 1_000.0) - 0.54).abs() < 0.06);
    assert!((amp(tail(&out), 1, 3_000.0) - 0.36).abs() < 0.05);
    assert!(amp(tail(&out), 0, 3_000.0) < 0.03);
    assert!(amp(tail(&out), 1, 1_000.0) < 0.03);
}

#[test]
fn without_a_pilot_the_output_stays_mono() {
    let rate = 1_024_000;
    let mut ch = channel(rate, 0.0, Deemphasis::Us50, true);
    let mut st = Station::new(rate, 0.0, "SUPERHET", "Hello");
    // The difference signal is there but the pilot is missing: it must not be decoded.
    let c = Content {
        left: 0.5,
        right: 0.1,
        pilot: 0.0,
        rds: 0.0,
    };
    let mut out = Vec::new();
    feed(&mut ch, &mut st, c, 1.0, &mut out);
    assert!(!ch.stereo_locked());
    let diff = tail(&out)
        .as_chunks::<2>()
        .0
        .iter()
        .map(|f| (f[0] - f[1]).abs())
        .fold(0.0, f32::max);
    assert!(diff < 1e-4, "channels differ by {diff}");
    assert!(amp(tail(&out), 0, 1_000.0) > 0.1);
}

#[test]
fn a_vanishing_pilot_fades_to_mono_and_comes_back() {
    let rate = 1_024_000;
    let mut ch = channel(rate, 0.0, Deemphasis::None, true);
    let mut st = Station::new(rate, 0.0, "SUPERHET", "Hello");
    let on = Content {
        left: 0.5,
        right: 0.0,
        pilot: 0.1,
        rds: 0.0,
    };
    let off = Content { pilot: 0.0, ..on };
    let mut out = Vec::new();
    feed(&mut ch, &mut st, on, 1.0, &mut out);
    assert!(ch.stereo_locked());
    out.clear();
    feed(&mut ch, &mut st, off, 0.8, &mut out);
    assert!(!ch.stereo_locked());
    let (left, right) = (amp(tail(&out), 0, 1_000.0), amp(tail(&out), 1, 1_000.0));
    assert!((left - right).abs() < 0.01, "left {left} right {right}");
    out.clear();
    feed(&mut ch, &mut st, on, 1.0, &mut out);
    assert!(ch.stereo_locked());
    assert!(amp(tail(&out), 1, 1_000.0) < 0.1 * amp(tail(&out), 0, 1_000.0));
}

#[test]
fn a_weak_pilot_does_not_engage_stereo() {
    let rate = 1_024_000;
    let mut ch = channel(rate, 0.0, Deemphasis::None, true);
    let mut st = Station::new(rate, 0.0, "SUPERHET", "Hello");
    let c = Content {
        left: 0.5,
        right: 0.0,
        pilot: 0.015,
        rds: 0.0,
    };
    let mut out = Vec::new();
    feed(&mut ch, &mut st, c, 1.0, &mut out);
    assert!(!ch.stereo_locked());
}

#[test]
fn mono_output_keeps_its_frame_count() {
    let rate = 1_024_000;
    let mut ch = channel(rate, 0.0, Deemphasis::None, false);
    assert_eq!(ch.output_channels(), 1);
    let mut st = Station::new(rate, 0.0, "SUPERHET", "Hello");
    let c = Content {
        left: 0.5,
        right: 0.0,
        pilot: 0.1,
        rds: 0.0,
    };
    let mut out = Vec::new();
    feed(&mut ch, &mut st, c, 0.5, &mut out);
    assert!((out.len() as i64 - 24_000).abs() < 64, "{}", out.len());
}

#[test]
fn rds_is_decoded_from_the_fm_signal() {
    let rate = 1_024_000;
    for (offset, noise) in [(0.0, 0.0), (120_000.0, 0.004)] {
        let mut ch = channel(rate, offset, Deemphasis::Us50, false);
        let mut st = Station::new(rate, offset, "SUPERHET", "Hello RDS world");
        st.noise_amp = noise;
        let c = Content {
            left: 0.4,
            right: 0.4,
            pilot: 0.1,
            rds: 0.035,
        };
        let mut out = Vec::new();
        feed(&mut ch, &mut st, c, 4.0, &mut out);
        let info = ch.rds_info().expect("wfm has rds");
        assert_eq!(info.pi, Some(0xD3C2), "offset {offset}");
        assert_eq!(info.pty, Some(10));
        assert_eq!(info.ps.as_deref(), Some("SUPERHET"));
        assert_eq!(info.rt.as_deref(), Some("Hello RDS world"));
        assert!(ch.rds_synced());
        assert!(ch.rds_revision() > 0);

        // Tuning to another station drops the old text and the pilot lock.
        ch.set_offset(offset + 200_000.0).expect("retune");
        assert!(ch.rds_info().is_some_and(RdsInfo::is_empty));
        assert!(!ch.stereo_locked());
    }
}

#[test]
fn rds_works_in_stereo_mode_too() {
    let rate = 1_024_000;
    let mut ch = channel(rate, 0.0, Deemphasis::Us50, true);
    let mut st = Station::new(rate, 0.0, "SUPERHET", "Stereo and RDS");
    let c = Content {
        left: 0.4,
        right: 0.4,
        pilot: 0.1,
        rds: 0.035,
    };
    let mut out = Vec::new();
    feed(&mut ch, &mut st, c, 4.0, &mut out);
    let info = ch.rds_info().expect("wfm has rds");
    assert_eq!(info.ps.as_deref(), Some("SUPERHET"));
    assert_eq!(info.rt.as_deref(), Some("Stereo and RDS"));
    assert!(ch.stereo_locked());
}

#[test]
#[ignore = "sensitivity sweep, prints a table"]
fn rds_sensitivity_sweep() {
    let rate = 1_024_000;
    for noise in [0.0, 0.02, 0.05, 0.1, 0.15, 0.2] {
        for rds in [0.015, 0.035] {
            let mut ch = channel(rate, 0.0, Deemphasis::Us50, true);
            let mut st = Station::new(rate, 0.0, "SUPERHET", "Hello RDS world");
            st.noise_amp = noise;
            let c = Content {
                left: 0.4,
                right: 0.4,
                pilot: 0.1,
                rds,
            };
            let mut out = Vec::new();
            feed(&mut ch, &mut st, c, 10.0, &mut out);
            let info = ch.rds_info().expect("wfm");
            println!(
                "noise {noise} rds {rds}: locked {} synced {} ps {:?} rt {:?}",
                ch.stereo_locked(),
                ch.rds_synced(),
                info.ps,
                info.rt
            );
        }
    }
}

#[test]
fn non_wfm_modes_have_no_rds() {
    let ch = Channel::new(ChannelConfig {
        input_rate: 1_024_000,
        offset_hz: 0.0,
        mode: Mode::nfm(),
        bandwidth_hz: None,
        squelch_dbfs: None,
    })
    .expect("channel");
    assert!(ch.rds_info().is_none());
    assert_eq!(ch.output_channels(), 1);
}
