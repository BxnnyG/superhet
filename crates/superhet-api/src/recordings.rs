//! Recordings and instant replay over REST (ADR-0028, `docs/recordings.md`, `docs/recordings.md`).
//!
//! - `GET /api/v1/recordings`, `GET /api/v1/recordings/config`, `GET|PATCH|DELETE
//!   /api/v1/recordings/{id}`, `GET /api/v1/recordings/{id}/file` (Range, download),
//!   `POST /api/v1/recordings/{id}/stop`
//! - `POST /api/v1/devices/{dev}/recordings` starts an audio or IQ recording
//! - `GET /api/v1/devices/{dev}/vfos/{id}/replay` plays the instant replay,
//!   `POST .../replay/save` turns it into a recording
//!
//! Roles: accounts (not guests) list, play and download what the device policy lets them see;
//! operators and admins start, stop, rename, keep and delete, and need control of the device for
//! that. Files are addressed by database id only; the path is rebuilt from a generated stem.

use std::sync::Arc;

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
use superhet_db::recordings as db;
use superhet_recording::files::{self, Kind, Part};
use superhet_recording::sigmf::Datatype;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::access::Principal;
use crate::device_access::{self, Need, Subject};
use crate::devices::{DevicesState, ctl_state, failure, guard_mutation, json};
use crate::recorder::{
    self, AudioStart, IqSource, IqStart, Manager, Owner, SaveReplay, StartError, Stop,
};

/// Largest accepted request body.
const MAX_BODY_BYTES: usize = 4096;
const MAX_TITLE_CHARS: usize = 200;
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

/// Shared state of the recording routes.
#[derive(Clone)]
pub struct RecordingsState {
    devices: DevicesState,
    mgr: Arc<Manager>,
}

impl RecordingsState {
    /// Create the state.
    #[must_use]
    pub fn new(devices: DevicesState, mgr: Arc<Manager>) -> Self {
        Self { devices, mgr }
    }
}

/// Routes, behind the authentication hook.
pub fn router(state: RecordingsState) -> Router {
    Router::new()
        .route("/api/v1/recordings", get(list))
        .route("/api/v1/recordings/config", get(config))
        .route(
            "/api/v1/recordings/{id}",
            get(show).patch(update).delete(remove),
        )
        .route("/api/v1/recordings/{id}/file", get(file))
        .route("/api/v1/recordings/{id}/stop", post(stop))
        .route(
            "/api/v1/devices/{dev}/recordings",
            post(start).layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .route("/api/v1/devices/{dev}/vfos/{id}/replay", get(replay))
        .route(
            "/api/v1/devices/{dev}/vfos/{id}/replay/save",
            post(save_replay).layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .with_state(state)
}

fn owner_of(principal: &Principal) -> Owner {
    Owner {
        name: principal.name.clone(),
        id: principal.owner_id(),
    }
}

fn start_failure(err: StartError) -> Response {
    match err {
        StartError::NoDatabase => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "no database",
        ),
        StartError::NoBus => failure(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "no bus"),
        StartError::QuotaFull => failure(
            StatusCode::INSUFFICIENT_STORAGE,
            "quota_full",
            "the recordings quota is used up; delete recordings or raise SUPERHET_RECORDINGS_MAX_GB",
        ),
        StartError::TooMany => failure(
            StatusCode::CONFLICT,
            "too_many",
            "too many recordings running",
        ),
        StartError::Invalid(msg) => failure(StatusCode::BAD_REQUEST, "invalid", &msg),
        StartError::Daemon(msg) => failure(StatusCode::BAD_GATEWAY, "daemon", &msg),
        StartError::Empty => failure(StatusCode::NOT_FOUND, "empty", "nothing buffered yet"),
        StartError::Internal(msg) => failure(StatusCode::INTERNAL_SERVER_ERROR, "internal", &msg),
    }
}

fn internal(err: &dyn std::fmt::Display) -> Response {
    tracing::error!(%err, "recordings database error");
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "database error",
    )
}

fn invalid(message: &str) -> Response {
    failure(StatusCode::BAD_REQUEST, "invalid", message)
}

