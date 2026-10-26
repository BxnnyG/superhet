//! Waveform-level tests of the RDS receiver: a shaped biphase signal at the mixed-down
//! baseband with noise, carrier offset, phase noise and bit clock error.

use std::f64::consts::TAU;

use num_complex::Complex32;

use crate::rds::RdsDecoder;
use crate::testsig::{Baseband, ShapedBaseband, cycle, differential, group_bits};

const FS: f64 = 256_000.0;
const PS: &str = "SUPERHET";
const RT: &str = "Hello RDS world";
const PI: u16 = 0xD3C2;

/// Small deterministic generator with Gaussian output.
struct Rng(u64);

impl Rng {
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> (f64, f64) {
        let r = (-2.0 * self.next_f64().ln()).sqrt();
        let a = TAU * self.next_f64();
        (r * a.cos(), r * a.sin())
    }
}

#[derive(Clone, Copy, Debug)]
struct Scenario {
    /// Energy per bit over noise density in dB.
    ebn0_db: f64,
    /// Residual carrier frequency error in Hz.
    cfo_hz: f64,
    /// Random-walk carrier phase noise in rad per square root second.
    phase_walk: f64,
    /// Bit clock error in ppm.
    ppm: f64,
    seconds: f64,
    seed: u64,
    /// Plain sine-cycle pulses instead of the pulse shape of the standard.
    sine: bool,
    /// No signal at all, only the noise of the given level.
    silent: bool,
    /// Take the PI from clean blocks only (no soft combining of block A).
    hard_pi: bool,
}

impl Scenario {
    fn clean(ebn0_db: f64) -> Self {
        Self {
            ebn0_db,
            cfo_hz: 0.0,
            phase_walk: 0.0,
            ppm: 0.0,
            seconds: 12.0,
            seed: 1,
            sine: false,
            silent: false,
            hard_pi: false,
        }
    }

    /// What a real receiver sees: a few Hz of pilot-PLL frequency error, phase jitter and a
    /// crystal that is off by 60 ppm.
    fn harsh(ebn0_db: f64) -> Self {
        Self {
            cfo_hz: 8.0,
            phase_walk: 0.6,
            ppm: 60.0,
            ..Self::clean(ebn0_db)
        }
    }
}

#[derive(Debug, Default)]
struct Outcome {
    /// Seconds until PS and RadioText were both correct, if ever.
    full_at: Option<f64>,
    /// Seconds until the PI was correct, if ever.
    pi_at: Option<f64>,
    synced: bool,
    good: u64,
    bad: u64,
    transmitted: u64,
    ps: Option<String>,
    rt: Option<String>,
    pi: Option<u16>,
    /// True if a wrong PS or RadioText was ever shown.
    garbage: bool,
    /// Raw bit error rate of the decided bits after the first second.
    ber: f64,
    /// Blocks delivered with a correction, and how many of them are not what was sent.
    corrected: u64,
    miscorrected: u64,
    /// Blocks delivered without a correction that are not what was sent.
    false_clean: u64,
}

impl Outcome {
    /// Share of in-sync blocks that failed.
    fn bler(&self) -> f64 {
        let n = self.good + self.bad;
        if n == 0 {
            1.0
        } else {
            self.bad as f64 / n as f64
        }
    }

    /// Share of the transmitted blocks that were delivered.
    fn coverage(&self) -> f64 {
        self.good as f64 / self.transmitted as f64
    }
}

/// Bit error rate of `log` against the endless repetition of `data` at the best alignment.
fn bit_error_rate(log: &[bool], data: &[bool]) -> f64 {
    let skip = 1_500.min(log.len() / 2);
    let tail = &log[skip..];
    let n = tail.len().min(6_000);
    let best = (0..data.len())
        .map(|shift| {
            (0..n)
                .filter(|&i| tail[i] != data[(skip + i + shift) % data.len()])
                .count()
        })
        .min()
        .unwrap_or(n);
    best as f64 / n.max(1) as f64
}

