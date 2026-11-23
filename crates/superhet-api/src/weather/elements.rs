//! Orbital element sets for pass prediction: where they come from, how they are kept.
//!
//! The admin decides. By default nothing is fetched and the admin can paste a TLE or OMM text
//! (kept as `manual`). With the download switched on, one fixed URL on CelesTrak is asked at most
//! every [`MIN_INTERVAL_MS`], for the weather group only, with the plain User-Agent
//! `Superhet/<version>`. The request carries nothing about the receiver: the element sets are
//! the same for everybody, and the position never leaves the server.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use superhet_satpass::{Elements, catalogue, parse_elements};

/// Key in `superhet.settings`.
pub const SETTINGS_KEY: &str = "weather.elements";
/// The one URL that is ever fetched (OMM JSON of CelesTrak's weather group).
pub const SOURCE_URL: &str =
    "https://celestrak.org/NORAD/elements/gp.php?GROUP=weather&FORMAT=json";
/// Name shown in the UI.
pub const SOURCE_NAME: &str = "CelesTrak";
/// Page with the data terms.
pub const SOURCE_INFO_URL: &str = "https://celestrak.org/";
/// CelesTrak updates its sets every two hours and asks clients not to ask more often.
pub const MIN_INTERVAL_MS: i64 = 2 * 3600 * 1000;
/// How old the sets may get before the background task fetches new ones.
pub const AUTO_INTERVAL_MS: i64 = 12 * 3600 * 1000;
/// Most bytes a download or a pasted text may have.
pub const MAX_BYTES: usize = 512 * 1024;

/// What is stored: the switch, where the sets came from and the sets themselves (OMM records).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Doc {
    /// The admin allows the download.
    #[serde(default)]
    pub enabled: bool,
    /// `download` or `manual`; `None` while there is nothing.
    #[serde(default)]
    pub source: Option<String>,
    /// When the sets were stored, Unix milliseconds.
    #[serde(default)]
    pub fetched_ms: Option<i64>,
    /// Last attempt to download (also failed ones), Unix milliseconds.
    #[serde(default)]
    pub attempt_ms: Option<i64>,
    /// Why the last download failed.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub sets: Vec<Value>,
}

