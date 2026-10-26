//! End-to-end tone detection on a synthetic PMR446-style NFM signal: complex IQ with 12.5 kHz
//! channel spacing, +-2.5 kHz peak deviation, pre-emphasised audio, thermal noise, a CTCSS
//! sub-tone and DTMF digits, run through the real channel (NCO, filters, discriminator, audio
//! chain) and its tone detector.

use std::f64::consts::TAU;

use num_complex::Complex32;
use superhet_dsp::{Channel, ChannelConfig, Deemphasis, Mode};

/// Peak deviation of the transmitter; the receiver scales 5 kHz to full scale.
const PEAK_DEV_HZ: f64 = 2_500.0;
const AUDIO_RATE: f64 = 48_000.0;

struct Rng(u64);

impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        (-2.0 * self.uniform().ln()).sqrt() * (TAU * self.uniform()).cos()
    }
}

const ROWS: [f64; 4] = [697.0, 770.0, 852.0, 941.0];
const COLS: [f64; 4] = [1209.0, 1336.0, 1477.0, 1633.0];
const KEYS: &str = "123A456B789C*0#D";

fn key_tones(key: char) -> (f64, f64) {
    let i = KEYS.find(key).expect("valid DTMF key");
    (ROWS[i / 4], COLS[i % 4])
}

struct Scenario {
    keys: &'static str,
    on_s: f64,
    off_s: f64,
    /// Column over row level in dB (positive = reverse twist) at the modulator input.
    twist_db: f64,
    /// Pre-emphasis time constant in seconds (0 = none).
    preemph_tau_s: f64,
    /// Receiver de-emphasis.
    deemphasis: Deemphasis,
    /// SNR in the 12.5 kHz channel in dB (`None` = noise free).
    snr_db: Option<f64>,
    /// DTMF level as a share of the peak deviation (both tones together).
    dtmf_level: f64,
    /// CTCSS tone and its share of the peak deviation.
    ctcss: Option<(f64, f64)>,
    /// A "voice" burst of this length before the digits.
    voice_s: f64,
    /// Mean pitch of the "voice" in Hz.
    voice_f0: f64,
    squelch_dbfs: Option<f32>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            keys: "5#",
            on_s: 0.06,
            off_s: 0.06,
            twist_db: 0.0,
            preemph_tau_s: 0.0,
            deemphasis: Deemphasis::None,
            snr_db: Some(18.0),
            dtmf_level: 0.5,
            ctcss: Some((88.5, 0.1)),
            voice_s: 0.0,
            voice_f0: 140.0,
            squelch_dbfs: None,
        }
    }
}

struct Outcome {
    tail: String,
    ctcss: Option<f32>,
}