fn run(sc: Scenario) -> Outcome {
    let groups = cycle(PI, 10, PS, RT);
    let bits = differential(&group_bits(&groups));
    let mut gen_ = ShapedBaseband::new(bits.clone(), FS);
    gen_.ppm = sc.ppm;
    // Mean power of the clean waveform fixes the noise level for the requested Eb/N0.
    let mut probe = ShapedBaseband::new(bits.clone(), FS);
    let n_probe = (FS * 2.0) as usize;
    let power = (0..n_probe).map(|_| probe.sample().powi(2)).sum::<f64>() / n_probe as f64;
    let mut sine = Baseband::new(bits, FS);
    let eb = power / crate::rds::BIT_RATE_HZ;
    let n0 = eb / 10f64.powf(sc.ebn0_db / 10.0);
    let sigma = (n0 * FS / 2.0).sqrt();

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ sc.seed.wrapping_mul(0x1234_5678_9ABC_DEF1));
    let mut dec = RdsDecoder::new(FS);
    dec.set_hard_pi_only(sc.hard_pi);
    let mut phase = 0.7;
    let mut out = Outcome::default();
    let total = (FS * sc.seconds) as usize;
    let walk = sc.phase_walk * (1.0 / FS).sqrt();
    let mut buf = Vec::with_capacity(4099);
    let mut done = 0usize;
    while done < total {
        buf.clear();
        for _ in 0..4099.min(total - done) {
            let s = if sc.sine {
                sine.sample() * 0.8
            } else {
                gen_.sample()
            };
            let s = if sc.silent { 0.0 } else { s };
            let (ni, nq) = rng.gauss();
            phase += TAU * sc.cfo_hz / FS + walk * rng.gauss().0;
            let z = Complex32::from_polar(1.0, phase as f32);
            buf.push(
                z * Complex32::new(s as f32, 0.0)
                    + Complex32::new((ni * sigma) as f32, (nq * sigma) as f32),
            );
        }
        done += buf.len();
        dec.process(&buf);
        let info = dec.info();
        let bad_ps = info.ps.as_deref().is_some_and(|p| p != PS);
        let bad_rt = info.rt.as_deref().is_some_and(|r| !RT.starts_with(r));
        let bad_pi = info.pi.is_some_and(|p| p != PI);
        out.garbage |= bad_ps || bad_rt || bad_pi;
        if out.pi_at.is_none() && info.pi == Some(PI) {
            out.pi_at = Some(done as f64 / FS);
        }
        if out.full_at.is_none() && info.ps.as_deref() == Some(PS) && info.rt.as_deref() == Some(RT)
        {
            out.full_at = Some(done as f64 / FS);
        }
    }
    out.ber = bit_error_rate(&dec.bit_log, &group_bits(&groups));
    for &(pos, info, corrected) in &dec.block_log {
        let sent = groups.iter().any(|g| {
            if pos == 2 {
                g[2] == info
            } else {
                g[usize::from(pos)] == info
            }
        });
        out.corrected += u64::from(corrected);
        out.miscorrected += u64::from(corrected && !sent);
        out.false_clean += u64::from(!corrected && !sent);
    }
    let info = dec.info();
    out.synced = dec.synced();
    (out.good, out.bad) = dec.block_counts();
    out.transmitted = (sc.seconds * crate::rds::BIT_RATE_HZ / 26.0) as u64;
    out.ps = info.ps.clone();
    out.rt = info.rt.clone();
    out.pi = info.pi;
    out
}

#[test]
fn decodes_at_low_snr_with_clock_drift_and_carrier_offset() {
    // 7 dB Eb/N0 is a weak station: raw bit error rate around one percent.
    for seed in 1..=3 {
        let o = run(Scenario {
            cfo_hz: 20.0,
            phase_walk: 1.0,
            ppm: 100.0,
            seconds: 6.0,
            seed,
            ..Scenario::clean(7.0)
        });
        assert!(o.full_at.is_some_and(|t| t < 4.0), "seed {seed}: {o:?}");
        assert_eq!(o.pi, Some(PI));
        assert!(!o.garbage, "seed {seed}");
        assert!(o.bler() < 0.08, "seed {seed}: BLER {}", o.bler());
    }
}

