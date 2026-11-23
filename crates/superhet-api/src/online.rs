//! Online aircraft details for the ADS-B panel (ADR-0033, `docs/plugins-adsb.md`): the full type
//! name and owner, the route of a flight and a photo, from two public services. Off by default:
//! an admin switches it on (and each source separately), because every lookup tells a third
//! party which aircraft and which flight number somebody looked at.
//!
//! ```text
//! GET /api/v1/adsb/online?icao=..&callsign=..   details of one aircraft (everybody who sees decoders)
//! GET /api/v1/admin/adsb/online                 settings, lock state and the sources (admin)
//! PUT /api/v1/admin/adsb/online                 {"enabled":..,"adsbdb":..,"planespotters":..}
//! ```
//!
//! What leaves this server, per lookup: the 24-bit ICAO address and, when the aircraft sends one,
//! its callsign. Both are public ADS-B broadcasts. Never the receiver position, an account, a
//! viewer address or anything else; the request carries the honest `Superhet/<version>` agent
//! with the project URL.
//!
//! The browser never talks to these services for data. Two rules shape the code:
//!
//! - **adsbdb** (aircraft, route): its route data may not be copied or put into other databases
//!   (credit in its README), so answers are kept in memory only, for hours, never on disk, in the
//!   database or in a backup. Its own photo links (a third service) are not used.
//! - **Planespotters** (photo): the API may be called from here and its answer cached for at most
//!   24 h, but the image itself must be fetched by the viewer's browser straight from the URL
//!   the API gives, unchanged: no proxy, no copy. This module therefore passes the URL on after
//!   checking that it points at one of two known image hosts, and the page's CSP allows exactly
//!   those hosts in `img-src` while the source is on (`web::set_image_origins`).
//!
//! Requests are bounded: fixed hosts, a validated address and callsign in the path, no redirects,
//! timeouts, a size cap, two connections and 60 requests a minute per service, and a pause after
//! an error or a refusal. Nothing here works around a block.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::{
    Extension, Router,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use superhet_bus::audit::Outcome;
use tokio::sync::Semaphore;

use crate::access::Principal;
use crate::devices::{DevicesState, failure, guard_mutation, json};

/// Settings key of the stored switches.
pub const KEY_ONLINE: &str = "adsb_online";
/// Environment override: `off` locks everything off, a list (`adsbdb,planespotters`, or `all`)
/// switches exactly those sources on; the admin console then shows the state as locked.
pub const ENV_OVERRIDE: &str = "SUPERHET_ADSB_ONLINE";

const ADSBDB_BASE: &str = "https://api.adsbdb.com/v0";
const PHOTOS_BASE: &str = "https://api.planespotters.net/pub/photos";
/// Hosts the photo service serves its images from (checked on the API's answer, put into the
/// CSP while the source is on).
pub const PHOTO_HOSTS: [&str; 2] = ["t.plnspttrs.net", "cdn.planespotters.net"];
const PHOTO_PAGE_HOST: &str = "www.planespotters.net";

const MAX_BODY: usize = 256 * 1024;
const CONNECTIONS: usize = 2;
const PER_MINUTE: u32 = 60;
const TTL_AIRCRAFT: Duration = Duration::from_secs(24 * 3600);
const TTL_ROUTE: Duration = Duration::from_secs(6 * 3600);
/// The photo service allows its answers to be kept for 24 h at most.
const TTL_PHOTO: Duration = Duration::from_secs(6 * 3600);
const TTL_MISS: Duration = Duration::from_secs(3600);
const MAX_ENTRIES: usize = 4000;
const BACKOFF: Duration = Duration::from_secs(60);
const BACKOFF_REFUSED: Duration = Duration::from_secs(300);

/// What an admin allows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OnlineSettings {
    /// Master switch: nothing is sent while this is off.
    pub enabled: bool,
    /// adsbdb.com: full type name, owner, route and airline.
    pub adsbdb: bool,
    /// planespotters.net: a photo of the aircraft.
    pub planespotters: bool,
}

