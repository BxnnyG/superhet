//! ISM-band sensors through rtl_433.
//!
//! rtl_433 reads `cu8` baseband at 250 kHz from stdin (`-r cu8:-`; this is the "test mode" file
//! input, so the `time` it prints is relative and not used) and prints one JSON object per
//! decoded transmission with `-F json`:
//!
//! ```text
//! {"time" : "@0.050000s", "model" : "Nexus-TH", "id" : 165, "channel" : 2, ...}
//! ```
//!
//! The object becomes the event data unchanged, except for fields that only describe the
//! receiver (`time`, `freq*`, `rssi`, `snr`, `noise`, `mod`); a `summary` string is added.

use serde_json::{Value, json};

use crate::event::{event, truncate};

/// Fields that describe the capture, not the sensor.
const DROPPED: [&str; 9] = [
    "time", "freq", "freq1", "freq2", "rssi", "snr", "noise", "mod", "protocol",
];

/// Instance configuration: optionally restrict the decoders (`protocols`: rtl_433 protocol
/// numbers, see `rtl_433 -R help`).
///
/// # Errors
/// A message naming the offending key.
pub fn protocols_from_config(config: &Value) -> Result<Vec<u32>, String> {
    let object = match config {
        Value::Null => return Ok(Vec::new()),
        Value::Object(object) => object,
        _ => return Err("config must be an object".into()),
    };
    let mut protocols = Vec::new();
    for (key, value) in object {
        if key != "protocols" {
            return Err(format!("unknown config key {key:?}"));
        }
        let list = value.as_array().ok_or("protocols must be an array")?;
        for entry in list {
            let number = entry
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (1..=9999).contains(n))
                .ok_or("protocols entries must be protocol numbers")?;
            protocols.push(number);
        }
    }
    Ok(protocols)
}

/// Arguments for rtl_433 (input on stdin, JSON on stdout).
#[must_use]
pub fn args(protocols: &[u32]) -> Vec<String> {
    let mut args: Vec<String> = ["-r", "cu8:-", "-s", "250k", "-F", "json"]
        .map(String::from)
        .into();
    for protocol in protocols {
        args.extend(["-R".to_owned(), protocol.to_string()]);
    }
    args
}

/// Map one output line of rtl_433 to an `rtl433.reading` event.
#[must_use]
pub fn parse_line(line: &str) -> Option<Value> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    let Value::Object(mut data) = serde_json::from_str::<Value>(line).ok()? else {
        return None;
    };
    let model = data.get("model")?.as_str()?.to_owned();
    for key in DROPPED {
        data.remove(key);
    }
    let mut summary = model;
    if let Some(id) = data.get("id") {
        summary.push_str(&format!(" id={}", scalar(id)));
    }
    for (key, value) in &data {
        if key != "model" && key != "id" && !value.is_object() && !value.is_array() {
            summary.push_str(&format!(" {key}={}", scalar(value)));
        }
    }
    data.insert("summary".into(), json!(truncate(&summary, 256)));
    Some(event("rtl433.reading", data))
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_drops_receiver_fields_and_adds_summary() {
        let line = r#"{"time" : "@0.050000s", "protocol" : 19, "model" : "Nexus-TH", "id" : 165, "channel" : 2, "battery_ok" : 1, "temperature_C" : 21.500, "humidity" : 48, "mod" : "ASK", "freq" : 433.919, "rssi" : -2.2}"#;
        let event = parse_line(line).expect("event");
        let d = &event["data"];
        assert_eq!(event["type"], "rtl433.reading");
        assert_eq!(d["model"], "Nexus-TH");
        assert_eq!(d["id"], 165);
        assert_eq!(d["temperature_C"], 21.5);
        assert!(d.get("time").is_none() && d.get("freq").is_none() && d.get("rssi").is_none());
        assert_eq!(
            d["summary"],
            "Nexus-TH id=165 battery_ok=1 channel=2 humidity=48 temperature_C=21.5"
        );
    }

    #[test]
    fn ignores_log_lines_and_objects_without_model() {
        assert!(parse_line("Registered 191 out of 223 device decoding protocols").is_none());
        assert!(parse_line(r#"{"time":"x"}"#).is_none());
        assert!(parse_line("{broken").is_none());
    }

    #[test]
    fn config() {
        assert_eq!(protocols_from_config(&Value::Null), Ok(vec![]));
        assert_eq!(
            protocols_from_config(&json!({"protocols": [19, 20]})),
            Ok(vec![19, 20])
        );
        assert!(protocols_from_config(&json!({"protocols": [0]})).is_err());
        assert!(protocols_from_config(&json!({"x": 1})).is_err());
        assert_eq!(args(&[19]).join(" "), "-r cu8:- -s 250k -F json -R 19");
    }
}
