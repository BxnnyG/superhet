//! ADS-B extras over REST (ADR-0025, `docs/plugins-adsb.md`): the aircraft database (lookup,
//! description, admin download / import) and the receiver range statistics.
//!
//! ```text
//! GET    /api/v1/adsb/aircraft-db            what is imported (source, licence) + import progress
//! GET    /api/v1/adsb/lookup?icao=..&operator=..   registration, type, airline names (local)
//! GET    /api/v1/adsb/range?device=&hours=   sector/band maxima + message rate / aircraft history
//! DELETE /api/v1/adsb/range?device=          forget the statistics of a device (operator)
//! POST   /api/v1/admin/adsb/aircraft-db      {"source":"download"|"file"} import in the background
//! DELETE /api/v1/admin/adsb/aircraft-db      remove the imported database
//! ```
//!
//! Lookups are plain queries on the imported tables; nothing here talks to a third party except
//! the admin's explicit "download", which fetches one fixed URL (`aircraft_db::DOWNLOAD_URL`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use axum::{
    Extension, Router,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;
use superhet_db::{PgPool, adsb};

use crate::access::Principal;
use crate::aircraft_db::{self, LoadError};
use crate::devices::{DevicesState, failure, guard_mutation, json};

/// Longest accepted `icao` / `operator` query value.
const MAX_QUERY_CHARS: usize = 8_000;
/// Longest history served (days); the worker prunes by `SUPERHET_EVENT_RETENTION_DAYS`.
const MAX_HOURS: i64 = 24 * 30;
/// Points of the activity history at most.
const MAX_POINTS: i64 = 720;
const MS_PER_HOUR: i64 = 3_600_000;

/// Progress of the background import.
#[derive(Debug, Clone, Default)]
struct Job {
    /// `idle`, `downloading`, `importing`, `done` or `failed`.
    state: &'static str,
    message: String,
    finished_ms: Option<i64>,
}

/// Shared state of the ADS-B routes.
#[derive(Clone)]
pub struct AdsbState {
    devices: DevicesState,
    db: Option<PgPool>,
    job: Arc<Mutex<Job>>,
    /// Where the downloaded archive is kept.
    dir: PathBuf,
    /// `SUPERHET_AIRCRAFT_DB_FILE`.
    file: Option<PathBuf>,
    /// What "download" fetches (a test points it at a local server).
    url: String,
}

impl AdsbState {
    /// Create the state from the device routes' state (origin policy, audit) and the database;
    /// the directory and the import file come from the environment.
    #[must_use]
    pub fn new(devices: DevicesState, db: Option<PgPool>) -> Self {
        Self {
            devices,
            db,
            job: Arc::new(Mutex::new(Job {
                state: "idle",
                ..Job::default()
            })),
            dir: aircraft_db::data_dir(),
            file: aircraft_db::import_file(),
            url: aircraft_db::DOWNLOAD_URL.to_owned(),
        }
    }

