//! Tone signalling on demodulated NFM / AM audio: CTCSS sub-tones and DTMF digits.
//!
//! The detector takes the 48 kHz mono audio of a [`Channel`](crate::Channel) (before the squelch
//! gate and after the DC block, so the sub-audible band is intact) and decimates it twice:
//!
//! - to 8 kHz for DTMF: eight Goertzel filters (697-941 Hz rows, 1209-1633 Hz columns) plus the
//!   second harmonics of the rows over a 205-sample window (25.6 ms) every 8 ms. A window counts
//!   as a digit when the two strongest tones dominate the window energy, each stands out from the
//!   rest of its group, the twist is within ITU-T Q.24 limits (+4 dB reverse, -8 dB normal) and the
//!   second harmonic of the row tone is small (talk-off guard against speech and music). A digit
//!   is reported after three consecutive hits (about 42 ms minimum tone length) and again only
//!   after at least two window hops without a hit (inter-digit pause of 34 ms or more) or when a
//!   different digit follows.
//! - to 2 kHz for CTCSS: 50 Hann-windowed Goertzel filters (the standard tones 67.0 to 254.1 Hz)
//!   over 0.5 s every 125 ms. The strongest tone must exceed the mean power of the tones further
//!   than 6.5 Hz away by 10.8 dB and must be stationary (both halves of the window carry similar
//!   power, which rejects a drifting voice pitch). It locks after two consecutive hits and is released after five
//!   misses, so a tone is reported about 0.6 s after it starts and drops about 0.6 s after it ends.
//!
//! Pure DSP: no I/O, no threads.

use crate::channel::OUTPUT_RATE;
use crate::fir::{DecimFir, design_lowpass};

/// The 50 standard CTCSS tones in Hz (EIA / TIA-603 plus the common extensions).
pub const CTCSS_TONES_HZ: [f32; 50] = [
    67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2,
    110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2,
    165.5, 167.9, 171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5,
    206.5, 210.7, 218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];

/// Digits kept in [`ToneDetector::dtmf_tail`].
pub const DTMF_TAIL_LEN: usize = 32;

/// Returns the standard CTCSS tone within 0.05 Hz of `hz`.
#[must_use]
pub fn canonical_ctcss(hz: f32) -> Option<f32> {
    CTCSS_TONES_HZ
        .iter()
        .copied()
        .find(|t| (t - hz).abs() <= 0.05)
}

const DTMF_RATE: f64 = 8_000.0;
const DTMF_N: usize = 205;
const DTMF_HOP: usize = 64;
const DTMF_ROWS: [f64; 4] = [697.0, 770.0, 852.0, 941.0];
const DTMF_COLS: [f64; 4] = [1209.0, 1336.0, 1477.0, 1633.0];
const DTMF_KEYS: [[char; 4]; 4] = [
    ['1', '2', '3', 'A'],
    ['4', '5', '6', 'B'],
    ['7', '8', '9', 'C'],
    ['*', '0', '#', 'D'],
];
/// Weakest accepted tone power (amplitude squared over two; about -34 dBFS per tone).
const DTMF_MIN_POWER: f64 = 2e-4;
/// The two tones must carry at least this share of the window energy.
const DTMF_DOMINANCE: f64 = 0.7;
/// A tone must exceed the runner-up of its group by this power ratio (10 dB).
const DTMF_GROUP_RATIO: f64 = 10.0;
/// The row tone must exceed its own second harmonic by this power ratio (10 dB).
const DTMF_HARMONIC_RATIO: f64 = 10.0;
/// Column power over row power: -8 dB (normal twist) ..= +4 dB (reverse twist).
const DTMF_TWIST: std::ops::RangeInclusive<f64> = 0.158..=2.512;
/// Consecutive hits that confirm a digit.
const DTMF_CONFIRM: u8 = 3;
/// Consecutive misses that end a digit.
const DTMF_RELEASE: u8 = 2;

