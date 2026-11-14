//! The vessel memory behind the AIS plugin: merges what the messages of one MMSI tell (position
//! reports and static data arrive separately and minutes apart), and decides which of them become
//! `ais.vessel` events.
//!
//! A busy harbour sends thousands of position reports a minute, and every event is stored in the
//! decoder history, so events are rationed like the ADS-B plugin's: one the first time a vessel is
//! heard, one when it first gets a position, one when its static data (name, type, destination)
//! arrives or changes, and then at most one per vessel per `event_interval` while it keeps
//! sending positions. Every event carries everything known about the vessel (merged), so the map
//! can be rebuilt from the newest event per MMSI.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::payload::{
    Class, Fragments, Message, aton_type_text, decode, parse_sentence, ship_type_text, status_text,
};
use crate::event::{event, truncate};

/// Vessels remembered at most; the least recently heard go first.
const MAX_VESSELS: usize = 8192;
/// A vessel not heard for this long is forgotten when the memory is full.
const FORGET_AFTER: Duration = Duration::from_secs(3600);

/// Everything known about one vessel.
#[derive(Debug, Default, Clone, PartialEq)]
struct Record {
    class: Class,
    status: Option<u8>,
    lat: Option<f64>,
    lon: Option<f64>,
    sog_kn: Option<f64>,
    cog_deg: Option<f64>,
    heading_deg: Option<u16>,
    altitude_m: Option<u16>,
    name: Option<String>,
    callsign: Option<String>,
    imo: Option<u32>,
    ship_type: Option<u8>,
    destination: Option<String>,
    eta: Option<String>,
    draught_m: Option<f64>,
    length_m: Option<u16>,
    beam_m: Option<u16>,
}

struct Vessel {
    record: Record,
    last_seen: Instant,
    last_emit: Option<Instant>,
}

/// What a message changed that is worth an event of its own.
struct Delta {
    new_position: bool,
    new_static: bool,
}

impl Record {
    fn merge(&mut self, m: &Message) -> Delta {
        let had_position = self.lat.is_some();
        self.class = m.class;
        let mut new_static = false;
        let mut set_text = |slot: &mut Option<String>, value: &Option<String>| {
            if value.is_some() && slot != value {
                new_static = true;
                slot.clone_from(value);
            }
        };
        set_text(&mut self.name, &m.name);
        set_text(&mut self.callsign, &m.callsign);
        set_text(&mut self.destination, &m.destination);
        set_text(&mut self.eta, &m.eta);
        if m.ship_type.is_some() && self.ship_type != m.ship_type {
            new_static = true;
            self.ship_type = m.ship_type;
        }
        if let (Some(lat), Some(lon)) = (m.lat, m.lon) {
            // 0/0 is what transponders without a fix send; nobody sails there.
            if lat != 0.0 || lon != 0.0 {
                self.lat = Some(lat);
                self.lon = Some(lon);
            }
        }
        if m.status.is_some() {
            self.status = m.status;
        }
        if m.sog_kn.is_some() || m.lat.is_some() {
            self.sog_kn = m.sog_kn;
            self.cog_deg = m.cog_deg;
            self.heading_deg = m.heading_deg;
        }
        if m.altitude_m.is_some() {
            self.altitude_m = m.altitude_m;
        }
        if m.imo.is_some() {
            self.imo = m.imo;
        }
        if m.draught_m.is_some() {
            self.draught_m = m.draught_m;
        }
        if m.length_m.is_some() {
            self.length_m = m.length_m;
            self.beam_m = m.beam_m;
        }
        Delta {
            new_position: !had_position && self.lat.is_some(),
            new_static,
        }
    }

