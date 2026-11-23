//! Satellite element sets and pass prediction (ADR-0034).
//!
//! Pure Rust: the `sgp4` crate propagates an element set (a TLE or an OMM record as published by
//! CelesTrak) to a position in the TEME frame; this crate turns that into look angles for an
//! observer on the ground and finds the passes in a time window. Nothing here touches the
//! network or knows where the observer is: callers pass the position in and keep it to
//! themselves.
//!
//! Accuracy is what weather-satellite reception needs, not what orbit determination needs:
//! the Earth is rotated with the mean sidereal angle (no polar motion, no nutation) and the
//! atmosphere is ignored. Times of a pass are good to a second or two for fresh elements; the
//! error of the element set itself (days old: seconds to a minute) is larger.

use std::f64::consts::PI;

use serde::Serialize;
use sgp4::{Constants, Elements, MinutesSinceEpoch};

/// Most element sets parsed from one text (a CelesTrak group has a few hundred).
pub const MAX_ELEMENTS: usize = 2000;
/// Longest prediction window (hours).
pub const MAX_WINDOW_HOURS: u32 = 7 * 24;

/// WGS84 equatorial radius (km).
const WGS84_A_KM: f64 = 6378.137;
/// WGS84 flattening.
const WGS84_F: f64 = 1.0 / 298.257_223_563;
/// Coarse step of the pass search (seconds); a LEO pass lasts several minutes.
const SEARCH_STEP_MS: i64 = 20_000;
/// Resolution of the refined AOS / LOS / maximum (milliseconds).
const REFINE_MS: i64 = 250;

/// A satellite ready to be propagated.
#[derive(Debug, Clone)]
pub struct Satellite {
    /// NORAD catalogue number.
    pub norad_id: u64,
    /// Name from the element set (may be empty for a bare two-line set).
    pub name: String,
    epoch_ms: i64,
    constants: Constants,
}

/// Why an element set cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The text is neither TLE nor OMM JSON, or a record is malformed.
    #[error("{0}")]
    Parse(String),
    /// The orbit cannot be propagated (decayed or invalid elements).
    #[error("{0}")]
    Elements(String),
}

fn unix_ms_of(dt: &sgp4::chrono::NaiveDateTime) -> i64 {
    dt.and_utc().timestamp_millis()
}

impl Satellite {
    /// Prepare `elements` for propagation.
    ///
    /// # Errors
    /// [`Error::Elements`] when the orbit is not one SGP4 can handle.
    pub fn from_elements(elements: &Elements) -> Result<Self, Error> {
        let constants =
            Constants::from_elements(elements).map_err(|e| Error::Elements(e.to_string()))?;
        Ok(Self {
            norad_id: elements.norad_id,
            name: elements.object_name.clone().unwrap_or_default(),
            epoch_ms: unix_ms_of(&elements.datetime),
            constants,
        })
    }

    /// Epoch of the element set, Unix milliseconds.
    #[must_use]
    pub fn epoch_ms(&self) -> i64 {
        self.epoch_ms
    }

    /// Position in the ECEF frame (km) at `unix_ms`; `None` when the propagation fails (decayed
    /// orbit, a time too far from the epoch).
    #[must_use]
    pub fn ecef_km(&self, unix_ms: i64) -> Option<[f64; 3]> {
        let minutes = (unix_ms - self.epoch_ms) as f64 / 60_000.0;
        let p = self.constants.propagate(MinutesSinceEpoch(minutes)).ok()?;
        let theta = gmst_rad(unix_ms);
        let (s, c) = theta.sin_cos();
        // TEME to ECEF: rotate by the sidereal angle about the pole.
        Some([
            c * p.position[0] + s * p.position[1],
            -s * p.position[0] + c * p.position[1],
            p.position[2],
        ])
    }

