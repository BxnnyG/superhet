//! Browser `Origin` policy shared by the WebSocket upgrade and state-changing HTTP routes.

use axum::http::{HeaderMap, header};

/// Environment variable with a comma-separated list of allowed `Origin` values.
pub const ENV_ALLOWED_ORIGINS: &str = "SUPERHET_ALLOWED_ORIGINS";

/// Decides which browser origins may open a WebSocket or send state-changing requests (CSRF
/// protection).
///
/// Requests without an `Origin` header come from non-browser clients and are let through (they
/// are subject to authentication, not to CSRF). With an explicit list only exact matches pass;
/// without one the origin must be the same as the `Host` header (same-origin).
#[derive(Debug, Clone, Default)]
pub struct OriginPolicy {
    allowed: Vec<String>,
}

impl OriginPolicy {
    /// Policy from a list of origins such as `https://sdr.example.org`.
    #[must_use]
    pub fn new<I, S>(allowed: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            allowed: allowed
                .into_iter()
                .map(|o| o.as_ref().trim().trim_end_matches('/').to_ascii_lowercase())
                .filter(|o| !o.is_empty())
                .collect(),
        }
    }

    /// Policy from `SUPERHET_ALLOWED_ORIGINS` (comma separated; unset means same-origin only).
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(
            std::env::var(ENV_ALLOWED_ORIGINS)
                .unwrap_or_default()
                .split(','),
        )
    }

    /// Whether a request with these headers may upgrade / proceed.
    #[must_use]
    pub fn allows(&self, headers: &HeaderMap) -> bool {
        let Some(origin) = headers.get(header::ORIGIN) else {
            return true;
        };
        let Ok(origin) = origin.to_str() else {
            return false;
        };
        let origin = origin.trim_end_matches('/').to_ascii_lowercase();
        if !self.allowed.is_empty() {
            return self.allowed.contains(&origin);
        }
        let authority = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"));
        match (
            authority,
            headers.get(header::HOST).and_then(|h| h.to_str().ok()),
        ) {
            (Some(authority), Some(host)) => authority == host.to_ascii_lowercase(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(origin: Option<&str>, host: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(o) = origin {
            h.insert(header::ORIGIN, HeaderValue::from_str(o).expect("header"));
        }
        if let Some(o) = host {
            h.insert(header::HOST, HeaderValue::from_str(o).expect("header"));
        }
        h
    }

    #[test]
    fn same_origin_is_the_default() {
        let p = OriginPolicy::default();
        assert!(p.allows(&headers(None, Some("sdr.local:8073"))));
        assert!(p.allows(&headers(
            Some("https://SDR.local:8073"),
            Some("sdr.local:8073")
        )));
        assert!(!p.allows(&headers(
            Some("https://evil.example"),
            Some("sdr.local:8073")
        )));
        assert!(!p.allows(&headers(Some("null"), Some("sdr.local:8073"))));
        assert!(!p.allows(&headers(Some("https://sdr.local:8073"), None)));
    }

    #[test]
    fn explicit_list_overrides_host_matching() {
        let p = OriginPolicy::new(["https://sdr.example.org/", " http://localhost:5173 ", ""]);
        assert!(p.allows(&headers(
            Some("https://sdr.example.org"),
            Some("internal:8073")
        )));
        assert!(p.allows(&headers(Some("http://localhost:5173"), None)));
        assert!(!p.allows(&headers(
            Some("https://internal:8073"),
            Some("internal:8073")
        )));
        assert!(p.allows(&headers(None, None)));
    }
}
