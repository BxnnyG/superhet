//! What a browser reports about itself at session start (`POST /api/v1/auth/client-report`):
//! User-Agent Client Hints, screen, languages, time zone, hardware hints, connection type and,
//! when the admin asks for it, the public address the browser got from a third-party service.
//!
//! Everything here is **claimed by the client** and unverifiable: the shape is validated and
//! size-limited, nothing is trusted. It is shown to admins marked as such and never feeds bans,
//! rate limits or access decisions.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ip_summary;
use crate::audit::store_ip;
use crate::settings::IpMode;

/// Longest accepted request body, bytes.
pub const MAX_BODY_BYTES: usize = 8 * 1024;
const TEXT_MAX: usize = 64;
const LIST_MAX: usize = 12;

/// How the public-address lookup in the browser ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicIpStatus {
    /// The service answered with an address.
    Ok,
    /// The browser refused the request (content security policy, ad blocker, offline).
    Blocked,
    /// The service answered something that is no address, or timed out.
    Failed,
}

/// One entry of the browser's brand list (`fullVersionList`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Brand {
    pub brand: String,
    pub version: String,
}

/// Screen size and pixel ratio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub dpr: f64,
}

/// Network information API values, where the browser has them (Chromium).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Connection {
    /// `wifi`, `cellular`, `ethernet` ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `4g`, `3g` ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downlink_mbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_data: Option<bool>,
}

/// The report as sent and as stored. Every field is optional: browsers differ in what they give.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ClientReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_version: Option<String>,
    /// Device model (Android Chromium only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mobile: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub brands: Vec<Brand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<Screen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touch_points: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_cores: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<Connection>,
    /// The public address the browser got from the configured lookup service.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_ip_status: Option<PublicIpStatus>,
}

fn text(field: &str, value: &mut String, max: usize) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{field} contains control characters"));
    }
    if trimmed.chars().count() > max {
        return Err(format!("{field} is longer than {max} characters"));
    }
    *value = trimmed.to_owned();
    Ok(())
}

fn opt_text(field: &str, value: &mut Option<String>, max: usize) -> Result<(), String> {
    if let Some(v) = value {
        text(field, v, max)?;
        if v.is_empty() {
            *value = None;
        }
    }
    Ok(())
}

