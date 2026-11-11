//! Tests of the HF comfort blocks with synthetic signals.

use std::f64::consts::TAU;

use num_complex::Complex32;

use crate::channel::{Channel, ChannelConfig, Mode};
use crate::comfort::{
    AutoNotch, ComfortConfig, ManualNotch, NoiseBlanker, NoiseReducer, NotchSetting,
};
use crate::fir::design_lowpass;

struct Rng(u32);

impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        f64::from(self.0) / f64::from(u32::MAX) - 0.5
    }

    /// Approximately Gaussian with unit variance.
    fn gauss(&mut self) -> f64 {
        (0..12).map(|_| self.uniform() + 0.5).sum::<f64>() - 6.0
    }
}

fn convolve(x: &[f32], h: &[f32]) -> Vec<f32> {
    (0..x.len())
        .map(|n| {
            h.iter()
                .enumerate()
                .take(n + 1)
                .map(|(k, c)| c * x[n - k])
                .sum()
        })
        .collect()
}

/// Band-limited noise 300..2700 Hz scaled to `rms`: a speech stand-in without steady components.
fn speech_like(rate: f64, n: usize, rms: f64, seed: u32) -> Vec<f32> {
    let mut rng = Rng(seed);
    let white: Vec<f32> = (0..n).map(|_| rng.gauss() as f32).collect();
    let hi = design_lowpass(rate, 2_700.0, 3_300.0, 50.0);
    let lo = design_lowpass(rate, 250.0, 350.0, 50.0);
    let a = convolve(&white, &hi);
    let b = convolve(&white, &lo);
    let y: Vec<f32> = a.iter().zip(&b).map(|(p, q)| p - q).collect();
    let tail = &y[n / 2..];
    let p = tail.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / tail.len() as f64;
    let g = (rms / p.sqrt()) as f32;
    y.iter().map(|v| v * g).collect()
}

fn tone(rate: f64, n: usize, f: f64, amp: f64) -> Vec<f32> {
    (0..n)
        .map(|i| (amp * (TAU * f * i as f64 / rate).sin()) as f32)
        .collect()
}

fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

fn power(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64
}

/// Least-squares sine fit at `f`: (amplitude, power of the residual).
fn fit(x: &[f32], rate: f64, f: f64) -> (f64, f64) {
    let (mut c, mut s) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let ph = TAU * f * i as f64 / rate;
        c += f64::from(*v) * ph.cos();
        s += f64::from(*v) * ph.sin();
    }
    let n = x.len() as f64;
    let (a, b) = (2.0 * c / n, 2.0 * s / n);
    let resid: f64 = x
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let ph = TAU * f * i as f64 / rate;
            (f64::from(*v) - a * ph.cos() - b * ph.sin()).powi(2)
        })
        .sum::<f64>()
        / n;
    (a.hypot(b), resid)
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

const FS: f64 = 32_000.0;

#[test]
fn manual_notch_removes_carrier_and_keeps_the_voice_band() {
    let n = 64_000;
    let speech = speech_like(FS, n, 0.1, 7);
    let carrier = tone(FS, n, 1_000.0, 0.3);
    let mut y = mix(&speech, &carrier);
    let mut n0 = ManualNotch::new(
        FS,
        NotchSetting {
            freq_hz: 1_000.0,
            width_hz: 60.0,
        },
    );
    n0.process(&mut y);
    let tail = &y[n / 2..];
    let (amp, resid) = fit(tail, FS, 1_000.0);
    let att = db((0.3_f64 / amp).powi(2));
    assert!(att >= 30.0, "carrier attenuated by only {att:.1} dB");
    let kept = db(resid / power(&speech[n / 2..]));
    assert!(kept > -1.5, "speech band changed by {kept:.2} dB");
}

#[test]
fn auto_notch_removes_a_steady_carrier_and_keeps_the_voice_band() {
    let n = 96_000;
    let speech = speech_like(FS, n, 0.1, 11);
    let carrier = tone(FS, n, 1_250.0, 0.3);
    let mut y = mix(&speech, &carrier);
    let mut a = AutoNotch::new(FS, 0.5);
    a.process(&mut y);
    let tail = &y[2 * n / 3..];
    let (amp, resid) = fit(tail, FS, 1_250.0);
    let att = db((0.3_f64 / amp).powi(2));
    assert!(att >= 30.0, "carrier attenuated by only {att:.1} dB");
    let kept = db(resid / power(&speech[2 * n / 3..]));
    assert!(kept > -3.0, "speech band changed by {kept:.2} dB");
}