const CTCSS_RATE: f64 = 2_000.0;
const CTCSS_N: usize = 1_000;
const CTCSS_HOP: usize = 250;
/// Tones closer than this to the peak (Hann main lobe) are left out of the noise reference.
const CTCSS_EXCLUDE_HZ: f32 = 6.5;
/// The peak must exceed the mean power of the reference tones by this ratio (10.8 dB).
const CTCSS_RATIO: f64 = 12.0;
/// Weakest accepted tone amplitude (about -48 dBFS).
const CTCSS_MIN_AMPLITUDE: f64 = 0.004;
/// Power of the weaker half window over the stronger one that still counts as stationary.
const CTCSS_STATIONARITY: f64 = 0.4;
/// Consecutive hits that lock a tone.
const CTCSS_LOCK: u8 = 2;
/// Consecutive misses that release the lock.
const CTCSS_UNLOCK: u8 = 5;

/// Goertzel recursion coefficient `2 cos(2 pi f / fs)`.
fn coeff(f: f64, fs: f64) -> f64 {
    2.0 * (std::f64::consts::TAU * f / fs).cos()
}

/// DTMF digit detector on 8 kHz audio.
struct Dtmf {
    /// Rows, columns, second harmonics of the rows.
    coeffs: [f64; 12],
    ring: Vec<f32>,
    pos: usize,
    filled: usize,
    since: usize,
    candidate: Option<(char, u8)>,
    active: Option<char>,
    misses: u8,
}

impl Dtmf {
    fn new() -> Self {
        let mut coeffs = [0.0; 12];
        for (i, c) in coeffs.iter_mut().enumerate() {
            let f = match i {
                0..4 => DTMF_ROWS[i],
                4..8 => DTMF_COLS[i - 4],
                _ => 2.0 * DTMF_ROWS[i - 8],
            };
            *c = coeff(f, DTMF_RATE);
        }
        Self {
            coeffs,
            ring: vec![0.0; DTMF_N],
            pos: 0,
            filled: 0,
            since: 0,
            candidate: None,
            active: None,
            misses: 0,
        }
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.pos = 0;
        self.filled = 0;
        self.since = 0;
        self.candidate = None;
        self.active = None;
        self.misses = 0;
    }

    fn process(&mut self, x: &[f32], digits: &mut Vec<char>) {
        for &s in x {
            self.ring[self.pos] = s;
            self.pos = (self.pos + 1) % DTMF_N;
            self.filled = (self.filled + 1).min(DTMF_N);
            self.since += 1;
            if self.filled == DTMF_N && self.since >= DTMF_HOP {
                self.since = 0;
                let hit = self.analyse();
                self.step(hit, digits);
            }
        }
    }

    /// Digit of the current window, if it passes all checks.
    fn analyse(&self) -> Option<char> {
        let (older, newer) = self.ring.split_at(self.pos);
        let mut s1 = [0.0f64; 12];
        let mut s2 = [0.0f64; 12];
        let mut energy = 0.0f64;
        for &x in newer.iter().chain(older) {
            let x = f64::from(x);
            energy += x * x;
            for ((a, b), c) in s1.iter_mut().zip(&mut s2).zip(&self.coeffs) {
                let s = x + c * *a - *b;
                *b = *a;
                *a = s;
            }
        }
        let n2 = (DTMF_N * DTMF_N) as f64;
        let mut power = [0.0f64; 12];
        for (((p, a), b), c) in power.iter_mut().zip(&s1).zip(&s2).zip(&self.coeffs) {
            *p = (a * a + b * b - c * a * b) * 2.0 / n2;
        }
        let total = energy / DTMF_N as f64;
        let (row, row_p, row_second) = strongest(&power[0..4]);
        let (col, col_p, col_second) = strongest(&power[4..8]);
        let ok = row_p >= DTMF_MIN_POWER
            && col_p >= DTMF_MIN_POWER
            && row_p + col_p >= DTMF_DOMINANCE * total
            && row_p >= DTMF_GROUP_RATIO * row_second
            && col_p >= DTMF_GROUP_RATIO * col_second
            && DTMF_TWIST.contains(&(col_p / row_p))
            && row_p >= DTMF_HARMONIC_RATIO * power[8 + row];
        ok.then(|| DTMF_KEYS[row][col])
    }