fn pool(state: &RecordingsState) -> Result<&superhet_db::PgPool, Box<Response>> {
    state.mgr.pool().ok_or_else(|| {
        Box::new(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "no database",
        ))
    })
}

/// A recording as the API shows it; running recordings report their live size and duration.
fn view(state: &RecordingsState, rec: &db::Record, principal: &Principal) -> Value {
    let mut size = rec.size_bytes;
    let mut duration = rec.duration_ms;
    if rec.state == "recording"
        && let Some((bytes, ms)) = state.mgr.live(rec.id)
    {
        size = i64::try_from(bytes).unwrap_or(size);
        duration = i64::try_from(ms).unwrap_or(duration);
    }
    let mut doc = json!({
        "id": rec.id, "kind": rec.kind, "source": rec.source, "state": rec.state,
        "title": rec.title, "device": rec.device, "vfo": rec.vfo, "freq_hz": rec.freq_hz,
        "mode": rec.mode, "started_ms": rec.started_ms, "stopped_ms": rec.stopped_ms,
        "duration_ms": duration, "size_bytes": size, "keep": rec.keep, "meta": rec.meta,
    });
    if principal.sees_all_vfos()
        && let Some(map) = doc.as_object_mut()
    {
        map.insert("owner".into(), json!(rec.owner_name));
    }
    doc
}

/// Load a recording and check that `principal` may use its device for `need`.
async fn load(
    state: &RecordingsState,
    principal: &Principal,
    id: i64,
    need: Need,
    action: &str,
) -> Result<db::Record, Box<Response>> {
    let rec = db::get(pool(state)?, id)
        .await
        .map_err(|e| Box::new(internal(&e)))?
        .ok_or_else(|| {
            Box::new(failure(
                StatusCode::NOT_FOUND,
                "not_found",
                "no such recording",
            ))
        })?;
    device_access::check(&state.devices.auth, principal, &rec.device, need, action).await?;
    Ok(rec)
}

fn clean_title(raw: &str) -> Result<String, &'static str> {
    let title: String = raw.trim().chars().collect();
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err("title is too long");
    }
    if title.chars().any(char::is_control) {
        return Err("title has control characters");
    }
    Ok(title)
}

// ---- reading -----------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    dev: Option<String>,
    kind: Option<String>,
    state: Option<String>,
    keep: Option<bool>,
    q: Option<String>,
    before: Option<i64>,
    limit: Option<i64>,
}

async fn list(
    State(state): State<RecordingsState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ListQuery>,
) -> Response {
    let pool = match pool(&state) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let auth = &state.devices.auth;
    let mut filter = db::Filter {
        device: query.dev.clone(),
        kind: query.kind.clone().filter(|k| Kind::parse(k).is_some()),
        state: query.state.clone(),
        keep: query.keep,
        search: query
            .q
            .clone()
            .filter(|q| !q.is_empty() && q.chars().count() <= 100),
        before_id: query.before,
        limit: query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        ..db::Filter::default()
    };
    if query
        .kind
        .as_deref()
        .is_some_and(|k| Kind::parse(k).is_none())
    {
        return invalid("kind must be audio or iq");
    }
    if let Some(device) = &query.dev {
        if let Err(r) =
            device_access::check(auth, &principal, device, Need::View, "GET recordings").await
        {
            return *r;
        }
    } else {
        match auth
            .device_policies()
            .denied(&Subject::from(&principal), Need::View)
            .await
        {
            Ok(denied) => filter.exclude_devices = denied,
            Err(_) => {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "device policy unavailable",
                );
            }
        }
    }
    match db::list(pool, &filter).await {
        Ok(rows) => {
            let next = (rows.len() as i64 == filter.limit)
                .then(|| rows.last().map(|r| r.id))
                .flatten();
            let items: Vec<Value> = rows.iter().map(|r| view(&state, r, &principal)).collect();
            json(
                StatusCode::OK,
                json!({"v": 1, "recordings": items, "next_before": next}).to_string(),
            )
        }
        Err(err) => internal(&err),
    }
}

