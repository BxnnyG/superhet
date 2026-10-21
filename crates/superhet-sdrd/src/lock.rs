//! Tuning lock (`docs/ARCHITECTURE.md` section 3.4): while any holder exists, centre and sample
//! rate cannot change; VFOs may still move inside the window.
//!
//! Holders are a list, not a flag, so recordings, decoders and exclusive claims can join later
//! without changing the state document or the error shape. Today only the manual `user` lock
//! exists (`ctl.lock.acquire` / `ctl.lock.release`).

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::control::{CtlError, ErrorCode};

/// Who holds a lock. More kinds (`recording`, `decoder`, `claim`) follow with their features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HolderKind {
    /// The user's manual lock.
    User,
}

/// One lock holder as shown in the state document and in `locked` errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    /// Kind of holder.
    pub kind: HolderKind,
    /// Identifies the holder within its kind (`user` for the manual lock).
    pub id: String,
    /// Unix time in milliseconds when the lock was taken.
    pub since_ms: u64,
}

impl Holder {
    /// The manual user lock, taken now.
    #[must_use]
    pub fn user() -> Self {
        Self::user_since(unix_ms())
    }

    /// The manual user lock with a known start time (restored from a snapshot).
    #[must_use]
    pub fn user_since(since_ms: u64) -> Self {
        Self {
            kind: HolderKind::User,
            id: "user".into(),
            since_ms,
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The holders of one device.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LockSet {
    holders: Vec<Holder>,
}

impl LockSet {
    /// Add a holder; a holder with the same kind and id is kept as is. Returns whether it is new.
    pub fn acquire(&mut self, holder: Holder) -> bool {
        if self
            .holders
            .iter()
            .any(|h| h.kind == holder.kind && h.id == holder.id)
        {
            return false;
        }
        self.holders.push(holder);
        true
    }

    /// Remove a holder. Returns whether it was present.
    pub fn release(&mut self, kind: HolderKind, id: &str) -> bool {
        let before = self.holders.len();
        self.holders.retain(|h| !(h.kind == kind && h.id == id));
        self.holders.len() != before
    }

    /// Whether any holder exists.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        !self.holders.is_empty()
    }

    /// All holders, oldest first.
    #[must_use]
    pub fn holders(&self) -> &[Holder] {
        &self.holders
    }

    /// The `locked` error naming every holder.
    #[must_use]
    pub fn error(&self, what: &str) -> CtlError {
        let names: Vec<String> = self
            .holders
            .iter()
            .map(|h| match h.kind {
                HolderKind::User => "user".to_owned(),
            })
            .collect();
        CtlError {
            code: ErrorCode::Locked,
            message: format!("{what} is locked by: {}", names.join(", ")),
            holders: self.holders.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release_are_idempotent() {
        let mut locks = LockSet::default();
        assert!(!locks.is_locked());
        assert!(locks.acquire(Holder::user()));
        assert!(!locks.acquire(Holder::user()));
        assert_eq!(locks.holders().len(), 1);
        assert!(locks.is_locked());
        assert!(locks.release(HolderKind::User, "user"));
        assert!(!locks.release(HolderKind::User, "user"));
        assert!(!locks.is_locked());
    }

    #[test]
    fn error_lists_the_holders() {
        let mut locks = LockSet::default();
        locks.acquire(Holder::user_since(5));
        let err = locks.error("tuning");
        assert_eq!(err.code, ErrorCode::Locked);
        assert_eq!(err.message, "tuning is locked by: user");
        assert_eq!(err.holders, vec![Holder::user_since(5)]);
    }
}
