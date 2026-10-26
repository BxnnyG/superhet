//! System audit events (`sh.v1.audit.<source>`, ADR-0012): published by services that have no
//! database access of their own (sdrd device errors, plugin host crashes ...) and persisted by
//! the worker into `audit_events`. Account-driven events are written by the API directly.

use serde::{Deserialize, Serialize};

/// Longest accepted action / target / actor name, in bytes.
pub const FIELD_MAX: usize = 128;
/// Largest accepted `details` document, in bytes of JSON.
pub const DETAILS_MAX: usize = 4096;

/// Result of the audited action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// It happened.
    Ok,
    /// It was refused (role, policy, limit).
    Denied,
    /// It failed.
    Error,
}

impl Outcome {
    /// The value stored in the database.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Denied => "denied",
            Self::Error => "error",
        }
    }
}

/// One system event as published on the bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemEvent {
    /// Protocol version, always 1.
    pub v: u32,
    /// Event time, Unix milliseconds (the worker uses its own clock when absent).
    #[serde(default)]
    pub ts_ms: Option<i64>,
    /// Dotted action name such as `plugin.crash` or `device.error`.
    pub action: String,
    /// Publishing component (shown as the actor name), e.g. `sdrd`.
    pub source: String,
    /// What the event is about (device id, plugin id ...).
    #[serde(default)]
    pub target: Option<String>,
    /// Result.
    pub outcome: Outcome,
    /// Small extra document; never secrets, IQ or audio.
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

impl SystemEvent {
    /// Check the field limits a publisher could violate.
    ///
    /// # Errors
    /// A reason string for the first violated rule.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.v != 1 {
            return Err("unsupported version");
        }
        let name_ok = |s: &str| {
            !s.is_empty()
                && s.len() <= FIELD_MAX
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
        };
        if !name_ok(&self.action) {
            return Err("invalid action");
        }
        if !name_ok(&self.source) {
            return Err("invalid source");
        }
        if self
            .target
            .as_deref()
            .is_some_and(|t| t.len() > FIELD_MAX || t.chars().any(char::is_control))
        {
            return Err("invalid target");
        }
        if let Some(details) = &self.details
            && (!details.is_object() || details.to_string().len() > DETAILS_MAX)
        {
            return Err("invalid details");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(action: &str) -> SystemEvent {
        SystemEvent {
            v: 1,
            ts_ms: None,
            action: action.into(),
            source: "sdrd".into(),
            target: Some("dev0".into()),
            outcome: Outcome::Error,
            details: Some(serde_json::json!({"reason": "usb"})),
        }
    }

    #[test]
    fn round_trips_and_validates() {
        let e = event("device.error");
        let text = serde_json::to_string(&e).expect("json");
        assert!(text.contains(r#""outcome":"error""#));
        let back: SystemEvent = serde_json::from_str(&text).expect("parse");
        assert_eq!(back, e);
        assert_eq!(back.validate(), Ok(()));
    }

    #[test]
    fn rejects_bad_fields() {
        assert!(event("").validate().is_err());
        assert!(event("has space").validate().is_err());
        let mut e = event("ok.action");
        e.v = 2;
        assert!(e.validate().is_err());
        e.v = 1;
        e.details = Some(serde_json::json!([1]));
        assert!(e.validate().is_err());
        e.details = Some(serde_json::json!({"x": "a".repeat(DETAILS_MAX)}));
        assert!(e.validate().is_err());
        assert!(
            serde_json::from_str::<SystemEvent>(
                r#"{"v":1,"action":"a","source":"s","outcome":"ok","x":1}"#
            )
            .is_err()
        );
    }
}