async fn config(
    State(state): State<RecordingsState>,
    Extension(_): Extension<Principal>,
) -> Response {
    let cfg = state.mgr.config();
    let used: i64 = match state.mgr.pool() {
        Some(pool) => match db::finished_sizes(pool).await {
            Ok(rows) => rows.iter().map(|r| r.1).sum(),
            Err(err) => return internal(&err),
        },
        None => 0,
    };
    let (rings, ring_bytes) = state.mgr.replay_usage();
    json(
        StatusCode::OK,
        json!({
            "v": 1,
            "available": state.mgr.has_db(),
            "replay_seconds": cfg.replay_seconds,
            "max_bytes": cfg.max_bytes,
            "used_bytes": used,
            "quota_exhausted": state.mgr.quota_exhausted(),
            "replay_buffers": {"vfos": rings, "bytes": ring_bytes},
            "hit_recording": state.mgr.hit_recording_devices(),
            "limits": {
                "audio_max_seconds": recorder::AUDIO_MAX_SECONDS,
                "iq_default_seconds": recorder::IQ_DEFAULT_SECONDS,
                "iq_max_seconds": recorder::IQ_MAX_SECONDS,
                "iq_default_mb": recorder::IQ_DEFAULT_MB,
                "iq_max_mb": recorder::IQ_MAX_MB,
            },
        })
        .to_string(),
    )
}

async fn show(
    State(state): State<RecordingsState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<i64>,
) -> Response {
    match load(&state, &principal, id, Need::View, "GET recording").await {
        Ok(rec) => json(StatusCode::OK, view(&state, &rec, &principal).to_string()),
        Err(r) => *r,
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery {
    download: Option<bool>,
    part: Option<String>,
}

async fn file(
    State(state): State<RecordingsState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<i64>,
    Query(query): Query<FileQuery>,
    request: Request<Body>,
) -> Response {
    let rec = match load(&state, &principal, id, Need::View, "GET recording file").await {
        Ok(rec) => rec,
        Err(r) => return *r,
    };
    let Some(kind) = Kind::parse(&rec.kind) else {
        return failure(StatusCode::INTERNAL_SERVER_ERROR, "internal", "bad record");
    };
    let part = match query.part.as_deref() {
        None | Some("data" | "audio") => Part::Main,
        Some("meta") => Part::Meta,
        Some(_) => return invalid("part must be data or meta"),
    };
    let Some(path) = files::resolve(&state.mgr.config().dir, kind, &rec.stem, part) else {
        return failure(StatusCode::NOT_FOUND, "not_found", "no such file");
    };
    let mime = kind.mime(part);
    let Ok(mime_type) = mime.parse() else {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "bad media type",
        );
    };
    let ext = kind.extension(part).unwrap_or("bin");
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
        HeaderValue::from_static("private, max-age=0"),
    );
    if query.download == Some(true) || kind == Kind::Iq {
        let name = files::download_name(&rec.title, ext);
        if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
            headers.insert(header::CONTENT_DISPOSITION, value);
        }
    }
    response
}

// ---- changing ----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    title: Option<String>,
    keep: Option<bool>,
}

async fn update(
    State(state): State<RecordingsState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, true) {
        return *r;
    }
    let Ok(request) = serde_json::from_slice::<UpdateBody>(&body) else {
        return invalid("body must be {\"title\"?:string,\"keep\"?:bool}");
    };
    if request.title.is_none() && request.keep.is_none() {
        return invalid("title or keep is required");
    }
    let title = match request.title.as_deref().map(clean_title).transpose() {
        Ok(t) => t,
        Err(m) => return invalid(m),
    };
    let rec = match load(&state, &principal, id, Need::Control, "PATCH recording").await {
        Ok(rec) => rec,
        Err(r) => return *r,
    };
    let pool = match pool(&state) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if let Err(err) = db::update(pool, id, title.as_deref(), request.keep).await {
        return internal(&err);
    }
    state
        .devices
        .auth
        .audit()
        .log(
            &principal.who(),
            "recording.update",
            Some(&rec.device),
            Outcome::Ok,
            json!({"id": id, "title": title, "keep": request.keep}),
        )
        .await;
    match db::get(pool, id).await {
        Ok(Some(rec)) => json(StatusCode::OK, view(&state, &rec, &principal).to_string()),
        Ok(None) => failure(StatusCode::NOT_FOUND, "not_found", "no such recording"),
        Err(err) => internal(&err),
    }
}

