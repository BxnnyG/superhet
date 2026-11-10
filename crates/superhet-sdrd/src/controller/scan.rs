//! The scanner's hooks into the controller (`crate::scanner`): the scan holds a lock holder of
//! kind `scanner`, retunes only while nobody else holds the tuning, and its state is part of
//! the state document.

use std::sync::atomic::Ordering;

use superhet_device::TuneRequest;

use super::{Controller, ErrorCode, Holder, HolderKind};
use crate::control::CtlError;
use crate::scanner::plan::Channel;

/// What the scanner learns when it takes the device.
pub struct ScanBegin {
    /// Current centre, Hz.
    pub center_hz: u64,
    /// Current sample rate, Hz.
    pub sample_rate: u32,
    /// `Some` when someone else holds the tuning lock: the scan may not leave this window.
    pub retune_blocked: Option<CtlError>,
}

impl Controller {
    /// Join the lock set as the scanner.
    ///
    /// # Errors
    /// `locked` while an exclusive plugin holds the device.
    pub fn scan_begin(&self) -> Result<ScanBegin, CtlError> {
        let mut locks = self.gate();
        if locks.holders().iter().any(|h| h.kind == HolderKind::Claim) {
            return Err(locks.error("scanning"));
        }
        locks.acquire(Holder::scanner());
        self.scan_preempt.store(false, Ordering::Release);
        let state = self.control.state();
        let retune_blocked = (!locks.only_scanner()).then(|| locks.error("tuning"));
        self.publish(&locks, &state);
        Ok(ScanBegin {
            center_hz: state.center_hz,
            sample_rate: state.sample_rate,
            retune_blocked,
        })
    }

    /// Leave the lock set (idempotent).
    pub fn scan_end(&self) {
        let mut locks = self.gate();
        if locks.release_kind(HolderKind::Scanner) {
            self.publish(&locks, &self.control.state());
        }
    }

    /// Whether an exclusive claim took the device from the scanner since the last call.
    pub fn scan_preempted(&self) -> bool {
        self.scan_preempt.swap(false, Ordering::AcqRel)
    }

    /// Move the centre for the scan; refused unless the scanner is the only lock holder.
    ///
    /// # Errors
    /// `locked`, or the device's error. Returns the applied centre.
    pub fn scan_retune(&self, center_hz: u64) -> Result<u64, CtlError> {
        let locks = self.gate();
        if !locks.only_scanner() {
            return Err(if locks.is_locked() {
                locks.error("tuning")
            } else {
                CtlError::new(ErrorCode::Locked, "the scanner no longer holds the tuning")
            });
        }
        let current = self.control.state();
        if current.center_hz == center_hz {
            return Ok(center_hz);
        }
        let request = TuneRequest {
            center_hz: Some(center_hz),
            ..TuneRequest::default()
        };
        match self.control.apply(&request) {
            Ok(state) => {
                self.publish(&locks, &state);
                Ok(state.center_hz)
            }
            Err(err) => {
                self.publish(&locks, &self.control.state());
                Err(err.into())
            }
        }
    }

    /// Ids of the VFOs.
    #[must_use]
    pub fn scan_vfo_ids(&self) -> Vec<u32> {
        self.vfos.snapshot().1.iter().map(|v| v.id).collect()
    }

    /// Put VFO `id` on `channel` (frequency, mode, bandwidth).
    ///
    /// # Errors
    /// `not_found` for an unknown VFO, `invalid` if the VFO cannot take the channel.
    pub fn scan_move_vfo(&self, id: u32, channel: &Channel) -> Result<(), CtlError> {
        let payload = serde_json::json!({
            "v": 1,
            "id": id,
            "freq_hz": channel.freq_hz,
            "mode": channel.mode,
            "bandwidth_hz": channel.bandwidth_hz,
        });
        self.vfo("vfo.update", payload.to_string().as_bytes())
            .map(|_| ())
    }

    /// Set the scanner part of the state document and publish.
    pub fn scan_publish(&self, view: Option<serde_json::Value>) {
        *self
            .scan_view
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = view;
        let locks = self.gate();
        self.publish(&locks, &self.control.state());
    }

    pub(super) fn scan_view_json(&self) -> Option<serde_json::Value> {
        self.scan_view
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