    fn job(&self) -> Job {
        self.job
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_job(&self, state: &'static str, message: &str, finished: bool) {
        *self.job.lock().unwrap_or_else(PoisonError::into_inner) = Job {
            state,
            message: message.to_owned(),
            finished_ms: finished.then(now_ms),
        };
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Routes of the ADS-B extras, behind the access middleware.
pub fn router(state: AdsbState) -> Router {
    Router::new()
        .route("/api/v1/adsb/aircraft-db", get(db_status))
        .route("/api/v1/adsb/lookup", get(lookup))
        .route("/api/v1/adsb/range", get(range).delete(reset_range))
        .route(
            "/api/v1/admin/adsb/aircraft-db",
            axum::routing::post(update_db).delete(remove_db),
        )
        .with_state(state)
}

fn pool(state: &AdsbState) -> Result<&PgPool, Box<Response>> {
    state.db.as_ref().ok_or_else(|| {
        Box::new(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "no database",
        ))
    })
}

fn internal(err: &dyn std::fmt::Display) -> Box<Response> {
    tracing::error!(%err, "adsb database error");
    Box::new(failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "database error",
    ))
}

fn invalid(message: &str) -> Box<Response> {
    Box::new(failure(StatusCode::BAD_REQUEST, "invalid", message))
}

fn source_json(info: &adsb::DatabaseInfo) -> Value {
    json!({
        "source": info.source.source,
        "source_url": info.source.source_url,
        "licence": info.source.licence,
        "licence_url": info.source.licence_url,
        "version": info.source.version,
        "origin": info.source.origin,
        "aircraft": info.aircraft,
        "types": info.types,
        "operators": info.operators,
        "imported_ms": info.imported_ms,
    })
}

/// What the admin button offers, so the UI can show source and licence before anything is fetched.
fn offer(state: &AdsbState) -> Value {
    json!({
        "source": aircraft_db::SOURCE_NAME,
        "source_url": aircraft_db::SOURCE_URL,
        "licence": aircraft_db::LICENCE,
        "licence_url": aircraft_db::LICENCE_URL,
        "download_url": aircraft_db::DOWNLOAD_URL,
        "file": state.file.as_ref().map(|p| p.display().to_string()),
        "archive": state.dir.join(aircraft_db::ARCHIVE_NAME).is_file(),
    })
}

async fn db_status(State(state): State<AdsbState>) -> Response {
    let result = async {
        let info = adsb::database_info(pool(&state)?)
            .await
            .map_err(|e| internal(&e))?;
        let job = state.job();
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            json!({
                "v": 1,
                "database": info.as_ref().map(source_json),
                "offer": offer(&state),
                "job": {"state": job.state, "message": job.message, "finished_ms": job.finished_ms},
            })
            .to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

#[derive(Deserialize)]
struct LookupQuery {
    #[serde(default)]
    icao: String,
    #[serde(default)]
    operator: String,
}

/// Parse a comma separated list of up to [`adsb::MAX_LOOKUP`] items.
fn list(value: &str) -> Result<Vec<&str>, Box<Response>> {
    if value.len() > MAX_QUERY_CHARS {
        return Err(invalid("query value too long"));
    }
    let items: Vec<&str> = value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if items.len() > adsb::MAX_LOOKUP {
        return Err(invalid(&format!("at most {} values", adsb::MAX_LOOKUP)));
    }
    Ok(items)
}

async fn lookup(State(state): State<AdsbState>, Query(q): Query<LookupQuery>) -> Response {
    let result = async {
        let icao: Vec<u32> = list(&q.icao)?
            .into_iter()
            .map(|s| {
                (s.len() == 6)
                    .then(|| u32::from_str_radix(s, 16).ok())
                    .flatten()
                    .ok_or_else(|| invalid("icao must be six hex digits"))
            })
            .collect::<Result<_, _>>()?;
        let operators: Vec<String> = list(&q.operator)?
            .into_iter()
            .map(|s| {
                (s.len() <= 16 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
                    .then(|| s.to_ascii_uppercase())
                    .ok_or_else(|| invalid("operator must be an ICAO airline designator"))
            })
            .collect::<Result<_, _>>()?;
        let pool = pool(&state)?;
        let aircraft = adsb::lookup_aircraft(pool, &icao)
            .await
            .map_err(|e| internal(&e))?;
        let operators = adsb::lookup_operators(pool, &operators)
            .await
            .map_err(|e| internal(&e))?;
        let aircraft: serde_json::Map<String, Value> = aircraft
            .iter()
            .map(|f| {
                (
                    format!("{:06x}", f.icao),
                    json!({
                        "reg": f.registration,
                        "type": f.type_code,
                        "type_desc": f.type_description,
                        "class": f.class,
                        "wake": f.wake,
                        "military": f.flags & 1 != 0,
                    }),
                )
            })
            .collect();
        let operators: serde_json::Map<String, Value> = operators
            .iter()
            .map(|o| {
                (
                    o.code.clone(),
                    json!({"name": o.name, "country": o.country, "callsign": o.callsign}),
                )
            })
            .collect();
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            json!({"v": 1, "aircraft": aircraft, "operators": operators}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

/// A device id as it appears in API paths and subjects.
fn valid_device(id: &str) -> bool {
    superhet_bus::tap::is_valid_id(id)
}

#[derive(Deserialize)]
struct RangeQuery {
    device: Option<String>,
    hours: Option<i64>,
}

async fn range(State(state): State<AdsbState>, Query(q): Query<RangeQuery>) -> Response {
    let result = async {
        let device = q.device.as_deref().unwrap_or("dev0");
        if !valid_device(device) {
            return Err(invalid("invalid device id"));
        }
        let hours = q.hours.unwrap_or(24);
        if !(1..=MAX_HOURS).contains(&hours) {
            return Err(invalid(&format!("hours must be 1 to {MAX_HOURS}")));
        }
        let pool = pool(&state)?;
        let summary = adsb::range_summary(pool, device)
            .await
            .map_err(|e| internal(&e))?;
        let now = now_ms();
        let since = now - hours * MS_PER_HOUR;
        // One bucket per minute at most, coarser so the answer stays near MAX_POINTS points.
        let minutes = (hours * 60 / MAX_POINTS).max(1);
        let points = adsb::activity(pool, device, since, minutes * 60_000)
            .await
            .map_err(|e| internal(&e))?;
        let cells: Vec<Value> = summary
            .cells
            .iter()
            .map(|c| json!([c.sector, c.band, c.max_km, c.icao]))
            .collect();
        let series: Vec<Value> = points
            .iter()
            .map(|p| json!([p.ts_ms, p.msgs_per_s, p.aircraft, p.with_position]))
            .collect();
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            json!({
                "v": 1,
                "device": device,
                "receiver": summary.receiver.map(|(lat, lon)| json!({"lat": lat, "lon": lon})),
                "since_ms": summary.since_ms,
                "sector_deg": 360 / adsb::SECTORS,
                "bands": adsb::BANDS,
                "cells": cells,
                "bucket_s": minutes * 60,
                "series": series,
            })
            .to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn reset_range(
    State(state): State<AdsbState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<RangeQuery>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, false)?;
        let device = q.device.as_deref().unwrap_or("dev0");
        if !valid_device(device) {
            return Err(invalid("invalid device id"));
        }
        let removed = adsb::reset_range(pool(&state)?, device)
            .await
            .map_err(|e| internal(&e))?;
        state
            .devices
            .auth
            .audit()
            .log(
                &principal.who(),
                "adsb.range.reset",
                Some(device),
                Outcome::Ok,
                json!({"cells": removed}),
            )
            .await;
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            json!({"v": 1, "removed_cells": removed}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

#[derive(Deserialize)]
struct UpdateRequest {
    source: String,
}

async fn update_db(
    State(state): State<AdsbState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let request: UpdateRequest = serde_json::from_slice(&body)
            .map_err(|_| invalid("expected {\"source\":\"download\"} or {\"source\":\"file\"}"))?;
        let download = match request.source.as_str() {
            "download" => true,
            "file" => false,
            _ => return Err(invalid("source must be \"download\" or \"file\"")),
        };
        let pool = pool(&state)?.clone();
        // A file import reads the configured file, else the archive of an earlier download.
        let file = if download {
            None
        } else {
            let archive = state.dir.join(aircraft_db::ARCHIVE_NAME);
            let path = state
                .file
                .clone()
                .or_else(|| archive.is_file().then_some(archive));
            Some(path.ok_or_else(|| {
                invalid(
                    "no file to import: set SUPERHET_AIRCRAFT_DB_FILE or download the database first",
                )
            })?)
        };
        {
            let mut job = state.job.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(job.state, "downloading" | "importing") {
                return Err(Box::new(failure(
                    StatusCode::CONFLICT,
                    "busy",
                    "an import is already running",
                )));
            }
            *job = Job {
                state: if download { "downloading" } else { "importing" },
                message: String::new(),
                finished_ms: None,
            };
        }
        let origin = if download { "download" } else { "file" };
        let who = principal.who();
        let task_state = state.clone();
        tokio::spawn(async move {
            let outcome = run_import(&task_state, &pool, file).await;
            let (action_outcome, details) = match &outcome {
                Ok(message) => {
                    task_state.set_job("done", message, true);
                    (Outcome::Ok, json!({"source": origin, "result": message}))
                }
                Err(err) => {
                    tracing::warn!(%err, "aircraft database import failed");
                    task_state.set_job("failed", &err.to_string(), true);
                    (
                        Outcome::Error,
                        json!({"source": origin, "error": err.to_string()}),
                    )
                }
            };
            task_state
                .devices
                .auth
                .audit()
                .log(
                    &who,
                    "adsb.aircraft_db.update",
                    Some(origin),
                    action_outcome,
                    details,
                )
                .await;
        });
        Ok::<_, Box<Response>>(json(
            StatusCode::ACCEPTED,
            json!({"v": 1, "ok": true, "state": state.job().state}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

/// Download (if `file` is `None`), read and import. Returns the summary line for the UI.
async fn run_import(
    state: &AdsbState,
    pool: &PgPool,
    file: Option<PathBuf>,
) -> Result<String, ImportError> {
    let (path, origin) = match file {
        Some(path) => (path, "file"),
        None => (
            aircraft_db::download(&state.url, &state.dir, aircraft_db::MAX_DOWNLOAD_BYTES).await?,
            "download",
        ),
    };
    state.set_job("importing", "", false);
    let loaded = tokio::task::spawn_blocking(move || aircraft_db::load(&path))
        .await
        .map_err(|e| ImportError::Task(e.to_string()))??;
    let source = aircraft_db::source_for(&loaded, origin);
    let info = adsb::replace_database(pool, &loaded.dataset, &source)
        .await
        .map_err(|e| ImportError::Db(e.to_string()))?;
    Ok(format!(
        "{} aircraft, {} types, {} airlines",
        info.aircraft, info.types, info.operators
    ))
}

#[derive(Debug, thiserror::Error)]
enum ImportError {
    #[error(transparent)]
    Load(#[from] LoadError),
    #[error("database error: {0}")]
    Db(String),
    #[error("import task failed: {0}")]
    Task(String),
}

async fn remove_db(
    State(state): State<AdsbState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, false)?;
        if matches!(state.job().state, "downloading" | "importing") {
            return Err(Box::new(failure(
                StatusCode::CONFLICT,
                "busy",
                "an import is running",
            )));
        }
        adsb::clear_database(pool(&state)?)
            .await
            .map_err(|e| internal(&e))?;
        state.set_job("idle", "", false);
        state
            .devices
            .auth
            .audit()
            .log(
                &principal.who(),
                "adsb.aircraft_db.delete",
                None,
                Outcome::Ok,
                json!({}),
            )
            .await;
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            json!({"v": 1, "ok": true}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_split_trimmed_and_capped() {
        assert_eq!(list("a, b ,,c").ok(), Some(vec!["a", "b", "c"]));
        assert_eq!(list("").ok(), Some(vec![]));
        let many = vec!["aaaaaa"; adsb::MAX_LOOKUP + 1].join(",");
        assert!(list(&many).is_err());
        assert!(list(&"x".repeat(MAX_QUERY_CHARS + 1)).is_err());
    }

    #[test]
    fn device_ids_are_checked() {
        assert!(valid_device("dev0"));
        assert!(valid_device("rtl-1_a"));
        for bad in ["", "a b", "a/b", "dev.0", &"x".repeat(65)] {
            assert!(!valid_device(bad), "{bad}");
        }
    }
}
