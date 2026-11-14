//! ACARS aircraft datalink on 129 to 132 MHz through acarsdec (ADR-0031).
//!
//! acarsdec 4.x (GPL-2.0, a separate process; Thierry Leconte's decoder as maintained by
//! Thibaut Varène) reads `cu8` baseband from stdin (`--iqfile file=/dev/stdin,...`), decodes the
//! channels it is told about and prints one JSON object per message (`--output json:file:path=-`):
//!
//! ```text
//! {"channel":0,"freq":131.725,"level":-22.1,"noise":-50.0,"error":0,"mode":"2","label":"H1",
//!  "block_id":"3","ack":false,"tail":"D-AIPA","flight":"LH1234","msgno":"M23A","text":"...","end":true}
//! ```
//!
//! The object becomes an `acars.message` event; receiver bookkeeping (`channel`, `station_id`,
//! `app`) is dropped and a `summary` is added. The VFO the plugin is bound to is the centre of the
//! IQ; the channels default to that frequency and can be extended with `frequencies_mhz` as far as
//! the 240 kHz sample rate reaches.

use std::collections::BTreeSet;
use std::process::ExitCode;

use serde_json::{Map, Value, json};

use crate::event::{MAX_TEXT, event, truncate};
use crate::run::run;

/// Sample rate of the plugin's input (`plugins/acars/plugin.toml`): 20 times 12 kHz, which is what
/// acarsdec's decimator wants.
pub const SAMPLE_RATE: u32 = 240_000;
/// Channels further than this from the centre are outside the filtered band of the VFO.
pub const MAX_OFFSET_HZ: u64 = 100_000;
/// Longest decoded sub-document (ADS-C, CPDLC, ...) copied into an event, in bytes.
const MAX_DECODED: usize = 16 * 1024;

/// The ACARS channels (Hz) of a configuration: `frequencies_mhz` or, without it, the centre.
///
/// # Errors
/// A message naming the offending value.
pub fn channels(
    config: &Map<String, Value>,
    centre_hz: Option<u64>,
    max_offset_hz: u64,
) -> Result<Vec<u64>, String> {
    let Some(centre) = centre_hz else {
        return Err(
            "the decoder needs the frequency of its receiver; start it on a receiver (VFO)".into(),
        );
    };
    let list = match config.get("frequencies_mhz") {
        None | Some(Value::Null) => return Ok(vec![centre]),
        Some(Value::Array(list)) => list,
        Some(_) => return Err("frequencies_mhz must be an array of frequencies in MHz".into()),
    };
    if list.is_empty() || list.len() > 8 {
        return Err("frequencies_mhz needs 1 to 8 frequencies".into());
    }
    let mut out = BTreeSet::new();
    for entry in list {
        let mhz = entry
            .as_f64()
            .filter(|m| (100.0..=160.0).contains(m))
            .ok_or("frequencies_mhz entries must be frequencies in MHz (100 to 160)")?;
        let hz = (mhz * 1e6).round() as u64;
        if hz.abs_diff(centre) > max_offset_hz {
            return Err(format!(
                "{mhz} MHz is more than {} kHz from the receiver's {} MHz; tune the receiver \
                 closer or use a second decoder",
                max_offset_hz / 1000,
                centre as f64 / 1e6
            ));
        }
        out.insert(hz);
    }
    Ok(out.into_iter().collect())
}

/// Instance configuration (`plugins/acars/schema/config.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Centre frequency of the IQ (the receiver's), Hz.
    pub centre_hz: u64,
    /// Channels to decode, Hz.
    pub channels_hz: Vec<u64>,
    /// Also report messages without text (acknowledgements, polls).
    pub include_empty: bool,
}

impl Config {
    /// Read the instance configuration (`null` or an object); `centre_hz` is the frequency the
    /// instance was started on. Unknown keys are refused.
    ///
    /// # Errors
    /// A message naming the offending key.
    pub fn from_json(config: &Value, centre_hz: Option<u64>) -> Result<Self, String> {
        let empty = Map::new();
        let map = match config {
            Value::Null => &empty,
            Value::Object(map) => map,
            _ => return Err("config must be an object".into()),
        };
        if let Some(key) = map
            .keys()
            .find(|k| !["frequencies_mhz", "include_empty"].contains(&k.as_str()))
        {
            return Err(format!(
                "unknown config key {key:?} (frequencies_mhz, include_empty)"
            ));
        }
        let channels_hz = channels(map, centre_hz, MAX_OFFSET_HZ)?;
        let include_empty = match map.get("include_empty") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("include_empty must be true or false".into()),
        };
        Ok(Self {
            centre_hz: centre_hz.unwrap_or_default(),
            channels_hz,
            include_empty,
        })
    }
}

