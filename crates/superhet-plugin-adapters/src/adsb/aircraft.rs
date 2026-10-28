//! Reading readsb's `aircraft.json` (`--write-json`): one snapshot of every aircraft the decoder
//! currently tracks. Fields readsb leaves out (nothing heard yet) stay `None`; entries without a
//! valid ICAO address are skipped.

use serde_json::{Map, Value, json};

use crate::event::round5;

/// One tracked aircraft.
#[derive(Debug, Clone, PartialEq)]
pub struct Aircraft {
    /// 24-bit ICAO address, six lowercase hex digits (`~` prefix: non-ICAO address, e.g. TIS-B).
    pub icao: String,
    /// Flight id / callsign, trimmed.
    pub callsign: Option<String>,
    /// Barometric altitude in feet (`None` on the ground or unknown).
    pub alt_ft: Option<i64>,
    /// Reported as on the ground.
    pub on_ground: bool,
    /// Ground speed in knots.
    pub speed_kt: Option<f64>,
    /// Ground track in degrees (0..360).
    pub track_deg: Option<f64>,
    /// Barometric climb rate in feet per minute.
    pub vrate_fpm: Option<i64>,
    /// Position, degrees (both or neither).
    pub lat: Option<f64>,
    /// See `lat`.
    pub lon: Option<f64>,
    /// Mean signal level in dBFS (negative).
    pub rssi: Option<f64>,
    /// Mode A squawk (four octal digits).
    pub squawk: Option<String>,
    /// Emitter category (`A3`, `B2`, ...).
    pub category: Option<String>,
    /// Messages received from this aircraft.
    pub messages: u64,
    /// Seconds since the last message of any kind.
    pub seen_s: f64,
    /// Seconds since the last position.
    pub seen_pos_s: Option<f64>,
}

/// A parsed `aircraft.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// readsb's clock when it wrote the file (Unix seconds).
    pub now: f64,
    /// Messages decoded since readsb started.
    pub messages: u64,
    pub aircraft: Vec<Aircraft>,
}

/// Parse the text of an `aircraft.json`.
///
/// # Errors
/// The text is not JSON or has no `aircraft` array.
pub fn parse_snapshot(text: &str) -> Result<Snapshot, String> {
    let doc: Value = serde_json::from_str(text).map_err(|err| format!("aircraft.json: {err}"))?;
    let list = doc["aircraft"]
        .as_array()
        .ok_or("aircraft.json has no aircraft array")?;
    Ok(Snapshot {
        now: doc["now"].as_f64().unwrap_or(0.0),
        messages: doc["messages"].as_u64().unwrap_or(0),
        aircraft: list.iter().filter_map(parse_aircraft).collect(),
    })
}

/// `hex` as a normalised ICAO address, if it is one.
fn icao_of(hex: &str) -> Option<String> {
    let (prefix, digits) = match hex.strip_prefix('~') {
        Some(rest) => ("~", rest),
        None => ("", hex),
    };
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("{prefix}{}", digits.to_ascii_lowercase()))
}

fn text_of(value: &Value, max: usize) -> Option<String> {
    let clean: String = value
        .as_str()?
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect();
    (!clean.is_empty()).then_some(clean)
}

fn number(value: &Value, lo: f64, hi: f64) -> Option<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && (lo..=hi).contains(v))
}

#[allow(clippy::cast_possible_truncation)]
fn rounded(value: &Value, lo: f64, hi: f64) -> Option<i64> {
    number(value, lo, hi).map(|v| v.round() as i64)
}

fn parse_aircraft(entry: &Value) -> Option<Aircraft> {
    let icao = icao_of(entry["hex"].as_str()?)?;
    // Both coordinates or none: half a position is of no use on a map.
    let (lat, lon) = match (
        number(&entry["lat"], -90.0, 90.0),
        number(&entry["lon"], -180.0, 180.0),
    ) {
        (Some(lat), Some(lon)) => (Some(round5(lat)), Some(round5(lon))),
        _ => (None, None),
    };
    Some(Aircraft {
        icao,
        callsign: text_of(&entry["flight"], 8),
        alt_ft: rounded(&entry["alt_baro"], -2_000.0, 100_000.0),
        on_ground: entry["alt_baro"].as_str() == Some("ground"),
        speed_kt: number(&entry["gs"], 0.0, 2_000.0),
        track_deg: number(&entry["track"], 0.0, 360.0),
        vrate_fpm: rounded(&entry["baro_rate"], -20_000.0, 20_000.0),
        lat,
        lon,
        rssi: number(&entry["rssi"], -100.0, 10.0),
        squawk: text_of(&entry["squawk"], 4),
        category: text_of(&entry["category"], 2),
        messages: entry["messages"].as_u64().unwrap_or(0),
        seen_s: number(&entry["seen"], 0.0, 1.0e6).unwrap_or(0.0),
        seen_pos_s: number(&entry["seen_pos"], 0.0, 1.0e6),
    })
}

