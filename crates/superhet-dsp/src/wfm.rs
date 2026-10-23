//! Broadcast-FM multiplex (MPX) processing: 19 kHz pilot PLL for stereo and the RDS carrier.
//!
//! The MPX signal is the FM discriminator output scaled so that 75 kHz deviation is 1.0:
//!
//! `mpx = (L+R)/2 + 0.1 sin(w t) + (L-R)/2 sin(2 w t) + rds sin(3 w t + psi)`
//!
//! with `w = 2 pi 19 kHz`. [`StereoMpx`] locks a second-order PLL to the pilot and derives from
//! its phase `phi` everything else without any filtering of the MPX itself (so that the audio
//! paths stay phase aligned):
//!
//! - `2 mpx sin(2 phi)` is the L-R signal before the audio low-pass,
//! - `mpx e^(-j 3 phi)` is the RDS subcarrier mixed to DC (see [`crate::rds`]),
//! - lock detection (pilot level and phase error, with hysteresis) tells whether stereo is
//!   usable; the caller crossfades towards mono when it is not.

use num_complex::Complex32;

use crate::demod::one_pole_coeff;

/// Pilot frequency in Hz.
pub const PILOT_HZ: f64 = 19_000.0;
/// Nominal pilot amplitude relative to full deviation (10 %).
const NOMINAL_PILOT: f32 = 0.1;
/// Natural frequency of the PLL in Hz.
const LOOP_HZ: f64 = 25.0;
/// Damping factor of the PLL.
const LOOP_DAMPING: f64 = 0.707;
/// The PLL may deviate this far from 19 kHz.
const MAX_OFFSET_HZ: f64 = 150.0;
/// Smoothing of the lock detector.
const DETECT_TAU_S: f64 = 0.03;
/// Pilot amplitude (fraction of full deviation) above which stereo may engage.
const LOCK_ON_LEVEL: f32 = 0.04;
/// Pilot amplitude below which stereo is dropped.
const LOCK_OFF_LEVEL: f32 = 0.025;
/// Phase error limit (as quadrature/in-phase ratio) to engage.
const LOCK_ON_ERROR: f32 = 0.5;
/// Phase error limit to stay engaged.
const LOCK_OFF_ERROR: f32 = 0.9;
/// The conditions must hold this long before the lock is declared.
const LOCK_ON_S: f64 = 0.15;
/// The lock is dropped after the conditions failed for this long.
const LOCK_OFF_S: f64 = 0.08;

/// Pilot PLL with stereo carrier and RDS mixer outputs.
#[derive(Debug, Clone)]
pub struct StereoMpx {
    phase: f64,
    integ: f64,
    w0: f64,
    kp: f64,
    ki: f64,
    integ_limit: f64,
    detect: f32,
    pi_lp: f32,
    pq_lp: f32,
    locked: bool,
    streak: usize,
    on_samples: usize,
    off_samples: usize,
}

impl StereoMpx {
    /// Creates a PLL for MPX samples at `rate_hz` (>= 120 kHz).
    #[must_use]
    pub fn new(rate_hz: f64) -> Self {
        let wn = std::f64::consts::TAU * LOOP_HZ / rate_hz;
        Self {
            phase: 0.0,
            integ: 0.0,
            w0: std::f64::consts::TAU * PILOT_HZ / rate_hz,
            kp: 2.0 * LOOP_DAMPING * wn,
            ki: wn * wn,
            integ_limit: std::f64::consts::TAU * MAX_OFFSET_HZ / rate_hz,
            detect: one_pole_coeff(rate_hz, DETECT_TAU_S),
            pi_lp: 0.0,
            pq_lp: 0.0,
            locked: false,
            streak: 0,
            on_samples: (rate_hz * LOCK_ON_S) as usize,
            off_samples: (rate_hz * LOCK_OFF_S) as usize,
        }
    }

    /// Forgets the lock and the phase (call on retune).
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.integ = 0.0;
        self.pi_lp = 0.0;
        self.pq_lp = 0.0;
        self.locked = false;
        self.streak = 0;
    }

    /// True while a usable pilot is locked (stereo may be decoded).
    #[must_use]
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Estimated pilot amplitude as a fraction of full deviation (nominally 0.1).
    #[must_use]
    pub fn pilot_level(&self) -> f32 {
        2.0 * self.pi_lp
    }

    /// Runs the PLL over `mpx`. Appends `2 mpx sin(2 phi)` to `diff` (only if `want_diff`) and
    /// `mpx e^(-j 3 phi)` to `rds_mix`; both get one value per input sample.
    pub fn process(
        &mut self,
        mpx: &[f32],
        want_diff: bool,
        diff: &mut Vec<f32>,
        rds_mix: &mut Vec<Complex32>,
    ) {
        if want_diff {
            diff.reserve(mpx.len());
        }
        rds_mix.reserve(mpx.len());
        for &x in mpx {
            let (s64, c64) = self.phase.sin_cos();
            let (s, c) = (s64 as f32, c64 as f32);

            // Phase detector: x cos(phi) = A/2 sin(error) for a pilot A sin(phase).
            let pi = x * s;
            let pq = x * c;
            self.pi_lp += self.detect * (pi - self.pi_lp);
            self.pq_lp += self.detect * (pq - self.pq_lp);
            // No limiter here: clipping the (large) stereo sidebands would bias the detector.
            let e = f64::from(pq) / f64::from(NOMINAL_PILOT / 2.0);
            self.integ = (self.integ + self.ki * e).clamp(-self.integ_limit, self.integ_limit);
            self.phase += self.w0 + self.integ + self.kp * e;
            if self.phase >= std::f64::consts::TAU {
                self.phase -= std::f64::consts::TAU;
            } else if self.phase < 0.0 {
                self.phase += std::f64::consts::TAU;
            }
            self.update_lock();

            if want_diff {
                diff.push(4.0 * x * s * c);
            }
            // e^(-j 3 phi): cos(3 phi) = 4 c^3 - 3 c, sin(3 phi) = 3 s - 4 s^3.
            let cos3 = c * (4.0 * c * c - 3.0);
            let sin3 = s * (3.0 - 4.0 * s * s);
            rds_mix.push(Complex32::new(x * cos3, -x * sin3));
        }
    }

    fn update_lock(&mut self) {
        let level = 2.0 * self.pi_lp;
        let error = if self.pi_lp > 1e-9 {
            self.pq_lp.abs() / self.pi_lp
        } else {
            f32::INFINITY
        };
        if self.locked {
            let bad = level < LOCK_OFF_LEVEL || error > LOCK_OFF_ERROR;
            self.streak = if bad { self.streak + 1 } else { 0 };
            if self.streak >= self.off_samples {
                self.locked = false;
                self.streak = 0;
            }
        } else {
            let good = level > LOCK_ON_LEVEL && error < LOCK_ON_ERROR;
            self.streak = if good { self.streak + 1 } else { 0 };
            if self.streak >= self.on_samples {
                self.locked = true;
                self.streak = 0;
            }
        }
    }
}
