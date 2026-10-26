//! RDS through the production channel code under realistic RTL-SDR conditions: a broadcast
//! multiplex with programme material (not just two tones), sampled at 2.048 / 2.4 MS/s, the
//! station away from the centre, receiver noise, DC offset, IQ imbalance and 8-bit quantisation.

use std::collections::VecDeque;
use std::f64::consts::TAU;

use num_complex::Complex32;

use crate::channel::{Channel, ChannelConfig, Deemphasis, Mode};
use crate::fir::{DecimFir, design_lowpass};
use crate::testsig::{ShapedBaseband, cycle, differential, group_bits};

const PS: &str = "SUPERHET";
const RT: &str = "Hello RDS world";
const AUDIO_RATE: f64 = 128_000.0;

struct Rng(u64);

impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    }

    /// Approximately Gaussian, unit variance.
    fn gauss(&mut self) -> f64 {
        (0..12).map(|_| self.uniform() + 0.5).sum::<f64>() - 6.0
    }
}

/// Band-limited programme-like audio: pink-ish noise with a 15 kHz brick-wall, 50 us pre-emphasis.
struct Programme {
    rng: Rng,
    lp: [f64; 3],
    fir: DecimFir<f32>,
    pre: f64,
}

impl Programme {
    fn new(seed: u64) -> Self {
        Self {
            rng: Rng(seed),
            lp: [0.0; 3],
            fir: DecimFir::new(design_lowpass(AUDIO_RATE, 14_500.0, 16_000.0, 70.0), 1),
            pre: 0.0,
        }
    }

    fn block(&mut self, n: usize) -> Vec<f32> {
        let mut raw = Vec::with_capacity(n);
        for _ in 0..n {
            let w = self.rng.gauss();
            self.lp[0] += 0.15 * (w - self.lp[0]);
            self.lp[1] += 0.40 * (w - self.lp[1]);
            self.lp[2] += 0.02 * (w - self.lp[2]);
            let x = 0.6 * self.lp[0] + 0.3 * self.lp[1] + 4.0 * self.lp[2];
            // 50 us pre-emphasis (corner 3.18 kHz).
            let a = 1.0 - (-TAU * 3_183.0 / AUDIO_RATE).exp();
            self.pre += a * (x - self.pre);
            raw.push((x + 2.0 * (x - self.pre)) as f32);
        }
        let mut out = Vec::new();
        self.fir.process(&raw, &mut out);
        out
    }
}

/// Complete broadcast station with programme audio.
struct Broadcast {
    fs: f64,
    offset: f64,
    n: u64,
    phase: f64,
    rds: ShapedBaseband,
    left: Programme,
    right: Programme,
    common: Programme,
    queue: VecDeque<(f64, f64)>,
    /// Audio samples at index `k` and `k + 1`.
    a0: (f64, f64),
    a1: (f64, f64),
    k: u64,
    rds_level: f64,
    audio_level: f64,
    noise: Rng,
    noise_amp: f64,
}

impl Broadcast {
    fn new(fs: u32, offset: f64, rds_level: f64, noise_amp: f64, audio_level: f64) -> Self {
        let bits = differential(&group_bits(&cycle(0xD3C2, 10, PS, RT)));
        let mut s = Self {
            fs: f64::from(fs),
            offset,
            n: 0,
            phase: 0.0,
            rds: ShapedBaseband::new(bits, f64::from(fs)),
            left: Programme::new(0x1234_5678_9abc),
            right: Programme::new(0x0fed_cba9_8765),
            common: Programme::new(0x2468_ace0_1357),
            queue: VecDeque::new(),
            a0: (0.0, 0.0),
            a1: (0.0, 0.0),
            k: 0,
            rds_level,
            audio_level,
            noise: Rng(0xdead_beef_1234),
            noise_amp,
        };
        s.a0 = s.pop();
        s.a1 = s.pop();
        s
    }

