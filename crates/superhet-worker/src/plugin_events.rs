//! Plugin events into the decoder history.
//!
//! The plugin host publishes every valid event of a plugin instance into the durable JetStream
//! stream `EVENTS` (`sh.v1.events.<plugin>.<type>`, JSON `HostEvent`, `docs/plugin-contract.md`).
//! This consumer is a durable pull consumer on that stream: it stores each event in
//! `decoder_events` (`source` = plugin id, `kind` = event type), announces the stored row on
//! `sh.v1.events.core.decoder.<dev>` for live UIs and acknowledges the message only after the
//! insert, so a worker restart or a database outage loses nothing within the stream's retention.

use std::time::Duration;

use async_nats::jetstream::{
    self, AckKind,
    consumer::{AckPolicy, pull},
};
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use superhet_bus::subjects;
use superhet_db::{PgPool, decoder_events, decoder_events::NewEvent};

/// Durable consumer name.
const CONSUMER: &str = "worker-plugin-events";
/// Delivery attempts before the server gives up on a message the database keeps refusing.
const MAX_DELIVER: i64 = 5;
/// Redelivery delay after a failed insert.
const RETRY_DELAY: Duration = Duration::from_secs(5);
/// Longest `value` stored (characters); the column allows 256.
const VALUE_CHARS: usize = 256;
/// Data fields that make a good one-line summary, in order of preference.
const SUMMARY_KEYS: [&str; 4] = ["summary", "text", "message", "value"];

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// The one-line value of an event: the first string among [`SUMMARY_KEYS`], else the compact
/// JSON of the data.
fn summary(data: &Map<String, Value>) -> String {
    let text = SUMMARY_KEYS
        .iter()
        .find_map(|key| data.get(*key).and_then(Value::as_str))
        .map_or_else(|| Value::Object(data.clone()).to_string(), str::to_owned);
    truncate_chars(&text, VALUE_CHARS)
}

/// Turn the JSON of a stream message into a history row. `None` for messages that are not plugin
/// events (the stream also holds the worker's own `core.decoder` announcements) or that violate
/// the row rules.
#[must_use]
pub fn to_event(payload: &[u8]) -> Option<NewEvent> {
    let doc: Value = serde_json::from_slice(payload).ok()?;
    let plugin_id = doc.get("plugin_id")?.as_str()?;
    let kind = doc.get("type")?.as_str()?;
    if !decoder_events::is_plugin_kind(kind) || plugin_id.is_empty() || plugin_id.len() > 128 {
        return None;
    }
    let data = doc.get("data")?.as_object()?;
    let ts_ms = i64::try_from(doc.get("host_ts_ms")?.as_u64()?).ok()?;
    let device = doc
        .get("device")
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty() && d.len() <= 64)
        .unwrap_or("-");
    let vfo = doc
        .get("vfo")
        .and_then(Value::as_u64)
        .and_then(|v| i32::try_from(v).ok());
    let freq_hz = doc
        .get("freq_hz")
        .and_then(Value::as_u64)
        .filter(|f| *f <= 10_000_000_000_000)
        .and_then(|f| i64::try_from(f).ok());
    Some(NewEvent {
        ts_ms,
        device: device.to_owned(),
        vfo,
        freq_hz,
        mode: None,
        kind: kind.to_owned(),
        source: plugin_id.to_owned(),
        value: summary(data),
        meta: json!({
            "instance": doc.get("instance_id").cloned().unwrap_or(Value::Null),
            "ts": doc.get("ts").cloned().unwrap_or(Value::Null),
            "data": data,
        }),
    })
}

/// Store one event and announce it. Returns whether the message may be acknowledged.
async fn store(pool: &PgPool, client: &async_nats::Client, new: &NewEvent) -> bool {
    match decoder_events::insert(pool, new).await {
        Ok(stored) => {
            crate::metrics::event_inserted("plugin");
            if let Ok(subject) = subjects::decoder_events(&stored.data.device) {
                let payload = stored.to_json().to_string();
                if let Err(err) = client.publish(subject, payload.into()).await {
                    tracing::debug!(%err, "plugin event announce failed");
                }
            }
            true
        }
        Err(err) => {
            crate::metrics::insert_failed();
            tracing::warn!(%err, kind = %new.kind, source = %new.source, "storing a plugin event failed");
            false
        }
    }
}

