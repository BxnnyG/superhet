//! Profile data: the preset a device can be switched to (`docs/api-profiles.md`).
//!
//! The same JSON document is stored in PostgreSQL by the API, sent to the daemon in
//! `ctl.profile.load` and built from a device state document by "save as profile". Keeping the
//! type here means both sides agree on the shape and the basic limits.

use serde::{Deserialize, Serialize};

/// Profile document version.
pub const VERSION: u32 = 1;
/// Most VFOs a profile (and a device) can hold.
pub const MAX_VFOS: usize = 8;
/// Most reserved decoder entries a profile may carry.
pub const MAX_DECODERS: usize = 32;

const MAX_CENTER_HZ: u64 = 1_000_000_000_000;
const GAIN_RANGE_DB: std::ops::RangeInclusive<f32> = -20.0..=100.0;
const PPM_RANGE: std::ops::RangeInclusive<i32> = -1000..=1000;
const BANDWIDTH_RANGE: std::ops::RangeInclusive<u32> = 1_000..=1_000_000;
const SQUELCH_RANGE: std::ops::RangeInclusive<f32> = -200.0..=0.0;

/// Tuner gain mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GainMode {
    /// Automatic gain control.
    Auto,
    /// Fixed gain in dB.
    Manual,
}

/// Gain setting: `{"mode":"auto"}` or `{"mode":"manual","db":28.0}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gain {
    /// Mode.
    pub mode: GainMode,
    /// Gain in dB; required for `manual`, `null`/absent for `auto`.
    #[serde(default)]
    pub db: Option<f32>,
}

/// Demodulation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Broadcast FM.
    Wfm,
    /// Narrow-band FM.
    Nfm,
    /// Envelope AM.
    Am,
}

/// De-emphasis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Deemphasis {
    /// No de-emphasis.
    #[serde(rename = "none")]
    None,
    /// 50 us.
    #[serde(rename = "50us")]
    Us50,
    /// 75 us.
    #[serde(rename = "75us")]
    Us75,
}

/// A VFO in a profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vfo {
    /// Absolute frequency in Hz.
    pub freq_hz: u64,
    /// Mode.
    pub mode: Mode,
    /// Bandwidth in Hz; absent/`null` = mode default.
    #[serde(default)]
    pub bandwidth_hz: Option<u32>,
    /// Squelch threshold in dBFS; absent/`null` = off.
    #[serde(default)]
    pub squelch_dbfs: Option<f32>,
    /// De-emphasis; absent/`null` = mode default (never for AM).
    #[serde(default)]
    pub deemphasis: Option<Deemphasis>,
}

/// The preset itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileData {
    /// Document version, [`VERSION`].
    pub v: u32,
    /// Centre frequency in Hz.
    pub center_hz: u64,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Gain; `null`/absent leaves the gain alone (devices without gain control).
    #[serde(default)]
    pub gain: Option<Gain>,
    /// Crystal correction in ppm.
    #[serde(default)]
    pub ppm: i32,
    /// Bias tee.
    #[serde(default)]
    pub bias_tee: bool,
    /// VFOs; loading a profile replaces the device's VFO set with these.
    #[serde(default)]
    pub vfos: Vec<Vfo>,
    /// Hold the user tuning lock after loading.
    #[serde(default)]
    pub lock: bool,
    /// Reserved for decoder instances (P2); stored and returned untouched, ignored by the daemon.
    #[serde(default)]
    pub decoders: Vec<serde_json::Value>,
}