#[test]
fn auto_notch_follows_a_drifting_carrier() {
    let n = 96_000;
    let mut y: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / FS;
            // 1.0 kHz drifting by 100 Hz/s.
            (0.3 * (TAU * (1_000.0 * t + 50.0 * t * t)).sin()) as f32
        })
        .collect();
    let mut a = AutoNotch::new(FS, 0.8);
    a.process(&mut y);
    let tail = power(&y[n / 2..]);
    assert!(db(tail / (0.3_f64.powi(2) / 2.0)) < -25.0);
}

#[test]
fn noise_blanker_suppresses_impulses() {
    let rate = 24_000.0;
    let n = 48_000;
    let mut rng = Rng(3);
    let clean: Vec<Complex32> = (0..n)
        .map(|i| {
            let ph = TAU * 1_000.0 * i as f64 / rate;
            Complex32::new((0.3 * ph.cos()) as f32, (0.3 * ph.sin()) as f32)
        })
        .collect();
    let noisy: Vec<Complex32> = clean
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let imp = if i % 3_000 == 700 { 5.0 } else { 0.0 };
            *c + Complex32::new(
                (0.005 * rng.gauss() + imp) as f32,
                (0.005 * rng.gauss()) as f32,
            )
        })
        .collect();
    let err = |x: &[Complex32], delay: usize| -> f64 {
        let skip = 2_000;
        let e: f64 = (skip..n - delay)
            .map(|i| f64::from((x[i + delay] - clean[i]).norm_sqr()))
            .sum();
        e / (n - delay - skip) as f64
    };
    let mut nb = NoiseBlanker::new(rate, 0.5);
    let mut out = noisy.clone();
    for chunk in out.chunks_mut(997) {
        nb.process(chunk);
    }
    let before = err(&noisy, 0);
    let after = err(&out, nb.delay());
    assert!(
        db(before / after) > 12.0,
        "only {:.1} dB better",
        db(before / after)
    );
}

#[test]
fn noise_blanker_leaves_a_clean_signal_alone() {
    let rate = 24_000.0;
    let n = 24_000;
    let mut rng = Rng(5);
    let sig: Vec<Complex32> = (0..n)
        .map(|i| {
            let ph = TAU * 700.0 * i as f64 / rate;
            Complex32::new(
                (0.3 * ph.cos() + 0.03 * rng.gauss()) as f32,
                (0.3 * ph.sin() + 0.03 * rng.gauss()) as f32,
            )
        })
        .collect();
    let mut nb = NoiseBlanker::new(rate, 1.0);
    let mut out = sig.clone();
    nb.process(&mut out);
    let d = nb.delay();
    let changed = (2_000..n - d)
        .filter(|&i| (out[i + d] - sig[i]).norm() > 1e-6)
        .count();
    assert!(
        changed < (n - d - 2_000) / 50,
        "{changed} of {n} samples blanked"
    );
}

#[test]
fn noise_reduction_improves_snr_without_killing_the_tone() {
    let n = 160_000;
    let mut rng = Rng(21);
    let noise: Vec<f32> = (0..n).map(|_| (0.1 * rng.gauss()) as f32).collect();
    let t = tone(FS, n, 1_000.0, 0.1);
    let input = mix(&t, &noise);
    let mut y = input.clone();
    let mut nr = NoiseReducer::new(FS, 0.7);
    for chunk in y.chunks_mut(1_013) {
        nr.process(chunk);
    }
    let d = nr.delay();
    let (lo, hi) = (n / 2, n - d);
    let (a_in, r_in) = fit(&input[lo..hi], FS, 1_000.0);
    let (a_out, r_out) = fit(&y[lo + d..hi + d], FS, 1_000.0);
    let kept = db((a_out / a_in).powi(2));
    assert!(kept > -1.5, "tone changed by {kept:.2} dB");
    let snr_gain = db((a_out * a_out / r_out) / (a_in * a_in / r_in));
    assert!(snr_gain > 8.0, "SNR improved by only {snr_gain:.1} dB");
}

