//! Range statistics of the receiver: the farthest position heard per bearing sector and altitude
//! band, plus the message rate and aircraft counts, reported once a minute as one `adsb.range`
//! event (ADR-0025).
//!
//! The plugin sees every snapshot (about once a second), so it is the only place where the maxima
//! can be built without gaps; the worker stores them. To keep a reset in the database meaningful,
//! an event carries only the maxima seen *in its own minute*; the database keeps the larger
//! value, so a repeated or late event changes nothing and a reset starts from zero again.
//!
//! Positions are only used when the receiver position is configured. A position farther than
//! [`MAX_RANGE_KM`] is dropped as a decoding error (readsb's own limit is 300 NM, about 556 km).

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::aircraft::Snapshot;

/// Width of a bearing sector in degrees.
pub const SECTOR_DEG: f64 = 5.0;
/// Number of bearing sectors (0 = north, clockwise).
pub const SECTORS: usize = 72;
/// Upper edges of the altitude bands in feet; the last band is open-ended (above 40000 ft).
pub const BAND_EDGES_FT: [i64; 5] = [5_000, 10_000, 20_000, 30_000, 40_000];
/// Positions farther away than this are not believed (kilometres).
pub const MAX_RANGE_KM: f64 = 1_000.0;
/// Length of the window one event describes.
const WINDOW_MS: u64 = 60_000;
/// Mean Earth radius in kilometres.
const EARTH_KM: f64 = 6371.0;

/// Altitude band of an aircraft: 0 is the ground and everything below 5000 ft, then 5000-10000,
/// 10000-20000, 20000-30000, 30000-40000 and above 40000 ft (band 5).
#[must_use]
pub fn band_of(alt_ft: Option<i64>, on_ground: bool) -> usize {
    if on_ground {
        return 0;
    }
    let Some(alt) = alt_ft else { return 0 };
    BAND_EDGES_FT
        .iter()
        .position(|edge| alt < *edge)
        .unwrap_or(BAND_EDGES_FT.len())
}