impl OnlineSettings {
    #[must_use]
    pub fn adsbdb_on(&self) -> bool {
        self.enabled && self.adsbdb
    }

    #[must_use]
    pub fn planespotters_on(&self) -> bool {
        self.enabled && self.planespotters
    }

    /// Image origins the page's CSP must allow (`img-src`).
    #[must_use]
    pub fn image_origins(&self) -> Vec<String> {
        if self.planespotters_on() {
            PHOTO_HOSTS.iter().map(|h| format!("https://{h}")).collect()
        } else {
            Vec::new()
        }
    }

    /// The settings an [`ENV_OVERRIDE`] value stands for; unknown words are ignored.
    #[must_use]
    pub fn from_override(value: &str) -> Self {
        let mut s = Self::default();
        for token in value.split(',').map(|t| t.trim().to_ascii_lowercase()) {
            match token.as_str() {
                "off" | "none" | "false" | "no" | "0" => return Self::default(),
                "on" | "all" | "true" | "yes" | "1" => {
                    s.adsbdb = true;
                    s.planespotters = true;
                }
                "adsbdb" => s.adsbdb = true,
                "planespotters" => s.planespotters = true,
                "" => {}
                other => tracing::warn!(
                    value = other,
                    "{ENV_OVERRIDE}: unknown source ignored (adsbdb, planespotters, all, off)"
                ),
            }
        }
        s.enabled = s.adsbdb || s.planespotters;
        s
    }
}

/// The settings in force: the environment override (then `locked`) or the stored switches.
#[must_use]
pub fn effective_with(stored: OnlineSettings, env: Option<&str>) -> (OnlineSettings, bool) {
    match env.map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => (OnlineSettings::from_override(value), true),
        None => (stored, false),
    }
}

/// [`effective_with`] for the process environment.
#[must_use]
pub fn effective(stored: OnlineSettings) -> (OnlineSettings, bool) {
    effective_with(stored, std::env::var(ENV_OVERRIDE).ok().as_deref())
}

/// The two services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Adsbdb,
    Planespotters,
}

/// How one lookup ended.
#[derive(Debug, Clone, PartialEq)]
enum Fetched {
    /// Not asked (source off, nothing to ask for).
    Skipped,
    Found(Value),
    /// The service does not know it.
    Missing,
    /// Unreachable, an error status or an unreadable answer.
    Unavailable,
    /// Our own request budget or the service's pause is in the way.
    Limited,
}

struct Cached {
    value: Option<Value>,
    until: Instant,
}

struct GateState {
    blocked_until: Option<Instant>,
    window_start: Instant,
    used: u32,
}

/// Connection, rate and pause limits of one service.
struct Gate {
    permits: Semaphore,
    state: Mutex<GateState>,
}