impl Doc {
    /// Read the stored document; anything unreadable is an empty one.
    #[must_use]
    pub fn from_value(value: Option<Value>) -> Self {
        value
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// As stored.
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// The usable element sets.
    #[must_use]
    pub fn elements(&self) -> Vec<Elements> {
        self.sets
            .iter()
            .filter_map(|v| serde_json::from_value::<Elements>(v.clone()).ok())
            .collect()
    }

    /// Replace the sets (already filtered by [`keep_known`]).
    pub fn set_elements(&mut self, sets: &[Elements], source: &str, now_ms: i64) {
        self.sets = sets
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        self.source = Some(source.to_owned());
        self.fetched_ms = Some(now_ms);
        self.error = None;
    }

    /// Whether the background task should fetch now.
    #[must_use]
    pub fn due(&self, now_ms: i64) -> bool {
        self.enabled
            && self
                .attempt_ms
                .is_none_or(|a| now_ms - a >= MIN_INTERVAL_MS)
            && self.fetched_ms.is_none_or(|f| {
                self.source != Some("download".into()) || now_ms - f >= AUTO_INTERVAL_MS
            })
    }

    /// Milliseconds until a download is allowed again (0 = now).
    #[must_use]
    pub fn wait_ms(&self, now_ms: i64) -> i64 {
        self.attempt_ms
            .map_or(0, |a| (MIN_INTERVAL_MS - (now_ms - a)).max(0))
    }
}

/// Keep the sets of satellites in the catalogue, the newest one of each.
#[must_use]
pub fn keep_known(sets: Vec<Elements>) -> Vec<Elements> {
    let mut best: Vec<Elements> = Vec::new();
    for set in sets {
        if catalogue::by_norad(set.norad_id).is_none() {
            continue;
        }
        match best.iter_mut().find(|b| b.norad_id == set.norad_id) {
            Some(old) if set.datetime > old.datetime => *old = set,
            Some(_) => {}
            None => best.push(set),
        }
    }
    best
}

/// Parse a pasted TLE / OMM text and keep what is known.
///
/// # Errors
/// A reason fit for the admin.
pub fn parse_manual(text: &str) -> Result<Vec<Elements>, String> {
    if text.len() > MAX_BYTES {
        return Err("the text is too long".into());
    }
    let known = keep_known(parse_elements(text).map_err(|e| e.to_string())?);
    if known.is_empty() {
        return Err(
            "none of the element sets is a satellite Superhet tracks (Meteor-M2 3 = 57166, Meteor-M2 4 = 59051)".into(),
        );
    }
    Ok(known)
}

/// Download the weather group from `url`: no redirects, a size limit, the plain User-Agent.
///
/// # Errors
/// A short reason (network, status, size, format).
pub async fn fetch(url: &str) -> Result<Vec<Elements>, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .user_agent(concat!("Superhet/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !response.status().is_success() {
        return Err(format!("the server answered {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("the answer is too large".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| e.without_url().to_string())?
    {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err("the answer is too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(body).map_err(|_| "the answer is not text".to_owned())?;
    parse_manual(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const L1: &str = "1 59051U 24039A   26327.50722175 -.00000013  00000+0  13685-4 0  9998";
    const L2: &str = "2 59051  98.7256 284.9730 0008115  71.3466 288.8592 14.22444296141910";

    #[test]
    fn unknown_satellites_are_dropped_and_the_newest_set_wins() {
        let sets = parse_elements(&format!("{L1}\n{L2}\n")).expect("parse");
        let mut newer = sets[0].clone();
        newer.datetime += superhet_satpass::chrono::Duration::hours(3);
        let mut other = sets[0].clone();
        other.norad_id = 25544;
        let kept = keep_known(vec![sets[0].clone(), other, newer.clone()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].datetime, newer.datetime);
        assert!(parse_manual("hello").is_err());
        let iss = "ISS (ZARYA)\n1 25544U 98067A   08264.51782528 -.00002182  00000-0 -11606-4 0  2927\n2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.72125391563537\n";
        assert!(
            parse_manual(iss)
                .expect_err("only the ISS")
                .contains("Meteor-M2 4")
        );
        assert!(parse_manual(&format!("{L1}\n{L2}\n")).is_ok());
    }

    #[test]
    fn the_document_round_trips_and_schedules_downloads() {
        let sets = parse_elements(&format!("{L1}\n{L2}\n")).expect("parse");
        let mut doc = Doc {
            enabled: true,
            ..Doc::default()
        };
        assert!(doc.due(1_000));
        doc.set_elements(&sets, "download", 10_000_000);
        doc.attempt_ms = Some(10_000_000);
        let back = Doc::from_value(Some(doc.to_value()));
        assert_eq!(back, doc);
        assert_eq!(back.elements().len(), 1);
        assert_eq!(back.elements()[0].norad_id, 59051);
        // Fresh: not due. After the auto interval: due. A failed attempt blocks for two hours.
        assert!(!doc.due(10_000_000 + AUTO_INTERVAL_MS - 1));
        assert!(doc.due(10_000_000 + AUTO_INTERVAL_MS));
        doc.attempt_ms = Some(20_000_000);
        assert!(!doc.due(20_000_000 + MIN_INTERVAL_MS - 1));
        assert_eq!(doc.wait_ms(20_000_000 + 1000), MIN_INTERVAL_MS - 1000);
        assert_eq!(doc.wait_ms(20_000_000 + MIN_INTERVAL_MS), 0);
        // A disabled download is never due; pasted sets do not age into a download.
        doc.enabled = false;
        assert!(!doc.due(i64::MAX / 2));
        assert_eq!(Doc::from_value(None), Doc::default());
        assert_eq!(
            Doc::from_value(Some(serde_json::json!("junk"))),
            Doc::default()
        );
    }
}
