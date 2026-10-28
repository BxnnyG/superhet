//! Parser for `welle-cli`'s `GET /mux.json` (welle.io 2.4, `src/welle-cli/jsonconvert.cpp`).
//!
//! Only what the plugin needs is read, and every field is optional: the document changes
//! between welle.io releases and while the receiver is still locking on, so a missing or
//! oddly typed field never fails the parse.

use serde_json::Value;

/// Audio coding of a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// MPEG-1/2 Layer II (classic DAB).
    Dab,
    /// HE-AAC v2 in DAB+ superframes.
    DabPlus,
    /// No audio component (data service).
    Data,
}

impl Codec {
    /// Name for events and the UI.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dab => "DAB",
            Self::DabPlus => "DAB+",
            Self::Data => "data",
        }
    }
}

/// The ensemble as far as it is decoded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ensemble {
    /// Ensemble label (16 characters, trailing blanks trimmed).
    pub label: String,
    /// Ensemble id as four upper-case hex digits (`D218`).
    pub eid: String,
    /// Extended country code as two hex digits.
    pub ecc: String,
}

impl Ensemble {
    /// Whether the receiver has decoded an ensemble id at all.
    #[must_use]
    pub fn known(&self) -> bool {
        !self.eid.trim_start_matches('0').is_empty()
    }
}

/// One service (station).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// Service id, numeric.
    pub sid: u32,
    /// Service id as welle prints it (four upper-case hex digits, eight for data).
    pub sid_hex: String,
    /// Service label, trimmed; empty until the label was received.
    pub label: String,
    /// Programme type name (`News`, `Pop Music`, ...), empty if unknown.
    pub pty: String,
    /// Language name, empty if unknown.
    pub language: String,
    /// Audio coding.
    pub codec: Codec,
    /// Bit rate of the primary audio component (kbit/s).
    pub bitrate: Option<u32>,
    /// Protection profile as welle names it.
    pub protection: String,
    /// Dynamic label text, only while the receiver decodes this service.
    pub dls: Option<String>,
    /// Change counter of the MOT slideshow (Unix seconds in welle), 0 if none was received.
    pub mot_changed: i64,
    /// Reed-Solomon / AAC / frame errors counted while decoding (all three added up).
    pub audio_errors: u64,
    /// Whether welle offers an MP3 stream for the service.
    pub has_stream: bool,
}

/// What `mux.json` says.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mux {
    /// The ensemble.
    pub ensemble: Ensemble,
    /// Services in the order welle lists them.
    pub services: Vec<Service>,
    /// Demodulator signal-to-noise ratio (dB); 0 until sync.
    pub snr: f64,
    /// FIC CRC errors since the channel was tuned.
    pub fic_crc_errors: u64,
    /// Frequency correction the demodulator applies (Hz).
    pub freq_correction_hz: f64,
}

fn text(value: &Value) -> String {
    // Labels are objects (`{"label": "...", "shortlabel": ...}`) in welle.io 2.x, plain
    // strings elsewhere.
    let raw = value
        .as_str()
        .or_else(|| value.get("label").and_then(Value::as_str))
        .unwrap_or_default();
    raw.trim().to_owned()
}

fn hex_upper(value: &Value) -> String {
    value
        .as_str()
        .map(|s| s.trim().trim_start_matches("0x").to_ascii_uppercase())
        .unwrap_or_default()
}