    fn data(&self, mmsi: u32) -> Map<String, Value> {
        let mut d = Map::new();
        d.insert("mmsi".into(), json!(mmsi));
        d.insert("class".into(), json!(self.class.code()));
        let mut put = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                d.insert(key.into(), value);
            }
        };
        put("name", self.name.as_ref().map(|v| json!(v)));
        put("callsign", self.callsign.as_ref().map(|v| json!(v)));
        put("imo", self.imo.map(|v| json!(v)));
        if self.class == Class::Aton {
            put("aton_type", self.ship_type.map(|v| json!(v)));
            put(
                "aton_type_text",
                self.ship_type.map(|v| json!(aton_type_text(v))),
            );
        } else {
            put("ship_type", self.ship_type.map(|v| json!(v)));
            put(
                "ship_type_text",
                self.ship_type.map(|v| json!(ship_type_text(v))),
            );
        }
        put("status", self.status.map(|v| json!(v)));
        put("status_text", self.status.map(|v| json!(status_text(v))));
        put("lat", self.lat.map(|v| json!(v)));
        put("lon", self.lon.map(|v| json!(v)));
        put("sog_kn", self.sog_kn.map(|v| json!(v)));
        put(
            "sog_kmh",
            self.sog_kn
                .map(|v| json!((v * 1.852 * 10.0).round() / 10.0)),
        );
        put("cog_deg", self.cog_deg.map(|v| json!(v)));
        put("heading_deg", self.heading_deg.map(|v| json!(v)));
        put("altitude_m", self.altitude_m.map(|v| json!(v)));
        put("destination", self.destination.as_ref().map(|v| json!(v)));
        put("eta", self.eta.as_ref().map(|v| json!(v)));
        put("draught_m", self.draught_m.map(|v| json!(v)));
        put("length_m", self.length_m.map(|v| json!(v)));
        put("beam_m", self.beam_m.map(|v| json!(v)));
        d.insert("summary".into(), json!(truncate(&self.summary(mmsi), 256)));
        d
    }

    fn summary(&self, mmsi: u32) -> String {
        let mut out = match &self.name {
            Some(name) => format!("{name} ({mmsi})"),
            None => format!("MMSI {mmsi}"),
        };
        if let Some(sog) = self.sog_kn {
            out.push_str(&format!(" {sog:.1} kn"));
        }
        if let Some(cog) = self.cog_deg {
            out.push_str(&format!(" {cog:.0}°"));
        }
        out
    }
}

/// The vessels heard so far and the event rationing.
pub struct Tracker {
    interval: Duration,
    fragments: Fragments,
    vessels: HashMap<u32, Vessel>,
}

