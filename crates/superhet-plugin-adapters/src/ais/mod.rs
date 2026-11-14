//! AIS ship tracking on 161.975 / 162.025 MHz through AIS-catcher (ADR-0031).
//!
//! AIS-catcher (GPL-3.0, a separate process) reads `cu8` baseband at 192 kHz from stdin
//! (`-r cu8 .`, the `.` is its name for standard input) and prints one NMEA sentence per message
//! (`-o 1`). [`payload`] decodes the sentences (ITU-R M.1371), [`tracker`] merges what the
//! messages of one MMSI tell and rations the `ais.vessel` events that go into the decoder
//! history; the web map is drawn from those events, like the APRS map.

pub mod payload;
pub mod tracker;

use std::process::ExitCode;
use std::time::Duration;

use serde_json::Value;

use crate::run::run;
use tracker::Tracker;

/// Default shortest time between two history events of one vessel.
const DEFAULT_EVENT_INTERVAL_S: u64 = 60;

/// Sample rate of the plugin's input (`plugins/ais/plugin.toml`).
pub const SAMPLE_RATE: u32 = 192_000;

/// Instance configuration (`plugins/ais/schema/config.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Shortest time between two history events of one vessel.
    pub event_interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            event_interval: Duration::from_secs(DEFAULT_EVENT_INTERVAL_S),
        }
    }
}

impl Config {
    /// Read the instance configuration (`null` or an object). Unknown keys are refused.
    ///
    /// # Errors
    /// A message naming the offending key.
    pub fn from_json(config: &Value) -> Result<Self, String> {
        let map = match config {
            Value::Null => return Ok(Self::default()),
            Value::Object(map) => map,
            _ => return Err("config must be an object".into()),
        };
        let mut out = Self::default();
        for (key, value) in map {
            match key.as_str() {
                "event_interval_s" => {
                    let secs = value
                        .as_u64()
                        .filter(|s| (5..=3600).contains(s))
                        .ok_or("event_interval_s must be a whole number from 5 to 3600")?;
                    out.event_interval = Duration::from_secs(secs);
                }
                other => {
                    return Err(format!("unknown config key {other:?} (event_interval_s)"));
                }
            }
        }
        Ok(out)
    }
}

/// AIS-catcher's command line: IQ from stdin, NMEA sentences on stdout.
#[must_use]
pub fn args() -> Vec<String> {
    // `-s` belongs to the input before it, so it comes after `-r`.
    let rate = SAMPLE_RATE.to_string();
    ["-r", "cu8", ".", "-s", rate.as_str(), "-o", "1"]
        .map(String::from)
        .into()
}

/// Run `program` (AIS-catcher) and forward its messages as events.
pub fn start(program: &str, config: &Value) -> Result<ExitCode, String> {
    let config = Config::from_json(config)?;
    let mut tracker = Tracker::new(config.event_interval);
    Ok(run(program, &args(), move |line| tracker.line(line)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config() {
        assert_eq!(Config::from_json(&Value::Null), Ok(Config::default()));
        let c = Config::from_json(&json!({"event_interval_s": 30})).expect("valid");
        assert_eq!(c.event_interval, Duration::from_secs(30));
        assert!(Config::from_json(&json!({"event_interval_s": 1})).is_err());
        assert!(Config::from_json(&json!({"event_interval_s": 3601})).is_err());
        assert!(Config::from_json(&json!({"x": 1})).is_err());
        assert!(Config::from_json(&json!([1])).is_err());
    }

    #[test]
    fn command_line_reads_stdin_and_prints_nmea() {
        assert_eq!(args().join(" "), "-r cu8 . -s 192000 -o 1");
        assert_eq!(SAMPLE_RATE, 192_000);
    }
}
