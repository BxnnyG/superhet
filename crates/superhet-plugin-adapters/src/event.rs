//! Building contract events.

use serde_json::{Map, Value, json};

/// Longest free text copied into an event (characters).
pub const MAX_TEXT: usize = 4096;

/// An event without a timestamp (the host stamps its receive time).
#[must_use]
pub fn event(kind: &str, data: Map<String, Value>) -> Value {
    json!({ "v": 1, "type": kind, "data": data })
}

/// `text` cut to at most `max` characters.
#[must_use]
pub fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// `value` rounded to five decimals (about a metre), for coordinates.
#[must_use]
pub fn round5(value: f64) -> f64 {
    (value * 1e5).round() / 1e5
}