impl Tracker {
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            fragments: Fragments::default(),
            vessels: HashMap::new(),
        }
    }

    /// Vessels in memory.
    #[must_use]
    pub fn len(&self) -> usize {
        self.vessels.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vessels.is_empty()
    }

    /// One output line of AIS-catcher at the current time.
    pub fn line(&mut self, line: &str) -> Option<Value> {
        self.line_at(line, Instant::now())
    }

    /// One output line of AIS-catcher; the event it makes due, if any.
    pub fn line_at(&mut self, line: &str, now: Instant) -> Option<Value> {
        let sentence = parse_sentence(line)?;
        let (payload, fill) = self.fragments.push(sentence)?;
        let message = decode(&payload, fill)?;
        let vessel = self.vessels.entry(message.mmsi).or_insert_with(|| Vessel {
            record: Record::default(),
            last_seen: now,
            last_emit: None,
        });
        vessel.last_seen = now;
        let delta = vessel.record.merge(&message);
        let due = match vessel.last_emit {
            None => true,
            Some(at) => {
                delta.new_position
                    || delta.new_static
                    || (message.lat.is_some() && now.duration_since(at) >= self.interval)
            }
        };
        if !due {
            return None;
        }
        vessel.last_emit = Some(now);
        let data = vessel.record.data(message.mmsi);
        self.trim(now);
        Some(event("ais.vessel", data))
    }

    fn trim(&mut self, now: Instant) {
        if self.vessels.len() <= MAX_VESSELS {
            return;
        }
        self.vessels
            .retain(|_, v| now.duration_since(v.last_seen) < FORGET_AFTER);
        while self.vessels.len() > MAX_VESSELS {
            let Some(oldest) = self
                .vessels
                .iter()
                .min_by_key(|(_, v)| v.last_seen)
                .map(|(mmsi, _)| *mmsi)
            else {
                return;
            };
            self.vessels.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::payload::testing::{position_report, static_report};
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(60);

    fn feed(t: &mut Tracker, line: &str, now: Instant) -> Option<Value> {
        t.line_at(line, now)
    }

    #[test]
    fn rations_position_events_per_vessel() {
        let mut t = Tracker::new(INTERVAL);
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let r1 = position_report(244_670_316, 52.0, 4.0, 100, 900);
        let first = feed(&mut t, &r1, at(0)).expect("first report");
        assert_eq!(first["type"], "ais.vessel");
        assert_eq!(first["data"]["mmsi"], 244_670_316);
        assert_eq!(first["data"]["sog_kn"], 10.0);
        assert_eq!(first["data"]["sog_kmh"], 18.5);
        assert!(feed(&mut t, &r1, at(10)).is_none());
        assert!(feed(&mut t, &r1, at(59)).is_none());
        assert!(feed(&mut t, &r1, at(61)).is_some());
        // Another vessel is rationed on its own.
        let other = position_report(211_000_001, 53.0, 5.0, 0, 0);
        assert!(feed(&mut t, &other, at(62)).is_some());
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn static_data_is_merged_into_the_next_events() {
        let mut t = Tracker::new(INTERVAL);
        let t0 = Instant::now();
        let report = position_report(244_670_316, 52.0, 4.0, 123, 2704);
        assert!(feed(&mut t, &report, t0).is_some());
        let lines = static_report(244_670_316, "EVER GIVEN", "NLRTM");
        assert!(feed(&mut t, &lines[0], t0).is_none());
        let named = feed(&mut t, &lines[1], t0).expect("name is news");
        let d = &named["data"];
        assert_eq!(d["name"], "EVER GIVEN");
        assert_eq!(d["ship_type_text"], "Cargo");
        assert_eq!(d["destination"], "NLRTM");
        assert_eq!(d["length_m"], 400);
        // The position of the earlier report is part of it.
        assert_eq!(d["lat"], 52.0);
        assert_eq!(d["summary"], "EVER GIVEN (244670316) 12.3 kn 270°");
        // The same static data again is no news.
        assert!(feed(&mut t, &lines[0], t0).is_none());
        assert!(feed(&mut t, &lines[1], t0).is_none());
    }

    #[test]
    fn a_vessel_first_heard_by_static_data_announces_its_first_position() {
        let mut t = Tracker::new(INTERVAL);
        let t0 = Instant::now();
        let lines = static_report(244_670_316, "NO FIX YET", "ROTTERDAM");
        assert!(feed(&mut t, &lines[0], t0).is_none());
        let e = feed(&mut t, &lines[1], t0).expect("first sight");
        assert!(e["data"].get("lat").is_none());
        let report = position_report(244_670_316, 51.9, 4.1, 5, 10);
        let e = feed(&mut t, &report, t0 + Duration::from_secs(1)).expect("first position");
        assert_eq!(e["data"]["lat"], 51.9);
        assert_eq!(e["data"]["name"], "NO FIX YET");
    }

    #[test]
    fn null_island_is_not_a_position() {
        let mut t = Tracker::new(INTERVAL);
        let e = feed(
            &mut t,
            &position_report(244_000_002, 0.0, 0.0, 0, 0),
            Instant::now(),
        )
        .expect("heard");
        assert!(e["data"].get("lat").is_none());
    }

    #[test]
    fn garbage_lines_make_no_events() {
        let mut t = Tracker::new(INTERVAL);
        let now = Instant::now();
        assert!(feed(&mut t, "AIS-catcher (build 1)", now).is_none());
        assert!(feed(&mut t, "!AIVDM,1,1,,A,zz,0*00", now).is_none());
        assert!(t.is_empty());
    }

    #[test]
    fn memory_is_capped() {
        let mut t = Tracker::new(INTERVAL);
        let t0 = Instant::now();
        let cap = u32::try_from(MAX_VESSELS).expect("small");
        for n in 0..cap + 5 {
            let line = position_report(200_000_000 + n, 50.0, 8.0, 0, 0);
            feed(&mut t, &line, t0 + Duration::from_millis(u64::from(n)));
        }
        assert!(t.len() <= MAX_VESSELS);
        // The oldest were dropped.
        assert!(!t.vessels.contains_key(&200_000_000));
    }
}