    fn pop(&mut self) -> (f64, f64) {
        if self.queue.is_empty() {
            let (l, r, c) = (
                self.left.block(4096),
                self.right.block(4096),
                self.common.block(4096),
            );
            for i in 0..l.len() {
                self.queue.push_back((
                    f64::from(0.6 * c[i] + 0.4 * l[i]),
                    f64::from(0.6 * c[i] + 0.4 * r[i]),
                ));
            }
        }
        self.queue.pop_front().unwrap_or_default()
    }

    fn next(&mut self) -> Complex32 {
        let t = self.n as f64 / self.fs;
        let x = t * AUDIO_RATE;
        let idx = x.floor() as u64;
        let frac = x - idx as f64;
        while self.k < idx {
            self.a0 = self.a1;
            self.a1 = self.pop();
            self.k += 1;
        }
        let l = (self.a0.0 + frac * (self.a1.0 - self.a0.0)) * self.audio_level;
        let r = (self.a0.1 + frac * (self.a1.1 - self.a0.1)) * self.audio_level;
        self.n += 1;
        let w = TAU * 19_000.0 * t;
        let mpx = 0.9 * ((l + r) / 2.0 + (l - r) / 2.0 * (2.0 * w).sin())
            + 0.09 * w.sin()
            + self.rds_level * self.rds.sample() * (3.0 * w + 0.9).sin();
        self.phase += TAU * (self.offset + 75_000.0 * mpx) / self.fs;
        Complex32::new(
            (0.5 * self.phase.cos() + self.noise_amp * self.noise.gauss()) as f32,
            (0.5 * self.phase.sin() + self.noise_amp * self.noise.gauss()) as f32,
        )
    }
}

/// RTL-SDR front end: gain, DC offset, IQ gain imbalance, unsigned 8 bit.
fn cu8(iq: Complex32, gain: f32, dc: (f32, f32), imbalance: f32) -> Complex32 {
    let q = |v: f32| ((v * 127.5 + 127.5).round().clamp(0.0, 255.0) - 127.5) / 127.5;
    Complex32::new(
        q(iq.re * gain + dc.0),
        q(iq.im * gain * (1.0 + imbalance) + dc.1),
    )
}

/// Other stations in the band: constant-envelope FM carriers with sinusoidal modulation.
struct Crowd {
    carriers: Vec<(f64, f64, f64, f64)>,
    amp: f64,
    fs: f64,
    n: u64,
}

impl Crowd {
    /// `count` carriers spread over the band (not on `skip` +- 150 kHz), each with amplitude `amp`.
    fn new(count: usize, amp: f64, fs: f64, skip: f64) -> Self {
        let mut rng = Rng(0x5eed_1234_abcd);
        let mut carriers = Vec::new();
        while carriers.len() < count {
            let off = ((rng.uniform() + 0.5) * 0.9 - 0.45) * fs;
            if (off - skip).abs() < 150_000.0 || off.abs() < 40_000.0 {
                continue;
            }
            let mod_hz = 300.0 + 6_000.0 * (rng.uniform() + 0.5);
            carriers.push((off, 0.0, mod_hz, 2.0 * 70_000.0 / 3.0));
        }
        Self {
            carriers,
            amp,
            fs,
            n: 0,
        }
    }

    fn next(&mut self) -> Complex32 {
        let t = self.n as f64 / self.fs;
        self.n += 1;
        let mut sum = Complex32::new(0.0, 0.0);
        for (off, phase, mod_hz, dev) in &mut self.carriers {
            *phase += TAU * (*off + *dev * (TAU * *mod_hz * t).sin()) / self.fs;
            sum += Complex32::new(phase.cos() as f32, phase.sin() as f32) * self.amp as f32;
        }
        sum
    }
}

/// What the receiver sees.
#[derive(Clone, Copy)]
struct Scene {
    rate: u32,
    offset: f64,
    /// RDS subcarrier amplitude (fraction of the full deviation).
    rds_level: f64,
    /// Receiver noise amplitude per component before the ADC.
    noise_amp: f64,
    /// Programme level.
    audio: f64,
    /// Amplitude of the wanted carrier at the ADC input (1.0 = full scale).
    wanted: f64,
    /// Number of other stations and the amplitude of each.
    crowd: (usize, f64),
    seconds: f64,
}