/// Modulating signal (1.0 = peak deviation) at the audio rate.
fn message(sc: &Scenario, secs: f64) -> Vec<f64> {
    let n = (secs * AUDIO_RATE) as usize;
    let mut m = vec![0.0f64; n];
    let col_gain = 10f64.powf(sc.twist_db / 20.0);
    let norm = sc.dtmf_level / (1.0 + col_gain);
    let t0 = 0.2 + sc.voice_s;
    for (k, key) in sc.keys.chars().enumerate() {
        let (row, col) = key_tones(key);
        let start = t0 + k as f64 * (sc.on_s + sc.off_s);
        let a = (start * AUDIO_RATE) as usize;
        let b = ((start + sc.on_s) * AUDIO_RATE) as usize;
        for (i, v) in m.iter_mut().enumerate().take(b.min(n)).skip(a) {
            let t = i as f64 / AUDIO_RATE;
            *v += norm * ((TAU * row * t).sin() + col_gain * (TAU * col * t).sin());
        }
    }
    if sc.voice_s > 0.0 {
        // Vowel-like harmonic stack with a wandering pitch and a syllable envelope.
        let mut phase = 0.0f64;
        let end = ((0.2 + sc.voice_s) * AUDIO_RATE) as usize;
        for (i, v) in m.iter_mut().enumerate().take(end) {
            let t = i as f64 / AUDIO_RATE;
            if t < 0.2 {
                continue;
            }
            let f0 = sc.voice_f0 + 0.3 * sc.voice_f0 * (TAU * 1.7 * t).sin();
            phase += TAU * f0 / AUDIO_RATE;
            let env = 0.5 + 0.5 * (TAU * 4.0 * t).sin();
            let s: f64 = (1..=14)
                .map(|h| (phase * f64::from(h)).sin() / f64::from(h).powf(1.2))
                .sum();
            *v += 0.2 * env * s;
        }
    }
    if let Some((hz, share)) = sc.ctcss {
        for (i, v) in m.iter_mut().enumerate() {
            *v += share * (TAU * hz * i as f64 / AUDIO_RATE).sin();
        }
    }
    if sc.preemph_tau_s > 0.0 {
        // Pre-emphasis y = x + tau * dx/dt, normalised to unity gain at 300 Hz.
        let c = sc.preemph_tau_s * AUDIO_RATE;
        let norm = 1.0 / (1.0 + (TAU * 300.0 * sc.preemph_tau_s).powi(2)).sqrt();
        let mut prev = 0.0;
        for v in &mut m {
            let x = *v;
            *v = (x + c * (x - prev)) * norm;
            prev = x;
        }
    }
    // A radio's limiter keeps the deviation within the channel.
    for v in &mut m {
        *v = v.clamp(-1.0, 1.0);
    }
    m
}

fn iq(sc: &Scenario, rate: u32, offset_hz: f64, secs: f64) -> Vec<Complex32> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let m = message(sc, secs);
    let fs = f64::from(rate);
    let n = (secs * fs) as usize;
    // Carrier amplitude 0.5 (about -6 dBFS); noise per I/Q component from the SNR in 12.5 kHz.
    let amp = 0.5;
    let sigma = sc.snr_db.map_or(0.0, |snr| {
        let noise_total = amp * amp / 10f64.powf(snr / 10.0) * fs / 12_500.0;
        (noise_total / 2.0).sqrt()
    });
    let mut phase = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // Linear interpolation of the 48 kHz message.
        let pos = i as f64 / fs * AUDIO_RATE;
        let k = (pos as usize).min(m.len() - 2);
        let frac = pos - k as f64;
        let mv = m[k] * (1.0 - frac) + m[k + 1] * frac;
        phase += TAU * (offset_hz + PEAK_DEV_HZ * mv) / fs;
        out.push(Complex32::new(
            (amp * phase.cos() + sigma * rng.gauss()) as f32,
            (amp * phase.sin() + sigma * rng.gauss()) as f32,
        ));
    }
    out
}

fn run(sc: &Scenario, rate: u32, offset_hz: f64, secs: f64) -> Outcome {
    let signal = iq(sc, rate, offset_hz, secs);
    let mut ch = Channel::new(ChannelConfig {
        input_rate: rate,
        offset_hz,
        mode: Mode::Nfm {
            deemphasis: sc.deemphasis,
        },
        bandwidth_hz: None,
        squelch_dbfs: sc.squelch_dbfs,
    })
    .expect("valid config");
    ch.set_tones(true);
    let mut out = Vec::new();
    for block in signal.chunks(rate as usize / 20) {
        ch.process(block, &mut out);
    }
    let tones = ch.tones().expect("detector runs for nfm");
    Outcome {
        tail: tones.dtmf_tail().to_owned(),
        ctcss: tones.ctcss_hz(),
    }
}

const ALL_KEYS: &str = "123A456B789C*0#D";