/// Great-circle distance in kilometres.
#[must_use]
pub fn distance_km(from: (f64, f64), to: (f64, f64)) -> f64 {
    let (lat1, lat2) = (from.0.to_radians(), to.0.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (to.1 - from.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_KM * h.sqrt().min(1.0).asin()
}

/// Initial bearing from `from` to `to` in degrees, 0 to 360 (north, clockwise).
#[must_use]
pub fn bearing_deg(from: (f64, f64), to: (f64, f64)) -> f64 {
    let (lat1, lat2) = (from.0.to_radians(), to.0.to_radians());
    let dlon = (to.1 - from.1).to_radians();
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

/// Sector (0..72) a bearing falls into; sector 0 covers 0 to 5 degrees.
#[must_use]
pub fn sector_of(bearing: f64) -> usize {
    let index = (bearing.rem_euclid(360.0) / SECTOR_DEG).floor();
    // `index` is 0..=71; the clamp only guards against 360.0 after rounding.
    (index as usize).min(SECTORS - 1)
}

/// Farthest aircraft of one cell in the current window.
struct Best {
    km: f64,
    icao: String,
}

/// Statistics of the window in progress.
pub struct RangeWindow {
    receiver: Option<(f64, f64)>,
    /// Minute (Unix ms, rounded down) the window describes; `None` before the first snapshot.
    minute_ms: Option<u64>,
    cells: HashMap<(usize, usize), Best>,
    rate_sum: f64,
    rate_n: u32,
    aircraft: usize,
    with_position: usize,
}

impl RangeWindow {
    /// `receiver`: the configured position; without one only the activity numbers are kept.
    #[must_use]
    pub fn new(receiver: Option<(f64, f64)>) -> Self {
        Self {
            receiver,
            minute_ms: None,
            cells: HashMap::new(),
            rate_sum: 0.0,
            rate_n: 0,
            aircraft: 0,
            with_position: 0,
        }
    }

    /// Take a snapshot at `now_ms` (wall clock). `live` are the aircraft of the live picture
    /// (callers pass the tracker's count), `rate` the current message rate. Returns the data of
    /// the `adsb.range` event of the previous minute when a new minute began.
    pub fn observe(
        &mut self,
        snap: &Snapshot,
        now_ms: u64,
        live: usize,
        rate: Option<f64>,
    ) -> Option<Map<String, Value>> {
        let minute = now_ms - now_ms % WINDOW_MS;
        let finished = match self.minute_ms {
            Some(current) if current != minute => self.finish(),
            _ => None,
        };
        self.minute_ms = Some(minute);
        self.aircraft = self.aircraft.max(live);
        if let Some(rate) = rate.filter(|r| r.is_finite() && *r >= 0.0) {
            self.rate_sum += rate;
            self.rate_n += 1;
        }
        let mut with_position = 0;
        for a in &snap.aircraft {
            let (Some(lat), Some(lon)) = (a.lat, a.lon) else {
                continue;
            };
            with_position += 1;
            if a.icao.starts_with('~') {
                // Non-ICAO addresses (TIS-B, ADS-R relays) are not what this receiver heard.
                continue;
            }
            let Some(rx) = self.receiver else { continue };
            let km = distance_km(rx, (lat, lon));
            if !km.is_finite() || km > MAX_RANGE_KM {
                continue;
            }
            let cell = (
                sector_of(bearing_deg(rx, (lat, lon))),
                band_of(a.alt_ft, a.on_ground),
            );
            match self.cells.get_mut(&cell) {
                Some(best) if best.km >= km => {}
                Some(best) => {
                    best.km = km;
                    best.icao.clone_from(&a.icao);
                }
                None => {
                    self.cells.insert(
                        cell,
                        Best {
                            km,
                            icao: a.icao.clone(),
                        },
                    );
                }
            }
        }
        self.with_position = self.with_position.max(with_position);
        finished
    }

    /// Close the window in progress and return its event data (also used when the adapter ends).
    pub fn finish(&mut self) -> Option<Map<String, Value>> {
        let minute = self.minute_ms.take()?;
        let mut cells: Vec<((usize, usize), Best)> = self.cells.drain().collect();
        cells.sort_by_key(|(key, _)| *key);
        let sectors: Vec<Value> = cells
            .into_iter()
            .map(|((sector, band), best)| json!([sector, band, round2(best.km), best.icao]))
            .collect();
        let rate = if self.rate_n == 0 {
            0.0
        } else {
            self.rate_sum / f64::from(self.rate_n)
        };
        let aircraft = std::mem::take(&mut self.aircraft);
        let with_position = std::mem::take(&mut self.with_position);
        self.rate_sum = 0.0;
        self.rate_n = 0;
        let mut data = Map::new();
        data.insert(
            "summary".into(),
            json!(format!(
                "range statistics: {aircraft} aircraft, {rate:.0} msg/s, {} new sector maxima",
                sectors.len()
            )),
        );
        data.insert("minute_ms".into(), json!(minute));
        if let Some((lat, lon)) = self.receiver {
            data.insert("lat".into(), json!(lat));
            data.insert("lon".into(), json!(lon));
        }
        data.insert("msgs_per_s".into(), json!(round1(rate)));
        data.insert("aircraft".into(), json!(aircraft));
        data.insert("with_position".into(), json!(with_position));
        data.insert("sectors".into(), Value::Array(sectors));
        Some(data)
    }
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::super::aircraft::parse_snapshot;
    use super::*;

    const HOME: (f64, f64) = (51.0, 7.0);

    fn snap(aircraft: &str) -> Snapshot {
        parse_snapshot(&format!(
            r#"{{"now":1,"messages":9,"aircraft":[{aircraft}]}}"#
        ))
        .expect("fixture")
    }

    #[test]
    fn bearings_follow_the_compass() {
        let north = bearing_deg(HOME, (52.0, 7.0));
        assert!(north < 0.01 || north > 359.99, "{north}");
        assert!((bearing_deg(HOME, (51.0, 8.0)) - 90.0).abs() < 0.5);
        assert!((bearing_deg(HOME, (50.0, 7.0)) - 180.0).abs() < 0.01);
        assert!((bearing_deg(HOME, (51.0, 6.0)) - 270.0).abs() < 0.5);
        for deg in [0.0, 4.99, 5.0, 359.9, 360.0, -1.0] {
            assert!(sector_of(deg) < SECTORS, "{deg}");
        }
        assert_eq!(sector_of(0.0), 0);
        assert_eq!(sector_of(4.99), 0);
        assert_eq!(sector_of(5.0), 1);
        assert_eq!(sector_of(90.0), 18);
        assert_eq!(sector_of(359.99), 71);
        assert_eq!(sector_of(-1.0), 71);
    }

    #[test]
    fn distance_matches_known_values() {
        // One degree of latitude is about 111.2 km.
        assert!((distance_km(HOME, (52.0, 7.0)) - 111.19).abs() < 0.1);
        assert!(distance_km(HOME, HOME) < 1e-9);
        // Cologne to Berlin, about 477 km.
        assert!((distance_km((50.9375, 6.9603), (52.52, 13.405)) - 477.0).abs() < 5.0);
    }

    #[test]
    fn altitude_bands() {
        assert_eq!(band_of(None, true), 0);
        assert_eq!(band_of(Some(30_000), true), 0);
        assert_eq!(band_of(None, false), 0);
        assert_eq!(band_of(Some(-200), false), 0);
        assert_eq!(band_of(Some(4_999), false), 0);
        assert_eq!(band_of(Some(5_000), false), 1);
        assert_eq!(band_of(Some(19_999), false), 2);
        assert_eq!(band_of(Some(35_000), false), 4);
        assert_eq!(band_of(Some(40_000), false), 5);
        assert_eq!(band_of(Some(51_000), false), 5);
    }

    #[test]
    fn keeps_the_farthest_per_sector_and_band_and_reports_on_the_minute() {
        let mut window = RangeWindow::new(Some(HOME));
        let t0 = 1_790_000_010_000;
        // Two aircraft due north (sector 0) in the same band, one east, one without position.
        let first = snap(
            r#"{"hex":"3c6444","lat":51.5,"lon":7.0,"alt_baro":35000,"messages":9,"seen":1.0},
               {"hex":"3c6445","lat":52.0,"lon":7.0,"alt_baro":36000,"messages":9,"seen":1.0},
               {"hex":"406a3d","lat":51.0,"lon":8.0,"alt_baro":"ground","messages":9,"seen":1.0},
               {"hex":"4ca7b2","messages":9,"seen":1.0}"#,
        );
        assert!(window.observe(&first, t0, 4, Some(100.0)).is_none());
        // A nearer aircraft later in the same cell does not lower the maximum.
        let second = snap(
            r#"{"hex":"3c6446","lat":51.2,"lon":7.0,"alt_baro":34000,"messages":9,"seen":1.0}"#,
        );
        assert!(
            window
                .observe(&second, t0 + 1_000, 2, Some(200.0))
                .is_none()
        );

        // The next minute begins: the previous one is reported.
        let data = window
            .observe(&snap(""), t0 + 60_000, 1, Some(50.0))
            .expect("event of the first minute");
        assert_eq!(data["minute_ms"], 1_789_999_980_000u64);
        assert_eq!(data["lat"], 51.0);
        assert_eq!(data["aircraft"], 4);
        assert_eq!(data["with_position"], 3);
        assert_eq!(data["msgs_per_s"], 150.0);
        let sectors = data["sectors"].as_array().expect("sectors");
        assert_eq!(sectors.len(), 2);
        // North, 30000-40000 ft: the farther one (52.0 N is 111.19 km away).
        assert_eq!(sectors[0][0], 0);
        assert_eq!(sectors[0][1], 4);
        assert_eq!(sectors[0][2], 111.19);
        assert_eq!(sectors[0][3], "3c6445");
        // East on the ground: the great-circle bearing is 89.6 degrees, sector 17, band 0.
        assert_eq!(sectors[1][0], 17);
        assert_eq!(sectors[1][1], 0);

        // The second minute starts clean: maxima are per window.
        let data = window.finish().expect("second minute");
        assert_eq!(data["sectors"].as_array().expect("sectors").len(), 0);
        assert_eq!(data["aircraft"], 1);
        assert_eq!(data["msgs_per_s"], 50.0);
        assert!(window.finish().is_none());
    }

    #[test]
    fn implausible_and_foreign_positions_are_ignored() {
        let mut window = RangeWindow::new(Some(HOME));
        let s = snap(
            r#"{"hex":"3c6444","lat":-30.0,"lon":150.0,"alt_baro":35000,"messages":9,"seen":1.0},
               {"hex":"~a1b2c3","lat":51.5,"lon":7.0,"alt_baro":35000,"messages":9,"seen":1.0}"#,
        );
        assert!(window.observe(&s, 60_000, 2, None).is_none());
        let data = window.finish().expect("event");
        assert!(data["sectors"].as_array().expect("sectors").is_empty());
        assert_eq!(data["with_position"], 2);
        assert_eq!(data["msgs_per_s"], 0.0);
    }

    #[test]
    fn without_a_receiver_position_only_the_activity_is_reported() {
        let mut window = RangeWindow::new(None);
        let s = snap(
            r#"{"hex":"3c6444","lat":51.5,"lon":7.0,"alt_baro":35000,"messages":9,"seen":1.0}"#,
        );
        let _ = window.observe(&s, 120_000, 1, Some(10.0));
        let data = window.finish().expect("event");
        assert!(data.get("lat").is_none());
        assert!(data["sectors"].as_array().expect("sectors").is_empty());
        assert_eq!(data["aircraft"], 1);
    }
}
