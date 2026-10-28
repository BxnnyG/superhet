//! Map tile source of the ADS-B view (ADR-0017). The default is OpenStreetMap's public tile
//! server; operators can point it at another server with `SUPERHET_MAP_TILE_URL` (a
//! `{z}/{x}/{y}` template), `SUPERHET_MAP_ATTRIBUTION` (plain text) and
//! `SUPERHET_MAP_ATTRIBUTION_URL`. The browser reads the source from `GET /api/v1/map-config`.
//!
//! `SUPERHET_MAP_TILE_MODE` picks how tiles reach the browser: `proxy` (default) serves them
//! through this server's caching proxy (`tiles`), so the CSP needs no extra `img-src` origin;
//! `direct` lets the browser fetch from the tile server itself and the CSP `img-src` follows the
//! tile origin (`web::csp_header`).

use std::sync::{Arc, PoisonError, RwLock};

use axum::{Json, Router, http::StatusCode, routing::get};
use serde::Serialize;

/// OSM's standard tile server (its tile usage policy requires the attribution and a Referer).
pub const DEFAULT_TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
/// Origin of [`DEFAULT_TILE_URL`]; the CSP in `web.rs` names it and is rewritten for others.
pub const DEFAULT_TILE_ORIGIN: &str = "https://tile.openstreetmap.org";
const DEFAULT_ATTRIBUTION: &str = "\u{a9} OpenStreetMap contributors";
const DEFAULT_ATTRIBUTION_URL: &str = "https://www.openstreetmap.org/copyright";

/// Tile URL the browser uses in proxy mode (same origin, behind the login).
pub const PROXY_TILE_URL: &str = "/api/v1/map/tiles/{z}/{x}/{y}.png";

/// How tiles reach the browser.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TileMode {
    /// Through the caching proxy of this server (default).
    #[default]
    Proxy,
    /// The browser asks the tile server itself.
    Direct,
}

/// Where the map gets its tiles from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapConfig {
    /// Upstream tile URL template with `{z}`, `{x}` and `{y}`.
    pub tile_url: String,
    pub mode: TileMode,
    /// Plain-text attribution the tile provider asks for.
    pub attribution: String,
    /// Link of the attribution, if any (`http(s)` only).
    pub attribution_url: Option<String>,
}

impl Default for MapConfig {
    fn default() -> Self {
        Self {
            tile_url: DEFAULT_TILE_URL.to_owned(),
            mode: TileMode::Proxy,
            attribution: DEFAULT_ATTRIBUTION.to_owned(),
            attribution_url: Some(DEFAULT_ATTRIBUTION_URL.to_owned()),
        }
    }
}

static CONFIG: RwLock<Option<MapConfig>> = RwLock::new(None);

/// The tile source in effect (the default until [`init_from_env`] ran).
pub fn current() -> MapConfig {
    CONFIG
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_default()
}

/// Origin (`scheme://host[:port]`) the CSP must allow in `img-src`; `None` when the proxy serves
/// the tiles from our own origin.
pub fn tile_origin() -> Option<String> {
    let config = current();
    if config.mode == TileMode::Proxy {
        return None;
    }
    // The stored URL was validated, so an origin always exists.
    Some(origin_of(&config.tile_url).unwrap_or_else(|| DEFAULT_TILE_ORIGIN.to_owned()))
}

/// What `GET /api/v1/map-config` tells the browser.
#[derive(Debug, Serialize)]
struct Served {
    /// Template the map loads tiles from: our proxy or, in direct mode, the tile server.
    tile_url: String,
    attribution: String,
    attribution_url: Option<String>,
    /// Tiles come through this server (the browser never contacts the tile provider).
    proxied: bool,
}

impl From<MapConfig> for Served {
    fn from(c: MapConfig) -> Self {
        let proxied = c.mode == TileMode::Proxy;
        Self {
            tile_url: if proxied {
                PROXY_TILE_URL.to_owned()
            } else {
                c.tile_url
            },
            attribution: c.attribution,
            attribution_url: c.attribution_url,
            proxied,
        }
    }
}

/// Read the `SUPERHET_MAP_*` variables. A bad value is logged and the default stays: a typo
/// must not take the API down.
pub fn init_from_env() {
    let config = from_values(
        std::env::var("SUPERHET_MAP_TILE_URL").ok().as_deref(),
        std::env::var("SUPERHET_MAP_ATTRIBUTION").ok().as_deref(),
        std::env::var("SUPERHET_MAP_ATTRIBUTION_URL")
            .ok()
            .as_deref(),
    )
    .unwrap_or_else(|err| {
        tracing::warn!(%err, "ignoring SUPERHET_MAP_*; using the OpenStreetMap default");
        MapConfig::default()
    });
    let mut config = config;
    config.mode = match std::env::var("SUPERHET_MAP_TILE_MODE")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("" | "proxy") => TileMode::Proxy,
        Some("direct") => TileMode::Direct,
        Some(other) => {
            tracing::warn!(
                mode = other,
                "SUPERHET_MAP_TILE_MODE must be proxy or direct; using proxy"
            );
            TileMode::Proxy
        }
    };
    if config != MapConfig::default() {
        tracing::info!(tiles = %config.tile_url, "custom map tile source");
    }
    *CONFIG.write().unwrap_or_else(PoisonError::into_inner) = Some(config);
}

