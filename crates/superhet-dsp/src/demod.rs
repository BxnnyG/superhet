//! Demodulators and audio post-processing blocks (all streaming, state carried across calls).

use num_complex::Complex32;

use crate::nco::Nco;

/// Reference peak deviation for NFM (full-scale audio at 5 kHz deviation).
pub const NFM_REF_DEVIATION_HZ: f32 = 5_000.0;
/// Peak deviation of broadcast FM (full-scale audio at 75 kHz).
pub const WFM_DEVIATION_HZ: f32 = 75_000.0;

/// Polar (quadrature) FM discriminator: `arg(x[n] * conj(x[n-1]))`, scaled so that a
/// deviation of `ref_deviation_hz` yields +-1.0.
#[derive(Debug, Clone)]
pub struct FmDemod {
    prev: Complex32,
    scale: f32,
}

impl FmDemod {
    /// Creates a discriminator for complex input at `rate_hz`.
    pub fn new(rate_hz: f64, ref_deviation_hz: f32) -> Self {
        Self {
            prev: Complex32::new(1.0, 0.0),
            scale: (rate_hz / (std::f64::consts::TAU * f64::from(ref_deviation_hz))) as f32,
        }
    }

    /// Clears the one-sample memory.
    pub fn reset(&mut self) {
        self.prev = Complex32::new(1.0, 0.0);
    }

    /// Demodulates `iq`, appending to `out`.
    pub fn process(&mut self, iq: &[Complex32], out: &mut Vec<f32>) {
        out.reserve(iq.len());
        for &x in iq {
            let d = x * self.prev.conj();
            out.push(d.im.atan2(d.re) * self.scale);
            self.prev = x;
        }
    }
}

/// Peak the AGC steers the envelope towards (after DC removal: +-half of it).
const AM_TARGET_PEAK: f32 = 0.9;
/// Lowest carrier level the AGC divides by (limits gain to `target / floor`).
const AM_LEVEL_FLOOR: f32 = 1e-3;
const AM_ATTACK_S: f64 = 0.005;
const AM_RELEASE_S: f64 = 1.0;
const AM_DC_HZ: f64 = 20.0;

/// Envelope AM demodulator with peak-tracking AGC and DC removal.
#[derive(Debug, Clone)]
pub struct AmDemod {
    level: f32,
    primed: bool,
    attack: f32,
    release: f32,
    dc: DcBlock,
}

impl AmDemod {
    /// Creates an AM demodulator for complex input at `rate_hz`.
    pub fn new(rate_hz: f64) -> Self {
        Self {
            level: 0.0,
            primed: false,
            attack: one_pole_coeff(rate_hz, AM_ATTACK_S),
            release: one_pole_coeff(rate_hz, AM_RELEASE_S),
            dc: DcBlock::new(rate_hz, AM_DC_HZ),
        }
    }

    /// Forgets the AGC level and DC estimate.
    pub fn reset(&mut self) {
        self.level = 0.0;
        self.primed = false;
        self.dc.reset();
    }

    /// Demodulates `iq`, appending to `out`.
    pub fn process(&mut self, iq: &[Complex32], out: &mut Vec<f32>) {
        out.reserve(iq.len());
        for &x in iq {
            let env = x.norm();
            if !self.primed {
                self.level = env;
                self.primed = true;
            }
            let a = if env > self.level {
                self.attack
            } else {
                self.release
            };
            self.level += a * (env - self.level);
            let gain = AM_TARGET_PEAK / self.level.max(AM_LEVEL_FLOOR);
            out.push(self.dc.process(env * gain).clamp(-1.0, 1.0));
        }
    }
}

/// Output peak the SSB / CW AGC steers towards.
const SSB_TARGET_PEAK: f32 = 0.7;
/// Lowest signal level the SSB / CW AGC divides by (limits gain to `target / floor`).
const SSB_LEVEL_FLOOR: f32 = 1e-3;

/// Time constants of a [`PeakAgc`].
#[derive(Debug, Clone, Copy)]
pub struct AgcTimes {
    /// Attack time constant in seconds (level rising).
    pub attack_s: f64,
    /// Time the level is held after the last peak before it may decay.
    pub hang_s: f64,
    /// Decay time constant in seconds (level falling, after the hang).
    pub decay_s: f64,
}

impl AgcTimes {
    /// SSB speech: fast attack, 250 ms hang, slow decay.
    pub const SSB: Self = Self {
        attack_s: 0.002,
        hang_s: 0.25,
        decay_s: 0.4,
    };
    /// CW: a little slower attack (no pumping on the tone's own ripple), shorter hang and decay
    /// so keying gaps do not blow up the noise for long.
    pub const CW: Self = Self {
        attack_s: 0.003,
        hang_s: 0.1,
        decay_s: 0.2,
    };
}

/// Peak-tracking AGC with fast attack, hang and slow decay, operating on a real signal.
#[derive(Debug, Clone)]
pub struct PeakAgc {
    level: f32,
    hang_left: u32,
    hang: u32,
    attack: f32,
    decay: f32,
}

impl PeakAgc {
    /// Creates an AGC for a signal at `rate_hz`.
    pub fn new(rate_hz: f64, times: AgcTimes) -> Self {
        Self {
            level: 0.0,
            hang_left: 0,
            hang: (rate_hz * times.hang_s) as u32,
            attack: one_pole_coeff(rate_hz, times.attack_s),
            decay: one_pole_coeff(rate_hz, times.decay_s),
        }
    }

    /// Forgets the level.
    pub fn reset(&mut self) {
        self.level = 0.0;
        self.hang_left = 0;
    }