#[test]
fn noise_reduction_has_a_frame_of_latency() {
    let nr = NoiseReducer::new(24_000.0, 0.5);
    assert_eq!(nr.delay(), 256);
    assert!(nr.delay() as f64 / 24_000.0 < 0.012);
}

fn ssb_channel(rate: u32, comfort: ComfortConfig) -> Channel {
    let mut ch = Channel::new(ChannelConfig {
        input_rate: rate,
        offset_hz: 0.0,
        mode: Mode::Usb,
        bandwidth_hz: None,
        squelch_dbfs: None,
    })
    .expect("channel");
    ch.set_comfort(comfort);
    ch
}

fn run_channel(ch: &mut Channel, iq: &[Complex32]) -> Vec<f32> {
    let mut out = Vec::new();
    for chunk in iq.chunks(4_093) {
        ch.process(chunk, &mut out);
    }
    out
}

#[test]
fn channel_auto_notch_removes_a_whistle_in_ssb() {
    let rate = 240_000u32;
    let secs = 5;
    let n = rate as usize * secs;
    let mut rng = Rng(9);
    let noise: Vec<Complex32> = (0..n)
        .map(|_| Complex32::new((0.05 * rng.gauss()) as f32, (0.05 * rng.gauss()) as f32))
        .collect();
    // A USB carrier at 1.5 kHz above the VFO comes out as a 1.5 kHz whistle.
    let whistle: Vec<Complex32> = noise
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let ph = TAU * 1_500.0 * i as f64 / f64::from(rate);
            *c + Complex32::new((0.2 * ph.cos()) as f32, (0.2 * ph.sin()) as f32)
        })
        .collect();
    let on = ComfortConfig {
        auto_notch: Some(0.6),
        ..ComfortConfig::default()
    };
    let plain_whistle = run_channel(&mut ssb_channel(rate, ComfortConfig::default()), &whistle);
    let plain_noise = run_channel(&mut ssb_channel(rate, ComfortConfig::default()), &noise);
    let notched = run_channel(&mut ssb_channel(rate, on), &whistle);
    let from = 48_000 * 3;
    let (a_plain, _) = fit(&plain_whistle[from..], 48_000.0, 1_500.0);
    let (a_notched, _) = fit(&notched[from..], 48_000.0, 1_500.0);
    let att = db((a_plain / a_notched).powi(2));
    assert!(att >= 30.0, "whistle attenuated by only {att:.1} dB");
    // What is left sounds like the noise-only reception: level within 3 dB.
    let ratio = db(power(&notched[from..]) / power(&plain_noise[from..]));
    assert!(
        ratio.abs() < 3.0,
        "voice band level changed by {ratio:.1} dB"
    );
}

#[test]
fn channel_noise_blanker_suppresses_impulses_in_am() {
    let rate = 240_000u32;
    let n = rate as usize * 2;
    let mut rng = Rng(13);
    let iq: Vec<Complex32> = (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(rate);
            let env = 0.2 * (1.0 + 0.5 * (TAU * 1_000.0 * t).cos());
            let imp = if i % 6_000 == 1_000 { 30.0 } else { 0.0 };
            Complex32::new(
                (env + imp + 0.005 * rng.gauss()) as f32,
                (0.005 * rng.gauss()) as f32,
            )
        })
        .collect();
    let run = |comfort: ComfortConfig| {
        let mut ch = Channel::new(ChannelConfig {
            input_rate: rate,
            offset_hz: 0.0,
            mode: Mode::Am,
            bandwidth_hz: Some(6_000),
            squelch_dbfs: None,
        })
        .expect("channel");
        ch.set_comfort(comfort);
        let out = run_channel(&mut ch, &iq);
        let (a, r) = fit(&out[48_000..], 48_000.0, 1_000.0);
        db(a * a / r)
    };
    let off = run(ComfortConfig::default());
    let on = run(ComfortConfig {
        noise_blanker: Some(0.5),
        ..ComfortConfig::default()
    });
    assert!(on - off > 10.0, "SNR {off:.1} dB -> {on:.1} dB");
}