    fn step(&mut self, hit: Option<char>, digits: &mut Vec<char>) {
        match hit {
            Some(d) => {
                self.misses = 0;
                if self.active == Some(d) {
                    self.candidate = None;
                    return;
                }
                let n = match self.candidate {
                    Some((c, n)) if c == d => n + 1,
                    _ => 1,
                };
                if n >= DTMF_CONFIRM {
                    digits.push(d);
                    self.active = Some(d);
                    self.candidate = None;
                } else {
                    self.candidate = Some((d, n));
                }
            }
            None => {
                self.candidate = None;
                if self.active.is_some() {
                    self.misses += 1;
                    if self.misses >= DTMF_RELEASE {
                        self.active = None;
                        self.misses = 0;
                    }
                }
            }
        }
    }
}

/// Index, power and the runner-up power of the strongest entry.
fn strongest(p: &[f64]) -> (usize, f64, f64) {
    let mut best = 0;
    for (i, v) in p.iter().enumerate() {
        if *v > p[best] {
            best = i;
        }
    }
    let second = p
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != best)
        .map(|(_, v)| *v)
        .fold(0.0, f64::max);
    (best, p[best], second)
}

/// CTCSS detector on 2 kHz audio.
struct Ctcss {
    coeffs: [f64; 50],
    window: Vec<f64>,
    /// Hann window of half the length, for the stationarity check.
    half_window: Vec<f64>,
    ring: Vec<f32>,
    scratch: Vec<f64>,
    pos: usize,
    filled: usize,
    since: usize,
    candidate: Option<(usize, u8)>,
    locked: Option<usize>,
    misses: u8,
}

impl Ctcss {
    fn new() -> Self {
        let mut coeffs = [0.0; 50];
        for (c, f) in coeffs.iter_mut().zip(CTCSS_TONES_HZ) {
            *c = coeff(f64::from(f), CTCSS_RATE);
        }
        let window = (0..CTCSS_N)
            .map(|i| 0.5 * (1.0 - (std::f64::consts::TAU * i as f64 / (CTCSS_N - 1) as f64).cos()))
            .collect();
        let half_window = (0..CTCSS_N / 2)
            .map(|i| {
                0.5 * (1.0 - (std::f64::consts::TAU * i as f64 / (CTCSS_N / 2 - 1) as f64).cos())
            })
            .collect();
        Self {
            coeffs,
            window,
            half_window,
            ring: vec![0.0; CTCSS_N],
            scratch: vec![0.0; CTCSS_N],
            pos: 0,
            filled: 0,
            since: 0,
            candidate: None,
            locked: None,
            misses: 0,
        }
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.pos = 0;
        self.filled = 0;
        self.since = 0;
        self.candidate = None;
        self.locked = None;
        self.misses = 0;
    }

    fn process(&mut self, x: &[f32]) {
        for &s in x {
            self.ring[self.pos] = s;
            self.pos = (self.pos + 1) % CTCSS_N;
            self.filled = (self.filled + 1).min(CTCSS_N);
            self.since += 1;
            if self.filled == CTCSS_N && self.since >= CTCSS_HOP {
                self.since = 0;
                let hit = self.analyse();
                self.step(hit);
            }
        }
    }

