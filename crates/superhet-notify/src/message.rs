//! The rendered notification as stored in the delivery outbox and sent to targets.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::rule::{context, with_extras};

/// A notification: the rendered title and body, and the event that caused it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Short heading.
    pub title: String,
    /// Text body.
    pub body: String,
    /// The decoder history row (`id`, `ts_ms`, `kind`, `source`, `value`, `meta`, ...).
    pub event: Value,
    /// Name of the rule at that time.
    pub rule: String,
    /// The rule, absent for test sends.
    #[serde(default)]
    pub rule_id: Option<i64>,
}

impl Message {
    /// The values a target's own templates (webhook body, headers, e-mail subject) can use:
    /// everything the rule templates see, plus `title` and `body`.
    #[must_use]
    pub fn context(&self) -> Value {
        let mut ctx = with_extras(&context(&self.event), &self.rule);
        if let Value::Object(map) = &mut ctx {
            map.insert("title".to_owned(), json!(self.title));
            map.insert("body".to_owned(), json!(self.body));
        }
        ctx
    }

    /// A message for the "send test" buttons.
    #[must_use]
    pub fn test(rule: &str, now_ms: i64, kind: Option<&str>, source: Option<&str>) -> Self {
        let event = json!({
            "id": 0,
            "ts_ms": now_ms,
            "device": "test",
            "vfo": null,
            "freq_hz": null,
            "mode": null,
            "kind": kind.unwrap_or("test"),
            "source": source.unwrap_or("core"),
            "value": "This is a test notification from Superhet.",
            "meta": {"data": {"test": true}},
        });
        Self {
            title: format!("Superhet test: {rule}"),
            body: "This is a test notification from Superhet.".to_owned(),
            event,
            rule: rule.to_owned(),
            rule_id: None,
        }
    }
}