impl Gate {
    fn new() -> Self {
        Self {
            permits: Semaphore::new(CONNECTIONS),
            state: Mutex::new(GateState {
                blocked_until: None,
                window_start: Instant::now(),
                used: 0,
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Count one request against the budget; `false` while paused or over budget.
    fn admit(&self) -> bool {
        let mut st = self.state();
        let now = Instant::now();
        if st.blocked_until.is_some_and(|until| until > now) {
            return false;
        }
        if now.duration_since(st.window_start) >= Duration::from_secs(60) {
            st.window_start = now;
            st.used = 0;
        }
        if st.used >= PER_MINUTE {
            return false;
        }
        st.used += 1;
        true
    }

    fn pause(&self, wait: Duration) {
        self.state().blocked_until = Some(Instant::now() + wait);
    }
}

/// Lookups, caches and limits. One per process.
pub struct Service {
    client: reqwest::Client,
    adsbdb: String,
    photos: String,
    cache: Mutex<HashMap<String, Cached>>,
    adsbdb_gate: Gate,
    photo_gate: Gate,
}

impl Service {
    /// The real services.
    #[must_use]
    pub fn production() -> Self {
        Self::with_bases(ADSBDB_BASE, PHOTOS_BASE)
    }

    /// Other base URLs (a test points them at a local server).
    #[must_use]
    pub fn with_bases(adsbdb: &str, photos: &str) -> Self {
        // reqwest is built without a bundled crypto provider; the workspace uses ring. Already
        // installed (by another component) is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "Superhet/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/BxnnyG/superhet)"
            ))
            // A redirect would move the request to a host nobody configured.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .unwrap_or_default();
        Self {
            client,
            adsbdb: adsbdb.trim_end_matches('/').to_owned(),
            photos: photos.trim_end_matches('/').to_owned(),
            cache: Mutex::new(HashMap::new()),
            adsbdb_gate: Gate::new(),
            photo_gate: Gate::new(),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, Cached>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cached(&self, key: &str) -> Option<Option<Value>> {
        let mut cache = self.cache();
        match cache.get(key) {
            Some(c) if c.until > Instant::now() => Some(c.value.clone()),
            Some(_) => {
                cache.remove(key);
                None
            }
            None => None,
        }
    }

    fn remember(&self, key: String, value: Option<Value>, ttl: Duration) {
        let mut cache = self.cache();
        if cache.len() >= MAX_ENTRIES {
            let now = Instant::now();
            cache.retain(|_, c| c.until > now);
            if cache.len() >= MAX_ENTRIES {
                // Still full of live entries: start over rather than grow without bound.
                cache.clear();
            }
        }
        cache.insert(
            key,
            Cached {
                value,
                until: Instant::now() + ttl,
            },
        );
    }

    /// One GET, cached. `parse` turns the service's JSON into what the UI gets (`None` = the
    /// service has nothing for this key).
    async fn fetch(
        &self,
        source: Source,
        key: String,
        url: String,
        ttl: Duration,
        parse: fn(&Value) -> Option<Value>,
    ) -> Fetched {
        if let Some(hit) = self.cached(&key) {
            return hit.map_or(Fetched::Missing, Fetched::Found);
        }
        let gate = match source {
            Source::Adsbdb => &self.adsbdb_gate,
            Source::Planespotters => &self.photo_gate,
        };
        if !gate.admit() {
            return Fetched::Limited;
        }
        let Ok(_permit) = gate.permits.acquire().await else {
            return Fetched::Unavailable;
        };
        let mut response = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(?source, %err, "online aircraft details: service unreachable");
                gate.pause(BACKOFF);
                return Fetched::Unavailable;
            }
        };
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            self.remember(key, None, TTL_MISS);
            return Fetched::Missing;
        }
        if !status.is_success() {
            tracing::warn!(?source, %status, "online aircraft details: service refused");
            let refused = matches!(status.as_u16(), 403 | 429);
            gate.pause(if refused { BACKOFF_REFUSED } else { BACKOFF });
            return if refused {
                Fetched::Limited
            } else {
                Fetched::Unavailable
            };
        }
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > MAX_BODY {
                        return Fetched::Unavailable;
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => return Fetched::Unavailable,
            }
        }
        let Ok(doc) = serde_json::from_slice::<Value>(&body) else {
            return Fetched::Unavailable;
        };
        match parse(&doc) {
            Some(value) => {
                self.remember(key, Some(value.clone()), ttl);
                Fetched::Found(value)
            }
            None => {
                self.remember(key, None, TTL_MISS);
                Fetched::Missing
            }
        }
    }

    async fn aircraft(&self, hex: &str) -> Fetched {
        self.fetch(
            Source::Adsbdb,
            format!("a:{hex}"),
            format!("{}/aircraft/{}", self.adsbdb, hex.to_ascii_uppercase()),
            TTL_AIRCRAFT,
            parse_aircraft,
        )
        .await
    }

    async fn flight(&self, callsign: &str) -> Fetched {
        self.fetch(
            Source::Adsbdb,
            format!("r:{callsign}"),
            format!("{}/callsign/{callsign}", self.adsbdb),
            TTL_ROUTE,
            parse_route,
        )
        .await
    }

