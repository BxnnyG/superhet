//! RDS state of a VFO, published as JSON on `sh.v1.dev.<dev>.rds.<vfo>`.
//!
//! One message is the complete decoded state of the VFO's station (not a delta): the daemon
//! publishes it on every change and repeats it about once a second while there is anything to
//! show, so a late subscriber catches up within a second. An empty state (everything `null`,
//! `synced:false`) tells consumers to clear their display (retune, mode change, signal lost).
//! The message is lossy live data on core NATS, like audio; the API forwards it unchanged as a
//! text frame on the audio WebSocket (`docs/protocol-audio.md`).

use serde::{Deserialize, Serialize};

/// Current message version.
pub const VERSION: u32 = 1;
/// Value of the `type` field.
pub const KIND: &str = "rds";
/// Largest message the API forwards (RadioText is at most 64 characters).
pub const MAX_LEN: usize = 2048;

/// Decoded RDS state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RdsMessage {
    /// Message version, [`VERSION`].
    pub v: u32,
    /// Always [`KIND`].
    #[serde(rename = "type")]
    pub kind: String,
    /// VFO the state belongs to.
    pub vfo: u32,
    /// The RDS block synchronisation currently holds (data is being received right now).
    pub synced: bool,
    /// Programme identification as four upper-case hex digits.
    pub pi: Option<String>,
    /// Programme type, 0..=31.
    pub pty: Option<u8>,
    /// Traffic programme flag.
    pub tp: Option<bool>,
    /// Traffic announcement flag.
    pub ta: Option<bool>,
    /// Music/speech switch: `true` = music.
    pub ms: Option<bool>,
    /// Programme service name, 8 characters.
    pub ps: Option<String>,
    /// RadioText, up to 64 characters.
    pub rt: Option<String>,
    /// Share of failed blocks in percent (0..=100) over the last ~10 seconds while the block
    /// sync held; absent until enough blocks have been seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bler: Option<u8>,
    /// Reception quality 0..=100 (100 minus `bler`, 0 without block sync); absent together with
    /// `bler`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
}

impl RdsMessage {
    /// An empty state for `vfo`.
    #[must_use]
    pub fn empty(vfo: u32) -> Self {
        Self {
            v: VERSION,
            kind: KIND.to_owned(),
            vfo,
            synced: false,
            pi: None,
            pty: None,
            tp: None,
            ta: None,
            ms: None,
            ps: None,
            rt: None,
            bler: None,
            quality: None,
        }
    }

    /// True if there is nothing to show.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.synced
            && self.pi.is_none()
            && self.pty.is_none()
            && self.tp.is_none()
            && self.ta.is_none()
            && self.ms.is_none()
            && self.ps.is_none()
            && self.rt.is_none()
            && self.quality.is_none()
    }

    /// Serialise to JSON.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        // A struct of strings, numbers and booleans always serialises.
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Parse and validate a message.
    ///
    /// # Errors
    /// A reason for malformed JSON, unknown fields, a wrong version or type, or out-of-range
    /// values.
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        if data.len() > MAX_LEN {
            return Err("rds message too large".into());
        }
        let msg: Self =
            serde_json::from_slice(data).map_err(|e| format!("malformed rds message: {e}"))?;
        if msg.v != VERSION {
            return Err(format!("unsupported rds message version {}", msg.v));
        }
        if msg.kind != KIND {
            return Err("not an rds message".into());
        }
        if msg.pty.is_some_and(|p| p > 31) {
            return Err("pty out of range".into());
        }
        if msg.bler.is_some_and(|b| b > 100) || msg.quality.is_some_and(|q| q > 100) {
            return Err("bler or quality out of range".into());
        }
        if msg
            .pi
            .as_ref()
            .is_some_and(|p| p.len() != 4 || !p.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err("pi is not four hex digits".into());
        }
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> RdsMessage {
        RdsMessage {
            synced: true,
            pi: Some("D3C2".into()),
            pty: Some(10),
            tp: Some(true),
            ta: Some(false),
            ms: Some(true),
            ps: Some("SUPERHET".into()),
            rt: Some("Now playing: Ünïcode ✓".into()),
            bler: Some(12),
            quality: Some(88),
            ..RdsMessage::empty(3)
        }
    }

    #[test]
    fn round_trips_and_has_the_documented_shape() {
        let msg = full();
        let bytes = msg.encode();
        assert_eq!(RdsMessage::parse(&bytes), Ok(msg));
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["v"], 1);
        assert_eq!(json["type"], "rds");
        assert_eq!(json["vfo"], 3);
        assert_eq!(json["ps"], "SUPERHET");
        assert_eq!(json["bler"], 12);
        assert_eq!(json["quality"], 88);
        let empty = RdsMessage::empty(3);
        assert!(empty.is_empty() && !full().is_empty());
        let json: serde_json::Value = serde_json::from_slice(&empty.encode()).expect("json");
        assert!(json["ps"].is_null() && json["pi"].is_null());
    }

    #[test]
    fn rejects_bad_messages() {
        let ok = full().encode();
        let text = String::from_utf8(ok).expect("utf8");
        for bad in [
            text.replace("\"v\":1", "\"v\":2"),
            text.replace("\"rds\"", "\"audio\""),
            text.replace("\"pty\":10", "\"pty\":32"),
            text.replace("D3C2", "D3C"),
            text.replace("D3C2", "D3CG"),
            text.replace("{", "{\"extra\":1,"),
            "nope".into(),
        ] {
            assert!(RdsMessage::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(RdsMessage::parse(&vec![b' '; MAX_LEN + 1]).is_err());
        assert!(RdsMessage::parse(text.replace("\"bler\":12", "\"bler\":101").as_bytes()).is_err());
    }

    #[test]
    fn quality_fields_are_optional() {
        // Messages of daemons that predate the fields still parse.
        let old = r#"{"v":1,"type":"rds","vfo":1,"synced":true,"pi":null,"pty":null,"tp":null,"ta":null,"ms":null,"ps":null,"rt":null}"#;
        let msg = RdsMessage::parse(old.as_bytes()).expect("old message");
        assert_eq!((msg.bler, msg.quality), (None, None));
        let text = String::from_utf8(RdsMessage::empty(1).encode()).expect("utf8");
        assert!(!text.contains("bler") && !text.contains("quality"));
    }
}
