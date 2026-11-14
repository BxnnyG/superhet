//! VDL Mode 2 aircraft datalink on 136.6 to 137 MHz through dumpvdl2 (ADR-0031).
//!
//! dumpvdl2 2.x (GPL-3.0, a separate process; with libacars, MIT) reads `cu8` baseband from stdin
//! (`--iq-file -`) and prints one JSON object per AVLC frame
//! (`--output decoded:json:file:path=-`), nested the way libacars formats protocol trees:
//!
//! ```text
//! {"vdl2":{"app":{...},"t":{"sec":1700000000,"usec":1},"freq":136975000,"sig_level":-27.1,
//!  "noise_level":-51.0,"freq_skew":1.2,"burst_len_octets":60,
//!  "avlc":{"src":{"addr":"3C6444","type":"Aircraft","status":"Airborne"},
//!          "dst":{"addr":"22A1B3","type":"Ground station"},"cr":"Command","frame_type":"I",
//!          "acars":{"err":false,"reg":".D-AIPA","mode":"2","label":"H1","blk_id":"3",
//!                   "flight":"LH1234","msg_num":"M23A","msg_text":"..."}}}}
//! ```
//!
//! Frames that carry a message (ACARS, X.25 with ATN or ADS-C / CPDLC content) become
//! `vdl2.message` events; link management frames (XID, supervisory and unnumbered frames) are
//! only reported with `all_frames`. The aircraft's 24-bit address is the same ICAO address its
//! ADS-B transponder sends, which lets the web UI link a message to the aircraft on the map.

use std::process::ExitCode;

use serde_json::{Map, Value, json};

use crate::acars::channels;
use crate::event::{MAX_TEXT, event, truncate};
use crate::run::run;

/// dumpvdl2 samples at 105 kHz times this factor.
pub const OVERSAMPLE: u32 = 4;
/// Sample rate of the plugin's input (`plugins/vdl2/plugin.toml`).
pub const SAMPLE_RATE: u32 = 105_000 * OVERSAMPLE;
/// Channels further than this from the centre are outside the filtered band of the VFO.
pub const MAX_OFFSET_HZ: u64 = 135_000;
/// Longest decoded sub-document copied into an event, in bytes.
const MAX_DECODED: usize = 16 * 1024;

/// Instance configuration (`plugins/vdl2/schema/config.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Centre frequency of the IQ (the receiver's), Hz.
    pub centre_hz: u64,
    /// Channels to decode, Hz.
    pub channels_hz: Vec<u64>,
    /// Also report link management frames without a message.
    pub all_frames: bool,
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
            .find(|k| !["frequencies_mhz", "all_frames"].contains(&k.as_str()))
        {
            return Err(format!(
                "unknown config key {key:?} (frequencies_mhz, all_frames)"
            ));
        }
        let channels_hz = channels(map, centre_hz, MAX_OFFSET_HZ)?;
        let all_frames = match map.get("all_frames") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("all_frames must be true or false".into()),
        };
        Ok(Self {
            centre_hz: centre_hz.unwrap_or_default(),
            channels_hz,
            all_frames,
        })
    }
}

/// dumpvdl2's command line: IQ from stdin, JSON on stdout.
#[must_use]
pub fn args(config: &Config) -> Vec<String> {
    let mut args: Vec<String> = ["--iq-file", "-", "--sample-format", "U8", "--oversample"]
        .map(String::from)
        .into();
    args.push(OVERSAMPLE.to_string());
    args.extend([
        "--centerfreq".into(),
        config.centre_hz.to_string(),
        "--output".into(),
        "decoded:json:file:path=-".into(),
    ]);
    args.extend(config.channels_hz.iter().map(u64::to_string));
    args
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The 24-bit address of an AVLC station as six lower-case hex digits.
fn address(station: &Value) -> Option<String> {
    text(station.get("addr")?).map(|a| a.to_ascii_lowercase())
}

/// Map one output line of dumpvdl2 to a `vdl2.message` event; frames without a message only when
/// `all_frames` is set.
#[must_use]
pub fn parse_line(line: &str, all_frames: bool) -> Option<Value> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    let root = serde_json::from_str::<Value>(line).ok()?;
    let vdl2 = root.get("vdl2")?;
    let avlc = vdl2.get("avlc")?;
    let mut d = Map::new();
    if let Some(freq) = vdl2.get("freq").and_then(Value::as_f64) {
        d.insert("freq_mhz".into(), json!(freq / 1e6));
    }
    for (from, to) in [("sig_level", "level"), ("noise_level", "noise")] {
        if let Some(v) = vdl2.get(from).and_then(Value::as_f64) {
            d.insert(to.into(), json!((v * 10.0).round() / 10.0));
        }
    }
    let src = avlc.get("src");
    let dst = avlc.get("dst");
    for (key, station) in [("src", src), ("dst", dst)] {
        let Some(station) = station else { continue };
        if let Some(addr) = address(station) {
            d.insert(format!("{key}_addr"), json!(addr));
        }
        if let Some(kind) = station.get("type").and_then(text) {
            d.insert(format!("{key}_type"), json!(kind));
        }
    }
    // The aircraft's address, whichever side it is on.
    let is_aircraft = |station: Option<&Value>| {
        station.is_some_and(|s| s.get("type").and_then(Value::as_str) == Some("Aircraft"))
    };
    if let Some(station) = [src, dst]
        .into_iter()
        .flatten()
        .find(|s| is_aircraft(Some(s)))
        && let Some(icao) = address(station)
    {
        d.insert("icao".into(), json!(icao));
    }
    if let Some(frame) = avlc.get("frame_type").and_then(text) {
        d.insert("frame".into(), json!(frame));
    }
    if let Some(cmd) = avlc.get("cmd").and_then(text) {
        d.insert("cmd".into(), json!(cmd));
    }

    let mut summary = String::new();
    let kind;
    if let Some(acars) = avlc.get("acars") {
        let err = acars.get("err").and_then(Value::as_bool) == Some(true);
        if err && !all_frames {
            return None;
        }
        kind = "acars";
        acars_fields(acars, &mut d, &mut summary);
    } else if let Some(x25) = avlc.get("x25") {
        kind = "x25";
        if x25.to_string().len() <= MAX_DECODED {
            d.insert("decoded".into(), x25.clone());
        }
        summary.push_str("X.25 data");
        if let Some(icao) = d.get("icao").and_then(Value::as_str) {
            summary = format!("{} {summary}", icao.to_ascii_uppercase());
        }
    } else if avlc.get("xid").is_some() {
        kind = "xid";
        summary.push_str("XID link management");
    } else {
        kind = "frame";
        summary.push_str("AVLC frame");
    }
    if !all_frames && (kind == "xid" || kind == "frame") {
        return None;
    }
    d.insert("kind".into(), json!(kind));
    d.insert("summary".into(), json!(truncate(&summary, 256)));
    Some(event("vdl2.message", d))
}