/// Consume the stream until `shutdown` resolves. Creates the stream and the consumer (waiting
/// for the bus) as needed.
pub async fn run(
    pool: PgPool,
    client: async_nats::Client,
    shutdown: impl std::future::Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    let js = jetstream::new(client.clone());
    let mut messages = loop {
        let setup = async {
            let stream = js
                .get_or_create_stream(superhet_bus::events_stream_config())
                .await
                .map_err(|e| e.to_string())?;
            let consumer = stream
                .get_or_create_consumer(
                    CONSUMER,
                    pull::Config {
                        durable_name: Some(CONSUMER.to_owned()),
                        ack_policy: AckPolicy::Explicit,
                        max_deliver: MAX_DELIVER,
                        ack_wait: Duration::from_secs(30),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            consumer.messages().await.map_err(|e| e.to_string())
        };
        tokio::select! {
            () = &mut shutdown => return,
            result = tokio::time::timeout(Duration::from_secs(10), setup) => match result {
                Ok(Ok(messages)) => break messages,
                Ok(Err(err)) => tracing::warn!(%err, "plugin event consumer not ready; retrying"),
                Err(_) => tracing::warn!("plugin event consumer setup timed out; retrying"),
            },
        }
        tokio::select! {
            () = &mut shutdown => return,
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    };
    tracing::info!("plugin event recorder running");
    loop {
        let message = tokio::select! {
            () = &mut shutdown => return,
            next = messages.next() => match next {
                Some(Ok(message)) => message,
                Some(Err(err)) => {
                    tracing::warn!(%err, "plugin event stream error");
                    continue;
                }
                None => return,
            },
        };
        let ack = match to_event(&message.message.payload) {
            // Not ours (decoder announcements, foreign payloads): nothing to store.
            None => message.ack().await,
            Some(new) => {
                if store(&pool, &client, &new).await {
                    message.ack().await
                } else {
                    message.ack_with(AckKind::Nak(Some(RETRY_DELAY))).await
                }
            }
        };
        if let Err(err) = ack {
            tracing::warn!(%err, "acknowledging a plugin event failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_event(extra: Value) -> Vec<u8> {
        let mut doc = json!({
            "v": 1,
            "plugin_id": "org.superhet.decoder.pocsag",
            "instance_id": "i-ab12cd34",
            "device": "rtl0",
            "vfo": 2,
            "freq_hz": 153_350_000u64,
            "type": "pocsag.message",
            "ts": "2026-10-27T12:00:00.000Z",
            "host_ts_ms": 1_800_000_000_123u64,
            "data": {"address": 1234, "text": "FIRE at main street"},
        });
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            doc[k] = v;
        }
        doc.to_string().into_bytes()
    }

    #[test]
    fn plugin_events_become_history_rows() {
        let e = to_event(&host_event(json!({}))).expect("event");
        assert_eq!(e.source, "org.superhet.decoder.pocsag");
        assert_eq!(e.kind, "pocsag.message");
        assert_eq!(e.device, "rtl0");
        assert_eq!((e.vfo, e.freq_hz), (Some(2), Some(153_350_000)));
        assert_eq!(e.ts_ms, 1_800_000_000_123);
        assert_eq!(e.value, "FIRE at main street");
        assert_eq!(e.meta["instance"], "i-ab12cd34");
        assert_eq!(e.meta["data"]["address"], 1234);
        assert_eq!(e.meta["ts"], "2026-10-27T12:00:00.000Z");
    }

    #[test]
    fn unbound_and_summaryless_events_still_store() {
        let e = to_event(&host_event(json!({
            "device": null, "vfo": null, "freq_hz": null,
            "data": {"temperature_c": 21.5, "id": 7},
        })))
        .expect("event");
        assert_eq!((e.device.as_str(), e.vfo, e.freq_hz), ("-", None, None));
        assert!(e.value.contains("temperature_c"), "{}", e.value);
    }

    #[test]
    fn long_values_are_cut_on_a_character_boundary() {
        let long = "ä".repeat(400);
        let e = to_event(&host_event(json!({"data": {"text": long}}))).expect("event");
        assert_eq!(e.value.chars().count(), VALUE_CHARS);
        assert!(e.value.ends_with('…'));
    }

    #[test]
    fn foreign_messages_are_skipped() {
        // The worker's own announcement has no plugin_id.
        let announce = json!({"id": 1, "ts_ms": 1, "device": "rtl0", "kind": "dtmf", "value": "1"});
        assert!(to_event(announce.to_string().as_bytes()).is_none());
        assert!(to_event(b"not json").is_none());
        assert!(to_event(&host_event(json!({"type": "Bad Type"}))).is_none());
        assert!(to_event(&host_event(json!({"data": [1]}))).is_none());
        assert!(to_event(&host_event(json!({"host_ts_ms": "x"}))).is_none());
    }
}
