//! SigMF (v1.2) metadata for IQ recordings: the `.sigmf-meta` JSON next to the raw
//! `.sigmf-data` samples, plus an incremental SHA-512 so the file is never read back.

use serde_json::{Value, json};
use sha2::{Digest, Sha512};

/// SigMF specification version written.
pub const VERSION: &str = "1.2.0";

/// Sample encodings (the SigMF `core:datatype` names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Datatype {
    /// Unsigned 8-bit I/Q, like `rtl_sdr` files.
    Cu8,
    /// Signed 16-bit little-endian I/Q.
    Ci16Le,
    /// 32-bit float little-endian I/Q.
    Cf32Le,
}

impl Datatype {
    /// The `core:datatype` string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cu8 => "cu8",
            Self::Ci16Le => "ci16_le",
            Self::Cf32Le => "cf32_le",
        }
    }

    /// Bytes per complex sample.
    #[must_use]
    pub fn sample_bytes(self) -> u64 {
        match self {
            Self::Cu8 => 2,
            Self::Ci16Le => 4,
            Self::Cf32Le => 8,
        }
    }
}

/// Days since 1970-01-01 to a civil date (proleptic Gregorian).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `2026-11-10T12:00:00.123Z` for Unix milliseconds (the SigMF datetime format).
#[must_use]
pub fn iso8601_utc(unix_ms: i64) -> String {
    let secs = unix_ms.div_euclid(1000);
    let ms = unix_ms.rem_euclid(1000);
    let (year, month, day) = civil(secs.div_euclid(86_400));
    let rest = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// What a recording knows about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    /// Sample encoding.
    pub datatype: Datatype,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Centre frequency of the samples in Hz.
    pub frequency_hz: f64,
    /// Start of the first sample, Unix milliseconds.
    pub start_unix_ms: i64,
    /// Human-readable description.
    pub description: String,
    /// Recorder name and version.
    pub recorder: String,
    /// Hardware description (the device label).
    pub hw: Option<String>,
    /// Occupied bandwidth of the content in Hz (VFO slices), for the annotation.
    pub bandwidth_hz: Option<f64>,
    /// Label for the annotation (mode, VFO).
    pub label: Option<String>,
}

impl Meta {
    /// The metadata document. `samples` and `sha512` are known once the recording is closed.
    #[must_use]
    pub fn to_json(&self, samples: Option<u64>, sha512: Option<&str>) -> Value {
        let mut global = json!({
            "core:datatype": self.datatype.as_str(),
            "core:sample_rate": self.sample_rate,
            "core:version": VERSION,
            "core:recorder": self.recorder,
            "core:description": self.description,
        });
        if let Some(hw) = &self.hw
            && let Some(map) = global.as_object_mut()
        {
            map.insert("core:hw".into(), json!(hw));
        }
        if let Some(hash) = sha512
            && let Some(map) = global.as_object_mut()
        {
            map.insert("core:sha512".into(), json!(hash));
        }
        let captures = json!([{
            "core:sample_start": 0,
            "core:frequency": self.frequency_hz,
            "core:datetime": iso8601_utc(self.start_unix_ms),
        }]);
        let mut annotations = Vec::new();
        if let Some(count) = samples.filter(|c| *c > 0) {
            let mut note = json!({"core:sample_start": 0, "core:sample_count": count});
            if let (Some(bw), Some(map)) = (self.bandwidth_hz, note.as_object_mut()) {
                map.insert(
                    "core:freq_lower_edge".into(),
                    json!(self.frequency_hz - bw / 2.0),
                );
                map.insert(
                    "core:freq_upper_edge".into(),
                    json!(self.frequency_hz + bw / 2.0),
                );
            }
            if let (Some(label), Some(map)) = (&self.label, note.as_object_mut()) {
                map.insert("core:label".into(), json!(label));
            }
            annotations.push(note);
        }
        json!({"global": global, "captures": captures, "annotations": annotations})
    }
}