    /// The point on the ground below the satellite: latitude, longitude (degrees) and height
    /// above the ellipsoid (km).
    #[must_use]
    pub fn subpoint(&self, unix_ms: i64) -> Option<(f64, f64, f64)> {
        let [x, y, z] = self.ecef_km(unix_ms)?;
        Some(geodetic_of(x, y, z))
    }
}

/// Greenwich mean sidereal time (radians, IAU 1982) for a UTC instant (UT1 taken as UTC).
fn gmst_rad(unix_ms: i64) -> f64 {
    let jd = unix_ms as f64 / 86_400_000.0 + 2_440_587.5;
    let t = (jd - 2_451_545.0) / 36_525.0;
    let seconds = 67_310.548_41 + (876_600.0 * 3600.0 + 8_640_184.812_866) * t + 0.093_104 * t * t
        - 6.2e-6 * t * t * t;
    (seconds.rem_euclid(86_400.0) / 86_400.0) * 2.0 * PI
}

/// Geodetic latitude / longitude (degrees) and height (km) of an ECEF position (Bowring).
fn geodetic_of(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    let e2 = WGS84_F * (2.0 - WGS84_F);
    let p = x.hypot(y);
    let lon = y.atan2(x);
    let mut lat = z.atan2(p * (1.0 - e2));
    let mut height = 0.0;
    for _ in 0..8 {
        let n = WGS84_A_KM / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        height = p / lat.cos() - n;
        lat = z.atan2(p * (1.0 - e2 * n / (n + height)));
    }
    (lat.to_degrees(), lon.to_degrees(), height)
}

/// A place on the ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observer {
    /// Geodetic latitude, degrees (-90 to 90).
    pub lat_deg: f64,
    /// Longitude, degrees east (-180 to 180).
    pub lon_deg: f64,
    /// Height above the ellipsoid, metres.
    pub alt_m: f64,
}

impl Observer {
    /// An observer at sea level.
    ///
    /// # Errors
    /// [`Error::Parse`] for coordinates out of range or not finite.
    pub fn new(lat_deg: f64, lon_deg: f64) -> Result<Self, Error> {
        if !(lat_deg.is_finite() && lon_deg.is_finite())
            || lat_deg.abs() > 90.0
            || lon_deg.abs() > 180.0
        {
            return Err(Error::Parse(
                "latitude is -90 to 90, longitude -180 to 180".into(),
            ));
        }
        Ok(Self {
            lat_deg,
            lon_deg,
            alt_m: 0.0,
        })
    }

    fn ecef_km(self) -> [f64; 3] {
        let (lat, lon) = (self.lat_deg.to_radians(), self.lon_deg.to_radians());
        let e2 = WGS84_F * (2.0 - WGS84_F);
        let n = WGS84_A_KM / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        let h = self.alt_m / 1000.0;
        [
            (n + h) * lat.cos() * lon.cos(),
            (n + h) * lat.cos() * lon.sin(),
            (n * (1.0 - e2) + h) * lat.sin(),
        ]
    }
}

/// Where a satellite is seen from an observer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LookAngle {
    /// Degrees clockwise from north (0 to 360).
    pub azimuth_deg: f64,
    /// Degrees above the horizon (negative: below).
    pub elevation_deg: f64,
    /// Distance, km.
    pub range_km: f64,
}

/// The look angle of `sat` from `observer` at `unix_ms`.
#[must_use]
pub fn look_angle(sat: &Satellite, observer: Observer, unix_ms: i64) -> Option<LookAngle> {
    let s = sat.ecef_km(unix_ms)?;
    let o = observer.ecef_km();
    let d = [s[0] - o[0], s[1] - o[1], s[2] - o[2]];
    let (lat, lon) = (observer.lat_deg.to_radians(), observer.lon_deg.to_radians());
    let east = -lon.sin() * d[0] + lon.cos() * d[1];
    let north = -lat.sin() * lon.cos() * d[0] - lat.sin() * lon.sin() * d[1] + lat.cos() * d[2];
    let up = lat.cos() * lon.cos() * d[0] + lat.cos() * lon.sin() * d[1] + lat.sin() * d[2];
    let range = (east * east + north * north + up * up).sqrt();
    Some(LookAngle {
        azimuth_deg: east.atan2(north).to_degrees().rem_euclid(360.0),
        elevation_deg: (up / range).asin().to_degrees(),
        range_km: range,
    })
}

