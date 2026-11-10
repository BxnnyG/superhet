//! ADS-B range statistics (ADR-0025): the plugin's `adsb.range` events are not decoder history;
//! they are turned into maxima per bearing sector and altitude band plus one activity row per
//! minute (`superhet_db::adsb`). The plugin event consumer hands them over before it would store
//! them in `decoder_events`.

use serde_json::Value;
use superhet_db::adsb::{Activity, BANDS, RangeCell, RangeUpdate, SECTORS};

/// Plugin whose range events are handled here.
const PLUGIN_ID: &str = "org.superhet.decoder.adsb";
/// Event type carrying the numbers of one minute.
const KIND: &str = "adsb.range";

/// An `adsb.range` event ready to store.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub device: String,
    pub update: RangeUpdate,
}

/// Whether the JSON of a stream message is an ADS-B range event (valid or not). Such a message is
/// never a history event.
#[must_use]
pub fn is_range_event(payload: &[u8]) -> bool {
    let Ok(doc) = serde_json::from_slice::<Value>(payload) else {
        return false;
    };
    doc.get("plugin_id").and_then(Value::as_str) == Some(PLUGIN_ID)
        && doc.get("type").and_then(Value::as_str) == Some(KIND)
}

/// Read a range event. `None` when it is not one or its content is unusable (the caller drops
/// it: retrying would not help).
#[must_use]
pub fn parse(payload: &[u8]) -> Option<Parsed> {
    let doc: Value = serde_json::from_slice(payload).ok()?;
    if doc.get("plugin_id")?.as_str()? != PLUGIN_ID || doc.get("type")?.as_str()? != KIND {
        return None;
    }
    let device = doc
        .get("device")?
        .as_str()
        .filter(|d| !d.is_empty() && d.len() <= 64)?;
    let data = doc.get("data")?.as_object()?;
    let minute_ms = i64::try_from(data.get("minute_ms")?.as_u64()?).ok()?;
    let receiver = match (
        data.get("lat").and_then(Value::as_f64),
        data.get("lon").and_then(Value::as_f64),
    ) {
        (Some(lat), Some(lon)) if lat.abs() <= 90.0 && lon.abs() <= 180.0 => Some((lat, lon)),
        _ => None,
    };
    let count = |key: &str| {
        data.get(key)
            .and_then(Value::as_u64)
            .map_or(0, |n| i16::try_from(n).unwrap_or(i16::MAX))
    };
    let mut cells = Vec::new();
    for entry in data.get("sectors")?.as_array()? {
        let entry = entry.as_array()?;
        let sector = i16::try_from(entry.first()?.as_i64()?).ok()?;
        let band = i16::try_from(entry.get(1)?.as_i64()?).ok()?;
        let km = entry.get(2)?.as_f64()?;
        if !(0..SECTORS).contains(&sector)
            || !(0..BANDS).contains(&band)
            || !km.is_finite()
            || km < 0.0
        {
            return None;
        }
        let icao = entry
            .get(3)
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(8)
            .collect();
        cells.push(RangeCell {
            sector,
            band,
            max_km: km as f32,
            icao,
        });
    }
    let rate = data
        .get("msgs_per_s")
        .and_then(Value::as_f64)
        .filter(|r| r.is_finite() && *r >= 0.0)
        .unwrap_or(0.0);
    Some(Parsed {
        device: device.to_owned(),
        update: RangeUpdate {
            ts_ms: minute_ms,
            receiver,
            cells,
            activity: Activity {
                msgs_per_s: rate as f32,
                aircraft: count("aircraft"),
                with_position: count("with_position"),
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(data: Value) -> Vec<u8> {
        json!({
            "v": 1, "plugin_id": PLUGIN_ID, "instance_id": "i-1", "device": "dev0",
            "type": KIND, "host_ts_ms": 1_790_000_061_000u64, "data": data,
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn reads_a_range_event() {
        let payload = event(json!({
            "summary": "range", "minute_ms": 1_790_000_040_000u64, "lat": 51.0, "lon": 7.0,
            "msgs_per_s": 123.4, "aircraft": 14, "with_position": 9,
            "sectors": [[3, 2, 120.5, "3c6444"], [71, 5, 300.0, ""]],
        }));
        assert!(is_range_event(&payload));
        let parsed = parse(&payload).expect("parsed");
        assert_eq!(parsed.device, "dev0");
        assert_eq!(parsed.update.ts_ms, 1_790_000_040_000);
        assert_eq!(parsed.update.receiver, Some((51.0, 7.0)));
        assert_eq!(parsed.update.cells.len(), 2);
        assert_eq!(parsed.update.cells[0].icao, "3c6444");
        assert!((parsed.update.cells[1].max_km - 300.0).abs() < f32::EPSILON);
        assert_eq!(parsed.update.activity.aircraft, 14);
        assert_eq!(parsed.update.activity.with_position, 9);
    }

    #[test]
    fn works_without_a_position_and_refuses_bad_cells() {
        let ok = event(
            json!({"minute_ms": 60_000, "msgs_per_s": 1.0, "aircraft": 1,
            "with_position": 0, "sectors": []}),
        );
        let parsed = parse(&ok).expect("parsed");
        assert!(parsed.update.receiver.is_none() && parsed.update.cells.is_empty());
        for sectors in [
            json!([[72, 0, 1.0, ""]]),
            json!([[0, 6, 1.0, ""]]),
            json!([[0, 0, -1.0, ""]]),
            json!([[0, 0]]),
            json!("no"),
        ] {
            let bad = event(
                json!({"minute_ms": 60_000, "msgs_per_s": 1.0, "aircraft": 1,
                "with_position": 0, "sectors": sectors}),
            );
            assert!(is_range_event(&bad));
            assert!(parse(&bad).is_none(), "{sectors}");
        }
    }

    #[test]
    fn other_events_are_not_range_events() {
        let other = json!({"plugin_id": PLUGIN_ID, "type": "adsb.aircraft", "device": "dev0",
            "host_ts_ms": 1, "data": {"summary": "x", "icao": "3c6444"}})
        .to_string()
        .into_bytes();
        assert!(!is_range_event(&other));
        assert!(parse(&other).is_none());
        let foreign = json!({"plugin_id": "org.example.x", "type": KIND, "device": "dev0",
            "host_ts_ms": 1, "data": {"minute_ms": 1}})
        .to_string()
        .into_bytes();
        assert!(!is_range_event(&foreign));
        assert!(!is_range_event(b"not json"));
    }
}
