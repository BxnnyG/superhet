//! Weather satellites: pass prediction, element sets and the image gallery (ADR-0034,
//! `docs/api-weather.md`, `docs/plugins-weather.md`).
//!
//! - `GET /api/v1/weather/config`: what is known about the satellites, the element sets and the
//!   store.
//! - `POST /api/v1/weather/passes`: upcoming passes over the receiver (position from the request
//!   or the one ADS-B already has; it is only used here, never sent anywhere).
//! - `GET /api/v1/weather/captures`, `GET|PATCH|DELETE /api/v1/weather/captures/{id}` and
//!   `GET .../captures/{id}/images/{name}`: the gallery the weather plugin fills.
//! - `PUT /api/v1/admin/weather/elements`, `POST .../elements/refresh`, `POST .../elements/manual`:
//!   the admin's say over where element sets come from.
//!
//! Roles follow the recordings: accounts (not guests) see the gallery of the devices they may
//! view; operators and admins with control of the device keep and delete.

pub mod elements;
pub mod store;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;
use superhet_db::PgPool;
use superhet_satpass::{Observer, Satellite, catalogue, passes};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::access::Principal;
use crate::device_access::{self, Need};
use crate::devices::{DevicesState, failure, guard_mutation, json};
use elements::Doc;
use store::Capture;

const MAX_BODY_BYTES: usize = 8 * 1024;
const MAX_MANUAL_BODY_BYTES: usize = elements::MAX_BYTES + 4096;
const DEFAULT_LIMIT: usize = 48;
const MAX_LIMIT: usize = 200;
/// Passes enriched with their maximum elevation per listing.
const MAX_ENRICH: usize = 24;
/// How often the background task trims the store and looks at the element sets.
const HOUSEKEEPING: std::time::Duration = std::time::Duration::from_secs(300);
/// Default size limit of the gallery in GB (`SUPERHET_WEATHER_MAX_GB`, 0 = none).
const DEFAULT_MAX_GB: f64 = 5.0;

/// Shared state of the weather routes.
#[derive(Clone)]
pub struct WeatherState {
    devices: DevicesState,
    db: Option<PgPool>,
    dir: PathBuf,
    max_bytes: u64,
    /// What "download" fetches (a test points it at a local server).
    url: String,
}

impl WeatherState {
    /// Create the state; the store is `SUPERHET_WEATHER_DIR` (default `weather` inside the
    /// recordings directory) with at most `SUPERHET_WEATHER_MAX_GB`.
    #[must_use]
    pub fn new(devices: DevicesState, db: Option<PgPool>) -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let dir = env("SUPERHET_WEATHER_DIR").map_or_else(
            || {
                PathBuf::from(
                    env("SUPERHET_RECORDINGS_DIR").unwrap_or_else(|| "/data/recordings".into()),
                )
                .join("weather")
            },
            PathBuf::from,
        );
        let gb = env("SUPERHET_WEATHER_MAX_GB")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(DEFAULT_MAX_GB);
        Self {
            devices,
            db,
            dir,
            max_bytes: (gb * 1024.0 * 1024.0 * 1024.0) as u64,
            url: elements::SOURCE_URL.to_owned(),
        }
    }

    /// Trim the gallery to its quota and refresh element sets when the admin allowed it.
    pub fn spawn_background(&self) {
        let state = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(HOUSEKEEPING);
            loop {
                tick.tick().await;
                state.housekeeping().await;
            }
        });
    }

    async fn housekeeping(&self) {
        let (dir, max) = (self.dir.clone(), self.max_bytes);
        if let Ok((_, removed)) =
            tokio::task::spawn_blocking(move || store::enforce_quota(&dir, max)).await
            && !removed.is_empty()
        {
            tracing::info!(
                removed = removed.len(),
                "weather gallery trimmed to its quota"
            );
        }
        if let Some(pool) = &self.db {
            let doc = load_doc(pool).await;
            if doc.due(now_ms()) {
                let _ = self.download(pool, doc, "system").await;
            }
        }
    }

    /// Fetch element sets and store the outcome (success or the reason).
    async fn download(&self, pool: &PgPool, mut doc: Doc, who: &str) -> Result<usize, String> {
        let now = now_ms();
        doc.attempt_ms = Some(now);
        let result = elements::fetch(&self.url).await;
        let outcome = match &result {
            Ok(sets) => {
                doc.set_elements(sets, "download", now);
                Ok(sets.len())
            }
            Err(reason) => {
                doc.error = Some(reason.clone());
                Err(reason.clone())
            }
        };
        if let Err(err) =
            superhet_db::settings::put(pool, elements::SETTINGS_KEY, &doc.to_value(), who).await
        {
            tracing::error!(%err, "storing the element sets failed");
        }
        outcome
    }
}

