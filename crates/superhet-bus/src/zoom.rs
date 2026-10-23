//! Zoom spectrum: control message and timing shared by `superhet-api` and `superhet-sdrd`.
//!
//! The gateway asks the daemon for a narrow slice of the band with a JSON message on
//! `sh.v1.dev.<dev>.ctl.spectrum.zoom` (core publish, no reply):
//!
//! - `{"v":1,"center_hz":<u64>,"span_hz":<u32>}` – compute a zoom spectrum that covers
//!   `center ± span / 2`;
//! - `{"v":1,"off":true}` – stop computing it.
//!
//! A request is a lease: the daemon stops the zoom spectrum [`LEASE`] after the last request, so
//! a crashed gateway never leaves it running. The gateway repeats its request every
//! [`REFRESH`] while a client needs it. The frames come back on
//! `sh.v1.dev.<dev>.spectrum.zoom` in the regular spectrum wire format
//! (`crate::spectrum::SpectrumFrame`); each header describes exactly the slice the bins cover.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Protocol version of the message.
pub const VERSION: u32 = 1;
/// The daemon stops the zoom spectrum this long after the last request.
pub const LEASE: Duration = Duration::from_secs(10);
/// The gateway repeats an active request this often.
pub const REFRESH: Duration = Duration::from_secs(3);

/// A zoom request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomCommand {
    /// Stop the zoom spectrum.
    Off,
    /// Cover `center_hz ± span_hz / 2`.
    View {
        /// Centre of the wanted slice in Hz.
        center_hz: u64,
        /// Width of the wanted slice in Hz (at least 1).
        span_hz: u32,
    },
}

/// Why a zoom request was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ZoomError {
    /// Not valid JSON, unknown fields, or missing/contradicting fields.
    #[error("malformed zoom request")]
    Malformed,
    /// Unsupported protocol version.
    #[error("unsupported zoom protocol version")]
    Version,
    /// The span is zero.
    #[error("zoom span must be positive")]
    Span,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    v: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    off: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    center_hz: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    span_hz: Option<u32>,
}

impl ZoomCommand {
    /// Serialise to the JSON message.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let wire = match *self {
            Self::Off => Wire {
                v: VERSION,
                off: true,
                center_hz: None,
                span_hz: None,
            },
            Self::View { center_hz, span_hz } => Wire {
                v: VERSION,
                off: false,
                center_hz: Some(center_hz),
                span_hz: Some(span_hz),
            },
        };
        serde_json::to_vec(&wire).unwrap_or_default()
    }

    /// Parse and validate the JSON message.
    ///
    /// # Errors
    /// See [`ZoomError`].
    pub fn decode(payload: &[u8]) -> Result<Self, ZoomError> {
        let wire: Wire = serde_json::from_slice(payload).map_err(|_| ZoomError::Malformed)?;
        if wire.v != VERSION {
            return Err(ZoomError::Version);
        }
        match (wire.off, wire.center_hz, wire.span_hz) {
            (true, None, None) => Ok(Self::Off),
            (false, Some(center_hz), Some(span_hz)) => {
                if span_hz == 0 {
                    Err(ZoomError::Span)
                } else {
                    Ok(Self::View { center_hz, span_hz })
                }
            }
            _ => Err(ZoomError::Malformed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for cmd in [
            ZoomCommand::Off,
            ZoomCommand::View {
                center_hz: 100_100_000,
                span_hz: 25_000,
            },
        ] {
            assert_eq!(ZoomCommand::decode(&cmd.encode()), Ok(cmd));
        }
        assert_eq!(ZoomCommand::Off.encode(), br#"{"v":1,"off":true}"#.to_vec());
        assert_eq!(
            ZoomCommand::View {
                center_hz: 7,
                span_hz: 8
            }
            .encode(),
            br#"{"v":1,"center_hz":7,"span_hz":8}"#.to_vec()
        );
    }

    #[test]
    fn rejects_bad_messages() {
        let bad = |s: &str| ZoomCommand::decode(s.as_bytes());
        assert_eq!(bad("nope"), Err(ZoomError::Malformed));
        assert_eq!(bad(r#"{"v":2,"off":true}"#), Err(ZoomError::Version));
        assert_eq!(bad(r#"{"v":1}"#), Err(ZoomError::Malformed));
        assert_eq!(bad(r#"{"v":1,"center_hz":5}"#), Err(ZoomError::Malformed));
        assert_eq!(
            bad(r#"{"v":1,"off":true,"center_hz":5,"span_hz":5}"#),
            Err(ZoomError::Malformed)
        );
        assert_eq!(
            bad(r#"{"v":1,"center_hz":5,"span_hz":0}"#),
            Err(ZoomError::Span)
        );
        assert_eq!(
            bad(r#"{"v":1,"center_hz":5,"span_hz":5,"x":1}"#),
            Err(ZoomError::Malformed)
        );
    }
}
