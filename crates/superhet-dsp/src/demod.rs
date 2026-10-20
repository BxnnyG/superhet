//! Demodulators and audio post-processing blocks (all streaming, state carried across calls).

use num_complex::Complex32;

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