/// Routes, behind the authentication hook.
pub fn router(state: WeatherState) -> Router {
    Router::new()
        .route("/api/v1/weather/config", get(config))
        .route(
            "/api/v1/weather/passes",
            post(predict).layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .route("/api/v1/weather/captures", get(list))
        .route(
            "/api/v1/weather/captures/{id}",
            get(show).patch(update).delete(remove),
        )
        .route("/api/v1/weather/captures/{id}/images/{name}", get(image))
        .route(
            "/api/v1/admin/weather/elements",
            axum::routing::put(set_enabled).layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .route("/api/v1/admin/weather/elements/refresh", post(refresh))
        .route(
            "/api/v1/admin/weather/elements/manual",
            post(manual).layer(DefaultBodyLimit::max(MAX_MANUAL_BODY_BYTES)),
        )
        .with_state(state)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn invalid(message: &str) -> Response {
    failure(StatusCode::BAD_REQUEST, "invalid", message)
}

fn unavailable() -> Response {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "no database",
    )
}

fn not_found() -> Response {
    failure(StatusCode::NOT_FOUND, "not_found", "no such capture")
}

async fn load_doc(pool: &PgPool) -> Doc {
    Doc::from_value(
        superhet_db::settings::get(pool, elements::SETTINGS_KEY)
            .await
            .ok()
            .flatten(),
    )
}

/// The element sets as the API shows them (no orbital numbers, only what the UI needs).
fn elements_view(doc: &Doc, now: i64) -> Value {
    let sats: Vec<Value> = doc
        .elements()
        .iter()
        .filter_map(|e| {
            let sat = Satellite::from_elements(e).ok()?;
            Some(json!({"norad_id": e.norad_id, "name": catalogue::by_norad(e.norad_id).map(|c| c.name),
                "epoch_ms": sat.epoch_ms()}))
        })
        .collect();
    json!({
        "enabled": doc.enabled,
        "source": doc.source,
        "fetched_ms": doc.fetched_ms,
        "attempt_ms": doc.attempt_ms,
        "error": doc.error,
        "wait_s": doc.wait_ms(now) / 1000,
        "satellites": sats,
        "download": {
            "name": elements::SOURCE_NAME,
            "url": elements::SOURCE_URL,
            "info_url": elements::SOURCE_INFO_URL,
            "min_interval_s": elements::MIN_INTERVAL_MS / 1000,
        },
    })
}

async fn config(
    State(state): State<WeatherState>,
    Extension(_principal): Extension<Principal>,
) -> Response {
    let now = now_ms();
    let doc = match &state.db {
        Some(pool) => load_doc(pool).await,
        None => Doc::default(),
    };
    let (dir, max) = (state.dir.clone(), state.max_bytes);
    let (count, used) = tokio::task::spawn_blocking(move || {
        let all = store::scan(&dir);
        (all.len(), all.iter().map(|c| c.size_bytes).sum::<u64>())
    })
    .await
    .unwrap_or((0, 0));
    json(
        StatusCode::OK,
        json!({
            "v": 1,
            "satellites": catalogue::CATALOGUE,
            "checked": "2026-11-23",
            "elements": elements_view(&doc, now),
            "store": {"available": state.dir.is_dir(), "captures": count, "used_bytes": used, "max_bytes": max},
        })
        .to_string(),
    )
}

// ---- pass prediction ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PassesBody {
    /// Device whose ADS-B receiver position is used when no `lat`/`lon` is given.
    dev: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    hours: Option<u32>,
    min_elevation_deg: Option<f64>,
}

/// The receiver position: from the request, else the one ADS-B stored for `dev`.
async fn observer_for(
    state: &WeatherState,
    dev: Option<&str>,
    lat: Option<f64>,
    lon: Option<f64>,
) -> Result<Option<(Observer, &'static str)>, Box<Response>> {
    if let (Some(lat), Some(lon)) = (lat, lon) {
        return Observer::new(lat, lon)
            .map(|o| Some((o, "request")))
            .map_err(|e| Box::new(invalid(&e.to_string())));
    }
    if lat.is_some() || lon.is_some() {
        return Err(Box::new(invalid("give both lat and lon, or neither")));
    }
    let (Some(pool), Some(dev)) = (&state.db, dev) else {
        return Ok(None);
    };
    match superhet_db::adsb::range_summary(pool, dev).await {
        Ok(summary) => Ok(summary
            .receiver
            .and_then(|(lat, lon)| Observer::new(lat, lon).ok())
            .map(|o| (o, "adsb"))),
        Err(err) => {
            tracing::error!(%err, "reading the receiver position failed");
            Err(Box::new(failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "database error",
            )))
        }
    }
}