fn service(value: &Value) -> Option<Service> {
    let sid_hex = hex_upper(&value["sid"]);
    let sid = u32::from_str_radix(&sid_hex, 16).ok()?;
    let mut codec = Codec::Data;
    let mut bitrate = None;
    let mut protection = String::new();
    for component in value["components"].as_array().into_iter().flatten() {
        if component["transportmode"] != "audio" {
            continue;
        }
        let this = match component["ascty"].as_str() {
            Some("DAB+") => Codec::DabPlus,
            Some("DAB") => Codec::Dab,
            _ => continue,
        };
        // The primary component decides; keep the first audio component otherwise.
        if codec == Codec::Data || component["primary"] == true {
            codec = this;
            bitrate = component["subchannel"]["bitrate"]
                .as_u64()
                .and_then(|b| u32::try_from(b).ok());
            protection = component["subchannel"]["protection"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
        }
    }
    let dls = value["dls"]["label"]
        .as_str()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let errors = &value["errorcounters"];
    let count = |key: &str| errors[key].as_u64().unwrap_or(0);
    Some(Service {
        sid,
        sid_hex,
        label: text(&value["label"]),
        pty: value["ptystring"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned(),
        language: value["languagestring"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned(),
        codec,
        bitrate,
        protection,
        dls,
        mot_changed: value["mot"]["lastchange"].as_i64().unwrap_or(0),
        audio_errors: count("frameerrors") + count("rserrors") + count("aacerrors"),
        has_stream: value["url_mp3"].is_string(),
    })
}

/// Parse a `mux.json` body.
///
/// # Errors
/// The text is not a JSON object.
pub fn parse(body: &[u8]) -> Result<Mux, String> {
    let doc: Value = serde_json::from_slice(body).map_err(|err| format!("mux.json: {err}"))?;
    if !doc.is_object() {
        return Err("mux.json is not an object".to_owned());
    }
    Ok(Mux {
        ensemble: Ensemble {
            label: text(&doc["ensemble"]["label"]),
            eid: hex_upper(&doc["ensemble"]["id"]),
            ecc: hex_upper(&doc["ensemble"]["ecc"]),
        },
        services: doc["services"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(service)
            .collect(),
        snr: doc["demodulator"]["snr"].as_f64().unwrap_or(0.0),
        fic_crc_errors: doc["demodulator"]["fic"]["numcrcerrors"]
            .as_u64()
            .unwrap_or(0),
        freq_correction_hz: doc["demodulator"]["frequencycorrection"]
            .as_f64()
            .unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCKED: &[u8] = include_bytes!("../../tests/dab/mux-locked.json");
    const EMPTY: &[u8] = include_bytes!("../../tests/dab/mux-nosignal.json");

    #[test]
    fn a_locked_ensemble_with_a_playing_service_parses() {
        let mux = parse(LOCKED).expect("parse");
        assert_eq!(mux.ensemble.label, "DR Deutschland");
        assert_eq!(
            (mux.ensemble.eid.as_str(), mux.ensemble.ecc.as_str()),
            ("D210", "E0")
        );
        assert!(mux.ensemble.known());
        assert_eq!(mux.services.len(), 3);
        assert!((mux.snr - 14.5).abs() < 1e-9);
        assert_eq!(mux.fic_crc_errors, 2);

        let first = &mux.services[0];
        assert_eq!(first.sid, 0xD210);
        assert_eq!(first.sid_hex, "D210");
        assert_eq!(first.label, "Deutschlandfunk");
        assert_eq!(first.pty, "Information");
        assert_eq!(first.codec, Codec::DabPlus);
        assert_eq!(first.bitrate, Some(72));
        assert_eq!(first.protection, "EEP 3-A");
        assert_eq!(
            first.dls.as_deref(),
            Some("Nachrichten: Bundestag debattiert")
        );
        assert_eq!(first.mot_changed, 1_790_000_000);
        assert_eq!(first.audio_errors, 3);
        assert!(first.has_stream);

        // A classic DAB service and a data service.
        assert_eq!(mux.services[1].codec, Codec::Dab);
        assert_eq!(mux.services[1].bitrate, Some(128));
        assert_eq!(mux.services[1].dls, None, "not decoded, no DLS");
        assert_eq!(mux.services[2].codec, Codec::Data);
        assert!(!mux.services[2].has_stream);
        assert_eq!(mux.services[2].sid_hex, "E0D2FF01");
    }

    #[test]
    fn a_receiver_without_sync_is_empty_not_an_error() {
        let mux = parse(EMPTY).expect("parse");
        assert!(!mux.ensemble.known());
        assert!(mux.services.is_empty());
        assert_eq!(mux.snr, 0.0);
    }

    #[test]
    fn garbage_is_an_error_and_odd_shapes_are_tolerated() {
        assert!(parse(b"nope").is_err());
        assert!(parse(b"[1]").is_err());
        let odd = parse(br#"{"ensemble":{"label":"Plain ","id":"0xd218"},"services":[{"sid":"zz"},{"sid":"D218","label":" Radio "}]}"#)
            .expect("parse");
        assert_eq!(odd.ensemble.label, "Plain");
        assert_eq!(odd.ensemble.eid, "D218");
        assert_eq!(odd.services.len(), 1, "the unparsable sid is skipped");
        assert_eq!(odd.services[0].label, "Radio");
        assert_eq!(odd.services[0].codec, Codec::Data);
    }
}