fn range<T: PartialOrd + Copy + std::fmt::Display>(
    field: &str,
    v: T,
    lo: T,
    hi: T,
) -> Result<(), String> {
    if v >= lo && v <= hi {
        Ok(())
    } else {
        Err(format!("{field} must be {lo}..={hi}"))
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

impl ClientReport {
    /// Parse and validate a request body.
    ///
    /// # Errors
    /// A message naming the problem: not JSON, unknown field, too long, out of range.
    pub fn parse(body: &[u8]) -> Result<Self, String> {
        if body.len() > MAX_BODY_BYTES {
            return Err("report is too large".into());
        }
        let mut r: Self =
            serde_json::from_slice(body).map_err(|e| format!("malformed report: {e}"))?;
        r.validate()?;
        Ok(r)
    }

    /// Check and normalise every field (trim, drop empty strings, round numbers).
    ///
    /// # Errors
    /// A message naming the first problem.
    pub fn validate(&mut self) -> Result<(), String> {
        opt_text("platform", &mut self.platform, TEXT_MAX)?;
        opt_text("platform_version", &mut self.platform_version, TEXT_MAX)?;
        opt_text("model", &mut self.model, TEXT_MAX)?;
        opt_text("architecture", &mut self.architecture, 16)?;
        opt_text("bitness", &mut self.bitness, 8)?;
        opt_text("timezone", &mut self.timezone, 64)?;
        if self.brands.len() > LIST_MAX {
            return Err(format!("at most {LIST_MAX} brands"));
        }
        for b in &mut self.brands {
            text("brand", &mut b.brand, TEXT_MAX)?;
            text("brand version", &mut b.version, TEXT_MAX)?;
        }
        self.brands.retain(|b| !b.brand.is_empty());
        if self.languages.len() > LIST_MAX {
            return Err(format!("at most {LIST_MAX} languages"));
        }
        for l in &mut self.languages {
            text("language", l, 35)?;
        }
        self.languages.retain(|l| !l.is_empty());
        if let Some(s) = &mut self.screen {
            range("screen.width", s.width, 1, 100_000)?;
            range("screen.height", s.height, 1, 100_000)?;
            if !s.dpr.is_finite() {
                return Err("screen.dpr must be a number".into());
            }
            range("screen.dpr", s.dpr, 0.1, 16.0)?;
            s.dpr = round2(s.dpr);
        }
        if let Some(t) = self.touch_points {
            range("touch_points", t, 0, 100)?;
        }
        if let Some(c) = self.cpu_cores {
            range("cpu_cores", c, 1, 4096)?;
        }
        if let Some(m) = &mut self.memory_gb {
            if !m.is_finite() {
                return Err("memory_gb must be a number".into());
            }
            range("memory_gb", *m, 0.1, 4096.0)?;
            *m = round2(*m);
        }
        if let Some(c) = &mut self.connection {
            opt_text("connection.kind", &mut c.kind, 16)?;
            opt_text("connection.effective_type", &mut c.effective_type, 16)?;
            if let Some(d) = &mut c.downlink_mbps {
                if !d.is_finite() {
                    return Err("connection.downlink_mbps must be a number".into());
                }
                range("connection.downlink_mbps", *d, 0.0, 100_000.0)?;
                *d = round2(*d);
            }
        }
        if let Some(ip) = &self.public_ip {
            let parsed: IpAddr = ip
                .trim()
                .parse()
                .map_err(|_| "public_ip is not an IP address".to_owned())?;
            // IPv4-mapped IPv6 reads as the IPv4 address, like everywhere else.
            let parsed = match parsed {
                IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
                v4 => v4,
            };
            self.public_ip = Some(parsed.to_string());
        }
        Ok(())
    }

    /// Whether the report says anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Drop what the policy does not ask for: the public address unless it was requested.
    pub fn apply_policy(&mut self, ask_public_ip: bool) {
        if !ask_public_ip {
            self.public_ip = None;
            self.public_ip_status = None;
        }
    }

    /// The document stored with the session: the report plus the time it arrived.
    #[must_use]
    pub fn to_stored(&self, now_ms: i64) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_else(|_| json!({}));
        if let Value::Object(m) = &mut v {
            m.insert("reported_at_ms".into(), json!(now_ms));
        }
        v
    }
}

/// A stored report as admins see it: the public address is shown in the address mode of the
/// session class (full, truncated or hidden) and classified. Everything stays marked as claimed
/// by the client in the web UI.
#[must_use]
pub fn present(stored: &Value, mode: IpMode) -> Value {
    let mut v = stored.clone();
    let Value::Object(m) = &mut v else {
        return v;
    };
    let ip: Option<IpAddr> = m
        .get("public_ip")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok());
    m.remove("public_ip");
    if let Some(ip) = ip
        && let Some(shown) = store_ip(Some(ip), mode)
    {
        // Classified from the full address (a truncated one is in the same network); no name or
        // location lookup is ever done for an address the client merely claims.
        m.insert("public_ip".into(), json!(shown));
        m.insert("public_ip_info".into(), Value::Object(ip_summary(ip)));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &Value) -> Result<ClientReport, String> {
        ClientReport::parse(v.to_string().as_bytes())
    }

    #[test]
    fn a_full_chromium_report_round_trips() {
        let r = parse(&json!({
            "platform": "Linux", "platform_version": "6.12.0", "model": "", "architecture": "x86",
            "bitness": "64", "mobile": false,
            "brands": [{"brand": "Chromium", "version": "131.0.6778.86"}],
            "screen": {"width": 2560, "height": 1440, "dpr": 1.2500001},
            "languages": ["de-DE", "en"], "timezone": "Europe/Berlin",
            "touch_points": 0, "cpu_cores": 8, "memory_gb": 8,
            "connection": {"kind": "wifi", "effective_type": "4g", "downlink_mbps": 10.456, "save_data": false},
            "public_ip": "203.0.113.7", "public_ip_status": "ok"
        }))
        .expect("valid");
        assert_eq!(r.model, None, "empty strings are dropped");
        assert_eq!(r.screen.as_ref().map(|s| s.dpr), Some(1.25));
        assert_eq!(
            r.connection.as_ref().and_then(|c| c.downlink_mbps),
            Some(10.46)
        );
        let stored = r.to_stored(42);
        assert_eq!(stored["reported_at_ms"], 42);
        assert_eq!(stored["platform"], "Linux");
        assert!(stored.get("model").is_none());
    }

    #[test]
    fn partial_and_empty_reports_are_fine() {
        assert!(parse(&json!({})).expect("empty").is_empty());
        let r = parse(&json!({"languages": ["fr"]})).expect("partial");
        assert!(!r.is_empty());
    }

    #[test]
    fn unknown_fields_garbage_and_size_are_refused() {
        assert!(parse(&json!({"cookie": "x"})).is_err());
        assert!(parse(&json!({"screen": {"width": 1, "height": 1, "dpr": 1, "x": 2}})).is_err());
        assert!(ClientReport::parse(b"not json").is_err());
        let huge = json!({"platform": "x".repeat(MAX_BODY_BYTES)});
        assert_eq!(parse(&huge).expect_err("too large"), "report is too large");
    }

    #[test]
    fn values_are_range_and_length_checked() {
        for bad in [
            json!({"platform": "x".repeat(65)}),
            json!({"platform": "Lin\u{0}ux"}),
            json!({"languages": vec!["en"; 13]}),
            json!({"brands": vec![json!({"brand": "a", "version": "1"}); 13]}),
            json!({"screen": {"width": 0, "height": 10, "dpr": 1}}),
            json!({"screen": {"width": 10, "height": 10, "dpr": 99}}),
            json!({"touch_points": 500}),
            json!({"cpu_cores": 0}),
            json!({"memory_gb": 99999}),
            json!({"connection": {"downlink_mbps": -1}}),
            json!({"public_ip": "not an ip"}),
            json!({"public_ip": "203.0.113.7/24"}),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_public_address_is_normalised_and_only_kept_when_asked_for() {
        let mut r = parse(&json!({"public_ip": " ::ffff:203.0.113.7 ", "public_ip_status": "ok"}))
            .expect("valid");
        assert_eq!(r.public_ip.as_deref(), Some("203.0.113.7"));
        r.apply_policy(true);
        assert!(r.public_ip.is_some());
        r.apply_policy(false);
        assert!(r.public_ip.is_none() && r.public_ip_status.is_none());
        assert!(r.is_empty());
    }

    #[test]
    fn presenting_applies_the_address_mode_and_classifies() {
        let stored = parse(&json!({"platform": "Linux", "public_ip": "203.0.113.77"}))
            .expect("valid")
            .to_stored(1);
        let full = present(&stored, IpMode::Full);
        assert_eq!(full["public_ip"], "203.0.113.77");
        assert_eq!(full["public_ip_info"]["class"], "reserved");
        let cut = present(&stored, IpMode::Truncated);
        assert_eq!(cut["public_ip"], "203.0.113.0");
        let none = present(&stored, IpMode::None);
        assert!(none.get("public_ip").is_none() && none.get("public_ip_info").is_none());
        assert_eq!(none["platform"], "Linux");
    }
}