async fn remove(
    State(state): State<RecordingsState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, false) {
        return *r;
    }
    let rec = match load(&state, &principal, id, Need::Control, "DELETE recording").await {
        Ok(rec) => rec,
        Err(r) => return *r,
    };
    if state.mgr.live(id).is_some() {
        return failure(
            StatusCode::CONFLICT,
            "recording_active",
            "stop the recording first",
        );
    }
    let pool = match pool(&state) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    match db::delete(pool, id).await {
        Ok(Some(row)) => {
            state.mgr.remove_files(&row).await;
            state
                .devices
                .auth
                .audit()
                .log(
                    &principal.who(),
                    "recording.delete",
                    Some(&rec.device),
                    Outcome::Ok,
                    json!({"id": id, "title": rec.title, "kind": rec.kind}),
                )
                .await;
            json(StatusCode::OK, json!({"v": 1, "ok": true}).to_string())
        }
        Ok(None) => failure(StatusCode::NOT_FOUND, "not_found", "no such recording"),
        Err(err) => internal(&err),
    }
}

async fn stop(
    State(state): State<RecordingsState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, false) {
        return *r;
    }
    let rec = match load(&state, &principal, id, Need::Control, "POST recording stop").await {
        Ok(rec) => rec,
        Err(r) => return *r,
    };
    if !state.mgr.stop(id, Stop::User).await {
        return failure(
            StatusCode::CONFLICT,
            "not_running",
            "the recording is not running",
        );
    }
    state
        .devices
        .auth
        .audit()
        .log(
            &principal.who(),
            "recording.stop",
            Some(&rec.device),
            Outcome::Ok,
            json!({"id": id}),
        )
        .await;
    match state.mgr.pool() {
        Some(pool) => match db::get(pool, id).await {
            Ok(Some(rec)) => json(StatusCode::OK, view(&state, &rec, &principal).to_string()),
            _ => json(StatusCode::OK, json!({"v": 1, "ok": true}).to_string()),
        },
        None => json(StatusCode::OK, json!({"v": 1, "ok": true}).to_string()),
    }
}

// ---- starting ----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartBody {
    kind: String,
    vfo: Option<u32>,
    #[serde(default)]
    gated: bool,
    title: Option<String>,
    max_seconds: Option<u64>,
    max_mb: Option<u64>,
    /// IQ: `window` or `vfo`.
    source: Option<String>,
    /// IQ: `cu8`, `cs16` or `cf32`.
    format: Option<String>,
    /// IQ slice rate in Hz.
    rate: Option<u32>,
}

fn parse_datatype(text: &str) -> Option<Datatype> {
    match text {
        "cu8" => Some(Datatype::Cu8),
        "cs16" | "ci16_le" => Some(Datatype::Ci16Le),
        "cf32" | "cf32_le" => Some(Datatype::Cf32Le),
        _ => None,
    }
}

/// The VFO object of a device state document.
fn vfo_of(state: &Value, id: u32) -> Option<&Value> {
    state
        .get("vfos")?
        .as_array()?
        .iter()
        .find(|v| v["id"].as_u64() == Some(u64::from(id)))
}

fn default_title(freq_hz: Option<i64>, mode: Option<&str>, prefix: &str) -> String {
    let mut parts = Vec::new();
    if !prefix.is_empty() {
        parts.push(prefix.to_owned());
    }
    if let Some(f) = freq_hz {
        let text = format!("{:.6}", f as f64 / 1e6);
        parts.push(format!(
            "{} MHz",
            text.trim_end_matches('0').trim_end_matches('.')
        ));
    }
    if let Some(mode) = mode {
        parts.push(mode.to_ascii_uppercase());
    }
    parts.join(" ")
}

