//! Plugin events: one JSON object per line (simple transport) or per `event` frame (framed).
//!
//! The envelope is fixed by the contract; the `data` object is plugin specific and checked
//! against the plugin's `events_schema`. [`EventValidator::check_line`] is what the host (and the
//! contract test suite) apply to every line a plugin writes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::PROTOCOL_VERSION;

/// Longest event line the host accepts (bytes, excluding the newline).
pub const MAX_EVENT_LINE: usize = 64 * 1024;

/// Reserved event type: liveness marker, never forwarded or checked against `events_schema`.
pub const HEARTBEAT_TYPE: &str = "heartbeat";

/// The envelope of an event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Contract version; must be 1.
    pub v: u32,
    /// Dotted type such as `pocsag.message`: 1-8 segments of `[a-z][a-z0-9-]*`, at most 64
    /// characters. `heartbeat` is reserved.
    #[serde(rename = "type")]
    pub event_type: String,
    /// Time of the observation, RFC 3339. The host stamps its own time when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
    /// Plugin specific payload (an object).
    #[serde(default)]
    pub data: Map<String, Value>,
}

/// An event as the host forwards it on the bus (`sh.v1.events.<plugin>.<type>`): the plugin's
/// envelope plus the identity of the instance and the host's receive time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostEvent {
    /// Contract version.
    pub v: u32,
    /// Plugin id.
    pub plugin_id: String,
    /// Instance id.
    pub instance_id: String,
    /// Device the instance is bound to.
    pub device: Option<String>,
    /// VFO the instance is fed from.
    #[serde(default)]
    pub vfo: Option<u32>,
    /// Frequency (Hz) the instance is bound to.
    pub freq_hz: Option<u64>,
    /// Event type.
    #[serde(rename = "type")]
    pub event_type: String,
    /// Time stamped by the plugin, if any.
    pub ts: Option<String>,
    /// Receive time at the host (Unix milliseconds).
    pub host_ts_ms: u64,
    /// Plugin specific payload.
    pub data: Map<String, Value>,
}

/// Why an event line was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EventError {
    /// The line exceeds [`MAX_EVENT_LINE`].
    #[error("line longer than {MAX_EVENT_LINE} bytes")]
    TooLong,
    /// The line is not UTF-8 JSON.
    #[error("not valid JSON: {0}")]
    NotJson(String),
    /// The envelope breaks the contract.
    #[error("bad envelope: {0}")]
    Envelope(String),
    /// The event does not satisfy the plugin's `events_schema`.
    #[error("violates events_schema: {0}")]
    Schema(String),
}

/// The `events_schema` of a plugin is not usable.
#[derive(Debug, thiserror::Error)]
#[error("events_schema is not a valid JSON Schema: {0}")]
pub struct SchemaError(pub(crate) String);

/// Result of checking one line.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// Liveness marker.
    Heartbeat,
    /// A well-formed event.
    Event(Event),
}

/// Checks lines against the envelope rules and an optional plugin schema.
pub struct EventValidator {
    schema: Option<jsonschema::Validator>,
}

impl std::fmt::Debug for EventValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventValidator")
            .field("schema", &self.schema.is_some())
            .finish()
    }
}

impl EventValidator {
    /// Build a validator; `schema` is the plugin's `events_schema` document, applied to the
    /// complete event object (envelope included), so a schema can switch on `type`.
    ///
    /// # Errors
    /// [`SchemaError`] when the document is not a valid JSON Schema.
    pub fn new(schema: Option<&Value>) -> Result<Self, SchemaError> {
        let schema = schema
            .map(|doc| jsonschema::validator_for(doc).map_err(|err| SchemaError(err.to_string())))
            .transpose()?;
        Ok(Self { schema })
    }

    /// Check one line (without the trailing newline).
    ///
    /// # Errors
    /// The first rule the line breaks.
    pub fn check_line(&self, line: &[u8]) -> Result<Line, EventError> {
        if line.len() > MAX_EVENT_LINE {
            return Err(EventError::TooLong);
        }
        let value: Value =
            serde_json::from_slice(line).map_err(|err| EventError::NotJson(err.to_string()))?;
        self.check_value(value)
    }