async fn predict(
    State(state): State<WeatherState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, true) {
        return *r;
    }
    let Ok(request) = serde_json::from_slice::<PassesBody>(&body) else {
        return invalid(
            "body must be {\"dev\"?:string,\"lat\"?:number,\"lon\"?:number,\"hours\"?:int,\"min_elevation_deg\"?:number}",
        );
    };
    let hours = request.hours.unwrap_or(24);
    if !(1..=superhet_satpass::MAX_WINDOW_HOURS.min(72)).contains(&hours) {
        return invalid("hours must be 1 to 72");
    }
    let min_el = request.min_elevation_deg.unwrap_or(10.0);
    if !(0.0..=85.0).contains(&min_el) {
        return invalid("min_elevation_deg must be 0 to 85");
    }
    if let Some(dev) = request.dev.as_deref()
        && let Err(r) = device_access::check(
            &state.devices.auth,
            &principal,
            dev,
            Need::View,
            "POST weather passes",
        )
        .await
    {
        return *r;
    }
    let observer =
        match observer_for(&state, request.dev.as_deref(), request.lat, request.lon).await {
            Ok(o) => o,
            Err(r) => return *r,
        };
    let doc = match &state.db {
        Some(pool) => load_doc(pool).await,
        None => return unavailable(),
    };
    let sets = doc.elements();
    if sets.is_empty() {
        return failure(
            StatusCode::CONFLICT,
            "no_elements",
            "no element sets: an admin has to allow the download or paste them (Weather tab)",
        );
    }
    let Some((observer, source)) = observer else {
        return failure(
            StatusCode::CONFLICT,
            "no_position",
            "no receiver position: start ADS-B with the position, or enter it here",
        );
    };
    let now = now_ms();
    let out = upcoming(&sets, observer, now, hours, min_el);
    json(
        StatusCode::OK,
        json!({
            "v": 1, "generated_ms": now, "position_from": source,
            "elements": {"source": doc.source, "fetched_ms": doc.fetched_ms},
            "passes": out,
        })
        .to_string(),
    )
}