async fn start(
    State(state): State<RecordingsState>,
    Extension(principal): Extension<Principal>,
    Path(device): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, true) {
        return *r;
    }
    let Ok(request) = serde_json::from_slice::<StartBody>(&body) else {
        return invalid("malformed request body");
    };
    let title = match request.title.as_deref().map(clean_title).transpose() {
        Ok(t) => t.filter(|t| !t.is_empty()),
        Err(m) => return invalid(m),
    };
    let doc = match ctl_state(&state.devices, &device).await {
        Ok(doc) => doc,
        Err(r) => return *r,
    };
    state.devices.auth.registry().note_device(&device);
    let max_bytes_quota = state.mgr.config().max_bytes;
    let started = match request.kind.as_str() {
        "audio" => {
            let Some(vfo) = request.vfo else {
                return invalid("audio recordings need a vfo");
            };
            let Some(v) = vfo_of(&doc, vfo) else {
                return failure(StatusCode::NOT_FOUND, "not_found", "no such VFO");
            };
            let freq_hz = v["freq_hz"].as_u64().and_then(|f| i64::try_from(f).ok());
            let mode = v["mode"].as_str().map(str::to_owned);
            state
                .mgr
                .start_audio(AudioStart {
                    device: device.clone(),
                    vfo,
                    gated: request.gated,
                    title: title.unwrap_or_else(|| default_title(freq_hz, mode.as_deref(), "")),
                    source: "manual",
                    owner: owner_of(&principal),
                    freq_hz,
                    mode,
                    max_seconds: request
                        .max_seconds
                        .unwrap_or(recorder::AUDIO_DEFAULT_SECONDS)
                        .clamp(1, recorder::AUDIO_MAX_SECONDS),
                    max_bytes: request.max_mb.unwrap_or(1024).clamp(1, 16_384) * 1024 * 1024,
                    idle_stop_frames: None,
                })
                .await
        }
        "iq" => {
            let datatype = match request.format.as_deref().map(parse_datatype) {
                None => None,
                Some(Some(d)) => Some(d),
                Some(None) => return invalid("format must be cu8, cs16 or cf32"),
            };
            let max_seconds = request
                .max_seconds
                .unwrap_or(recorder::IQ_DEFAULT_SECONDS)
                .clamp(1, recorder::IQ_MAX_SECONDS);
            // The default never exceeds the quota; an explicit larger value is a mistake.
            let mut max_bytes = request
                .max_mb
                .unwrap_or(recorder::IQ_DEFAULT_MB)
                .clamp(1, recorder::IQ_MAX_MB)
                * 1024
                * 1024;
            if max_bytes_quota > 0 && max_bytes > max_bytes_quota {
                if request.max_mb.is_some() {
                    return invalid("max_mb is larger than the recordings quota");
                }
                max_bytes = max_bytes_quota / 2;
            }
            let hw = doc["label"]
                .as_str()
                .or_else(|| doc["driver"].as_str())
                .map(str::to_owned);
            let window = match request.source.as_deref() {
                None | Some("window") => true,
                Some("vfo") => false,
                Some(_) => return invalid("source must be window or vfo"),
            };
            if window {
                let (Some(center), Some(rate)) = (
                    doc["center_hz"].as_u64(),
                    doc["sample_rate"]
                        .as_u64()
                        .and_then(|r| u32::try_from(r).ok()),
                ) else {
                    return failure(
                        StatusCode::CONFLICT,
                        "no_signal",
                        "the device has no tuning yet",
                    );
                };
                state
                    .mgr
                    .start_iq(IqStart {
                        device: device.clone(),
                        source: IqSource::Window,
                        datatype: datatype.unwrap_or(Datatype::Cu8),
                        rate,
                        center_hz: center,
                        title: title.unwrap_or_else(|| {
                            default_title(i64::try_from(center).ok(), None, "IQ")
                        }),
                        owner: owner_of(&principal),
                        mode: None,
                        bandwidth_hz: None,
                        hw,
                        max_seconds,
                        max_bytes,
                    })
                    .await
            } else {
                let Some(vfo) = request.vfo else {
                    return invalid("a VFO slice needs a vfo");
                };
                let Some(v) = vfo_of(&doc, vfo) else {
                    return failure(StatusCode::NOT_FOUND, "not_found", "no such VFO");
                };
                let Some(center) = v["freq_hz"].as_u64() else {
                    return failure(
                        StatusCode::CONFLICT,
                        "no_signal",
                        "the VFO has no frequency",
                    );
                };
                let mode = v["mode"].as_str().map(str::to_owned);
                state
                    .mgr
                    .start_iq(IqStart {
                        device: device.clone(),
                        source: IqSource::Vfo(vfo),
                        datatype: datatype.unwrap_or(Datatype::Ci16Le),
                        rate: request.rate.unwrap_or(48_000),
                        center_hz: center,
                        title: title.unwrap_or_else(|| {
                            default_title(i64::try_from(center).ok(), mode.as_deref(), "IQ")
                        }),
                        owner: owner_of(&principal),
                        mode,
                        bandwidth_hz: v["bandwidth_hz"].as_f64(),
                        hw,
                        max_seconds,
                        max_bytes,
                    })
                    .await
            }
        }
        _ => return invalid("kind must be audio or iq"),
    };
    match started {
        Ok(rec) => {
            state
                .devices
                .auth
                .audit()
                .log(&principal.who(), "recording.start", Some(&device), Outcome::Ok, json!({"id": rec.id, "kind": rec.kind, "freq_hz": rec.freq_hz, "gated": request.gated}))
                .await;
            json(
                StatusCode::CREATED,
                view(&state, &rec, &principal).to_string(),
            )
        }
        Err(err) => start_failure(err),
    }
}

