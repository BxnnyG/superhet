//! Turning successive `aircraft.json` snapshots into history events and the status document.
//!
//! The status document carries the complete live picture (every aircraft, refreshed about once a
//! second) for the map; it is not stored. Events go into the decoder history, which keeps every
//! event for the retention period, so they are rationed: one when an aircraft first shows an
//! identity or a position, one when a callsign or a position first becomes known later, and then
//! one per aircraft per `interval` while it keeps sending.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::aircraft::{Aircraft, Snapshot};

/// Aircraft not heard for this long leave the live picture (the map fades them out before).
pub const STALE_AFTER_S: f64 = 60.0;
/// Aircraft listed in the status document at most (the freshest ones; the limit is 256 KiB).
pub const MAX_LISTED: usize = 400;
/// Events one snapshot may produce at most (a burst after a restart must not flood the bus).
const MAX_EVENTS_PER_UPDATE: usize = 100;

struct Entry {
    aircraft: Aircraft,
    announced: bool,
    named: bool,
    located: bool,
    last_event: Instant,
    messages_at_event: u64,
}

/// What the plugin knows about the airspace.
pub struct Tracker {
    interval: Duration,
    entries: HashMap<String, Entry>,
    messages: u64,
    messages_per_s: Option<f64>,
    last_count: Option<(u64, f64)>,
}

impl Tracker {
    /// `interval`: shortest time between two history events of one aircraft.
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            entries: HashMap::new(),
            messages: 0,
            messages_per_s: None,
            last_count: None,
        }
    }

    /// Take a new snapshot; returns the data of the `adsb.aircraft` events to send.
    pub fn update(&mut self, snap: &Snapshot, at: Instant) -> Vec<Map<String, Value>> {
        self.messages = snap.messages;
        self.messages_per_s = match self.last_count {
            Some((count, now)) if snap.now > now && snap.messages >= count => {
                Some((snap.messages - count) as f64 / (snap.now - now))
            }
            _ => self.messages_per_s,
        };
        self.last_count = Some((snap.messages, snap.now));

        let live: Vec<&Aircraft> = snap
            .aircraft
            .iter()
            .filter(|a| a.seen_s <= STALE_AFTER_S)
            .collect();
        self.entries
            .retain(|icao, _| live.iter().any(|a| &a.icao == icao));

        let mut events = Vec::new();
        for aircraft in live {
            let named = aircraft.callsign.is_some();
            let located = aircraft.has_position();
            let entry = self
                .entries
                .entry(aircraft.icao.clone())
                .or_insert_with(|| Entry {
                    aircraft: aircraft.clone(),
                    announced: false,
                    named: false,
                    located: false,
                    last_event: at,
                    messages_at_event: 0,
                });
            entry.aircraft = aircraft.clone();
            let due = if !entry.announced {
                named || located
            } else {
                (named && !entry.named)
                    || (located && !entry.located)
                    || (at.duration_since(entry.last_event) >= self.interval
                        && aircraft.messages > entry.messages_at_event)
            };
            if due && events.len() < MAX_EVENTS_PER_UPDATE {
                entry.announced = true;
                entry.named |= named;
                entry.located |= located;
                entry.last_event = at;
                entry.messages_at_event = aircraft.messages;
                events.push(event_of(aircraft));
            }
        }
        events
    }

    /// Aircraft in the live picture.
    #[must_use]
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// The complete state for the host (`status` frame). `mode` is `starting` until readsb wrote
    /// its first file, then `running`; `error` names why the decoder stopped.
    #[must_use]
    pub fn status(
        &self,
        mode: &str,
        receiver: Option<(f64, f64)>,
        now_ms: u64,
        error: Option<&str>,
    ) -> Value {
        let mut list: Vec<&Aircraft> = self.entries.values().map(|e| &e.aircraft).collect();
        list.sort_by(|a, b| {
            a.seen_s
                .total_cmp(&b.seen_s)
                .then_with(|| a.icao.cmp(&b.icao))
        });
        let with_position = list.iter().filter(|a| a.has_position()).count();
        let total = list.len();
        let aircraft: Vec<Value> = list
            .into_iter()
            .take(MAX_LISTED)
            .map(|a| {
                let mut fields = a.fields();
                fields.insert("seen_s".into(), json!(round1(a.seen_s)));
                if let Some(seen) = a.seen_pos_s {
                    fields.insert("seen_pos_s".into(), json!(round1(seen)));
                }
                fields.insert("messages".into(), json!(a.messages));
                Value::Object(fields)
            })
            .collect();
        json!({
            "v": 1,
            "mode": mode,
            "updated_ms": now_ms,
            "stale_after_s": STALE_AFTER_S,
            "receiver": receiver.map(|(lat, lon)| json!({"lat": lat, "lon": lon})),
            "messages": self.messages,
            "messages_per_s": self.messages_per_s.map(round1),
            "total": total,
            "with_position": with_position,
            "truncated": total > MAX_LISTED,
            "error": error,
            "aircraft": aircraft,
        })
    }
}