    /// Index of the tone standing out in the current window, if any.
    fn analyse(&mut self) -> Option<usize> {
        let (older, newer) = self.ring.split_at(self.pos);
        for ((d, &x), w) in self
            .scratch
            .iter_mut()
            .zip(newer.iter().chain(older))
            .zip(&self.window)
        {
            *d = f64::from(x) * w;
        }
        let mut power = [0.0f64; 50];
        for (p, c) in power.iter_mut().zip(&self.coeffs) {
            let (mut s1, mut s2) = (0.0f64, 0.0f64);
            for &x in &self.scratch {
                let s = x + c * s1 - s2;
                s2 = s1;
                s1 = s;
            }
            *p = s1 * s1 + s2 * s2 - c * s1 * s2;
        }
        let mut best = 0;
        for (i, v) in power.iter().enumerate() {
            if *v > power[best] {
                best = i;
            }
        }
        let peak_hz = CTCSS_TONES_HZ[best];
        let (sum, n) = power
            .iter()
            .zip(CTCSS_TONES_HZ)
            .filter(|(_, f)| (f - peak_hz).abs() > CTCSS_EXCLUDE_HZ)
            .fold((0.0, 0u32), |(s, n), (p, _)| (s + p, n + 1));
        let reference = sum / f64::from(n.max(1));
        // Hann coherent gain is N / 2, a sine of amplitude A reads |X| = A N / 4.
        let amplitude = 4.0 * power[best].max(0.0).sqrt() / CTCSS_N as f64;
        if amplitude < CTCSS_MIN_AMPLITUDE || power[best] < CTCSS_RATIO * reference {
            return None;
        }
        // A real tone is stationary; a voice pitch drifts through the bin.
        let (older, newer) = self.ring.split_at(self.pos);
        let c = self.coeffs[best];
        let mut halves = [0.0f64; 2];
        let mut samples = newer.iter().chain(older);
        for h in &mut halves {
            let (mut s1, mut s2) = (0.0f64, 0.0f64);
            for (&x, w) in samples.by_ref().take(CTCSS_N / 2).zip(&self.half_window) {
                let s = f64::from(x) * w + c * s1 - s2;
                s2 = s1;
                s1 = s;
            }
            *h = s1 * s1 + s2 * s2 - c * s1 * s2;
        }
        let (lo, hi) = (halves[0].min(halves[1]), halves[0].max(halves[1]));
        (lo >= CTCSS_STATIONARITY * hi).then_some(best)
    }

    fn step(&mut self, hit: Option<usize>) {
        match hit {
            Some(i) if self.locked == Some(i) => {
                self.misses = 0;
                self.candidate = None;
            }
            Some(i) => {
                let n = match self.candidate {
                    Some((c, n)) if c == i => n + 1,
                    _ => 1,
                };
                if n >= CTCSS_LOCK {
                    self.locked = Some(i);
                    self.misses = 0;
                    self.candidate = None;
                } else {
                    self.candidate = Some((i, n));
                    self.miss();
                }
            }
            None => {
                self.candidate = None;
                self.miss();
            }
        }
    }

    fn miss(&mut self) {
        if self.locked.is_some() {
            self.misses += 1;
            if self.misses >= CTCSS_UNLOCK {
                self.locked = None;
                self.misses = 0;
            }
        }
    }
}

/// Per-channel CTCSS and DTMF detector; see the module docs.
pub struct ToneDetector {
    to16: DecimFir<f32>,
    to8: DecimFir<f32>,
    to2: DecimFir<f32>,
    buf16: Vec<f32>,
    buf8: Vec<f32>,
    buf2: Vec<f32>,
    dtmf: Dtmf,
    ctcss: Ctcss,
    digits: Vec<char>,
    count: u64,
    tail: String,
    /// The filters and windows hold audio (cleared by [`Self::idle`]).
    dirty: bool,
}

impl Default for ToneDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ToneDetector {
    /// Creates a detector for [`OUTPUT_RATE`] (48 kHz) mono audio.
    #[must_use]
    pub fn new() -> Self {
        let fs = f64::from(OUTPUT_RATE);
        Self {
            to16: DecimFir::new(design_lowpass(fs, 3_500.0, 12_500.0, 50.0), 3),
            to8: DecimFir::new(design_lowpass(fs / 3.0, 3_500.0, 4_500.0, 50.0), 2),
            to2: DecimFir::new(design_lowpass(DTMF_RATE, 300.0, 1_700.0, 50.0), 4),
            buf16: Vec::new(),
            buf8: Vec::new(),
            buf2: Vec::new(),
            dtmf: Dtmf::new(),
            ctcss: Ctcss::new(),
            digits: Vec::new(),
            count: 0,
            tail: String::new(),
            dirty: false,
        }
    }

    /// Feeds 48 kHz mono audio.
    pub fn process(&mut self, audio: &[f32]) {
        if audio.is_empty() {
            return;
        }
        self.dirty = true;
        self.buf16.clear();
        self.to16.process(audio, &mut self.buf16);
        self.buf8.clear();
        self.to8.process(&self.buf16, &mut self.buf8);
        self.dtmf.process(&self.buf8, &mut self.digits);
        self.buf2.clear();
        self.to2.process(&self.buf8, &mut self.buf2);
        self.ctcss.process(&self.buf2);
        for d in std::mem::take(&mut self.digits) {
            self.count += 1;
            self.tail.push(d);
            if self.tail.len() > DTMF_TAIL_LEN {
                self.tail.remove(0);
            }
        }
    }