impl ProfileData {
    /// Parse and validate a profile document.
    ///
    /// # Errors
    /// A human-readable reason for malformed JSON, unknown fields or out-of-range values.
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let data: Self =
            serde_json::from_slice(json).map_err(|e| format!("malformed profile: {e}"))?;
        data.validate()?;
        Ok(data)
    }

    /// Check version and ranges. The daemon additionally checks that every VFO is realisable at
    /// the profile's sample rate when it is loaded.
    ///
    /// # Errors
    /// A human-readable reason.
    pub fn validate(&self) -> Result<(), String> {
        if self.v != VERSION {
            return Err(format!("unsupported profile version {}", self.v));
        }
        if !(1..=MAX_CENTER_HZ).contains(&self.center_hz) {
            return Err(format!("center_hz {} out of range", self.center_hz));
        }
        if self.sample_rate == 0 {
            return Err("sample_rate must be non-zero".into());
        }
        if !PPM_RANGE.contains(&self.ppm) {
            return Err(format!("ppm {} out of range", self.ppm));
        }
        match self.gain {
            Some(Gain {
                mode: GainMode::Manual,
                db: Some(db),
            }) if GAIN_RANGE_DB.contains(&db) => {}
            Some(Gain {
                mode: GainMode::Manual,
                ..
            }) => return Err("manual gain needs a db value within -20..=100".into()),
            Some(Gain {
                mode: GainMode::Auto,
                db: Some(_),
            }) => return Err("auto gain takes no db value".into()),
            Some(Gain {
                mode: GainMode::Auto,
                db: None,
            })
            | None => {}
        }
        if self.vfos.len() > MAX_VFOS {
            return Err(format!("at most {MAX_VFOS} VFOs"));
        }
        for vfo in &self.vfos {
            if !(1..=MAX_CENTER_HZ).contains(&vfo.freq_hz) {
                return Err(format!("VFO freq_hz {} out of range", vfo.freq_hz));
            }
            if vfo
                .bandwidth_hz
                .is_some_and(|b| !BANDWIDTH_RANGE.contains(&b))
            {
                return Err("VFO bandwidth_hz outside 1000..=1000000".into());
            }
            if vfo
                .squelch_dbfs
                .is_some_and(|s| !SQUELCH_RANGE.contains(&s))
            {
                return Err("VFO squelch_dbfs outside -200..=0".into());
            }
            if vfo.mode == Mode::Am && vfo.deemphasis.is_some() {
                return Err("deemphasis does not apply to am".into());
            }
        }
        if self.decoders.len() > MAX_DECODERS || self.decoders.iter().any(|d| !d.is_object()) {
            return Err(format!(
                "decoders must be at most {MAX_DECODERS} objects (reserved)"
            ));
        }
        Ok(())
    }

    /// Snapshot a device state document (`docs/api-devices.md`) as a profile. VFOs take their
    /// effective bandwidth and de-emphasis; the lock flag is set when a user lock is held.
    ///
    /// # Errors
    /// A reason if the document is not a usable v1 state.
    pub fn from_state(state: &serde_json::Value) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct StateGain {
            mode: GainMode,
            db: Option<f32>,
        }
        #[derive(Deserialize)]
        struct StateVfo {
            freq_hz: u64,
            mode: Mode,
            bandwidth_hz: u32,
            squelch_dbfs: Option<f32>,
            deemphasis: Option<Deemphasis>,
        }
        #[derive(Deserialize)]
        struct StateLock {
            kind: String,
        }
        #[derive(Deserialize)]
        struct State {
            center_hz: u64,
            sample_rate: u32,
            gain: Option<StateGain>,
            ppm: i32,
            bias_tee: bool,
            #[serde(default)]
            vfos: Vec<StateVfo>,
            #[serde(default)]
            locks: Vec<StateLock>,
        }
        let state: State = serde_json::from_value(state.clone())
            .map_err(|e| format!("unusable device state: {e}"))?;
        let data = Self {
            v: VERSION,
            center_hz: state.center_hz,
            sample_rate: state.sample_rate,
            gain: state.gain.map(|g| Gain {
                mode: g.mode,
                db: match g.mode {
                    GainMode::Auto => None,
                    GainMode::Manual => g.db,
                },
            }),
            ppm: state.ppm,
            bias_tee: state.bias_tee,
            vfos: state
                .vfos
                .into_iter()
                .map(|v| Vfo {
                    freq_hz: v.freq_hz,
                    mode: v.mode,
                    bandwidth_hz: Some(v.bandwidth_hz),
                    squelch_dbfs: v.squelch_dbfs,
                    deemphasis: v.deemphasis,
                })
                .collect(),
            lock: state.locks.iter().any(|l| l.kind == "user"),
            decoders: Vec::new(),
        };
        data.validate()?;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{"v":1,"center_hz":433920000,"sample_rate":2048000,
        "gain":{"mode":"manual","db":28.0},"ppm":-2,"bias_tee":true,
        "vfos":[{"freq_hz":433970000,"mode":"nfm","bandwidth_hz":12500,"squelch_dbfs":-40.0,"deemphasis":"none"},
                {"freq_hz":433800000,"mode":"am"}],
        "lock":true,"decoders":[{"plugin":"x"}]}"#;

    #[test]
    fn parses_and_round_trips() {
        let data = ProfileData::parse(FULL.as_bytes()).expect("valid");
        assert_eq!(data.vfos.len(), 2);
        assert_eq!(data.vfos[1].bandwidth_hz, None);
        let again = ProfileData::parse(&serde_json::to_vec(&data).expect("json")).expect("valid");
        assert_eq!(data, again);
    }

    #[test]
    fn minimal_profile_uses_defaults() {
        let data = ProfileData::parse(br#"{"v":1,"center_hz":1,"sample_rate":2}"#).expect("valid");
        assert_eq!(
            (data.ppm, data.bias_tee, data.lock, data.gain),
            (0, false, false, None)
        );
    }

    #[test]
    fn rejects_bad_documents() {
        for bad in [
            "",
            "[]",
            r#"{"center_hz":1,"sample_rate":2}"#,
            r#"{"v":2,"center_hz":1,"sample_rate":2}"#,
            r#"{"v":1,"center_hz":0,"sample_rate":2}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":0}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"ppm":5000}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"extra":1}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"gain":{"mode":"manual"}}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"gain":{"mode":"auto","db":3}}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"gain":{"mode":"manual","db":500}}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"vfos":[{"freq_hz":0,"mode":"am"}]}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"vfos":[{"freq_hz":5,"mode":"usb"}]}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"vfos":[{"freq_hz":5,"mode":"am","deemphasis":"50us"}]}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"vfos":[{"freq_hz":5,"mode":"am","bandwidth_hz":5}]}"#,
            r#"{"v":1,"center_hz":1,"sample_rate":2,"decoders":[1]}"#,
        ] {
            assert!(ProfileData::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        let nine = format!(
            r#"{{"v":1,"center_hz":1,"sample_rate":2,"vfos":[{}]}}"#,
            [r#"{"freq_hz":5,"mode":"am"}"#; 9].join(",")
        );
        assert!(ProfileData::parse(nine.as_bytes()).is_err());
    }

    #[test]
    fn snapshots_a_state_document() {
        let state = serde_json::json!({
            "v": 1, "device": "rtl0", "driver": "rtlsdr", "center_hz": 100_000_000u64,
            "sample_rate": 2_048_000, "gain": {"mode": "manual", "db": 28.0}, "ppm": 3,
            "bias_tee": false, "tuning_epoch": 4, "gains_db": [],
            "vfos": [{"id": 7, "freq_hz": 100_300_000u64, "mode": "wfm", "bandwidth_hz": 180_000,
                      "squelch_dbfs": null, "deemphasis": "50us", "offset_hz": 300_000.0,
                      "status": "active"}],
            "locks": [{"kind": "user", "id": "user", "since_ms": 1}],
        });
        let data = ProfileData::from_state(&state).expect("usable");
        assert_eq!(
            (data.center_hz, data.ppm, data.lock),
            (100_000_000, 3, true)
        );
        assert_eq!(data.vfos[0].deemphasis, Some(Deemphasis::Us50));
        assert_eq!(data.vfos[0].bandwidth_hz, Some(180_000));
        let auto = serde_json::json!({
            "center_hz": 1, "sample_rate": 2, "gain": {"mode": "auto", "db": null},
            "ppm": 0, "bias_tee": false
        });
        let data = ProfileData::from_state(&auto).expect("usable");
        assert_eq!(
            data.gain,
            Some(Gain {
                mode: GainMode::Auto,
                db: None
            })
        );
        assert!(!data.lock);
        assert!(ProfileData::from_state(&serde_json::json!({"center_hz": 1})).is_err());
    }
}