    /// Normalises `buf` in place (output clamped to +-1).
    pub fn process_block(&mut self, buf: &mut [f32]) {
        for v in buf {
            let mag = v.abs();
            if mag >= self.level {
                self.level += self.attack * (mag - self.level);
                self.hang_left = self.hang;
            } else if self.hang_left > 0 {
                self.hang_left -= 1;
            } else {
                self.level += self.decay * (mag - self.level);
            }
            let gain = SSB_TARGET_PEAK / self.level.max(SSB_LEVEL_FLOOR);
            *v = (*v * gain).clamp(-1.0, 1.0);
        }
    }
}

/// Single-sideband / CW product detector with AGC.
///
/// The input is complex baseband that has already been band-limited around DC (for SSB after
/// shifting the wanted sideband there, for CW centred on the carrier). The detector shifts it by
/// `audio_shift_hz` (the centre of the wanted sideband for SSB, the BFO pitch for CW) and takes
/// the real part; because the input is complex, the other sideband / the image never folds in.
#[derive(Debug, Clone)]
pub struct SsbDemod {
    nco: Nco,
    scratch: Vec<Complex32>,
    agc: PeakAgc,
}

impl SsbDemod {
    /// Creates a detector for complex input at `rate_hz`; a baseband component at `f` is output
    /// at `f + audio_shift_hz`.
    pub fn new(rate_hz: f64, audio_shift_hz: f64, times: AgcTimes) -> Self {
        Self {
            // The NCO multiplies by exp(-j * offset), so a positive shift needs a negative offset.
            nco: Nco::new(-audio_shift_hz, rate_hz),
            scratch: Vec::new(),
            agc: PeakAgc::new(rate_hz, times),
        }
    }

    /// Clears oscillator phase and AGC level.
    pub fn reset(&mut self) {
        self.nco.reset();
        self.agc.reset();
    }

    /// Demodulates `iq`, appending to `out`.
    pub fn process(&mut self, iq: &[Complex32], out: &mut Vec<f32>) {
        self.nco.mix(iq, &mut self.scratch);
        let start = out.len();
        out.extend(self.scratch.iter().map(|x| x.re));
        self.agc.process_block(&mut out[start..]);
    }
}

/// Smoothing coefficient of a one-pole low-pass with time constant `tau_s`.
fn one_pole_coeff(rate_hz: f64, tau_s: f64) -> f32 {
    (1.0 - (-1.0 / (rate_hz * tau_s)).exp()) as f32
}

/// One-pole high-pass (DC blocker).
#[derive(Debug, Clone)]
pub struct DcBlock {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlock {
    /// Creates a blocker with the given -3 dB corner.
    pub fn new(rate_hz: f64, corner_hz: f64) -> Self {
        Self {
            r: (-std::f64::consts::TAU * corner_hz / rate_hz).exp() as f32,
            x1: 0.0,
            y1: 0.0,
        }
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    /// Processes a block in place.
    pub fn process_block(&mut self, buf: &mut [f32]) {
        for v in buf {
            *v = self.process(*v);
        }
    }
}

/// First-order de-emphasis filter (unity DC gain).
#[derive(Debug, Clone)]
pub struct Deemph {
    a: f32,
    y: f32,
}

impl Deemph {
    /// Creates a de-emphasis filter with time constant `tau_us` microseconds.
    pub fn new(rate_hz: f64, tau_us: f64) -> Self {
        Self {
            a: one_pole_coeff(rate_hz, tau_us * 1e-6),
            y: 0.0,
        }
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.y = 0.0;
    }

    /// Processes a block in place.
    pub fn process_block(&mut self, buf: &mut [f32]) {
        for v in buf {
            self.y += self.a * (*v - self.y);
            *v = self.y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: f64, amp: f32, secs: f64) -> Vec<f32> {
        (0..(rate * secs) as usize)
            .map(|i| amp * (std::f64::consts::TAU * 1000.0 * i as f64 / rate).sin() as f32)
            .collect()
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0, |m, v| m.max(v.abs()))
    }

    #[test]
    fn agc_attacks_fast_holds_then_decays_slowly() {
        let rate = 24_000.0;
        let mut agc = PeakAgc::new(rate, AgcTimes::SSB);
        // A loud burst: output settles near the target within a few attack constants.
        let mut loud = tone(rate, 0.5, 0.3);
        agc.process_block(&mut loud);
        assert!((0.5..=1.0).contains(&peak(&loud[loud.len() - 2400..])));
        // A weak signal right afterwards is held down during the hang (gain unchanged).
        let mut weak = tone(rate, 0.005, 0.2);
        agc.process_block(&mut weak);
        assert!(peak(&weak) < 0.02, "hang: {}", peak(&weak));
        // After the slow decay the gain has come up again.
        let mut later = tone(rate, 0.005, 3.0);
        agc.process_block(&mut later);
        assert!(peak(&later[later.len() - 2400..]) > 0.4);
    }

    #[test]
    fn product_detector_shifts_by_the_given_frequency() {
        let rate = 24_000.0;
        let mut d = SsbDemod::new(rate, 700.0, AgcTimes::CW);
        let iq: Vec<Complex32> = (0..24_000).map(|_| Complex32::new(0.2, 0.0)).collect();
        let mut out = Vec::new();
        d.process(&iq, &mut out);
        let tail = &out[12_000..];
        // 24000 / 700 is not integral, so count zero crossings: 700 Hz = 1400 per second,
        // 700 in the 0.5 s tail.
        let crossings = tail.windows(2).filter(|w| w[0] * w[1] < 0.0).count();
        assert!((698..=702).contains(&crossings), "{crossings}");
    }
}