    async fn photo(&self, hex: &str) -> Fetched {
        self.fetch(
            Source::Planespotters,
            format!("p:{hex}"),
            format!("{}/hex/{}", self.photos, hex.to_ascii_uppercase()),
            TTL_PHOTO,
            parse_photo,
        )
        .await
    }

    /// Everything the enabled sources know about one aircraft. Without any enabled source nothing
    /// is asked and nothing leaves the server.
    pub async fn lookup(&self, cfg: &OnlineSettings, hex: &str, callsign: Option<&str>) -> Value {
        let (aircraft, route, photo) = tokio::join!(
            async {
                if cfg.adsbdb_on() {
                    self.aircraft(hex).await
                } else {
                    Fetched::Skipped
                }
            },
            async {
                match callsign {
                    Some(cs) if cfg.adsbdb_on() => self.flight(cs).await,
                    _ => Fetched::Skipped,
                }
            },
            async {
                if cfg.planespotters_on() {
                    self.photo(hex).await
                } else {
                    Fetched::Skipped
                }
            },
        );
        let adsbdb_state = if cfg.adsbdb_on() {
            combine(&[&aircraft, &route])
        } else {
            "off"
        };
        let photo_state = if cfg.planespotters_on() {
            combine(&[&photo])
        } else {
            "off"
        };
        let value = |f: Fetched| match f {
            Fetched::Found(v) => v,
            _ => Value::Null,
        };
        json!({
            "v": 1,
            "enabled": cfg.enabled,
            "sources": {"adsbdb": adsbdb_state, "planespotters": photo_state},
            "aircraft": value(aircraft),
            "route": value(route),
            "photo": value(photo),
        })
    }
}

/// `ok`, `not_found`, `rate_limited` or `unavailable` from the lookups of one source.
fn combine(parts: &[&Fetched]) -> &'static str {
    let asked: Vec<&&Fetched> = parts
        .iter()
        .filter(|f| !matches!(***f, Fetched::Skipped))
        .collect();
    if asked.iter().any(|f| matches!(***f, Fetched::Found(_))) {
        "ok"
    } else if asked.iter().any(|f| matches!(***f, Fetched::Limited)) {
        "rate_limited"
    } else if asked.iter().any(|f| matches!(***f, Fetched::Unavailable)) {
        "unavailable"
    } else {
        "not_found"
    }
}

// ---- parsing: only fields we use, bounded, control characters removed -----------------------

fn text(v: &Value, key: &str, max: usize) -> Option<String> {
    let s: String = v
        .get(key)?
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let s = s.trim();
    (!s.is_empty() && s.chars().count() <= max).then(|| s.to_owned())
}

fn number(v: &Value, key: &str, min: f64, max: f64) -> Option<f64> {
    v.get(key)?
        .as_f64()
        .filter(|n| n.is_finite() && (min..=max).contains(n))
}

fn put(out: &mut Map<String, Value>, key: &str, value: Option<impl Into<Value>>) {
    if let Some(v) = value {
        out.insert(key.to_owned(), v.into());
    }
}

fn parse_aircraft(doc: &Value) -> Option<Value> {
    let a = doc.get("response")?.get("aircraft")?;
    let mut out = Map::new();
    put(&mut out, "registration", text(a, "registration", 16));
    put(&mut out, "type", text(a, "type", 80));
    put(&mut out, "icao_type", text(a, "icao_type", 8));
    put(&mut out, "manufacturer", text(a, "manufacturer", 80));
    put(&mut out, "owner", text(a, "registered_owner", 120));
    put(
        &mut out,
        "owner_country",
        text(a, "registered_owner_country_name", 80),
    );
    put(
        &mut out,
        "owner_country_iso",
        text(a, "registered_owner_country_iso_name", 3),
    );
    put(
        &mut out,
        "operator_code",
        text(a, "registered_owner_operator_flag_code", 8),
    );
    (!out.is_empty()).then_some(Value::Object(out))
}