    /// Drops the audio history and any detected tone, keeps the digit history. Call while there
    /// is nothing to listen to (squelch closed); cheap when repeated.
    pub fn idle(&mut self) {
        if self.dirty {
            self.reset_dsp();
        }
    }

    /// Drops everything including the digit history.
    pub fn reset(&mut self) {
        self.reset_dsp();
        self.count = 0;
        self.tail.clear();
    }

    fn reset_dsp(&mut self) {
        self.to16.reset();
        self.to8.reset();
        self.to2.reset();
        self.dtmf.reset();
        self.ctcss.reset();
        self.dirty = false;
    }

    /// The CTCSS tone currently locked, one of [`CTCSS_TONES_HZ`].
    #[must_use]
    pub fn ctcss_hz(&self) -> Option<f32> {
        self.ctcss.locked.map(|i| CTCSS_TONES_HZ[i])
    }

    /// Digits detected since the last [`Self::reset`].
    #[must_use]
    pub fn dtmf_count(&self) -> u64 {
        self.count
    }

    /// The last (up to [`DTMF_TAIL_LEN`]) detected digits, oldest first.
    #[must_use]
    pub fn dtmf_tail(&self) -> &str {
        &self.tail
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;

    const FS: f64 = 48_000.0;

    /// Deterministic white noise in `-1..1`.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        }
    }