    /// Check an already parsed value.
    ///
    /// # Errors
    /// The first rule the value breaks.
    pub fn check_value(&self, value: Value) -> Result<Line, EventError> {
        let event: Event = serde_json::from_value(value.clone())
            .map_err(|err| EventError::Envelope(err.to_string()))?;
        if event.v != PROTOCOL_VERSION {
            return Err(EventError::Envelope(format!(
                "v is {} but must be {PROTOCOL_VERSION}",
                event.v
            )));
        }
        if !is_valid_event_type(&event.event_type) {
            return Err(EventError::Envelope(format!(
                "type {:?} is not 1-8 dotted segments of [a-z][a-z0-9-]*",
                event.event_type
            )));
        }
        if let Some(ts) = &event.ts
            && time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
                .is_err()
        {
            return Err(EventError::Envelope(format!("ts {ts:?} is not RFC 3339")));
        }
        if event.event_type == HEARTBEAT_TYPE {
            return Ok(Line::Heartbeat);
        }
        if let Some(schema) = &self.schema
            && let Err(err) = schema.validate(&value)
        {
            return Err(EventError::Schema(err.to_string()));
        }
        Ok(Line::Event(event))
    }
}

/// Whether `ty` is a legal event type.
#[must_use]
pub fn is_valid_event_type(ty: &str) -> bool {
    if ty.is_empty() || ty.len() > 64 {
        return false;
    }
    let mut segments = 0;
    for segment in ty.split('.') {
        segments += 1;
        let mut chars = segment.chars();
        if !chars.next().is_some_and(|c| c.is_ascii_lowercase())
            || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return false;
        }
    }
    segments <= 8
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plain() -> EventValidator {
        EventValidator::new(None).expect("no schema")
    }

    #[test]
    fn accepts_envelope() {
        let line =
            br#"{"v":1,"type":"echo.block","ts":"2026-10-26T18:00:00.123Z","data":{"bytes":4}}"#;
        match plain().check_line(line) {
            Ok(Line::Event(e)) => assert_eq!(e.event_type, "echo.block"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            plain().check_line(br#"{"v":1,"type":"heartbeat"}"#),
            Ok(Line::Heartbeat)
        );
    }

    #[test]
    fn rejects_bad_envelopes() {
        let v = plain();
        for (line, want) in [
            (&br"not json"[..], "JSON"),
            (br#"{"v":2,"type":"a"}"#, "must be 1"),
            (br#"{"v":1,"type":"Bad"}"#, "segments"),
            (br#"{"v":1,"type":"a","data":[]}"#, "envelope"),
            (br#"{"v":1,"type":"a","extra":1}"#, "extra"),
            (br#"{"v":1,"type":"a","ts":"yesterday"}"#, "RFC 3339"),
            (br#"{"type":"a"}"#, "missing"),
        ] {
            let err = v.check_line(line).expect_err("rejected");
            assert!(err.to_string().contains(want), "{line:?}: {err}");
        }
        let long = vec![b' '; MAX_EVENT_LINE + 1];
        assert_eq!(v.check_line(&long), Err(EventError::TooLong));
    }

    #[test]
    fn applies_plugin_schema_but_not_to_heartbeats() {
        let schema = json!({
            "type": "object",
            "properties": {"data": {"type": "object", "required": ["n"],
                "properties": {"n": {"type": "integer"}}}}
        });
        let v = EventValidator::new(Some(&schema)).expect("schema");
        assert!(
            v.check_line(br#"{"v":1,"type":"a","data":{"n":3}}"#)
                .is_ok()
        );
        assert!(matches!(
            v.check_line(br#"{"v":1,"type":"a","data":{"n":"x"}}"#),
            Err(EventError::Schema(_))
        ));
        assert_eq!(
            v.check_line(br#"{"v":1,"type":"heartbeat"}"#),
            Ok(Line::Heartbeat)
        );
        assert!(EventValidator::new(Some(&json!({"type": 12}))).is_err());
    }
}