fn parse_airport(a: &Value) -> Option<Value> {
    let name = text(a, "name", 120)?;
    let mut out = Map::new();
    out.insert("name".into(), name.into());
    put(&mut out, "icao", text(a, "icao_code", 4));
    put(&mut out, "iata", text(a, "iata_code", 3));
    put(&mut out, "city", text(a, "municipality", 80));
    put(&mut out, "country", text(a, "country_name", 80));
    put(&mut out, "country_iso", text(a, "country_iso_name", 3));
    put(&mut out, "lat", number(a, "latitude", -90.0, 90.0));
    put(&mut out, "lon", number(a, "longitude", -180.0, 180.0));
    put(
        &mut out,
        "elevation_ft",
        number(a, "elevation", -2000.0, 30000.0),
    );
    Some(Value::Object(out))
}

fn parse_route(doc: &Value) -> Option<Value> {
    let r = doc.get("response")?.get("flightroute")?;
    let origin = parse_airport(r.get("origin")?)?;
    let destination = parse_airport(r.get("destination")?)?;
    let mut out = Map::new();
    out.insert("origin".into(), origin);
    out.insert("destination".into(), destination);
    if let Some(mid) = r.get("midpoint").and_then(parse_airport) {
        out.insert("midpoint".into(), mid);
    }
    put(&mut out, "callsign", text(r, "callsign", 10));
    put(&mut out, "callsign_iata", text(r, "callsign_iata", 10));
    if let Some(al) = r.get("airline").filter(|v| v.is_object()) {
        let mut airline = Map::new();
        put(&mut airline, "name", text(al, "name", 80));
        put(&mut airline, "icao", text(al, "icao", 4));
        put(&mut airline, "iata", text(al, "iata", 3));
        put(&mut airline, "country", text(al, "country", 80));
        put(&mut airline, "callsign", text(al, "callsign", 40));
        if !airline.is_empty() {
            out.insert("airline".into(), Value::Object(airline));
        }
    }
    Some(Value::Object(out))
}

/// `true` for an `https` URL on exactly one of `hosts`, without credentials or a port.
fn url_on(url: &str, hosts: &[&str]) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.port().is_none()
            && u.host_str().is_some_and(|h| hosts.contains(&h))
    })
}

fn parse_photo(doc: &Value) -> Option<Value> {
    let p = doc.get("photos")?.as_array()?.first()?;
    let large = p.get("thumbnail_large").filter(|t| t.is_object());
    let thumb = large.or_else(|| p.get("thumbnail"))?;
    let src = text(thumb, "src", 300).filter(|s| url_on(s, &PHOTO_HOSTS))?;
    // The link back to the photo page is part of the terms: without a valid one there is no photo.
    let link = text(p, "link", 300).filter(|l| url_on(l, &[PHOTO_PAGE_HOST]))?;
    let photographer = text(p, "photographer", 100)?;
    let size = thumb.get("size");
    let dim = |k: &str| {
        size.and_then(|s| number(s, k, 1.0, 4000.0))
            .map(|n| n.round() as u32)
    };
    let mut out = Map::new();
    out.insert("src".into(), src.into());
    out.insert("link".into(), link.into());
    out.insert("photographer".into(), photographer.into());
    put(&mut out, "width", dim("width"));
    put(&mut out, "height", dim("height"));
    Some(Value::Object(out))
}

// ---- HTTP ------------------------------------------------------------------------------------

/// Shared state of the routes.
#[derive(Clone)]
pub struct OnlineState {
    devices: DevicesState,
    service: Arc<Service>,
}

impl OnlineState {
    #[must_use]
    pub fn new(devices: DevicesState) -> Self {
        Self::with_service(devices, Arc::new(Service::production()))
    }

    #[must_use]
    pub fn with_service(devices: DevicesState, service: Arc<Service>) -> Self {
        Self { devices, service }
    }

