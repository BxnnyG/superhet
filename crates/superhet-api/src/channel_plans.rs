//! Custom channel plans over REST: `/api/v1/channel-plans[/{id}]`. Contract:
//! `docs/api-channel-plans.md`.
//!
//! The built-in plans (PMR446, Freenet, ...) ship with the web UI. This module only stores the
//! plans operators import. The API checks the structure and the size limits; the web UI validates
//! the content again (modes, regions, notes) and skips plans it cannot read.

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use serde_json::{Map, Value};
use superhet_db::{
    PgPool,
    channel_plans::{self, MAX_PLANS, StoredPlan, WriteError},
    profiles::parse_id,
};

use crate::devices::{DevicesState, failure, guard_mutation, json};

/// Largest accepted plan document.
const MAX_BODY_BYTES: usize = 512 * 1024;
const MAX_NAME_CHARS: usize = 100;
const MAX_CHANNELS: usize = 5_000;
const MAX_RASTERS: usize = 8;
const MAX_FREQ_HZ: u64 = 10_000_000_000;

/// Shared state of the channel plan routes.
#[derive(Clone)]
pub struct ChannelPlansState {
    devices: DevicesState,
    db: Option<PgPool>,
}

impl ChannelPlansState {
    /// Create the state from the device routes' state (origin policy) and the database.
    #[must_use]
    pub fn new(devices: DevicesState, db: Option<PgPool>) -> Self {
        Self { devices, db }
    }
}

/// Routes of the channel plan API, behind the authentication hook.
pub fn router(state: ChannelPlansState) -> Router {
    let limit = DefaultBodyLimit::max(MAX_BODY_BYTES);
    Router::new()
        .route(
            "/api/v1/channel-plans",
            axum::routing::get(list_plans)
                .post(create_plan)
                .layer(limit),
        )
        .route(
            "/api/v1/channel-plans/{id}",
            axum::routing::put(update_plan)
                .delete(delete_plan)
                .layer(limit),
        )
        .with_state(state)
}

fn pool(state: &ChannelPlansState) -> Result<&PgPool, Box<Response>> {
    state.db.as_ref().ok_or_else(|| {
        Box::new(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "no database",
        ))
    })
}

fn internal(err: &dyn std::fmt::Display) -> Box<Response> {
    tracing::error!(%err, "channel plan database error");
    Box::new(failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "database error",
    ))
}

fn invalid(message: &str) -> Box<Response> {
    Box::new(failure(StatusCode::BAD_REQUEST, "invalid", message))
}

fn not_found() -> Box<Response> {
    Box::new(failure(
        StatusCode::NOT_FOUND,
        "not_found",
        "no such channel plan",
    ))
}

fn write_failure(err: WriteError) -> Box<Response> {
    match err {
        WriteError::Limit => Box::new(failure(
            StatusCode::CONFLICT,
            "limit",
            &format!("at most {MAX_PLANS} custom channel plans can be stored"),
        )),
        WriteError::Db(e) => internal(&e),
    }
}

fn plan_json(p: &StoredPlan) -> Value {
    serde_json::json!({
        "id": p.id,
        "name": p.name,
        "doc": p.doc,
        "created_ms": p.created_ms,
        "updated_ms": p.updated_ms,
    })
}

/// Check the structure of a plan document and return its (trimmed) name. The document is stored
/// as sent, with the trimmed name written back.
fn validate(body: &[u8]) -> Result<(String, Value), String> {
    let mut doc: Map<String, Value> =
        serde_json::from_slice(body).map_err(|e| format!("malformed request: {e}"))?;
    if doc.get("v").and_then(Value::as_u64) != Some(1) {
        return Err("v must be 1".into());
    }
    let name = doc
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty() && n.chars().count() <= MAX_NAME_CHARS)
        .ok_or_else(|| format!("name must be 1 to {MAX_NAME_CHARS} characters"))?
        .to_owned();
    let channels = match doc.get("channels") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(list)) => list.as_slice(),
        Some(_) => return Err("channels must be a list".into()),
    };
    if channels.len() > MAX_CHANNELS {
        return Err(format!("at most {MAX_CHANNELS} channels"));
    }
    for (i, channel) in channels.iter().enumerate() {
        let freq = channel
            .as_object()
            .and_then(|c| c.get("freq_hz"))
            .and_then(Value::as_u64);
        if !freq.is_some_and(|f| (1..=MAX_FREQ_HZ).contains(&f)) {
            return Err(format!(
                "channel {}: freq_hz must be an integer in Hz",
                i + 1
            ));
        }
    }
    let rasters = match doc.get("rasters") {
        None | Some(Value::Null) => 0,
        Some(Value::Array(list)) if list.iter().all(Value::is_object) => list.len(),
        Some(_) => return Err("rasters must be a list of objects".into()),
    };
    if rasters > MAX_RASTERS {
        return Err(format!("at most {MAX_RASTERS} rasters"));
    }
    if channels.is_empty() && rasters == 0 {
        return Err("a plan needs at least one channel or raster".into());
    }
    doc.insert("name".into(), Value::String(name.clone()));
    Ok((name, Value::Object(doc)))
}