struct Outcome {
    ps_at: Option<f64>,
    rt_at: Option<f64>,
    synced: bool,
    pilot_locked: bool,
    power_dbfs: f32,
    clipped: f64,
}

fn receive(sc: Scene) -> Outcome {
    let mut ch = Channel::new(ChannelConfig {
        input_rate: sc.rate,
        offset_hz: sc.offset,
        mode: Mode::Wfm {
            deemphasis: Deemphasis::Us50,
        },
        bandwidth_hz: None,
        squelch_dbfs: None,
    })
    .expect("channel");
    let mut st = Broadcast::new(sc.rate, sc.offset, sc.rds_level, sc.noise_amp, sc.audio);
    let mut crowd = Crowd::new(sc.crowd.0, sc.crowd.1, f64::from(sc.rate), sc.offset);
    let mut out = Vec::new();
    let mut got = Outcome {
        ps_at: None,
        rt_at: None,
        synced: false,
        pilot_locked: false,
        power_dbfs: 0.0,
        clipped: 0.0,
    };
    let mut clipped = 0usize;
    let mut total = 0usize;
    let step_samples = (f64::from(sc.rate) * 0.5) as usize;
    for step in 1..=(sc.seconds / 0.5) as usize {
        let mut left = step_samples;
        while left > 0 {
            let len = left.min(4099);
            let block: Vec<Complex32> = (0..len)
                .map(|_| {
                    let iq = st.next() * (2.0 * sc.wanted) as f32 + crowd.next();
                    let q = cu8(iq, 1.0, (0.02, -0.03), 0.05);
                    clipped += usize::from(q.re.abs() >= 1.0) + usize::from(q.im.abs() >= 1.0);
                    total += 2;
                    q
                })
                .collect();
            let stats = ch.process(&block, &mut out);
            got.power_dbfs = stats.power_dbfs;
            out.clear();
            left -= len;
        }
        let info = ch.rds_info().expect("wfm has rds");
        let t = step as f64 * 0.5;
        if info.ps.as_deref() == Some(PS) && got.ps_at.is_none() {
            got.ps_at = Some(t);
        }
        if info.rt.as_deref() == Some(RT) && got.rt_at.is_none() {
            got.rt_at = Some(t);
        }
    }
    got.synced = ch.rds_synced();
    got.pilot_locked = ch.stereo_locked();
    got.clipped = clipped as f64 / total as f64;
    got
}

/// A realistic scene: programme material, 8-bit samples with DC offset and IQ imbalance.
fn scene(rate: u32, offset: f64) -> Scene {
    Scene {
        rate,
        offset,
        rds_level: 0.03,
        noise_amp: 0.005,
        audio: 0.3,
        wanted: 0.3,
        crowd: (0, 0.0),
        seconds: 6.0,
    }
}

#[test]
fn rds_is_decoded_at_rtl_rates_with_the_station_off_centre() {
    for (rate, offset) in [(2_048_000, -510_000.0), (2_400_000, 790_000.0)] {
        let o = receive(scene(rate, offset));
        assert!(o.pilot_locked, "pilot at {rate}/{offset}");
        assert!(o.ps_at.is_some(), "PS at {rate}/{offset}");
        assert!(o.rt_at.is_some(), "RT at {rate}/{offset}");
        assert!(o.synced);
    }
}

#[test]
fn rds_survives_a_crowded_band_with_a_clipping_adc() {
    // Ten more stations on top of the wanted one: about 18 % of the 8-bit samples clip.
    let o = receive(Scene {
        wanted: 0.56,
        crowd: (10, 0.27),
        ..scene(2_400_000, -609_000.0)
    });
    assert!(
        o.clipped > 0.1,
        "scene should overload the ADC: {}",
        o.clipped
    );
    assert!(o.power_dbfs > -10.0);
    assert!(o.pilot_locked);
    assert!(o.ps_at.is_some(), "PS under clipping");
}