/// Replace the tile source (tests of the CSP).
#[cfg(test)]
pub fn set_for_test(config: Option<MapConfig>) {
    *CONFIG.write().unwrap_or_else(PoisonError::into_inner) = config;
}

fn bad_char(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, ';' | ',' | '\'' | '"' | '<' | '>' | '\\')
}

/// `scheme://host[:port]` of an `http(s)` URL, `None` for anything that could end or extend a
/// CSP source expression.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https") || url.contains(bad_char) {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains(['@', '{', '}', '*']) {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

/// Validate the three settings; an unset tile URL means the default source.
fn from_values(
    tile_url: Option<&str>,
    attribution: Option<&str>,
    attribution_url: Option<&str>,
) -> Result<MapConfig, String> {
    let Some(tile_url) = tile_url.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(MapConfig::default());
    };
    // The placeholders contain braces, which `bad_char` allows; the host check rejects `{s}`.
    if origin_of(tile_url).is_none() {
        return Err("SUPERHET_MAP_TILE_URL must be an http(s) URL with a fixed host".into());
    }
    if ["{z}", "{x}", "{y}"].iter().any(|p| !tile_url.contains(p)) {
        return Err("SUPERHET_MAP_TILE_URL needs the placeholders {z}, {x} and {y}".into());
    }
    let attribution = attribution.map(str::trim).unwrap_or_default().to_owned();
    let attribution_url = attribution_url
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    if let Some(link) = &attribution_url
        && origin_of(link).is_none()
    {
        return Err("SUPERHET_MAP_ATTRIBUTION_URL must be an http(s) URL".into());
    }
    Ok(MapConfig {
        tile_url: tile_url.to_owned(),
        mode: TileMode::Proxy,
        attribution,
        attribution_url,
    })
}

/// Routes, behind the access middleware. The tile route answers 404 in direct mode.
pub fn router() -> Router {
    let config = current();
    let tiles = if config.mode == TileMode::Proxy {
        let proxy = Arc::new(crate::tiles::TileProxy::from_env(&config.tile_url));
        Router::new()
            .route("/api/v1/map/tiles/{z}/{x}/{y}", get(crate::tiles::serve))
            .with_state(proxy)
    } else {
        Router::new().route(
            "/api/v1/map/tiles/{z}/{x}/{y}",
            get(|| async { (StatusCode::NOT_FOUND, "tile proxy is disabled") }),
        )
    };
    Router::new()
        .route("/api/v1/map-config", get(map_config))
        .merge(tiles)
}

async fn map_config() -> Json<Served> {
    Json(current().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_means_openstreetmap() {
        let c = from_values(None, None, None).expect("valid");
        assert_eq!(c, MapConfig::default());
        assert_eq!(from_values(Some("  "), Some("x"), None).expect("valid"), c);
        assert_eq!(origin_of(&c.tile_url).as_deref(), Some(DEFAULT_TILE_ORIGIN));
    }

    #[test]
    fn a_custom_source_keeps_its_origin_and_attribution() {
        let c = from_values(
            Some("http://tiles.lan:8080/styles/dark/{z}/{x}/{y}.png"),
            Some(" Own tiles "),
            Some("https://example.org/about"),
        )
        .expect("valid");
        assert_eq!(c.attribution, "Own tiles");
        assert_eq!(
            c.attribution_url.as_deref(),
            Some("https://example.org/about")
        );
        assert_eq!(
            origin_of(&c.tile_url).as_deref(),
            Some("http://tiles.lan:8080")
        );
    }

    #[test]
    fn the_browser_gets_the_proxy_url_unless_it_fetches_directly() {
        let proxied = Served::from(MapConfig::default());
        assert!(proxied.proxied);
        assert_eq!(proxied.tile_url, PROXY_TILE_URL);
        let direct = Served::from(MapConfig {
            mode: TileMode::Direct,
            ..MapConfig::default()
        });
        assert!(!direct.proxied);
        assert_eq!(direct.tile_url, DEFAULT_TILE_URL);
    }

    #[test]
    fn unusable_values_are_refused() {
        let ok = "https://t.example/{z}/{x}/{y}.png";
        assert!(from_values(Some(ok), None, None).is_ok());
        // Not http(s), subdomain placeholders (unknowable origin), missing placeholders.
        assert!(from_values(Some("ftp://t.example/{z}/{x}/{y}"), None, None).is_err());
        assert!(from_values(Some("https://{s}.t.example/{z}/{x}/{y}"), None, None).is_err());
        assert!(from_values(Some("https://t.example/tiles.png"), None, None).is_err());
        // Anything that could end the CSP directive or add a source.
        let injected = "https://t.example; script-src */{z}/{x}/{y}";
        assert!(from_values(Some(injected), None, None).is_err());
        assert!(from_values(Some("https://a@t.example/{z}/{x}/{y}"), None, None).is_err());
        assert!(from_values(Some(ok), None, Some("javascript:alert(1)")).is_err());
    }
}