/// The passes of the operational satellites in the next `hours`, soonest first.
fn upcoming(
    sets: &[superhet_satpass::Elements],
    observer: Observer,
    now: i64,
    hours: u32,
    min_el: f64,
) -> Vec<Value> {
    let until = now + i64::from(hours) * 3_600_000;
    let mut out: Vec<(i64, Value)> = Vec::new();
    for entry in catalogue::operational() {
        let Some(set) = sets.iter().find(|s| s.norad_id == entry.norad_id) else {
            continue;
        };
        let Ok(sat) = Satellite::from_elements(set) else {
            continue;
        };
        let age_h = (now - sat.epoch_ms()) as f64 / 3_600_000.0;
        for pass in passes(&sat, observer, now, until, min_el) {
            out.push((
                pass.aos_ms,
                json!({
                    "satellite": entry.name, "norad_id": entry.norad_id, "format": entry.format,
                    "aos_ms": pass.aos_ms, "tca_ms": pass.tca_ms, "los_ms": pass.los_ms,
                    "max_elevation_deg": (pass.max_elevation_deg * 10.0).round() / 10.0,
                    "aos_azimuth_deg": pass.aos_azimuth_deg.round(),
                    "tca_azimuth_deg": pass.tca_azimuth_deg.round(),
                    "los_azimuth_deg": pass.los_azimuth_deg.round(),
                    "in_progress": pass.in_progress,
                    "downlinks": entry.downlinks,
                    "elements_age_h": (age_h * 10.0).round() / 10.0,
                }),
            ));
        }
    }
    out.sort_by_key(|(aos, _)| *aos);
    out.into_iter().map(|(_, p)| p).collect()
}

// ---- gallery -----------------------------------------------------------------------------

/// A capture as the API shows it.
fn capture_view(c: &Capture) -> Value {
    let d = &c.doc;
    json!({
        "id": c.id,
        "satellite": d["satellite"],
        "norad_id": d["satellite"].as_str().and_then(catalogue::by_name).map(|e| e.norad_id),
        "pipeline": d["pipeline"],
        "started_ms": d["started_ms"],
        "duration_s": d["duration_s"],
        "frequency_hz": d["frequency_hz"],
        "device": d["device"],
        "max_elevation_deg": d["max_elevation_deg"],
        "peak_db": d["peak_db"],
        "keep": c.keep(),
        "size_bytes": c.size_bytes,
        "images": d["images"],
    })
}

/// Check that `principal` may use the capture's device (captures without a device need only an
/// account).
async fn allow(
    state: &WeatherState,
    principal: &Principal,
    capture: &Capture,
    need: Need,
    action: &str,
) -> Result<(), Box<Response>> {
    match capture.device() {
        Some(dev) => device_access::check(&state.devices.auth, principal, dev, need, action).await,
        None => Ok(()),
    }
}

async fn find(state: &WeatherState, id: &str) -> Option<Capture> {
    let (dir, id) = (state.dir.clone(), id.to_owned());
    tokio::task::spawn_blocking(move || store::read(&dir, &id))
        .await
        .ok()
        .flatten()
}

/// Maximum elevation of the pass that matches the capture's time, from the element sets and the
/// receiver position; `Some(None)` when computed and there is no matching pass.
fn elevation_of(
    capture: &Capture,
    sets: &[superhet_satpass::Elements],
    observer: Observer,
) -> Option<Option<f64>> {
    let entry = capture.doc["satellite"]
        .as_str()
        .and_then(catalogue::by_name)?;
    let set = sets.iter().find(|s| s.norad_id == entry.norad_id)?;
    let sat = Satellite::from_elements(set).ok()?;
    let start = capture.started_ms();
    let end = capture.doc["ended_ms"].as_i64().unwrap_or(start);
    let found = passes(&sat, observer, start - 15 * 60_000, end + 15 * 60_000, 0.0)
        .into_iter()
        .filter(|p| p.los_ms >= start && p.aos_ms <= end)
        .max_by(|a, b| a.max_elevation_deg.total_cmp(&b.max_elevation_deg));
    Some(found.map(|p| (p.max_elevation_deg * 10.0).round() / 10.0))
}