/// acarsdec's command line: IQ from stdin, JSON on stdout.
#[must_use]
pub fn args(config: &Config) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if !config.include_empty {
        args.push("-e".into());
    }
    args.extend([
        "--output".into(),
        "json:file:path=-".into(),
        "--iqfile".into(),
        format!("file=/dev/stdin,sr={SAMPLE_RATE},format=U8"),
        "-c".into(),
        format!("{:.6}", config.centre_hz as f64 / 1e6),
    ]);
    args.extend(
        config
            .channels_hz
            .iter()
            .map(|hz| format!("{:.6}", *hz as f64 / 1e6)),
    );
    args
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Registration as acarsdec prints it: `.D-AIPA` or `D-AIPA`, `-------` for none.
fn registration(raw: &str) -> Option<String> {
    let reg = raw.trim().trim_start_matches('.');
    (!reg.is_empty() && !reg.chars().all(|c| c == '-')).then(|| reg.to_owned())
}

/// Map one output line of acarsdec to an `acars.message` event.
#[must_use]
pub fn parse_line(line: &str) -> Option<Value> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    let Value::Object(raw) = serde_json::from_str::<Value>(line).ok()? else {
        return None;
    };
    // Every acarsdec message has a label; objects without are other output (route JSON).
    let label = raw.get("label").and_then(text)?;
    let mut d = Map::new();
    if let Some(freq) = raw.get("freq").and_then(Value::as_f64) {
        d.insert("freq_mhz".into(), json!(freq));
    }
    for (from, to) in [("level", "level"), ("noise", "noise")] {
        if let Some(v) = raw.get(from).and_then(Value::as_f64) {
            d.insert(to.into(), json!(v));
        }
    }
    if let Some(n) = raw.get("error").and_then(Value::as_u64) {
        d.insert("errors_corrected".into(), json!(n));
    }
    if let Some(mode) = raw.get("mode").and_then(text) {
        d.insert("mode".into(), json!(mode));
    }
    d.insert("label".into(), json!(label));
    let block = raw.get("block_id").and_then(text);
    if let Some(block) = &block {
        d.insert("block_id".into(), json!(block));
        // Downlink (aircraft to ground) blocks are numbered 0 to 9, uplink ones lettered.
        let downlink = block.chars().next().is_some_and(|c| c.is_ascii_digit());
        d.insert(
            "direction".into(),
            json!(if downlink { "downlink" } else { "uplink" }),
        );
    }
    if let Some(ack) = raw.get("ack").and_then(text) {
        d.insert("ack".into(), json!(ack));
    }
    let tail = raw
        .get("tail")
        .and_then(text)
        .and_then(|t| registration(&t));
    if let Some(tail) = &tail {
        d.insert("tail".into(), json!(tail));
    }
    let flight = raw.get("flight").and_then(text);
    if let Some(flight) = &flight {
        d.insert("flight".into(), json!(flight));
    }
    for key in [
        "msgno", "sublabel", "mfi", "depa", "dsta", "eta", "gtout", "gtin", "wloff", "wlin",
    ] {
        if let Some(v) = raw.get(key).and_then(text) {
            d.insert(key.into(), json!(v));
        }
    }
    let body = raw
        .get("text")
        .and_then(Value::as_str)
        .map(|t| truncate(t.trim_end(), MAX_TEXT))
        .filter(|t| !t.is_empty());
    if let Some(body) = &body {
        d.insert("text".into(), json!(body));
    }
    if raw.get("end").and_then(Value::as_bool) == Some(true) {
        d.insert("end".into(), json!(true));
    }
    // ARINC 622 content (ADS-C, CPDLC) decoded by libacars.
    if let Some(decoded) = raw.get("libacars")
        && decoded.to_string().len() <= MAX_DECODED
    {
        d.insert("decoded".into(), decoded.clone());
    }
    let mut summary = String::new();
    for part in [tail.as_deref(), flight.as_deref()].into_iter().flatten() {
        summary.push_str(part);
        summary.push(' ');
    }
    summary.push_str(&format!("[{label}]"));
    if let Some(body) = &body {
        summary.push(' ');
        summary.push_str(&body.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    d.insert("summary".into(), json!(truncate(&summary, 256)));
    Some(event("acars.message", d))
}

/// Run `program` (acarsdec) and forward its messages as events.
pub fn start(program: &str, config: &Value, centre_hz: Option<u64>) -> Result<ExitCode, String> {
    let config = Config::from_json(config, centre_hz)?;
    Ok(run(program, &args(&config), parse_line))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r##"{"timestamp":1700000000.5,"station_id":"x","channel":0,"freq":131.725,"level":-22.1,"noise":-50.0,"error":0,"mode":"2","label":"H1","block_id":"3","ack":false,"tail":".D-AIPA","flight":"LH1234","msgno":"M23A","text":"#CFBFLT01/PSN 4811.2N01131.5E\r\n","end":true,"depa":"EDDM","dsta":"EDDF","app":{"name":"acarsdec","ver":"4.7"}}"##;

    #[test]
    fn message_becomes_an_event() {
        let e = parse_line(LINE).expect("event");
        assert_eq!(e["type"], "acars.message");
        let d = &e["data"];
        assert_eq!(d["tail"], "D-AIPA");
        assert_eq!(d["flight"], "LH1234");
        assert_eq!(d["label"], "H1");
        assert_eq!(d["direction"], "downlink");
        assert_eq!(d["freq_mhz"], 131.725);
        assert_eq!(d["level"], -22.1);
        assert_eq!(d["msgno"], "M23A");
        assert_eq!(d["depa"], "EDDM");
        assert_eq!(d["end"], true);
        assert_eq!(d["text"], "#CFBFLT01/PSN 4811.2N01131.5E");
        assert!(
            d.get("ack").is_none(),
            "no ack: acarsdec prints false for a NAK-less block"
        );
        assert!(d.get("station_id").is_none() && d.get("app").is_none());
        assert_eq!(
            d["summary"],
            "D-AIPA LH1234 [H1] #CFBFLT01/PSN 4811.2N01131.5E"
        );
    }

    #[test]
    fn uplink_without_registration_text() {
        let e = parse_line(
            r#"{"channel":1,"freq":131.825,"level":-30,"error":1,"mode":"2","label":"Q0","block_id":"A","ack":"A","tail":"-------"}"#,
        )
        .expect("event");
        let d = &e["data"];
        assert_eq!(d["direction"], "uplink");
        assert!(d.get("tail").is_none());
        assert_eq!(d["errors_corrected"], 1);
        assert_eq!(d["ack"], "A");
        assert_eq!(d["summary"], "[Q0]");
    }

    #[test]
    fn decoded_arinc622_content_is_kept() {
        let e = parse_line(
            r#"{"freq":131.55,"label":"AA","mode":"2","block_id":"1","tail":"N1","text":"/x","libacars":{"adsc":{"tags":[1,2]}}}"#,
        )
        .expect("event");
        assert_eq!(e["data"]["decoded"]["adsc"]["tags"][1], 2);
    }

    #[test]
    fn other_lines_are_ignored() {
        assert!(parse_line("[#1 (F:131.725 L: -22 E:0) 25/12/2016]").is_none());
        assert!(parse_line(r#"{"flight":"LH1","depa":"EDDM","dsta":"EDDF"}"#).is_none());
        assert!(parse_line("{broken").is_none());
        assert!(parse_line("").is_none());
    }

    #[test]
    fn channels_default_to_the_receiver_frequency() {
        let c = Config::from_json(&Value::Null, Some(131_725_000)).expect("config");
        assert_eq!(c.channels_hz, vec![131_725_000]);
        assert_eq!(
            args(&c).join(" "),
            "-e --output json:file:path=- --iqfile file=/dev/stdin,sr=240000,format=U8 -c 131.725000 131.725000"
        );
    }

    #[test]
    fn extra_channels_must_fit_the_band() {
        let c = Config::from_json(
            &json!({"frequencies_mhz": [131.825, 131.725, 131.775], "include_empty": true}),
            Some(131_775_000),
        )
        .expect("config");
        assert_eq!(c.channels_hz, vec![131_725_000, 131_775_000, 131_825_000]);
        assert!(!args(&c).contains(&"-e".to_owned()));
        let err = Config::from_json(&json!({"frequencies_mhz": [131.525]}), Some(131_775_000))
            .expect_err("too far");
        assert!(err.contains("131.525"), "{err}");
        assert!(Config::from_json(&json!({"frequencies_mhz": []}), Some(131_775_000)).is_err());
        assert!(Config::from_json(&json!({"frequencies_mhz": ["x"]}), Some(131_775_000)).is_err());
        assert!(Config::from_json(&json!({"y": 1}), Some(131_775_000)).is_err());
        assert!(Config::from_json(&Value::Null, None).is_err());
    }
}