/// One pass of a satellite over the horizon of an observer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Pass {
    /// Rise above the horizon, Unix milliseconds (the start of the window when the satellite
    /// was up already: see `in_progress`).
    pub aos_ms: i64,
    /// Time of the highest point.
    pub tca_ms: i64,
    /// Set below the horizon (the end of the window when it was still up).
    pub los_ms: i64,
    /// Highest elevation of the pass, degrees.
    pub max_elevation_deg: f64,
    /// Azimuth at rise, degrees.
    pub aos_azimuth_deg: f64,
    /// Azimuth at the highest point.
    pub tca_azimuth_deg: f64,
    /// Azimuth at set.
    pub los_azimuth_deg: f64,
    /// The satellite was above the horizon when the window began.
    pub in_progress: bool,
}

fn elevation_at(sat: &Satellite, obs: Observer, t: i64) -> Option<f64> {
    look_angle(sat, obs, t).map(|l| l.elevation_deg)
}

/// The instant between `below` (elevation < 0) and `above` (>= 0) where the horizon is crossed.
fn bisect_horizon(sat: &Satellite, obs: Observer, mut below: i64, mut above: i64) -> i64 {
    while (above - below).abs() > REFINE_MS {
        let mid = below + (above - below) / 2;
        match elevation_at(sat, obs, mid) {
            Some(e) if e >= 0.0 => above = mid,
            _ => below = mid,
        }
    }
    above
}

/// The time of the highest elevation inside `[lo, hi]` (ternary search; one maximum per pass).
fn highest(sat: &Satellite, obs: Observer, mut lo: i64, mut hi: i64) -> (i64, f64) {
    let at = |t: i64| elevation_at(sat, obs, t).unwrap_or(f64::NEG_INFINITY);
    while hi - lo > REFINE_MS * 2 {
        let third = (hi - lo) / 3;
        let (a, b) = (lo + third, hi - third);
        if at(a) < at(b) {
            lo = a;
        } else {
            hi = b;
        }
    }
    let t = lo + (hi - lo) / 2;
    (t, at(t))
}

/// Longest a pass can last (a LEO satellite is up for at most about 20 minutes); bounds the
/// search for the true rise and set of a pass that straddles the window's edges.
const MAX_PASS_MS: i64 = 40 * 60_000;

/// The passes of `sat` over `observer` that overlap `[from_ms, to_ms]` and whose highest
/// elevation is at least `min_elevation_deg`, oldest first. Times are the true rise and set,
/// also for a pass that began before the window (`in_progress` is set then).
#[must_use]
pub fn passes(
    sat: &Satellite,
    observer: Observer,
    from_ms: i64,
    to_ms: i64,
    min_elevation_deg: f64,
) -> Vec<Pass> {
    let mut out = Vec::new();
    if to_ms <= from_ms {
        return out;
    }
    let up = |t: i64| elevation_at(sat, observer, t).is_some_and(|e| e >= 0.0);
    let mut t = from_ms;
    // Start of the current above-horizon run, if one is going.
    let mut rise: Option<(i64, bool)> = if up(t) {
        // Walk back to the rise (the pass began before the window).
        let mut back = t;
        while up(back) && t - back < MAX_PASS_MS {
            back -= SEARCH_STEP_MS;
        }
        let aos = if up(back) {
            back
        } else {
            bisect_horizon(sat, observer, back, back + SEARCH_STEP_MS)
        };
        Some((aos, true))
    } else {
        None
    };
    loop {
        let next_t = t + SEARCH_STEP_MS;
        let was_up = up(t);
        let is_up = up(next_t);
        if !was_up && is_up && t < to_ms {
            rise = Some((bisect_horizon(sat, observer, t, next_t), false));
        } else if was_up
            && !is_up
            && let Some((aos, in_progress)) = rise.take()
        {
            let los = bisect_horizon(sat, observer, next_t, t);
            push_pass(
                sat,
                observer,
                &mut out,
                (aos, in_progress),
                los,
                min_elevation_deg,
            );
        }
        // Past the window only the set of a pass under way is still of interest.
        if next_t >= to_ms && rise.is_none() {
            break;
        }
        if next_t - to_ms > MAX_PASS_MS {
            break;
        }
        t = next_t;
    }
    out
}