/// A handheld's DTMF: 60 ms tones, 4 dB reverse twist from the pre-emphasis, voice just before,
/// CTCSS underneath, 15 dB SNR. Every key must come out exactly once and in order.
#[test]
fn every_key_of_a_handheld_is_decoded_under_ctcss_with_noise() {
    let sc = Scenario {
        keys: ALL_KEYS,
        preemph_tau_s: 530e-6,
        twist_db: 2.0,
        dtmf_level: 0.3,
        snr_db: Some(15.0),
        voice_s: 0.5,
        ..Scenario::default()
    };
    let o = run(
        &sc,
        240_000,
        0.0,
        0.4 + sc.voice_s + 16.0 * (sc.on_s + sc.off_s),
    );
    assert_eq!(o.tail, ALL_KEYS);
    assert_eq!(o.ctcss, Some(88.5));
}

/// The same at the RTL-SDR rate, with the VFO off the device centre and mistuned by 1.2 kHz,
/// 40 ms tones and a plain receiver (no de-emphasis) on a pre-emphasised transmitter.
#[test]
fn short_digits_survive_rtl_rate_offset_and_mistuning() {
    let sc = Scenario {
        keys: ALL_KEYS,
        on_s: 0.04,
        off_s: 0.04,
        preemph_tau_s: 530e-6,
        dtmf_level: 0.3,
        snr_db: Some(15.0),
        ..Scenario::default()
    };
    let o = run(&sc, 2_400_000, -300_000.0 + 1_200.0, 0.4 + 16.0 * 0.08);
    assert_eq!(o.tail, ALL_KEYS);
    assert_eq!(o.ctcss, Some(88.5));
}

/// The receiver's de-emphasis tilts the other way; both ends of the range must work.
#[test]
fn digits_work_with_receiver_deemphasis() {
    let sc = Scenario {
        keys: ALL_KEYS,
        preemph_tau_s: 530e-6,
        deemphasis: Deemphasis::Us75,
        dtmf_level: 0.3,
        ..Scenario::default()
    };
    let o = run(&sc, 240_000, 0.0, 0.4 + 16.0 * 0.12);
    assert_eq!(o.tail, ALL_KEYS);
}

/// Quiet digits (about 25 % of the peak deviation) in the same noise.
#[test]
fn quiet_digits_are_decoded() {
    let sc = Scenario {
        keys: ALL_KEYS,
        dtmf_level: 0.25,
        ..Scenario::default()
    };
    let o = run(&sc, 240_000, 0.0, 0.4 + 16.0 * 0.12);
    assert_eq!(o.tail, ALL_KEYS);
}

#[test]
fn ctcss_is_found_under_voice_and_noise() {
    for hz in [67.0, 88.5, 123.0, 162.2, 210.7, 254.1] {
        let sc = Scenario {
            keys: "",
            ctcss: Some((hz, 0.08)),
            voice_s: 3.0,
            snr_db: Some(15.0),
            ..Scenario::default()
        };
        let o = run(&sc, 240_000, 0.0, 3.6);
        assert_eq!(o.ctcss, Some(hz as f32), "tone {hz}");
        assert_eq!(o.tail, "");
    }
}

/// Voice and noise must not produce digits, and noise must not lock a sub-tone.
#[test]
fn voice_and_noise_produce_no_digits() {
    for f0 in [90.0, 140.0, 220.0, 310.0] {
        for ctcss in [None, Some((88.5, 0.1))] {
            let sc = Scenario {
                keys: "",
                ctcss,
                voice_s: 8.0,
                voice_f0: f0,
                ..Scenario::default()
            };
            let o = run(&sc, 240_000, 0.0, 8.5);
            assert_eq!(o.tail, "", "voice at {f0} Hz");
        }
    }
    for snr in [3.0, 8.0, 14.0] {
        let sc = Scenario {
            keys: "",
            ctcss: None,
            snr_db: Some(snr),
            ..Scenario::default()
        };
        let o = run(&sc, 240_000, 0.0, 8.0);
        assert_eq!(o.tail, "", "noise at {snr} dB");
        assert_eq!(o.ctcss, None, "noise at {snr} dB");
    }
}