/// Check a metadata document for the fields every SigMF reader needs.
///
/// # Errors
/// The first problem found.
pub fn validate(doc: &Value) -> Result<(), String> {
    let global = doc
        .get("global")
        .and_then(Value::as_object)
        .ok_or("no global object")?;
    let datatype = global
        .get("core:datatype")
        .and_then(Value::as_str)
        .ok_or("core:datatype missing")?;
    if !["cu8", "ci16_le", "cf32_le"].contains(&datatype) {
        return Err(format!("unsupported datatype {datatype}"));
    }
    if global
        .get("core:sample_rate")
        .and_then(Value::as_f64)
        .is_none_or(|r| r <= 0.0)
    {
        return Err("core:sample_rate missing or not positive".into());
    }
    let version = global
        .get("core:version")
        .and_then(Value::as_str)
        .ok_or("core:version missing")?;
    if version.split('.').count() != 3 {
        return Err("core:version is not semver".into());
    }
    let captures = doc
        .get("captures")
        .and_then(Value::as_array)
        .ok_or("captures missing")?;
    let first = captures
        .first()
        .and_then(Value::as_object)
        .ok_or("no capture segment")?;
    if first.get("core:sample_start").and_then(Value::as_u64) != Some(0) {
        return Err("first capture must start at sample 0".into());
    }
    if let Some(time) = first.get("core:datetime").and_then(Value::as_str)
        && !(time.len() >= 20 && time.ends_with('Z') && time.as_bytes()[10] == b'T')
    {
        return Err("core:datetime is not ISO 8601 UTC".into());
    }
    if doc.get("annotations").is_some_and(|a| !a.is_array()) {
        return Err("annotations must be an array".into());
    }
    Ok(())
}

/// Running SHA-512 of the data file.
#[derive(Default)]
pub struct DataHash(Sha512);

impl DataHash {
    /// A fresh hash.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed written bytes.
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    /// Lower-case hex digest.
    #[must_use]
    pub fn finish_hex(self) -> String {
        self.0
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta {
            datatype: Datatype::Cu8,
            sample_rate: 2_048_000,
            frequency_hz: 145_500_000.0,
            start_unix_ms: 1_794_312_000_123,
            description: "Superhet window recording".into(),
            recorder: "Superhet 0.1.0".into(),
            hw: Some("RTL-SDR".into()),
            bandwidth_hz: Some(12_500.0),
            label: Some("nfm".into()),
        }
    }

    #[test]
    fn dates_are_iso_8601() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601_utc(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso8601_utc(1_794_312_000_123), "2026-11-10T12:00:00.123Z");
        assert_eq!(iso8601_utc(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn the_meta_document_has_the_required_fields() {
        let doc = meta().to_json(Some(4_096_000), Some("ab"));
        validate(&doc).expect("valid");
        assert_eq!(doc["global"]["core:datatype"], "cu8");
        assert_eq!(doc["global"]["core:sample_rate"], 2_048_000);
        assert_eq!(doc["global"]["core:version"], VERSION);
        assert_eq!(doc["global"]["core:sha512"], "ab");
        assert_eq!(doc["global"]["core:hw"], "RTL-SDR");
        assert_eq!(doc["captures"][0]["core:sample_start"], 0);
        assert_eq!(doc["captures"][0]["core:frequency"], 145_500_000.0);
        assert_eq!(
            doc["captures"][0]["core:datetime"],
            "2026-11-10T12:00:00.123Z"
        );
        assert_eq!(doc["annotations"][0]["core:sample_count"], 4_096_000);
        assert_eq!(doc["annotations"][0]["core:freq_lower_edge"], 145_493_750.0);
        // Before the recording ends there is neither a hash nor an annotation.
        let open = meta().to_json(None, None);
        validate(&open).expect("valid");
        assert!(open["global"].get("core:sha512").is_none());
        assert_eq!(open["annotations"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn validation_rejects_broken_documents() {
        let good = meta().to_json(None, None);
        for (path, value) in [
            ("/global/core:datatype", json!("cf64_le")),
            ("/global/core:sample_rate", json!(0)),
            ("/global/core:version", json!("1.2")),
            ("/captures/0/core:sample_start", json!(5)),
            ("/captures/0/core:datetime", json!("yesterday")),
            ("/annotations", json!({})),
        ] {
            let mut doc = good.clone();
            if let Some(slot) = doc.pointer_mut(path) {
                *slot = value;
            }
            assert!(validate(&doc).is_err(), "{path}");
        }
        assert!(validate(&json!({})).is_err());
        assert!(validate(&json!({"global": {}, "captures": []})).is_err());
    }

    #[test]
    fn the_hash_matches_a_known_digest() {
        let mut hash = DataHash::new();
        hash.update(b"ab");
        hash.update(b"c");
        assert_eq!(
            hash.finish_hex(),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
    }

    #[test]
    fn datatypes_have_their_sample_sizes() {
        assert_eq!(Datatype::Cu8.sample_bytes(), 2);
        assert_eq!(Datatype::Ci16Le.sample_bytes(), 4);
        assert_eq!(Datatype::Cf32Le.sample_bytes(), 8);
    }
}