fn push_pass(
    sat: &Satellite,
    obs: Observer,
    out: &mut Vec<Pass>,
    (aos, in_progress): (i64, bool),
    los: i64,
    min_elevation_deg: f64,
) {
    let (tca, max_el) = highest(sat, obs, aos, los);
    if max_el < min_elevation_deg {
        return;
    }
    let az = |t: i64| look_angle(sat, obs, t).map_or(0.0, |l| l.azimuth_deg);
    out.push(Pass {
        aos_ms: aos,
        tca_ms: tca,
        los_ms: los,
        max_elevation_deg: max_el,
        aos_azimuth_deg: az(aos),
        tca_azimuth_deg: az(tca),
        los_azimuth_deg: az(los),
        in_progress,
    });
}

/// Parse element sets from text: an OMM JSON array or object (CelesTrak `FORMAT=json`), or
/// TLE text with two lines per satellite or three (name first).
///
/// # Errors
/// [`Error::Parse`] with a short reason.
pub fn parse_elements(text: &str) -> Result<Vec<Elements>, Error> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Error::Parse("no element sets".into()));
    }
    let sets: Vec<Elements> = if trimmed.starts_with('[') || trimmed.starts_with('{') {
        let value: serde_json::Value =
            serde_json::from_str(trimmed).map_err(|e| Error::Parse(format!("JSON: {e}")))?;
        let records = match value {
            serde_json::Value::Array(a) => a,
            other => vec![other],
        };
        if records.len() > MAX_ELEMENTS {
            return Err(Error::Parse("too many element sets".into()));
        }
        records
            .into_iter()
            .map(|r| {
                serde_json::from_value::<Elements>(r)
                    .map_err(|e| Error::Parse(format!("OMM record: {e}")))
            })
            .collect::<Result<_, _>>()?
    } else {
        let lines: Vec<&str> = trimmed
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .collect();
        if lines.len() > MAX_ELEMENTS * 3 {
            return Err(Error::Parse("too many element sets".into()));
        }
        let three = lines.first().is_some_and(|l| !l.starts_with("1 "));
        let mut sets = Vec::new();
        let step = if three { 3 } else { 2 };
        if !lines.len().is_multiple_of(step) {
            return Err(Error::Parse(
                "TLE text needs two lines per satellite (or three with the name first)".into(),
            ));
        }
        for chunk in lines.chunks(step) {
            let (name, l1, l2) = if three {
                (Some(chunk[0].trim().to_owned()), chunk[1], chunk[2])
            } else {
                (None, chunk[0], chunk[1])
            };
            let mut elements = Elements::from_tle(name, l1.as_bytes(), l2.as_bytes())
                .map_err(|e| Error::Parse(format!("TLE: {e}")))?;
            if let Some(name) = elements.object_name.as_mut() {
                // CelesTrak prefixes nothing but pads names; some lists start them with "0 ".
                *name = name.trim_start_matches("0 ").trim().to_owned();
            }
            sets.push(elements);
        }
        sets
    };
    if sets.is_empty() {
        return Err(Error::Parse("no element sets".into()));
    }
    Ok(sets)
}

/// Epoch of an element set, Unix milliseconds.
#[must_use]
pub fn epoch_ms(elements: &Elements) -> i64 {
    unix_ms_of(&elements.datetime)
}

#[cfg(test)]
mod tests;
