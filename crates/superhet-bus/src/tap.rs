//! Plugin input taps and exclusive device claims (`docs/plugin-contract.md`, "Feeding input").
//!
//! The plugin host asks the device daemon for a *tap*: a copy of what one VFO demodulates
//! (`audio`), of the complex baseband of one VFO (`iq`) or of the whole sample window (`window`,
//! exclusive claims), converted to the format and rate a plugin's manifest asks for. The daemon
//! publishes raw sample bytes in chunks on `sh.v1.dev.<dev>.tap.<id>`; concatenating the message
//! payloads in order gives the plugin's `stdin`.
//!
//! A tap and a claim are *leases*: the host repeats the `open` / `acquire` request (idempotent)
//! about every [`LEASE_REFRESH`], the daemon drops them after [`LEASE_TTL`] without a refresh.
//! That makes both survive a daemon restart and a crashed host without leaving the device claimed.
//! Requests go to `sh.v1.dev.<dev>.ctl.{tap.open,tap.close,claim.acquire,claim.release}`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Current request version.
pub const VERSION: u32 = 1;
/// A lease expires this long after its last refresh.
pub const LEASE_TTL: Duration = Duration::from_secs(15);
/// The host refreshes its leases this often.
pub const LEASE_REFRESH: Duration = Duration::from_secs(4);
/// Most taps per device.
pub const MAX_TAPS: usize = 16;
/// Lowest accepted tap sample rate (Hz).
pub const MIN_RATE: u32 = 4_000;
/// Highest accepted rate for `audio` taps (Hz).
pub const MAX_AUDIO_RATE: u32 = 192_000;
/// Highest accepted rate for `iq` and `window` taps (Hz).
pub const MAX_IQ_RATE: u32 = 20_000_000;
/// Longest tap id.
pub const MAX_ID_LEN: usize = 64;

/// What a tap carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapKind {
    /// Demodulated mono audio of a VFO (what the listener hears, squelch included).
    Audio,
    /// Complex baseband of a VFO: shifted to DC, band-limited to the VFO bandwidth.
    Iq,
    /// The whole sample window of the device, unprocessed (needs `rate` = device rate).
    Window,
}

/// Sample encoding of a tap; the names are those of the plugin manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapFormat {
    /// Signed 16-bit little-endian real samples.
    S16le,
    /// 32-bit float little-endian real samples.
    F32le,
    /// Interleaved unsigned 8-bit I/Q.
    Cu8,
    /// Interleaved signed 16-bit little-endian I/Q.
    Cs16,
    /// Interleaved 32-bit float little-endian I/Q.
    Cf32,
}

impl TapFormat {
    /// Whether the format carries I/Q pairs.
    #[must_use]
    pub fn is_complex(self) -> bool {
        matches!(self, Self::Cu8 | Self::Cs16 | Self::Cf32)
    }

    /// Bytes per sample (an I/Q pair counts as one sample).
    #[must_use]
    pub fn sample_bytes(self) -> usize {
        match self {
            Self::Cu8 | Self::S16le => 2,
            Self::F32le | Self::Cs16 => 4,
            Self::Cf32 => 8,
        }
    }
}

/// `tap.open`: create a tap or refresh its lease. Re-sending different parameters for an
/// existing id replaces the tap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TapOpen {
    /// Request version, [`VERSION`].
    pub v: u32,
    /// Tap id (a plugin instance id); one token of the data subject.
    pub id: String,
    /// What to carry.
    pub kind: TapKind,
    /// VFO to tap (`audio`, `iq`); absent for `window`.
    #[serde(default)]
    pub vfo: Option<u32>,
    /// Output sample rate (Hz).
    pub rate: u32,
    /// Output encoding; real formats for `audio`, complex ones for `iq` and `window`.
    pub format: TapFormat,
    /// Only blocks taken at this centre frequency (Hz) are sent (`window`).
    #[serde(default)]
    pub center_hz: Option<u64>,
}

/// Why a request is not acceptable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Invalid(pub String);

fn invalid(message: impl Into<String>) -> Invalid {
    Invalid(message.into())
}

