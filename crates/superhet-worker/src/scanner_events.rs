//! Scanner hits into the decoder history (`source` `scanner`, kind `scanner.hit`).
//!
//! `superhet-sdrd` publishes a JSON hit on `sh.v1.dev.<dev>.scanner.hit` each time the scanner
//! stops on a channel (`docs/scanner.md`); the consumer stores it like any decoder event, so
//! hits show up in the history, its export and the live event feed.

use serde_json::{Value, json};
use superhet_db::decoder_events::NewEvent;

/// `source` of scanner rows.
pub const SOURCE: &str = "scanner";
/// `kind` of scanner rows.
pub const KIND: &str = "scanner.hit";

/// Device id from `sh.v1.dev.<dev>.scanner.hit`.
#[must_use]
pub fn subject_device(subject: &str) -> Option<&str> {
    let mut it = subject.split('.');
    let (_, _, _, dev, scanner, hit) = (
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
    );
    (scanner == "scanner" && hit == "hit" && it.next().is_none() && !dev.is_empty()).then_some(dev)
}

fn mhz(freq_hz: i64) -> String {
    let text = format!("{:.6}", freq_hz as f64 / 1e6);
    let text = text.trim_end_matches('0');
    format!("{} MHz", text.trim_end_matches('.'))
}

/// A history row from a hit message, or `None` for anything malformed.
#[must_use]
pub fn to_event(device: &str, payload: &[u8]) -> Option<NewEvent> {
    let doc: Value = serde_json::from_slice(payload).ok()?;
    let freq_hz = i64::try_from(doc.get("freq_hz")?.as_u64()?)
        .ok()
        .filter(|f| (1..=10_000_000_000_000).contains(f))?;
    let ts_ms = i64::try_from(doc.get("ts_ms")?.as_u64()?).ok()?;
    let vfo = doc
        .get("vfo")
        .and_then(Value::as_u64)
        .and_then(|v| i32::try_from(v).ok());
    let mode = doc
        .get("mode")
        .and_then(Value::as_str)
        .filter(|m| m.len() <= 8)
        .map(str::to_owned);
    let label = doc
        .get("label")
        .and_then(Value::as_str)
        .map(|l| l.chars().take(64).collect::<String>());
    let margin = doc.get("margin_db").and_then(Value::as_f64);
    let mut value = mhz(freq_hz);
    if let Some(label) = label.as_deref().filter(|l| !l.is_empty()) {
        value = format!("{label} ({value})");
    }
    if let Some(margin) = margin.filter(|m| m.is_finite()) {
        value = format!("{value} +{margin:.0} dB");
    }
    Some(NewEvent {
        ts_ms,
        device: device.to_owned(),
        vfo,
        freq_hz: Some(freq_hz),
        mode,
        kind: KIND.to_owned(),
        source: SOURCE.to_owned(),
        value,
        meta: json!({
            "label": label,
            "bandwidth_hz": doc.get("bandwidth_hz").cloned().unwrap_or(Value::Null),
            "level_dbfs": doc.get("level_dbfs").cloned().unwrap_or(Value::Null),
            "margin_db": doc.get("margin_db").cloned().unwrap_or(Value::Null),
            "priority": doc.get("priority").and_then(Value::as_bool).unwrap_or(false),
            "scan": doc.get("source").cloned().unwrap_or(Value::Null),
            "list": doc.get("list").cloned().unwrap_or(Value::Null),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_subject() {
        assert_eq!(subject_device("sh.v1.dev.rtl0.scanner.hit"), Some("rtl0"));
        assert_eq!(subject_device("sh.v1.dev.rtl0.scanner.x"), None);
        assert_eq!(subject_device("sh.v1.dev.rtl0.state"), None);
        assert_eq!(subject_device("sh.v1.dev.rtl0.scanner.hit.more"), None);
    }

    #[test]
    fn a_hit_becomes_a_scanner_row() {
        let payload = br#"{"v":1,"ts_ms":1760000000000,"device":"dev0","vfo":2,
            "freq_hz":446006250,"mode":"nfm","bandwidth_hz":12500,"label":"PMR 1",
            "level_dbfs":-31.4,"margin_db":42.7,"priority":false,"source":"list","list":"PMR"}"#;
        let event = to_event("dev0", payload).expect("event");
        assert_eq!(event.source, "scanner");
        assert_eq!(event.kind, "scanner.hit");
        assert_eq!(event.vfo, Some(2));
        assert_eq!(event.freq_hz, Some(446_006_250));
        assert_eq!(event.mode.as_deref(), Some("nfm"));
        assert_eq!(event.value, "PMR 1 (446.00625 MHz) +43 dB");
        assert_eq!(event.meta["list"], "PMR");
        assert!(superhet_db::decoder_events::is_plugin_kind(&event.kind));
    }

    #[test]
    fn malformed_hits_are_ignored() {
        assert!(to_event("d", b"nope").is_none());
        assert!(to_event("d", br#"{"ts_ms":1}"#).is_none());
        assert!(to_event("d", br#"{"ts_ms":1,"freq_hz":0}"#).is_none());
    }

    #[test]
    fn frequencies_are_trimmed() {
        assert_eq!(mhz(145_500_000), "145.5 MHz");
        assert_eq!(mhz(100_000_000), "100 MHz");
    }
}