#[test]
fn comfort_blocks_are_off_by_default_and_inactive_in_wfm() {
    let mut ch = Channel::new(ChannelConfig {
        input_rate: 480_000,
        offset_hz: 0.0,
        mode: Mode::wfm(),
        bandwidth_hz: None,
        squelch_dbfs: None,
    })
    .expect("channel");
    assert!(ch.comfort().is_off());
    assert_eq!(ch.comfort_latency_s(), 0.0);
    ch.set_comfort(ComfortConfig {
        noise_reduction: Some(1.0),
        auto_notch: Some(1.0),
        noise_blanker: Some(1.0),
        notch: Some(NotchSetting {
            freq_hz: 1_000.0,
            width_hz: 50.0,
        }),
    });
    assert_eq!(ch.comfort_latency_s(), 0.0);
    ch.set_mode(Mode::Am).expect("am");
    assert!(ch.comfort_latency_s() > 0.0);
    ch.set_mode(Mode::wfm()).expect("wfm");
    assert_eq!(ch.comfort_latency_s(), 0.0);
}

#[test]
fn comfort_config_is_sanitized() {
    let c = ComfortConfig {
        noise_reduction: Some(7.0),
        auto_notch: Some(f32::NAN),
        notch: Some(NotchSetting {
            freq_hz: 1.0,
            width_hz: 1e9,
        }),
        noise_blanker: Some(-1.0),
    }
    .sanitized();
    assert_eq!(c.noise_reduction, Some(1.0));
    assert_eq!(c.auto_notch, Some(0.5));
    assert_eq!(c.noise_blanker, Some(0.0));
    let n = c.notch.expect("notch");
    assert!(n.freq_hz >= 50.0 && n.width_hz <= 1_000.0);
}

/// CPU cost per VFO at 2.4 MS/s: `cargo test --release -p superhet-dsp comfort_cost -- --ignored
/// --nocapture`. Prints the share of one core for each block on top of the plain channel.
#[test]
#[ignore = "benchmark"]
fn comfort_cost() {
    let rate = 2_400_000u32;
    let n = rate as usize; // one second of signal per run
    let mut rng = Rng(1);
    let iq: Vec<Complex32> = (0..n)
        .map(|i| {
            let ph = TAU * 1_500.0 * i as f64 / f64::from(rate);
            Complex32::new(
                (0.1 * ph.cos() + 0.02 * rng.gauss()) as f32,
                (0.1 * ph.sin() + 0.02 * rng.gauss()) as f32,
            )
        })
        .collect();
    let cases: [(&str, ComfortConfig); 6] = [
        ("none", ComfortConfig::default()),
        (
            "noise blanker",
            ComfortConfig {
                noise_blanker: Some(0.5),
                ..ComfortConfig::default()
            },
        ),
        (
            "manual notch",
            ComfortConfig {
                notch: Some(NotchSetting {
                    freq_hz: 1_000.0,
                    width_hz: 50.0,
                }),
                ..ComfortConfig::default()
            },
        ),
        (
            "auto notch",
            ComfortConfig {
                auto_notch: Some(0.5),
                ..ComfortConfig::default()
            },
        ),
        (
            "noise reduction",
            ComfortConfig {
                noise_reduction: Some(0.5),
                ..ComfortConfig::default()
            },
        ),
        (
            "all four",
            ComfortConfig {
                noise_blanker: Some(0.5),
                notch: Some(NotchSetting {
                    freq_hz: 1_000.0,
                    width_hz: 50.0,
                }),
                auto_notch: Some(0.5),
                noise_reduction: Some(0.5),
            },
        ),
    ];
    for mode in [Mode::Usb, Mode::Am] {
        for (name, cfg) in cases {
            let mut ch = Channel::new(ChannelConfig {
                input_rate: rate,
                offset_hz: 0.0,
                mode,
                bandwidth_hz: None,
                squelch_dbfs: None,
            })
            .expect("channel");
            ch.set_comfort(cfg);
            run_channel(&mut ch, &iq[..n / 4]); // warm-up
            let t = std::time::Instant::now();
            for _ in 0..8 {
                run_channel(&mut ch, &iq);
            }
            let share = t.elapsed().as_secs_f64() / 8.0 * 100.0;
            println!("{mode:?} {name:>16}: {share:.2} % of a core");
        }
    }
}