// ---- instant replay ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayQuery {
    seconds: Option<u32>,
    gated: Option<bool>,
}

async fn replay(
    State(state): State<RecordingsState>,
    Extension(principal): Extension<Principal>,
    Path((device, vfo)): Path<(String, u32)>,
    Query(query): Query<ReplayQuery>,
) -> Response {
    let seconds = query
        .seconds
        .unwrap_or(30)
        .clamp(1, state.mgr.config().replay_seconds);
    // The same rule as the live audio: own and shared VFOs for listeners, all for operators.
    if let Err(reason) = crate::audio_ws::may_listen(
        &state.devices,
        &principal,
        &device,
        &std::collections::BTreeSet::from([vfo]),
    )
    .await
    {
        return failure(StatusCode::FORBIDDEN, "forbidden", reason);
    }
    match state
        .mgr
        .replay_ogg(&device, vfo, seconds, query.gated.unwrap_or(true))
    {
        Some((bytes, _)) => {
            let mut response = Response::new(Body::from(bytes));
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/ogg"));
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            response
        }
        None => failure(
            StatusCode::NOT_FOUND,
            "empty",
            "nothing buffered for this VFO yet",
        ),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveBody {
    seconds: Option<u32>,
    gated: Option<bool>,
    title: Option<String>,
}

async fn save_replay(
    State(state): State<RecordingsState>,
    Extension(principal): Extension<Principal>,
    Path((device, vfo)): Path<(String, u32)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = guard_mutation(&state.devices, &headers, !body.is_empty()) {
        return *r;
    }
    let request = if body.iter().all(u8::is_ascii_whitespace) {
        SaveBody::default()
    } else {
        match serde_json::from_slice::<SaveBody>(&body) {
            Ok(r) => r,
            Err(_) => return invalid("malformed request body"),
        }
    };
    let title = match request.title.as_deref().map(clean_title).transpose() {
        Ok(t) => t.filter(|t| !t.is_empty()),
        Err(m) => return invalid(m),
    };
    let doc = match ctl_state(&state.devices, &device).await {
        Ok(doc) => doc,
        Err(r) => return *r,
    };
    let Some(v) = vfo_of(&doc, vfo) else {
        return failure(StatusCode::NOT_FOUND, "not_found", "no such VFO");
    };
    let freq_hz = v["freq_hz"].as_u64().and_then(|f| i64::try_from(f).ok());
    let mode = v["mode"].as_str().map(str::to_owned);
    let seconds = request
        .seconds
        .unwrap_or(30)
        .clamp(1, state.mgr.config().replay_seconds);
    let title = title.unwrap_or_else(|| default_title(freq_hz, mode.as_deref(), "Replay"));
    match state
        .mgr
        .save_replay(SaveReplay {
            device: &device,
            vfo,
            seconds,
            gated: request.gated.unwrap_or(true),
            title: &title,
            owner: &owner_of(&principal),
            freq_hz,
            mode,
        })
        .await
    {
        Ok(rec) => {
            state
                .devices
                .auth
                .audit()
                .log(
                    &principal.who(),
                    "recording.save_replay",
                    Some(&device),
                    Outcome::Ok,
                    json!({"id": rec.id, "seconds": seconds}),
                )
                .await;
            json(
                StatusCode::CREATED,
                view(&state, &rec, &principal).to_string(),
            )
        }
        Err(err) => start_failure(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use superhet_db::users::Role;
    use tower::ServiceExt;

    fn app(role: Role) -> Router {
        let dir = std::env::temp_dir().join("superhet-rec-test");
        let state = RecordingsState::new(DevicesState::for_tests(), Manager::for_tests(dir));
        router(state).layer(Extension(Principal::test(role)))
    }

    async fn call(role: Role, method: &str, path: &str, body: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .expect("request");
        let response = app(role).oneshot(request).await.expect("infallible");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[test]
    fn titles_are_validated() {
        assert_eq!(clean_title("  Tower  ").as_deref(), Ok("Tower"));
        assert!(clean_title("a\nb").is_err());
        assert!(clean_title(&"x".repeat(201)).is_err());
    }

    #[test]
    fn default_titles_read_well() {
        assert_eq!(
            default_title(Some(145_500_000), Some("nfm"), ""),
            "145.5 MHz NFM"
        );
        assert_eq!(
            default_title(Some(1_090_000_000), None, "IQ"),
            "IQ 1090 MHz"
        );
        assert_eq!(default_title(None, None, ""), "");
    }

    #[test]
    fn datatypes_parse() {
        assert_eq!(parse_datatype("cu8"), Some(Datatype::Cu8));
        assert_eq!(parse_datatype("cs16"), Some(Datatype::Ci16Le));
        assert_eq!(parse_datatype("float"), None);
    }

    #[tokio::test]
    async fn bodies_and_kinds_are_validated_before_anything_runs() {
        let (status, _) = call(
            Role::Operator,
            "POST",
            "/api/v1/devices/dev0/recordings",
            "nope",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(
            Role::Operator,
            "POST",
            "/api/v1/devices/dev0/recordings",
            r#"{"kind":"video"}"#,
        )
        .await;
        // No daemon behind the test state: the state lookup fails before the kind is checked.
        assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::SERVICE_UNAVAILABLE);
        let (status, _) = call(Role::Operator, "PATCH", "/api/v1/recordings/1", "{}").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(
            Role::Operator,
            "PATCH",
            "/api/v1/recordings/1",
            r#"{"title":"a\nb"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn without_a_database_the_library_is_unavailable() {
        let (status, _) = call(Role::Listener, "GET", "/api/v1/recordings", "").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (status, body) = call(Role::Listener, "GET", "/api/v1/recordings/config", "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["available"], false);
        assert_eq!(body["replay_seconds"], 60);
    }

    #[tokio::test]
    async fn replay_without_audio_is_a_404_and_foreign_origins_are_refused() {
        let (status, body) = call(
            Role::Operator,
            "GET",
            "/api/v1/devices/dev0/vfos/1/replay",
            "",
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::NOT_FOUND, Some("empty"))
        );
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/recordings/1/stop")
            .header("origin", "https://evil.example")
            .header("host", "sh.test")
            .body(Body::empty())
            .expect("request");
        let response = app(Role::Operator)
            .oneshot(request)
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