/// Fill in `max_elevation_deg` of captures that lack it, where the element sets and a receiver
/// position are known. Written back to `pass.json` so it is computed once.
async fn enrich(state: &WeatherState, captures: &mut [Capture]) {
    let Some(pool) = &state.db else {
        return;
    };
    let wanted: Vec<usize> = captures
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.doc["max_elevation_deg"].is_null() && c.doc["elevation_checked"].is_null()
        })
        .map(|(i, _)| i)
        .take(MAX_ENRICH)
        .collect();
    if wanted.is_empty() {
        return;
    }
    let sets = load_doc(pool).await.elements();
    if sets.is_empty() {
        return;
    }
    for i in wanted {
        let Some(dev) = captures[i].device().map(str::to_owned) else {
            continue;
        };
        let Ok(Some(observer)) = observer_for(state, Some(&dev), None, None)
            .await
            .map(|o| o.map(|(o, _)| o))
        else {
            continue;
        };
        let Some(result) = elevation_of(&captures[i], &sets, observer) else {
            continue;
        };
        let (dir, id) = (state.dir.clone(), captures[i].id.clone());
        let fields = match result {
            Some(el) => vec![("max_elevation_deg", json!(el))],
            None => vec![("elevation_checked", json!(true))],
        };
        let applied: Vec<(&'static str, Value)> = fields.clone();
        let wrote = tokio::task::spawn_blocking(move || store::set_fields(&dir, &id, &applied))
            .await
            .unwrap_or(false);
        // Show the value even when the directory cannot be written.
        let _ = wrote;
        for (key, value) in fields {
            captures[i].doc[key] = value;
        }
    }
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    dev: Option<String>,
    limit: Option<usize>,
    /// Only passes that started before this time (Unix milliseconds), for paging.
    before_ms: Option<i64>,
}

async fn list(
    State(state): State<WeatherState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ListQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let dir = state.dir.clone();
    let all = tokio::task::spawn_blocking(move || store::scan(&dir))
        .await
        .unwrap_or_default();
    let mut shown: Vec<Capture> = Vec::new();
    let mut allowed: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut more = false;
    for capture in all {
        if query.before_ms.is_some_and(|b| capture.started_ms() >= b) {
            continue;
        }
        if let Some(dev) = query.dev.as_deref()
            && capture.device() != Some(dev)
        {
            continue;
        }
        if let Some(dev) = capture.device() {
            let ok = match allowed.get(dev) {
                Some(ok) => *ok,
                None => {
                    let ok = device_access::check(
                        &state.devices.auth,
                        &principal,
                        dev,
                        Need::View,
                        "GET weather captures",
                    )
                    .await
                    .is_ok();
                    allowed.insert(dev.to_owned(), ok);
                    ok
                }
            };
            if !ok {
                continue;
            }
        }
        if shown.len() == limit {
            more = true;
            break;
        }
        shown.push(capture);
    }
    enrich(&state, &mut shown).await;
    let next_before = more
        .then(|| shown.last().map(Capture::started_ms))
        .flatten();
    json(
        StatusCode::OK,
        json!({
            "v": 1,
            "captures": shown.iter().map(capture_view).collect::<Vec<_>>(),
            "next_before_ms": next_before,
        })
        .to_string(),
    )
}