fn event_of(aircraft: &Aircraft) -> Map<String, Value> {
    let mut data = aircraft.fields();
    data.insert("summary".into(), json!(aircraft.summary()));
    data
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::super::aircraft::parse_snapshot;
    use super::*;

    fn snap(text: &str) -> Snapshot {
        parse_snapshot(text).expect("fixture")
    }

    const ONE: &str = include_str!("../../tests/adsb/aircraft-1.json");
    const TWO: &str = include_str!("../../tests/adsb/aircraft-2.json");

    fn icaos(events: &[Map<String, Value>]) -> Vec<String> {
        events
            .iter()
            .map(|e| e["icao"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn announces_aircraft_with_an_identity_or_a_position_once() {
        let mut tracker = Tracker::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let first = tracker.update(&snap(ONE), t0);
        // 4ca7b2 and ~a0b1c2 have neither callsign nor position yet.
        assert_eq!(icaos(&first), ["3c6444", "406a3d"].map(String::from));
        assert_eq!(first[0]["summary"], "DLH123 35000 ft 451 kt 123\u{b0}");
        // The same snapshot again a second later: nothing new.
        assert!(
            tracker
                .update(&snap(ONE), t0 + Duration::from_secs(1))
                .is_empty()
        );
    }

    #[test]
    fn a_new_callsign_or_position_is_reported_at_once_the_rest_per_interval() {
        let mut tracker = Tracker::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let _ = tracker.update(&snap(ONE), t0);
        // 4ca7b2 now has a callsign and a position; the others only moved.
        let second = tracker.update(&snap(TWO), t0 + Duration::from_secs(1));
        assert_eq!(icaos(&second), ["4ca7b2"]);
        assert_eq!(second[0]["callsign"], "RYR8QK");
        // After the interval a moving aircraft is reported again.
        let later = tracker.update(&snap(TWO), t0 + Duration::from_secs(62));
        assert_eq!(icaos(&later), ["3c6444"]);
    }

    #[test]
    fn aircraft_leave_the_picture_when_gone_or_stale() {
        let mut tracker = Tracker::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let _ = tracker.update(&snap(ONE), t0);
        assert_eq!(tracker.count(), 4);
        let _ = tracker.update(&snap(TWO), t0 + Duration::from_secs(1));
        assert_eq!(tracker.count(), 2);
        let stale = snap(
            r#"{"now":9,"messages":1,"aircraft":[{"hex":"3c6444","seen":75.0,"messages":2}]}"#,
        );
        let _ = tracker.update(&stale, t0 + Duration::from_secs(2));
        assert_eq!(tracker.count(), 0);
    }

    #[test]
    fn status_lists_the_freshest_first_with_counts_and_rate() {
        let mut tracker = Tracker::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let _ = tracker.update(&snap(ONE), t0);
        let _ = tracker.update(&snap(TWO), t0 + Duration::from_secs(1));
        let status = tracker.status("running", Some((51.0, 7.0)), 1_000, None);
        assert_eq!(status["total"], 2);
        assert_eq!(status["with_position"], 2);
        assert_eq!(status["receiver"]["lat"], 51.0);
        assert_eq!(status["messages"], 5390);
        // (5390 - 4821) messages in 1.007 s of readsb time.
        assert_eq!(status["messages_per_s"], 565.0);
        let list = status["aircraft"].as_array().expect("list");
        assert_eq!(list[0]["icao"], "3c6444");
        assert_eq!(list[0]["seen_s"], 0.1);
        assert_eq!(list[1]["callsign"], "RYR8QK");
        assert!(status["error"].is_null());
    }

    #[test]
    fn a_burst_is_capped() {
        let mut tracker = Tracker::new(Duration::from_secs(60));
        let many: Vec<String> = (0..300)
            .map(|i| format!(r#"{{"hex":"{i:06x}","flight":"AB{i}","messages":5,"seen":1.0}}"#))
            .collect();
        let text = format!(
            r#"{{"now":1,"messages":9,"aircraft":[{}]}}"#,
            many.join(",")
        );
        let events = tracker.update(&snap(&text), Instant::now());
        assert_eq!(events.len(), MAX_EVENTS_PER_UPDATE);
        // The rest follows on the next snapshots.
        let next = tracker.update(&snap(&text), Instant::now());
        assert_eq!(next.len(), MAX_EVENTS_PER_UPDATE);
    }
}