async fn list_plans(State(state): State<ChannelPlansState>) -> Response {
    let result = async {
        let all = channel_plans::list(pool(&state)?)
            .await
            .map_err(|e| internal(&e))?;
        let all: Vec<_> = all.iter().map(plan_json).collect();
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            serde_json::json!({"v": 1, "plans": all}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn create_plan(
    State(state): State<ChannelPlansState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let (name, doc) = validate(&body).map_err(|e| invalid(&e))?;
        let created = channel_plans::create(pool(&state)?, &name, &doc)
            .await
            .map_err(write_failure)?;
        let mut response = json(StatusCode::CREATED, plan_json(&created).to_string());
        if let Ok(location) =
            header::HeaderValue::from_str(&format!("/api/v1/channel-plans/{}", created.id))
        {
            response.headers_mut().insert(header::LOCATION, location);
        }
        Ok::<_, Box<Response>>(response)
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn update_plan(
    State(state): State<ChannelPlansState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let id = parse_id(&id).ok_or_else(not_found)?;
        let (name, doc) = validate(&body).map_err(|e| invalid(&e))?;
        let updated = channel_plans::update(pool(&state)?, &id, &name, &doc)
            .await
            .map_err(|e| internal(&e))?
            .ok_or_else(not_found)?;
        Ok::<_, Box<Response>>(json(StatusCode::OK, plan_json(&updated).to_string()))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn delete_plan(
    State(state): State<ChannelPlansState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, false)?;
        let id = parse_id(&id).ok_or_else(not_found)?;
        if !channel_plans::delete(pool(&state)?, &id)
            .await
            .map_err(|e| internal(&e))?
        {
            return Err(not_found());
        }
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        Ok::<_, Box<Response>>(response)
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(doc: &Value) -> Result<(String, Value), String> {
        validate(doc.to_string().as_bytes())
    }

    #[test]
    fn accepts_a_channel_list_and_trims_the_name() {
        let (name, doc) = check(&serde_json::json!({
            "v": 1, "name": "  Club  ",
            "channels": [{"freq_hz": 145_500_000, "number": "1"}]
        }))
        .expect("valid");
        assert_eq!(name, "Club");
        assert_eq!(doc["name"], "Club");
    }

    #[test]
    fn accepts_a_raster_only_plan() {
        assert!(
            check(&serde_json::json!({
                "v": 1, "name": "Air",
                "rasters": [{"start_hz": 118_000_000, "end_hz": 137_000_000, "step_hz": 25_000}]
            }))
            .is_ok()
        );
    }

    #[test]
    fn rejects_bad_documents() {
        let cases = [
            serde_json::json!({"name": "x", "channels": [{"freq_hz": 1}]}),
            serde_json::json!({"v": 2, "name": "x", "channels": [{"freq_hz": 1}]}),
            serde_json::json!({"v": 1, "name": " ", "channels": [{"freq_hz": 1}]}),
            serde_json::json!({"v": 1, "name": "x"}),
            serde_json::json!({"v": 1, "name": "x", "channels": []}),
            serde_json::json!({"v": 1, "name": "x", "channels": [{"freq_hz": 1.5}]}),
            serde_json::json!({"v": 1, "name": "x", "channels": [{"freq_hz": 0}]}),
            serde_json::json!({"v": 1, "name": "x", "channels": [{"freq_hz": 99_000_000_000u64}]}),
            serde_json::json!({"v": 1, "name": "x", "channels": "no"}),
            serde_json::json!({"v": 1, "name": "x", "rasters": [1]}),
        ];
        for case in cases {
            assert!(check(&case).is_err(), "{case}");
        }
        assert!(validate(b"[]").is_err());
        assert!(validate(b"not json").is_err());
    }

    #[test]
    fn limits_channels_and_name_length() {
        let many: Vec<Value> = (0..=MAX_CHANNELS)
            .map(|i| serde_json::json!({"freq_hz": 1_000_000 + i}))
            .collect();
        assert!(check(&serde_json::json!({"v": 1, "name": "x", "channels": many})).is_err());
        let long = "x".repeat(MAX_NAME_CHARS + 1);
        assert!(
            check(&serde_json::json!({"v": 1, "name": long, "channels": [{"freq_hz": 1}]}))
                .is_err()
        );
    }
}