async fn show(
    State(state): State<WeatherState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Response {
    let Some(capture) = find(&state, &id).await else {
        return not_found();
    };
    if let Err(r) = allow(
        &state,
        &principal,
        &capture,
        Need::View,
        "GET weather capture",
    )
    .await
    {
        return *r;
    }
    let mut one = [capture];
    enrich(&state, &mut one).await;
    json(StatusCode::OK, capture_view(&one[0]).to_string())
}

async fn image(
    State(state): State<WeatherState>,
    Extension(principal): Extension<Principal>,
    Path((id, name)): Path<(String, String)>,
    Query(query): Query<ImageQuery>,
    request: Request<Body>,
) -> Response {
    let Some(capture) = find(&state, &id).await else {
        return not_found();
    };
    if let Err(r) = allow(
        &state,
        &principal,
        &capture,
        Need::View,
        "GET weather image",
    )
    .await
    {
        return *r;
    }
    // `?thumb=true` asks for the thumbnail of image `name` (the full image when it has none).
    let wanted = if query.thumb == Some(true) {
        capture.doc["images"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["name"].as_str() == Some(&name)))
            .and_then(|i| i["thumb"].as_str().map(str::to_owned))
            .unwrap_or_else(|| name.clone())
    } else {
        name.clone()
    };
    let Some(path) = store::file_path(&state.dir, &capture, &wanted) else {
        return failure(StatusCode::NOT_FOUND, "not_found", "no such image");
    };
    let Ok(mime_type) = "image/png".parse() else {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "bad media type",
        );
    };
    let response = match ServeFile::new_with_mime(&path, &mime_type)
        .oneshot(request)
        .await
    {
        Ok(response) => response,
        Err(never) => match never {},
    };
    if response.status() == StatusCode::NOT_FOUND {
        return failure(StatusCode::NOT_FOUND, "not_found", "the file is gone");
    }
    let mut response = response.map(Body::new);
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    if query.download == Some(true)
        && let Ok(value) = HeaderValue::from_str(&format!(
            "attachment; filename=\"{}-{}\"",
            capture.id,
            name.replace('"', "")
        ))
    {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

#[derive(Debug, Deserialize)]
struct ImageQuery {
    thumb: Option<bool>,
    download: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    keep: Option<bool>,
}

async fn update(
    State(state): State<WeatherState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, true) {
        return *r;
    }
    let Ok(UpdateBody { keep: Some(keep) }) = serde_json::from_slice::<UpdateBody>(&body) else {
        return invalid("body must be {\"keep\":bool}");
    };
    let Some(capture) = find(&state, &id).await else {
        return not_found();
    };
    if let Err(r) = allow(
        &state,
        &principal,
        &capture,
        Need::Control,
        "PATCH weather capture",
    )
    .await
    {
        return *r;
    }
    let (dir, cid) = (state.dir.clone(), id.clone());
    let ok = tokio::task::spawn_blocking(move || {
        store::set_fields(&dir, &cid, &[("keep", json!(keep))])
    })
    .await
    .unwrap_or(false);
    if !ok {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "cannot write the capture",
        );
    }
    state
        .devices
        .auth
        .audit()
        .log(
            &principal.who(),
            "weather.keep",
            capture.device(),
            Outcome::Ok,
            json!({"id": id, "keep": keep}),
        )
        .await;
    match find(&state, &id).await {
        Some(c) => json(StatusCode::OK, capture_view(&c).to_string()),
        None => not_found(),
    }
}

async fn remove(
    State(state): State<WeatherState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, false) {
        return *r;
    }
    let Some(capture) = find(&state, &id).await else {
        return not_found();
    };
    if let Err(r) = allow(
        &state,
        &principal,
        &capture,
        Need::Control,
        "DELETE weather capture",
    )
    .await
    {
        return *r;
    }
    let (dir, cid) = (state.dir.clone(), id.clone());
    if !tokio::task::spawn_blocking(move || store::delete(&dir, &cid))
        .await
        .unwrap_or(false)
    {
        return not_found();
    }
    state
        .devices
        .auth
        .audit()
        .log(
            &principal.who(),
            "weather.delete",
            capture.device(),
            Outcome::Ok,
            json!({"id": id, "satellite": capture.doc["satellite"]}),
        )
        .await;
    json(StatusCode::OK, json!({"v": 1, "ok": true}).to_string())
}

