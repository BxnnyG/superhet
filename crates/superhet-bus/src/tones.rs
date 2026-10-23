//! Tone state of an NFM / AM VFO, published as JSON on `sh.v1.dev.<dev>.tones.<vfo>`.
//!
//! One message is the complete state (not a delta): the CTCSS tone currently detected, a running
//! count of the DTMF digits detected since the channel was (re)started and the last digits
//! themselves. A consumer appends `count - last_count` characters from the end of `dtmf_tail` to
//! its own digit string, so a lost or repeated message does not corrupt it. The daemon publishes
//! on every change and repeats the state about once a second while a tone is present or digits
//! exist; a state with no tone and `dtmf_count == 0` tells consumers to clear their display
//! (retune, mode change, tone detection switched off). Lossy live data on core NATS like audio;
//! the API forwards it unchanged as a text frame on the audio WebSocket
//! (`docs/protocol-audio.md`).

use serde::{Deserialize, Serialize};

/// Current message version.
pub const VERSION: u32 = 1;
/// Value of the `type` field.
pub const KIND: &str = "tones";
/// Largest message the API forwards.
pub const MAX_LEN: usize = 1024;
/// Most digits in `dtmf_tail`.
pub const MAX_TAIL: usize = 32;
/// Range of `ctcss_hz` (the standard tones span 67.0 to 254.1 Hz).
const CTCSS_RANGE: std::ops::RangeInclusive<f32> = 60.0..=260.0;

/// Detected tones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TonesMessage {
    /// Message version, [`VERSION`].
    pub v: u32,
    /// Always [`KIND`].
    #[serde(rename = "type")]
    pub kind: String,
    /// VFO the state belongs to.
    pub vfo: u32,
    /// CTCSS sub-tone currently present, one of the 50 standard tones.
    pub ctcss_hz: Option<f32>,
    /// DTMF digits detected since the channel (re)started.
    pub dtmf_count: u64,
    /// The last (up to [`MAX_TAIL`]) digits, oldest first: `0-9`, `A-D`, `*`, `#`.
    pub dtmf_tail: String,
}

impl TonesMessage {
    /// An empty state for `vfo`.
    #[must_use]
    pub fn empty(vfo: u32) -> Self {
        Self {
            v: VERSION,
            kind: KIND.to_owned(),
            vfo,
            ctcss_hz: None,
            dtmf_count: 0,
            dtmf_tail: String::new(),
        }
    }

    /// True if there is nothing to show.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ctcss_hz.is_none() && self.dtmf_count == 0
    }

    /// Serialise to JSON.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        // A struct of strings, numbers and options always serialises.
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Parse and validate a message.
    ///
    /// # Errors
    /// A reason for malformed JSON, unknown fields, a wrong version or type, or out-of-range
    /// values.
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        if data.len() > MAX_LEN {
            return Err("tones message too large".into());
        }
        let msg: Self =
            serde_json::from_slice(data).map_err(|e| format!("malformed tones message: {e}"))?;
        if msg.v != VERSION {
            return Err(format!("unsupported tones message version {}", msg.v));
        }
        if msg.kind != KIND {
            return Err("not a tones message".into());
        }
        if msg.ctcss_hz.is_some_and(|hz| !CTCSS_RANGE.contains(&hz)) {
            return Err("ctcss_hz out of range".into());
        }
        if msg.dtmf_tail.len() > MAX_TAIL
            || !msg
                .dtmf_tail
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'D').contains(&b) || b == b'*' || b == b'#')
        {
            return Err("dtmf_tail is not a digit string".into());
        }
        if (msg.dtmf_tail.len() as u64) > msg.dtmf_count {
            return Err("dtmf_tail longer than dtmf_count".into());
        }
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAIL: &str = "0123456789ABCD*#0123456789ABCD*#";

    fn full() -> TonesMessage {
        TonesMessage {
            ctcss_hz: Some(88.5),
            dtmf_count: 40,
            dtmf_tail: TAIL.into(),
            ..TonesMessage::empty(3)
        }
    }

    #[test]
    fn round_trips_and_has_the_documented_shape() {
        let msg = full();
        let bytes = msg.encode();
        assert_eq!(TonesMessage::parse(&bytes), Ok(msg));
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["v"], 1);
        assert_eq!(json["type"], "tones");
        assert_eq!(json["vfo"], 3);
        assert_eq!(json["dtmf_count"], 40);
        let empty = TonesMessage::empty(3);
        assert!(empty.is_empty() && !full().is_empty());
        let json: serde_json::Value = serde_json::from_slice(&empty.encode()).expect("json");
        assert!(json["ctcss_hz"].is_null());
        assert_eq!(json["dtmf_tail"], "");
    }

    #[test]
    fn rejects_bad_messages() {
        let text = String::from_utf8(full().encode()).expect("utf8");
        for bad in [
            text.replace("\"v\":1", "\"v\":2"),
            text.replace("\"tones\"", "\"rds\""),
            text.replace("88.5", "500.0"),
            text.replace("\"dtmf_count\":40", "\"dtmf_count\":3"),
            text.replace(TAIL, "12x4"),
            text.replace(TAIL, &"1".repeat(33)),
            text.replace('{', "{\"extra\":1,"),
            "nope".into(),
        ] {
            assert!(TonesMessage::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(TonesMessage::parse(&vec![b' '; MAX_LEN + 1]).is_err());
    }
}