#[test]
fn follows_a_residual_carrier_frequency_error() {
    // The third harmonic of a noisy pilot is not a perfect carrier.
    for cfo in [-120.0, 120.0] {
        let o = run(Scenario {
            cfo_hz: cfo,
            seconds: 6.0,
            ..Scenario::clean(10.0)
        });
        assert!(o.full_at.is_some_and(|t| t < 3.0), "{cfo} Hz: {o:?}");
        assert!(!o.garbage);
    }
}

#[test]
fn follows_large_sample_clock_errors() {
    // Cheap RTL-SDR crystals are off by tens of ppm, some by a few hundred.
    for ppm in [-300.0, 300.0] {
        let o = run(Scenario {
            ppm,
            seconds: 8.0,
            ..Scenario::clean(10.0)
        });
        assert!(o.full_at.is_some_and(|t| t < 3.0), "{ppm} ppm: {o:?}");
        assert!(o.bler() < 0.02, "{ppm} ppm: BLER {}", o.bler());
        assert!(!o.garbage);
    }
}

#[test]
fn survives_pulses_that_are_not_the_standard_shape() {
    // A plain sine-cycle biphase pulse is what a simple encoder might send.
    let o = run(Scenario {
        sine: true,
        seconds: 6.0,
        ..Scenario::clean(12.0)
    });
    assert!(o.full_at.is_some_and(|t| t < 3.0), "{o:?}");
    assert!(!o.garbage);
}

#[test]
fn noise_alone_decodes_nothing() {
    for seed in 1..=3 {
        let o = run(Scenario {
            silent: true,
            seconds: 40.0,
            seed,
            ..Scenario::clean(6.0)
        });
        assert!(o.ps.is_none() && o.rt.is_none(), "seed {seed}: {o:?}");
        assert!(!o.garbage, "seed {seed}");
        assert!(!o.synced, "seed {seed}");
    }
}

/// Result of [`pi_acquisition`].
#[derive(Debug)]
struct PiStats {
    /// Mean seconds until the PI was correct (runs that never got one count as the run length).
    mean: f64,
    /// Runs without a correct PI.
    never: u64,
    /// Runs in which a wrong PI was shown at some point.
    wrong: u64,
}

fn pi_acquisition(ebn0_db: f64, hard_pi: bool, seeds: u64) -> PiStats {
    let seconds = 8.0;
    let (mut total, mut never, mut wrong) = (0.0, 0, 0);
    for seed in 1..=seeds {
        let o = run(Scenario {
            seconds,
            seed,
            hard_pi,
            ..Scenario::harsh(ebn0_db)
        });
        total += o.pi_at.unwrap_or(seconds);
        never += u64::from(o.pi_at.is_none());
        wrong += u64::from(o.garbage);
    }
    PiStats {
        mean: total / seeds as f64,
        never,
        wrong,
    }
}

#[test]
fn soft_combining_never_hurts_the_pi_at_low_snr() {
    // The PI is mostly limited by the block sync here, so the gain is small; the unit test in
    // `rds.rs` shows the case soft combining is for.
    let seeds = 8;
    for ebn0 in [2.0, 1.0] {
        let hard = pi_acquisition(ebn0, true, seeds);
        let soft = pi_acquisition(ebn0, false, seeds);
        eprintln!("Eb/N0 {ebn0}: PI hard {hard:?}, soft {soft:?}");
        assert!(
            soft.never <= hard.never,
            "{ebn0} dB: soft combining lost PIs"
        );
        assert!(
            soft.wrong <= hard.wrong,
            "{ebn0} dB: soft combining shows wrong PIs"
        );
        assert!(soft.mean <= hard.mean + 0.05, "{ebn0} dB: PI came later");
    }
}

#[test]
fn soft_combining_does_not_invent_a_pi_from_noise() {
    for seed in 1..=4 {
        let o = run(Scenario {
            silent: true,
            seconds: 40.0,
            seed,
            ..Scenario::clean(6.0)
        });
        assert!(o.pi.is_none(), "seed {seed}: {o:?}");
    }
}