/// The ACARS block inside a VDL2 frame (libacars' field names).
fn acars_fields(acars: &Value, d: &mut Map<String, Value>, summary: &mut String) {
    let tail = acars
        .get("reg")
        .and_then(text)
        .map(|r| r.trim_start_matches('.').to_owned())
        .filter(|r| !r.is_empty() && !r.chars().all(|c| c == '-'));
    let flight = acars.get("flight").and_then(text);
    let label = acars.get("label").and_then(text);
    for (from, to) in [
        ("mode", "mode"),
        ("blk_id", "block_id"),
        ("msg_num", "msgno"),
        ("sublabel", "sublabel"),
        ("mfi", "mfi"),
    ] {
        if let Some(v) = acars.get(from).and_then(text) {
            d.insert(to.into(), json!(v));
        }
    }
    if let Some(ack) = acars.get("ack").and_then(text) {
        d.insert("ack".into(), json!(ack));
    }
    if let Some(tail) = &tail {
        d.insert("tail".into(), json!(tail));
    }
    if let Some(flight) = &flight {
        d.insert("flight".into(), json!(flight));
    }
    if let Some(label) = &label {
        d.insert("label".into(), json!(label));
    }
    let body = acars
        .get("msg_text")
        .and_then(Value::as_str)
        .map(|t| truncate(t.trim_end(), MAX_TEXT))
        .filter(|t| !t.is_empty());
    if let Some(body) = &body {
        d.insert("text".into(), json!(body));
    }
    // ARINC 622 content (ADS-C, CPDLC) that libacars decoded.
    const PLAIN: [&str; 14] = [
        "err",
        "crc_ok",
        "more",
        "reg",
        "mode",
        "label",
        "blk_id",
        "ack",
        "flight",
        "msg_num",
        "msg_num_seq",
        "sublabel",
        "mfi",
        "msg_text",
    ];
    if let Value::Object(map) = acars {
        let extra: Map<String, Value> = map
            .iter()
            .filter(|(k, _)| !PLAIN.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !extra.is_empty() && Value::Object(extra.clone()).to_string().len() <= MAX_DECODED {
            d.insert("decoded".into(), Value::Object(extra));
        }
    }
    for part in [tail.as_deref(), flight.as_deref()].into_iter().flatten() {
        summary.push_str(part);
        summary.push(' ');
    }
    match &label {
        Some(label) => summary.push_str(&format!("[{label}]")),
        None => summary.push_str("ACARS"),
    }
    if let Some(body) = &body {
        summary.push(' ');
        summary.push_str(&body.split_whitespace().collect::<Vec<_>>().join(" "));
    }
}

/// Run `program` (dumpvdl2) and forward its messages as events.
pub fn start(program: &str, config: &Value, centre_hz: Option<u64>) -> Result<ExitCode, String> {
    let config = Config::from_json(config, centre_hz)?;
    let all_frames = config.all_frames;
    Ok(run(program, &args(&config), move |line| {
        parse_line(line, all_frames)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACARS: &str = r##"{"vdl2":{"app":{"name":"dumpvdl2","ver":"2.7.0"},"t":{"sec":1700000000,"usec":1},"freq":136975000,"burst_len_octets":60,"hdr_bits_fixed":0,"octets_corrected_by_fec":0,"idx":0,"sig_level":-27.14,"noise_level":-51.0,"freq_skew":1.2,"avlc":{"src":{"addr":"3C6444","type":"Aircraft","status":"Airborne"},"dst":{"addr":"22A1B3","type":"Ground station"},"cr":"Command","frame_type":"I","rseq":1,"sseq":2,"poll":false,"acars":{"err":false,"crc_ok":true,"more":false,"reg":".D-AIPA","mode":"2","label":"H1","blk_id":"3","ack":false,"flight":"LH1234","msg_num":"M23A","msg_text":"#CFBFLT01/PSN 4811.2N01131.5E"}}}}"##;

    #[test]
    fn acars_over_vdl2() {
        let e = parse_line(ACARS, false).expect("event");
        assert_eq!(e["type"], "vdl2.message");
        let d = &e["data"];
        assert_eq!(d["kind"], "acars");
        assert_eq!(d["freq_mhz"], 136.975);
        assert_eq!(d["level"], -27.1);
        assert_eq!(d["src_addr"], "3c6444");
        assert_eq!(d["src_type"], "Aircraft");
        assert_eq!(d["dst_type"], "Ground station");
        assert_eq!(d["icao"], "3c6444");
        assert_eq!(d["tail"], "D-AIPA");
        assert_eq!(d["flight"], "LH1234");
        assert_eq!(d["label"], "H1");
        assert_eq!(d["msgno"], "M23A");
        assert_eq!(d["frame"], "I");
        assert_eq!(
            d["summary"],
            "D-AIPA LH1234 [H1] #CFBFLT01/PSN 4811.2N01131.5E"
        );
    }

    #[test]
    fn uplink_takes_the_aircraft_address_from_the_destination() {
        let line = ACARS.replace(
            r#""src":{"addr":"3C6444","type":"Aircraft","status":"Airborne"},"dst":{"addr":"22A1B3","type":"Ground station"}"#,
            r#""src":{"addr":"22A1B3","type":"Ground station"},"dst":{"addr":"3C6444","type":"Aircraft"}"#,
        );
        let e = parse_line(&line, false).expect("event");
        assert_eq!(e["data"]["icao"], "3c6444");
        assert_eq!(e["data"]["src_type"], "Ground station");
    }

    #[test]
    fn link_management_frames_need_all_frames() {
        let xid = r#"{"vdl2":{"freq":136975000,"avlc":{"src":{"addr":"3C6444","type":"Aircraft"},"dst":{"addr":"22A1B3","type":"Ground station"},"frame_type":"U","cmd":"XID","xid":{"type":"Handoff request"}}}}"#;
        assert!(parse_line(xid, false).is_none());
        let e = parse_line(xid, true).expect("all frames");
        assert_eq!(e["data"]["kind"], "xid");
        assert_eq!(e["data"]["icao"], "3c6444");
    }

    #[test]
    fn x25_payload_is_kept_decoded() {
        let line = r#"{"vdl2":{"freq":136725000,"avlc":{"src":{"addr":"3C6444","type":"Aircraft"},"dst":{"addr":"22A1B3","type":"Ground station"},"frame_type":"I","x25":{"pkt_type":"Data","clnp":{"cpdlc":{"msg":"x"}}}}}}"#;
        let e = parse_line(line, false).expect("event");
        assert_eq!(e["data"]["kind"], "x25");
        assert_eq!(e["data"]["decoded"]["clnp"]["cpdlc"]["msg"], "x");
        assert_eq!(e["data"]["summary"], "3C6444 X.25 data");
    }

    #[test]
    fn corrupt_acars_blocks_and_noise_are_dropped() {
        let bad = ACARS.replace(r#""err":false"#, r#""err":true"#);
        assert!(parse_line(&bad, false).is_none());
        assert!(parse_line(&bad, true).is_some());
        assert!(parse_line("Sampling rate set to 420000 sps", false).is_none());
        assert!(parse_line(r#"{"other":1}"#, false).is_none());
        assert!(parse_line("{broken", false).is_none());
    }

    #[test]
    fn command_line() {
        let c = Config::from_json(
            &json!({"frequencies_mhz": [136.725, 136.975]}),
            Some(136_850_000),
        )
        .expect("config");
        assert_eq!(c.channels_hz, vec![136_725_000, 136_975_000]);
        assert_eq!(SAMPLE_RATE, 420_000);
        assert_eq!(
            args(&c).join(" "),
            "--iq-file - --sample-format U8 --oversample 4 --centerfreq 136850000 \
             --output decoded:json:file:path=- 136725000 136975000"
        );
        assert!(
            Config::from_json(&json!({"frequencies_mhz": [137.5]}), Some(136_850_000)).is_err()
        );
        assert!(Config::from_json(&json!({"all_frames": 1}), Some(136_850_000)).is_err());
        assert!(Config::from_json(&Value::Null, None).is_err());
    }
}
