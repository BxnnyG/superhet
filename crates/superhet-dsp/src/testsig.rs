//! Synthetic broadcast-FM signals for tests: an RDS encoder (groups to bits to the shaped
//! baseband waveform) and a station generator (stereo multiplex with pilot and RDS, FM
//! modulated to IQ). Compiled for tests and with the `testsig` feature only.

use std::f64::consts::TAU;

use num_complex::Complex32;

use crate::rds::{BIT_RATE_HZ, OFFSETS, RT_END, encode_block};
use crate::wfm::PILOT_HZ;

/// Group type 0A: PS segment `seg` (0..=3) with two characters.
pub fn group_0a(pi: u16, pty: u8, seg: u16, chars: [u8; 2]) -> [u16; 4] {
    let b = (pty as u16) << 5 | 0x0008 | seg;
    [pi, b | 0x0400, 0x0000, u16::from_be_bytes(chars)]
}

/// Group type 2A: RadioText segment `seg` (0..=15) with four characters.
pub fn group_2a(pi: u16, pty: u8, ab: bool, seg: u16, chars: [u8; 4]) -> [u16; 4] {
    let b = 0x2000 | (pty as u16) << 5 | u16::from(ab) << 4 | seg | 0x0400;
    [
        pi,
        b,
        u16::from_be_bytes([chars[0], chars[1]]),
        u16::from_be_bytes([chars[2], chars[3]]),
    ]
}

/// The groups of one PS plus a RadioText cycle.
pub fn cycle(pi: u16, pty: u8, ps: &str, rt: &str) -> Vec<[u16; 4]> {
    let mut groups = Vec::new();
    let ps = ps.as_bytes();
    for seg in 0..4u16 {
        let i = usize::from(seg) * 2;
        groups.push(group_0a(pi, pty, seg, [ps[i], ps[i + 1]]));
    }
    let mut text = rt.as_bytes().to_vec();
    text.push(RT_END);
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    for (seg, chunk) in text.chunks(4).enumerate() {
        groups.push(group_2a(
            pi,
            pty,
            false,
            seg as u16,
            [chunk[0], chunk[1], chunk[2], chunk[3]],
        ));
    }
    groups
}

/// Bits of the groups (offset words A, B, C, D), not yet differentially encoded.
pub fn group_bits(groups: &[[u16; 4]]) -> Vec<bool> {
    let mut bits = Vec::new();
    for g in groups {
        for (i, &info) in g.iter().enumerate() {
            let offset = OFFSETS[[0, 1, 2, 4][i]];
            let block = encode_block(info, offset);
            for k in (0..26).rev() {
                bits.push(block >> k & 1 == 1);
            }
        }
    }
    bits
}

/// Differential encoding: `y[k] = x[k] xor y[k-1]`.
pub fn differential(bits: &[bool]) -> Vec<bool> {
    let mut prev = false;
    bits.iter()
        .map(|&x| {
            prev ^= x;
            prev
        })
        .collect()
}

/// Biphase waveform sampled at `fs`: each bit is one cycle of a 1187.5 Hz sine, signed by
/// the bit. Returns an endless generator over the repeated `bits`.
pub struct Baseband {
    bits: Vec<bool>,
    fs: f64,
    n: u64,
}

impl Baseband {
    pub fn new(bits: Vec<bool>, fs: f64) -> Self {
        Self { bits, fs, n: 0 }
    }

    pub fn next(&mut self) -> f64 {
        let t = self.n as f64 / self.fs * BIT_RATE_HZ;
        self.n += 1;
        let k = t.floor();
        let frac = t - k;
        let bit = self.bits[(k as usize) % self.bits.len()];
        let s = (std::f64::consts::TAU * frac).sin();
        if bit { s } else { -s }
    }
}

/// Synthetic station.
pub struct Station {
    /// Sample rate in Hz.
    pub rate: f64,
    offset: f64,
    n: u64,
    fm_phase: f64,
    rds: Baseband,
    noise: u32,
    pub noise_amp: f32,
}

/// What the station transmits.
#[derive(Clone, Copy, Debug)]
pub struct Content {
    /// Amplitude of the 1 kHz tone in the left channel.
    pub left: f64,
    /// Amplitude of the 3 kHz tone in the right channel.
    pub right: f64,
    /// Pilot amplitude (full deviation = 1.0, nominally 0.1).
    pub pilot: f64,
    /// RDS subcarrier amplitude (nominally 0.01 to 0.04).
    pub rds: f64,
}

impl Station {
    /// A station at `offset` Hz from the centre of a signal sampled at `rate`, sending `ps` and
    /// `rt` repeatedly.
    #[must_use]
    pub fn new(rate: u32, offset: f64, ps: &str, rt: &str) -> Self {
        let groups = cycle(0xD3C2, 10, ps, rt);
        let bits = differential(&group_bits(&groups));
        Self {
            rate: f64::from(rate),
            offset,
            n: 0,
            fm_phase: 0.0,
            rds: Baseband::new(bits, f64::from(rate)),
            noise: 0x9E37_79B9,
            noise_amp: 0.0,
        }
    }

    fn noise(&mut self) -> f32 {
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        (f64::from(self.noise) / f64::from(u32::MAX) - 0.5) as f32
    }

    /// Next IQ sample.
    pub fn next(&mut self, c: Content) -> Complex32 {
        let t = self.n as f64 / self.rate;
        self.n += 1;
        let l = c.left * (TAU * 1_000.0 * t).sin();
        let r = c.right * (TAU * 3_000.0 * t).sin();
        let w = TAU * PILOT_HZ * t;
        let rds_bit = self.rds.next();
        // The RDS data sits at an arbitrary fixed phase relative to the third pilot harmonic.
        let mpx = 0.9 * ((l + r) / 2.0 + (l - r) / 2.0 * (2.0 * w).sin())
            + c.pilot * w.sin()
            + c.rds * rds_bit * (3.0 * w + 0.9).sin();
        self.fm_phase += TAU * (self.offset + 75_000.0 * mpx) / self.rate;
        let iq = Complex32::new(
            (0.5 * self.fm_phase.cos()) as f32,
            (0.5 * self.fm_phase.sin()) as f32,
        );
        if self.noise_amp > 0.0 {
            iq + Complex32::new(self.noise(), self.noise()) * self.noise_amp
        } else {
            iq
        }
    }
}