    async fn settings(&self) -> (OnlineSettings, bool) {
        effective(self.devices.auth.settings().adsb_online().await)
    }
}

/// Routes, behind the access middleware.
pub fn router(state: OnlineState) -> Router {
    Router::new()
        .route("/api/v1/adsb/online", get(details))
        .route("/api/v1/admin/adsb/online", get(admin_get).put(admin_put))
        .with_state(state)
}

fn invalid(message: &str) -> Response {
    failure(StatusCode::BAD_REQUEST, "invalid", message)
}

#[derive(Deserialize)]
struct DetailsQuery {
    #[serde(default)]
    icao: String,
    #[serde(default)]
    callsign: String,
}

/// An ICAO callsign as ADS-B sends it: 2 to 8 letters and digits.
fn valid_callsign(s: &str) -> bool {
    (2..=8).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

async fn details(State(state): State<OnlineState>, Query(q): Query<DetailsQuery>) -> Response {
    let icao = q.icao.trim().to_ascii_lowercase();
    if icao.len() != 6 || !icao.bytes().all(|b| b.is_ascii_hexdigit()) {
        return invalid("icao must be six hex digits");
    }
    let callsign = q.callsign.trim().to_ascii_uppercase();
    if !callsign.is_empty() && !valid_callsign(&callsign) {
        return invalid("callsign must be 2 to 8 letters or digits");
    }
    let (cfg, locked) = state.settings().await;
    let mut body = state
        .service
        .lookup(
            &cfg,
            &icao,
            (!callsign.is_empty()).then_some(callsign.as_str()),
        )
        .await;
    body["locked"] = Value::Bool(locked);
    json(StatusCode::OK, body.to_string())
}

fn source_info() -> Value {
    json!([
        {
            "id": "adsbdb",
            "name": "adsbdb.com",
            "url": "https://www.adsbdb.com",
            "api": "https://api.adsbdb.com",
            "provides": "full type name, registered owner, route (origin and destination) and airline",
            "sends": "the aircraft's ICAO address and callsign",
        },
        {
            "id": "planespotters",
            "name": "Planespotters.net",
            "url": "https://www.planespotters.net",
            "api": "https://api.planespotters.net",
            "provides": "a photo of the aircraft with the photographer's credit",
            "sends": "the aircraft's ICAO address; each viewer's browser then loads the image from Planespotters' image servers",
        },
    ])
}

async fn admin_get(State(state): State<OnlineState>) -> Response {
    let stored = state.devices.auth.settings().adsb_online().await;
    let (eff, locked) = effective(stored);
    json(
        StatusCode::OK,
        json!({
            "v": 1,
            "settings": stored,
            "effective": eff,
            "locked": locked,
            "env": ENV_OVERRIDE,
            "sources": source_info(),
        })
        .to_string(),
    )
}

async fn admin_put(
    State(state): State<OnlineState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, true) {
        return *r;
    }
    let Ok(mut settings) = serde_json::from_slice::<OnlineSettings>(&body) else {
        return invalid("expected {\"enabled\":bool,\"adsbdb\":bool,\"planespotters\":bool}");
    };
    // Master switch on with no source is the same as off.
    settings.enabled = settings.enabled && (settings.adsbdb || settings.planespotters);
    if state
        .devices
        .auth
        .settings()
        .put_adsb_online(&settings, &principal.name)
        .await
        .is_err()
    {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "settings could not be saved",
        );
    }
    state
        .devices
        .auth
        .audit()
        .log(
            &principal.who(),
            "settings.change",
            Some("adsb_online"),
            Outcome::Ok,
            json!({
                "enabled": settings.enabled,
                "adsbdb": settings.adsbdb,
                "planespotters": settings.planespotters,
            }),
        )
        .await;
    let (eff, locked) = effective(settings);
    json(
        StatusCode::OK,
        json!({"v": 1, "ok": true, "settings": settings, "effective": eff, "locked": locked})
            .to_string(),
    )
}

#[cfg(test)]
mod tests;