// ---- element sets (admin) ----------------------------------------------------------------

async fn audit_elements(state: &WeatherState, principal: &Principal, what: &str, extra: Value) {
    state
        .devices
        .auth
        .audit()
        .log(
            &principal.who(),
            "weather.elements",
            None,
            Outcome::Ok,
            json!({"what": what, "info": extra}),
        )
        .await;
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnabledBody {
    enabled: bool,
}

/// Switch the download on or off. Switching on fetches at once (when allowed by the interval).
async fn set_enabled(
    State(state): State<WeatherState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, true) {
        return *r;
    }
    let Ok(request) = serde_json::from_slice::<EnabledBody>(&body) else {
        return invalid("body must be {\"enabled\":bool}");
    };
    let Some(pool) = &state.db else {
        return unavailable();
    };
    let mut doc = load_doc(pool).await;
    doc.enabled = request.enabled;
    if let Err(err) = superhet_db::settings::put(
        pool,
        elements::SETTINGS_KEY,
        &doc.to_value(),
        &principal.name,
    )
    .await
    {
        tracing::error!(%err, "storing the element settings failed");
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "database error",
        );
    }
    audit_elements(
        &state,
        &principal,
        if request.enabled {
            "download_on"
        } else {
            "download_off"
        },
        Value::Null,
    )
    .await;
    if request.enabled && doc.wait_ms(now_ms()) == 0 {
        let _ = state.download(pool, doc, &principal.name).await;
    }
    let doc = load_doc(pool).await;
    json(
        StatusCode::OK,
        json!({"v": 1, "elements": elements_view(&doc, now_ms())}).to_string(),
    )
}

async fn refresh(
    State(state): State<WeatherState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, false) {
        return *r;
    }
    let Some(pool) = &state.db else {
        return unavailable();
    };
    let doc = load_doc(pool).await;
    if !doc.enabled {
        return failure(
            StatusCode::CONFLICT,
            "download_off",
            "the download is switched off",
        );
    }
    let wait = doc.wait_ms(now_ms());
    if wait > 0 {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "too_soon",
            &format!(
                "CelesTrak asks for at most one download every 2 hours; try again in {} min",
                wait / 60_000 + 1
            ),
        );
    }
    let result = state.download(pool, doc, &principal.name).await;
    audit_elements(&state, &principal, "refresh", json!({"ok": result.is_ok()})).await;
    if let Err(reason) = result {
        return failure(StatusCode::BAD_GATEWAY, "download_failed", &reason);
    }
    let doc = load_doc(pool).await;
    json(
        StatusCode::OK,
        json!({"v": 1, "elements": elements_view(&doc, now_ms())}).to_string(),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManualBody {
    text: String,
}

async fn manual(
    State(state): State<WeatherState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, true) {
        return *r;
    }
    let Ok(request) = serde_json::from_slice::<ManualBody>(&body) else {
        return invalid("body must be {\"text\":\"TLE or OMM JSON\"}");
    };
    let sets = match elements::parse_manual(&request.text) {
        Ok(s) => s,
        Err(reason) => return invalid(&reason),
    };
    let Some(pool) = &state.db else {
        return unavailable();
    };
    let mut doc = load_doc(pool).await;
    doc.set_elements(&sets, "manual", now_ms());
    if let Err(err) = superhet_db::settings::put(
        pool,
        elements::SETTINGS_KEY,
        &doc.to_value(),
        &principal.name,
    )
    .await
    {
        tracing::error!(%err, "storing the element sets failed");
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "database error",
        );
    }
    audit_elements(
        &state,
        &principal,
        "manual",
        json!({"satellites": sets.len()}),
    )
    .await;
    json(
        StatusCode::OK,
        json!({"v": 1, "elements": elements_view(&doc, now_ms())}).to_string(),
    )
}

#[cfg(test)]
mod tests;
