//! Tuning snapshot shared between a device's control side and its read thread.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Centre, rate and generation, always read and written together so a block can never carry a
/// mix of old and new values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tuning {
    pub center_hz: u64,
    pub sample_rate: u32,
    pub epoch: u64,
    /// A hardware change is in progress; samples read now belong to no consistent tuning.
    pub settling: bool,
}

/// Mutex-protected [`Tuning`]. Only ever held for a few instructions (never across hardware
/// calls), so the read thread is not stalled by control operations.
#[derive(Debug)]
pub(crate) struct TuningCell(Mutex<Tuning>);

impl TuningCell {
    pub fn new(center_hz: u64, sample_rate: u32) -> Self {
        Self(Mutex::new(Tuning {
            center_hz,
            sample_rate,
            epoch: 0,
            settling: false,
        }))
    }

    pub fn lock(&self) -> MutexGuard<'_, Tuning> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn get(&self) -> Tuning {
        *self.lock()
    }

    /// Mark the start of a hardware change: readers must discard samples until
    /// [`Self::finish_change`].
    #[cfg(feature = "rtlsdr")]
    pub fn begin_change(&self) {
        self.lock().settling = true;
    }

    /// Store the final values and end the change; the epoch advances when `bump` is set.
    #[cfg(feature = "rtlsdr")]
    pub fn finish_change(&self, center_hz: u64, sample_rate: u32, bump: bool) -> Tuning {
        let mut t = self.lock();
        t.center_hz = center_hz;
        t.sample_rate = sample_rate;
        t.settling = false;
        if bump {
            t.epoch += 1;
        }
        *t
    }
}
