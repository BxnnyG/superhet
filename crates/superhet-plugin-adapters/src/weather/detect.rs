//! Finding a satellite pass in the IQ stream: the band is quiet until the satellite's carrier
//! rises above the noise, and quiet again when it sets. A pass is a stretch of blocks whose power
//! stands clearly over the noise floor.

/// Frames (complex samples) per measurement block at the plugin's input rate.
pub const BLOCK_FRAMES: usize = 48_000;
/// Bytes of one block of `cs16` IQ (two little-endian `i16` per frame).
pub const BLOCK_BYTES: usize = BLOCK_FRAMES * 4;
/// Input rate in Hz (`plugins/weather/plugin.toml`).
pub const SAMPLE_RATE: u32 = 192_000;
/// Seconds per block.
pub const BLOCK_SECONDS: f64 = BLOCK_FRAMES as f64 / SAMPLE_RATE as f64;
/// Blocks measured before detection starts (the noise floor of a quiet band).
const CALIBRATION_BLOCKS: usize = 12;

/// Mean power of a block of `cs16` samples in dB relative to full scale (0 dB = a full-scale
/// carrier). An empty block is silence.
#[must_use]
pub fn power_db(block: &[u8]) -> f64 {
    let mut sum = 0.0_f64;
    let mut n = 0usize;
    for pair in block.as_chunks::<2>().0 {
        let v = f64::from(i16::from_le_bytes(*pair)) / 32768.0;
        sum += v * v;
        n += 1;
    }
    if n == 0 {
        return -120.0;
    }
    // Two components per frame: the mean over frames is twice the mean over components.
    (10.0 * (2.0 * sum / n as f64).max(1e-12).log10()).max(-120.0)
}

/// What the detector decided for one block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Nothing changed.
    None,
    /// A pass has started: the signal stayed up for the arming time.
    Begin,
    /// The pass is over; the highest level above the noise floor seen during it, in dB.
    End { peak_db: f64 },
}

/// Tuning of the detector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// dB over the noise floor that count as signal.
    pub threshold_db: f64,
    /// Blocks the signal must stay up before a pass begins.
    pub arm_blocks: usize,
    /// Blocks it must stay down before the pass ends.
    pub hang_blocks: usize,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Calibrating { seen: usize },
    Idle,
    Arming { up: usize },
    Active { down: usize, peak_db: f64 },
}

/// Pass detector: feed it the power of one block after the other.
#[derive(Debug, Clone)]
pub struct Detector {
    params: Params,
    state: State,
    floor_db: f64,
}

impl Detector {
    /// A detector that first learns the noise floor.
    #[must_use]
    pub fn new(params: Params) -> Self {
        Self {
            params,
            state: State::Calibrating { seen: 0 },
            floor_db: 0.0,
        }
    }

    /// The noise floor learned so far (dB relative to full scale).
    #[must_use]
    pub fn floor_db(&self) -> f64 {
        self.floor_db
    }