impl Aircraft {
    #[must_use]
    pub fn has_position(&self) -> bool {
        self.lat.is_some()
    }

    /// The fields shared by events and the status document (absent means unknown).
    #[must_use]
    pub fn fields(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("icao".into(), json!(self.icao));
        let mut put = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                m.insert(key.into(), value);
            }
        };
        put("callsign", self.callsign.as_ref().map(|v| json!(v)));
        put("alt_ft", self.alt_ft.map(|v| json!(v)));
        put("on_ground", self.on_ground.then_some(json!(true)));
        put("speed_kt", self.speed_kt.map(|v| json!(round1(v))));
        put("track_deg", self.track_deg.map(|v| json!(round1(v))));
        put("vrate_fpm", self.vrate_fpm.map(|v| json!(v)));
        put("lat", self.lat.map(|v| json!(v)));
        put("lon", self.lon.map(|v| json!(v)));
        put("rssi", self.rssi.map(|v| json!(round1(v))));
        put("squawk", self.squawk.as_ref().map(|v| json!(v)));
        put("category", self.category.as_ref().map(|v| json!(v)));
        m
    }

    /// One line for the decoder history, e.g. `DLH123 35000 ft 451 kt 123°` (callsign, else ICAO).
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = vec![self.callsign.clone().unwrap_or_else(|| self.icao.clone())];
        if self.on_ground {
            parts.push("ground".into());
        } else if let Some(alt) = self.alt_ft {
            parts.push(format!("{alt} ft"));
        }
        if let Some(speed) = self.speed_kt {
            parts.push(format!("{speed:.0} kt"));
        }
        if let Some(track) = self.track_deg {
            parts.push(format!("{track:.0}\u{b0}"));
        }
        parts.join(" ")
    }
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../../tests/adsb/aircraft-1.json");

    #[test]
    fn parses_the_fields_readsb_writes() {
        let snap = parse_snapshot(SAMPLE).expect("parse");
        assert_eq!(snap.messages, 4821);
        // The entry with a malformed address is skipped.
        assert_eq!(snap.aircraft.len(), 4);
        let dlh = &snap.aircraft[0];
        assert_eq!(dlh.icao, "3c6444");
        assert_eq!(dlh.callsign.as_deref(), Some("DLH123"));
        assert_eq!(dlh.alt_ft, Some(35_000));
        assert_eq!(dlh.speed_kt, Some(451.3));
        assert_eq!(dlh.lat, Some(51.12345));
        assert_eq!(dlh.vrate_fpm, Some(-64));
        assert_eq!(dlh.squawk.as_deref(), Some("1000"));
        assert_eq!(dlh.summary(), "DLH123 35000 ft 451 kt 123\u{b0}");
    }

    #[test]
    fn partial_entries_keep_what_is_known() {
        let snap = parse_snapshot(SAMPLE).expect("parse");
        let bare = &snap.aircraft[1];
        assert_eq!(bare.icao, "4ca7b2");
        assert_eq!(bare.callsign, None);
        assert!(!bare.has_position());
        assert_eq!(bare.summary(), "4ca7b2");
        let ground = &snap.aircraft[2];
        assert!(ground.on_ground);
        assert_eq!(ground.alt_ft, None);
        assert_eq!(ground.summary(), "EZY45K ground 12 kt 270\u{b0}");
        // A latitude without a longitude is dropped.
        assert_eq!(snap.aircraft[3].icao, "~a0b1c2");
        assert!(!snap.aircraft[3].has_position());
    }

    #[test]
    fn fields_omit_what_is_unknown() {
        let snap = parse_snapshot(SAMPLE).expect("parse");
        let fields = snap.aircraft[1].fields();
        assert_eq!(fields["icao"], "4ca7b2");
        assert!(!fields.contains_key("callsign") && !fields.contains_key("lat"));
        assert_eq!(fields["rssi"], -21.4);
    }

    #[test]
    fn rejects_documents_without_an_aircraft_list() {
        assert!(parse_snapshot("not json").is_err());
        assert!(parse_snapshot(r#"{"now":1}"#).is_err());
        assert!(
            parse_snapshot(r#"{"now":1,"aircraft":[]}"#)
                .expect("empty")
                .aircraft
                .is_empty()
        );
    }
}