#[test]
#[ignore = "quick tuning probe"]
fn rds_waveform_probe() {
    for (name, sc) in [
        ("clean 6", Scenario::clean(6.0)),
        ("clean 5", Scenario::clean(5.0)),
        ("harsh 6", Scenario::harsh(6.0)),
        ("harsh 5", Scenario::harsh(5.0)),
        (
            "sine 6",
            Scenario {
                sine: true,
                ..Scenario::clean(6.0)
            },
        ),
    ] {
        let (mut bler, mut cov, mut garbage, mut ber) = (0.0, 0.0, 0, 0.0);
        let (mut corrected, mut miscorrected, mut false_clean) = (0, 0, 0);
        for seed in 1..=4 {
            let o = run(Scenario { seed, ..sc });
            bler += o.bler();
            cov += o.coverage();
            ber += o.ber;
            garbage += usize::from(o.garbage);
            corrected += o.corrected;
            miscorrected += o.miscorrected;
            false_clean += o.false_clean;
        }
        println!(
            "{name:<10} BER {:6.3} %  BLER {:5.1} %  coverage {:5.1} %  corrected {corrected} wrong {miscorrected} false-clean {false_clean} garbage {garbage}",
            25.0 * ber,
            25.0 * bler,
            25.0 * cov
        );
    }
}

#[test]
#[ignore = "impairment breakdown, prints a table"]
fn rds_waveform_impairments() {
    let mut cases: Vec<(String, Scenario)> = Vec::new();
    for ebn0 in [10.0, 7.0] {
        let b = Scenario::clean(ebn0);
        cases.push((format!("{ebn0} dB none"), b));
        for cfo in [8.0, 60.0, 150.0, -150.0, 300.0] {
            cases.push((
                format!("{ebn0} dB cfo {cfo}"),
                Scenario { cfo_hz: cfo, ..b },
            ));
        }
        for w in [0.3, 1.0, 3.0] {
            cases.push((
                format!("{ebn0} dB walk {w}"),
                Scenario { phase_walk: w, ..b },
            ));
        }
        for ppm in [20.0, 60.0, 150.0, -150.0] {
            cases.push((format!("{ebn0} dB ppm {ppm}"), Scenario { ppm, ..b }));
        }
    }
    for (name, sc) in cases {
        let (mut bler, mut cov, mut garbage) = (0.0, 0.0, 0);
        for seed in 1..=3 {
            let o = run(Scenario { seed, ..sc });
            bler += o.bler();
            cov += o.coverage();
            garbage += usize::from(o.garbage);
        }
        println!(
            "{name:<18} BLER {:5.1} %  coverage {:5.1} %  garbage {garbage}",
            100.0 * bler / 3.0,
            100.0 * cov / 3.0
        );
    }
}

#[test]
#[ignore = "sensitivity sweep, prints a table"]
fn rds_waveform_sweep() {
    let seeds = 4u64;
    for (name, make) in [
        ("clean", Scenario::clean as fn(f64) -> Scenario),
        ("harsh", Scenario::harsh),
    ] {
        for ebn0 in [14.0, 12.0, 10.0, 9.0, 8.0, 7.0, 6.0, 5.0] {
            let (mut bler, mut cov, mut full, mut times, mut garbage) = (0.0, 0.0, 0, 0.0, 0);
            for seed in 0..seeds {
                let o = run(Scenario {
                    seed: seed + 1,
                    ..make(ebn0)
                });
                bler += o.bler();
                cov += o.coverage();
                if let Some(t) = o.full_at {
                    full += 1;
                    times += t;
                }
                garbage += usize::from(o.garbage);
            }
            let n = seeds as f64;
            println!(
                "{name} Eb/N0 {ebn0:>4}: BLER {:5.1} %  coverage {:5.1} %  ps+rt ok {full}/{seeds} (mean {:.1} s)  garbage {garbage}",
                100.0 * bler / n,
                100.0 * cov / n,
                if full > 0 {
                    times / full as f64
                } else {
                    f64::NAN
                },
            );
        }
    }
}