    fn detect(audio: &[f32]) -> ToneDetector {
        let mut det = ToneDetector::new();
        // Awkward block sizes on purpose.
        let mut rest = audio;
        for size in [480, 1, 977, 4096, 33].iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let (a, b) = rest.split_at((*size).min(rest.len()));
            det.process(a);
            rest = b;
        }
        det
    }

    fn sine(f: f64, amp: f64, n: usize, out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o += (amp * (TAU * f * (n + i) as f64 / FS).sin()) as f32;
        }
    }

    /// DTMF burst train: each key `on_ms` long, `off_ms` of silence after it, noise on top.
    fn dtmf_train(keys: &str, on_ms: usize, off_ms: usize, amp: f64, noise: f64) -> Vec<f32> {
        let ms = (FS / 1000.0) as usize;
        let mut audio = vec![0.0f32; 100 * ms];
        for key in keys.chars() {
            let (r, c) = DTMF_KEYS
                .iter()
                .enumerate()
                .find_map(|(r, row)| row.iter().position(|k| *k == key).map(|c| (r, c)))
                .expect("valid key");
            let start = audio.len();
            audio.resize(start + on_ms * ms, 0.0);
            sine(DTMF_ROWS[r], amp, start, &mut audio[start..]);
            sine(DTMF_COLS[c], amp * 1.2, start, &mut audio[start..]);
            audio.resize(audio.len() + off_ms * ms, 0.0);
        }
        audio.resize(audio.len() + 200 * ms, 0.0);
        let mut rng = Noise(0x9E37_79B9_7F4A_7C15);
        for s in &mut audio {
            *s += (noise * rng.next()) as f32;
        }
        audio
    }

    #[test]
    fn standard_tone_table_is_sorted_and_complete() {
        assert_eq!(CTCSS_TONES_HZ.len(), 50);
        assert!(CTCSS_TONES_HZ.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(canonical_ctcss(88.5), Some(88.5));
        assert_eq!(canonical_ctcss(88.52), Some(88.5));
        assert_eq!(canonical_ctcss(90.0), None);
    }

    #[test]
    fn decodes_every_key_of_a_clean_sequence() {
        let keys = "123A456B789C*0#D";
        let det = detect(&dtmf_train(keys, 60, 60, 0.2, 0.0));
        assert_eq!(det.dtmf_tail(), keys);
        assert_eq!(det.dtmf_count(), 16);
    }

    #[test]
    fn repeated_digits_need_the_inter_digit_pause_only() {
        let det = detect(&dtmf_train("5551100", 50, 40, 0.2, 0.0));
        assert_eq!(det.dtmf_tail(), "5551100");
    }

    #[test]
    fn decodes_short_digits_in_noise() {
        // 45 ms tones at 0.1 amplitude with noise of about 0.1 rms (SNR a few dB per tone).
        let keys = "0123456789*#";
        let det = detect(&dtmf_train(keys, 45, 45, 0.1, 0.25));
        assert_eq!(det.dtmf_tail(), keys);
    }

    #[test]
    fn rejects_bursts_shorter_than_the_minimum() {
        let det = detect(&dtmf_train("1234", 15, 60, 0.3, 0.02));
        assert_eq!(det.dtmf_count(), 0, "{}", det.dtmf_tail());
    }

    #[test]
    fn ignores_twisted_and_single_tones() {
        let ms = 48;
        let mut audio = vec![0.0f32; 1000 * ms];
        // Row 697 Hz 20 dB above column 1209 Hz: far outside the twist window.
        sine(697.0, 0.4, 0, &mut audio[..300 * ms]);
        sine(1209.0, 0.04, 0, &mut audio[..300 * ms]);
        // A single tone and two tones of the same group.
        sine(941.0, 0.4, 0, &mut audio[300 * ms..500 * ms]);
        sine(770.0, 0.3, 0, &mut audio[500 * ms..700 * ms]);
        sine(852.0, 0.3, 0, &mut audio[500 * ms..700 * ms]);
        let det = detect(&audio);
        assert_eq!(det.dtmf_count(), 0, "{}", det.dtmf_tail());
    }

    /// Speech-like signal: harmonic stacks with a gliding pitch, formant weighting and pauses.
    fn speech_like(seconds: usize, seed: u64) -> Vec<f32> {
        let mut rng = Noise(seed);
        let n = seconds * FS as usize;
        let mut audio = vec![0.0f32; n];
        let mut phase = 0.0f64;
        let mut f0 = 120.0f64;
        let (mut f1, mut f2) = (700.0f64, 1_300.0f64);
        for (i, o) in audio.iter_mut().enumerate() {
            if i % 2400 == 0 {
                f0 = (f0 + 25.0 * rng.next()).clamp(80.0, 260.0);
                f1 = 300.0 + 700.0 * (rng.next() + 1.0) / 2.0;
                f2 = 900.0 + 1_800.0 * (rng.next() + 1.0) / 2.0;
            }
            phase += TAU * f0 / FS;
            let mut v = 0.0;
            for h in 1..=30 {
                let fh = f0 * f64::from(h);
                let g1 = 1.0 / (1.0 + ((fh - f1) / 120.0).powi(2));
                let g2 = 0.6 / (1.0 + ((fh - f2) / 200.0).powi(2));
                v += (g1 + g2) * (phase * f64::from(h)).sin();
            }
            let gate = if (i / 24_000) % 5 == 4 { 0.0 } else { 1.0 };
            *o = (0.08 * gate * v + 0.03 * rng.next()) as f32;
        }
        audio
    }

    #[test]
    fn speech_like_audio_produces_no_digits_and_no_tone() {
        for seed in 1..=6 {
            let det = detect(&speech_like(25, seed * 7919));
            assert_eq!(det.dtmf_count(), 0, "seed {seed}: {}", det.dtmf_tail());
            assert_eq!(det.ctcss_hz(), None, "seed {seed}");
        }
    }

    #[test]
    fn music_like_audio_produces_no_digits() {
        // Chords of harmonic notes around the DTMF frequencies, changing every 150 ms.
        let notes = [
            261.6, 293.7, 329.6, 349.2, 392.0, 440.0, 493.9, 523.3, 587.3, 659.3, 698.5, 784.0,
            880.0, 987.8, 1046.5, 1174.7, 1318.5, 1480.0,
        ];
        let mut rng = Noise(42);
        let n = 60 * FS as usize;
        let mut audio = vec![0.0f32; n];
        let mut chord = [0usize; 3];
        for start in (0..n).step_by(7200) {
            for c in &mut chord {
                *c = ((rng.next() + 1.0) / 2.0 * notes.len() as f64) as usize % notes.len();
            }
            let end = (start + 7200).min(n);
            for &c in &chord {
                for h in 1..=5 {
                    sine(
                        notes[c] * f64::from(h),
                        0.12 / f64::from(h),
                        start,
                        &mut audio[start..end],
                    );
                }
            }
        }
        let det = detect(&audio);
        assert_eq!(det.dtmf_count(), 0, "{}", det.dtmf_tail());
    }

    fn tone_audio(f: f64, amp: f64, noise: f64, seconds: f64) -> Vec<f32> {
        let n = (seconds * FS) as usize;
        let mut audio = vec![0.0f32; n];
        sine(f, amp, 0, &mut audio);
        let mut rng = Noise(0xDEAD_BEEF_1234_5679);
        for s in &mut audio {
            *s += (noise * rng.next()) as f32;
        }
        audio
    }

    #[test]
    fn locks_to_the_right_standard_tone() {
        for &tone in &CTCSS_TONES_HZ {
            let det = detect(&tone_audio(f64::from(tone), 0.1, 0.05, 2.0));
            assert_eq!(det.ctcss_hz(), Some(tone), "{tone} Hz");
        }
    }

    #[test]
    fn separates_neighbouring_tones() {
        for (a, b) in [(88.5, 91.5), (67.0, 69.3), (159.8, 162.2), (177.3, 179.9)] {
            let da = detect(&tone_audio(a, 0.1, 0.02, 1.5));
            let db = detect(&tone_audio(b, 0.1, 0.02, 1.5));
            assert_eq!(da.ctcss_hz(), Some(a as f32));
            assert_eq!(db.ctcss_hz(), Some(b as f32));
        }
    }

    #[test]
    fn tone_rides_under_speech_above_300_hz() {
        // TX audio is high-passed at 300 Hz, so voice energy below that does not exist.
        let mut audio = speech_like(8, 99);
        // Crude high-pass: remove everything below the second harmonic region by differencing.
        let mut prev = 0.0f32;
        for s in &mut audio {
            let x = *s;
            *s = x - prev * 0.97;
            prev = x;
        }
        let tone = tone_audio(94.8, 0.08, 0.0, 8.0);
        for (s, t) in audio.iter_mut().zip(&tone) {
            *s += *t;
        }
        let det = detect(&audio);
        assert_eq!(det.ctcss_hz(), Some(94.8));
    }

    #[test]
    fn noise_never_locks_and_a_vanished_tone_is_released() {
        let mut rng = Noise(5);
        let noise: Vec<f32> = (0..60 * FS as usize)
            .map(|_| (0.3 * rng.next()) as f32)
            .collect();
        assert_eq!(detect(&noise).ctcss_hz(), None);
        assert_eq!(detect(&noise).dtmf_count(), 0);

        let mut audio = tone_audio(123.0, 0.1, 0.0, 2.0);
        audio.extend(std::iter::repeat_n(0.0, 2 * FS as usize));
        let mut det = ToneDetector::new();
        det.process(&audio[..2 * FS as usize]);
        assert_eq!(det.ctcss_hz(), Some(123.0));
        det.process(&audio[2 * FS as usize..]);
        assert_eq!(det.ctcss_hz(), None);
    }

    #[test]
    fn idle_clears_the_tone_but_keeps_digits_and_reset_clears_both() {
        let mut audio = dtmf_train("42", 60, 60, 0.2, 0.0);
        audio.resize(3 * FS as usize, 0.0);
        for (s, t) in audio.iter_mut().zip(tone_audio(100.0, 0.1, 0.0, 3.0)) {
            *s += t;
        }
        let mut det = detect(&audio);
        assert_eq!(det.dtmf_tail(), "42");
        assert_eq!(det.ctcss_hz(), Some(100.0));
        det.idle();
        assert_eq!(det.ctcss_hz(), None);
        assert_eq!(det.dtmf_tail(), "42");
        det.reset();
        assert_eq!((det.dtmf_count(), det.dtmf_tail()), (0, ""));
    }

    #[test]
    fn tail_is_bounded() {
        let keys = "1234567890".repeat(5);
        let det = detect(&dtmf_train(&keys, 50, 50, 0.2, 0.0));
        assert_eq!(det.dtmf_count(), 50);
        assert_eq!(det.dtmf_tail().len(), DTMF_TAIL_LEN);
        assert!(keys.ends_with(det.dtmf_tail()));
    }
}