/// Whether `id` can be used as a tap id and in a subject.
#[must_use]
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl TapOpen {
    /// Check version, id, combination of kind / format / vfo and the rate range.
    ///
    /// # Errors
    /// The first problem found.
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.v != VERSION {
            return Err(invalid(format!("\"v\":{VERSION} is required")));
        }
        if !is_valid_id(&self.id) {
            return Err(invalid("id must be 1-64 characters of [A-Za-z0-9_-]"));
        }
        match self.kind {
            TapKind::Audio => {
                if self.format.is_complex() {
                    return Err(invalid("audio taps need a real format (s16le, f32le)"));
                }
                if self.vfo.is_none() {
                    return Err(invalid("audio taps need a vfo"));
                }
                if !(MIN_RATE..=MAX_AUDIO_RATE).contains(&self.rate) {
                    return Err(invalid(format!(
                        "audio rate must be {MIN_RATE}..={MAX_AUDIO_RATE} Hz"
                    )));
                }
            }
            TapKind::Iq | TapKind::Window => {
                if !self.format.is_complex() {
                    return Err(invalid("iq taps need a complex format (cu8, cs16, cf32)"));
                }
                if (self.kind == TapKind::Iq) != self.vfo.is_some() {
                    return Err(invalid("iq taps need a vfo, window taps must not have one"));
                }
                if !(MIN_RATE..=MAX_IQ_RATE).contains(&self.rate) {
                    return Err(invalid(format!(
                        "iq rate must be {MIN_RATE}..={MAX_IQ_RATE} Hz"
                    )));
                }
            }
        }
        if self.center_hz.is_some() && self.kind != TapKind::Window {
            return Err(invalid("center_hz only applies to window taps"));
        }
        Ok(())
    }
}

/// `tap.close` / `claim.release`: address one tap or claim by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ById {
    /// Request version, [`VERSION`].
    pub v: u32,
    /// Tap or claim id.
    pub id: String,
}

/// `claim.acquire`: take the device exclusively (retune, pause what does not fit) or refresh
/// the lease of an existing claim with the same id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimAcquire {
    /// Request version, [`VERSION`].
    pub v: u32,
    /// Claim id (a plugin instance id).
    pub id: String,
    /// Centre frequency to tune to (Hz).
    pub center_hz: u64,
    /// Sample rate to set (Hz); absent keeps the current rate.
    #[serde(default)]
    pub sample_rate: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio() -> TapOpen {
        TapOpen {
            v: 1,
            id: "i-ab12cd34".into(),
            kind: TapKind::Audio,
            vfo: Some(1),
            rate: 22_050,
            format: TapFormat::S16le,
            center_hz: None,
        }
    }

    #[test]
    fn valid_requests_pass() {
        assert_eq!(audio().validate(), Ok(()));
        let iq = TapOpen {
            kind: TapKind::Iq,
            format: TapFormat::Cu8,
            rate: 250_000,
            ..audio()
        };
        assert_eq!(iq.validate(), Ok(()));
        let window = TapOpen {
            kind: TapKind::Window,
            vfo: None,
            format: TapFormat::Cf32,
            rate: 2_000_000,
            center_hz: Some(1_090_000_000),
            ..audio()
        };
        assert_eq!(window.validate(), Ok(()));
    }

    #[test]
    fn invalid_combinations_are_refused() {
        let bad = [
            TapOpen { v: 2, ..audio() },
            TapOpen {
                id: "a.b".into(),
                ..audio()
            },
            TapOpen {
                id: String::new(),
                ..audio()
            },
            TapOpen {
                format: TapFormat::Cu8,
                ..audio()
            },
            TapOpen {
                vfo: None,
                ..audio()
            },
            TapOpen {
                rate: 100,
                ..audio()
            },
            TapOpen {
                rate: 500_000,
                ..audio()
            },
            TapOpen {
                kind: TapKind::Iq,
                ..audio()
            },
            TapOpen {
                kind: TapKind::Window,
                format: TapFormat::Cu8,
                ..audio()
            },
            TapOpen {
                center_hz: Some(1),
                ..audio()
            },
        ];
        for request in bad {
            assert!(request.validate().is_err(), "{request:?}");
        }
    }

    #[test]
    fn unknown_fields_are_errors_and_names_are_snake_case() {
        let text = r#"{"v":1,"id":"x","kind":"audio","vfo":1,"rate":8000,"format":"s16le"}"#;
        assert!(serde_json::from_str::<TapOpen>(text).is_ok());
        let extra = r#"{"v":1,"id":"x","kind":"audio","vfo":1,"rate":8000,"format":"s16le","x":1}"#;
        assert!(serde_json::from_str::<TapOpen>(extra).is_err());
        assert_eq!(TapFormat::Cf32.sample_bytes(), 8);
        assert_eq!(TapFormat::S16le.sample_bytes(), 2);
    }
}