    /// Whether a pass is going on (or about to be confirmed).
    #[must_use]
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Arming { .. } | State::Active { .. })
    }

    /// Forget a pass under way (it was cut off) and listen again, keeping the learned floor.
    pub fn reset_idle(&mut self) {
        self.state = State::Idle;
    }

    /// Take the power of the next block.
    pub fn push(&mut self, db: f64) -> Verdict {
        match self.state {
            State::Calibrating { seen } => {
                // The quietest block of the first seconds is the floor.
                self.floor_db = if seen == 0 { db } else { self.floor_db.min(db) };
                self.state = if seen + 1 >= CALIBRATION_BLOCKS {
                    State::Idle
                } else {
                    State::Calibrating { seen: seen + 1 }
                };
                Verdict::None
            }
            State::Idle => {
                if db - self.floor_db >= self.params.threshold_db {
                    self.state = State::Arming { up: 1 };
                    return self.arm(1);
                }
                self.track_floor(db);
                Verdict::None
            }
            State::Arming { up } => {
                if db - self.floor_db >= self.params.threshold_db {
                    self.state = State::Arming { up: up + 1 };
                    self.arm(up + 1)
                } else {
                    self.state = State::Idle;
                    self.track_floor(db);
                    Verdict::None
                }
            }
            State::Active { down, peak_db } => {
                let over = db - self.floor_db;
                let peak_db = peak_db.max(over);
                // Hysteresis: dropping below a little less than the trigger level counts as down.
                if over < (self.params.threshold_db - 1.0).max(0.5) {
                    if down + 1 >= self.params.hang_blocks {
                        self.state = State::Idle;
                        return Verdict::End { peak_db };
                    }
                    self.state = State::Active {
                        down: down + 1,
                        peak_db,
                    };
                } else {
                    self.state = State::Active { down: 0, peak_db };
                }
                Verdict::None
            }
        }
    }

    fn arm(&mut self, up: usize) -> Verdict {
        if up >= self.params.arm_blocks {
            self.state = State::Active {
                down: 0,
                peak_db: 0.0,
            };
            Verdict::Begin
        } else {
            Verdict::None
        }
    }

    /// Follow the floor: down quickly, up slowly (time constant of about a minute), so drift and
    /// a gain change do not look like a satellite.
    fn track_floor(&mut self, db: f64) {
        let alpha = if db < self.floor_db {
            0.25
        } else {
            1.0 / 240.0
        };
        self.floor_db += (db - self.floor_db) * alpha;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Params = Params {
        threshold_db: 3.0,
        arm_blocks: 4,
        hang_blocks: 6,
    };

    fn run(d: &mut Detector, levels: &[f64]) -> Vec<(usize, Verdict)> {
        levels
            .iter()
            .enumerate()
            .filter_map(|(i, l)| match d.push(*l) {
                Verdict::None => None,
                v => Some((i, v)),
            })
            .collect()
    }

    fn quiet(n: usize) -> Vec<f64> {
        vec![-40.0; n]
    }

    #[test]
    fn power_of_a_full_scale_carrier_is_zero_db() {
        let mut block = Vec::new();
        for _ in 0..1000 {
            block.extend_from_slice(&i16::MAX.to_le_bytes());
            block.extend_from_slice(&0i16.to_le_bytes());
        }
        assert!(power_db(&block).abs() < 0.01);
        assert!(power_db(&[]) <= -100.0);
        assert!(power_db(&vec![0u8; 400]) <= -100.0);
        // Half the amplitude is 6 dB down.
        let half: Vec<u8> = block
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|f| {
                let i = i16::from_le_bytes([f[0], f[1]]) / 2;
                [i.to_le_bytes()[0], i.to_le_bytes()[1], 0, 0]
            })
            .collect();
        assert!((power_db(&half) + 6.02).abs() < 0.05);
    }

    #[test]
    fn a_pass_begins_after_the_arming_time_and_ends_after_the_hang_time() {
        let mut d = Detector::new(P);
        let mut levels = quiet(20);
        levels.extend(vec![-30.0; 10]); // 10 dB up
        levels.extend(quiet(10));
        let got = run(&mut d, &levels);
        // Begin at the 4th loud block (index 20 + 3), end at the 6th quiet block after.
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0], (23, Verdict::Begin));
        match got[1] {
            (i, Verdict::End { peak_db }) => {
                assert_eq!(i, 30 + 5);
                assert!((peak_db - 10.0).abs() < 0.1, "{peak_db}");
            }
            other => panic!("{other:?}"),
        }
        assert!(!d.busy());
    }

    #[test]
    fn a_short_blip_is_not_a_pass() {
        let mut d = Detector::new(P);
        let mut levels = quiet(20);
        levels.extend(vec![-30.0; 3]);
        levels.extend(quiet(20));
        assert!(run(&mut d, &levels).is_empty());
    }

    #[test]
    fn fading_inside_a_pass_does_not_end_it() {
        let mut d = Detector::new(P);
        let mut levels = quiet(20);
        levels.extend(vec![-30.0; 6]);
        levels.extend(quiet(5)); // one block shorter than the hang time
        levels.extend(vec![-30.0; 6]);
        levels.extend(quiet(10));
        let got = run(&mut d, &levels);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(matches!(got[1].1, Verdict::End { .. }));
        assert_eq!(got[1].0, 20 + 6 + 5 + 6 + 5);
    }

    #[test]
    fn slow_drift_of_the_floor_is_followed_without_triggering() {
        let mut d = Detector::new(P);
        // 0.01 dB per block over 4 minutes: 10 dB total, always far under 3 dB per time constant.
        let levels: Vec<f64> = (0..960).map(|i| -40.0 + f64::from(i) * 0.01).collect();
        assert!(run(&mut d, &levels).is_empty());
        assert!(d.floor_db() > -40.0);
    }

    #[test]
    fn the_floor_is_learned_from_the_quietest_start_block() {
        let mut d = Detector::new(P);
        let mut levels = vec![-35.0, -41.0, -39.0];
        levels.extend(quiet(20));
        run(&mut d, &levels);
        assert!((d.floor_db() + 40.0).abs() < 1.5, "{}", d.floor_db());
    }

    #[test]
    fn block_constants_agree() {
        assert_eq!(BLOCK_BYTES, BLOCK_FRAMES * 4);
        assert!((BLOCK_SECONDS - 0.25).abs() < 1e-9);
    }
}
