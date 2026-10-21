//! Profiles over REST: CRUD on `/api/v1/profiles[/{id}]` and the device actions
//! `POST /api/v1/devices/{dev}/profile/save|load`. Contract: `docs/api-profiles.md`.
//!
//! Profiles live in PostgreSQL; the API validates the document (`superhet_bus::profile`) and
//! stores it normalised. Loading hands the stored document to the daemon (`ctl.profile.load`),
//! which applies it atomically; saving snapshots the daemon's current state.

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use superhet_bus::{profile::ProfileData, subjects};
use superhet_db::{PgPool, profiles};

use crate::devices::{DevicesState, call_daemon, ctl_state, failure, guard_mutation, json};

/// Largest accepted request body.
const MAX_BODY_BYTES: usize = 16 * 1024;

/// Shared state of the profile routes.
#[derive(Clone)]
pub struct ProfilesState {
    devices: DevicesState,
    db: Option<PgPool>,
}

impl ProfilesState {
    /// Create the state from the device routes' state (bus, origin policy) and the database.
    #[must_use]
    pub fn new(devices: DevicesState, db: Option<PgPool>) -> Self {
        Self { devices, db }
    }
}

/// Routes of the profile API, behind the authentication hook.
pub fn router(state: ProfilesState) -> Router {
    let limit = DefaultBodyLimit::max(MAX_BODY_BYTES);
    Router::new()
        .route(
            "/api/v1/profiles",
            get(list_profiles).post(create_profile).layer(limit),
        )
        .route(
            "/api/v1/profiles/{id}",
            get(get_profile)
                .put(update_profile)
                .delete(delete_profile)
                .layer(limit),
        )
        .route(
            "/api/v1/devices/{dev}/profile/save",
            post(save_profile).layer(limit),
        )
        .route(
            "/api/v1/devices/{dev}/profile/load",
            post(load_profile).layer(limit),
        )
        .with_state(state)
}

fn pool(state: &ProfilesState) -> Result<&PgPool, Box<Response>> {
    state.db.as_ref().ok_or_else(|| {
        Box::new(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "no database",
        ))
    })
}

fn internal(err: &dyn std::fmt::Display) -> Box<Response> {
    tracing::error!(%err, "profile database error");
    Box::new(failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "database error",
    ))
}

fn write_failure(err: profiles::WriteError) -> Box<Response> {
    match err {
        profiles::WriteError::NameTaken => Box::new(failure(
            StatusCode::CONFLICT,
            "conflict",
            "a profile with this name already exists",
        )),
        profiles::WriteError::Db(e) => internal(&e),
    }
}

fn not_found() -> Box<Response> {
    Box::new(failure(
        StatusCode::NOT_FOUND,
        "not_found",
        "no such profile",
    ))
}

fn invalid(message: &str) -> Box<Response> {
    Box::new(failure(StatusCode::BAD_REQUEST, "invalid", message))
}

fn profile_id(raw: &str) -> Result<String, Box<Response>> {
    profiles::parse_id(raw).ok_or_else(not_found)
}

fn profile_json(profile: &profiles::Profile) -> serde_json::Value {
    serde_json::json!({
        "id": profile.id,
        "name": profile.name,
        "data": profile.data,
        "created_at": profile.created_at,
        "updated_at": profile.updated_at,
    })
}

fn profile_response(status: StatusCode, profile: &profiles::Profile) -> Response {
    let mut response = json(
        status,
        serde_json::json!({"v": 1, "profile": profile_json(profile)}).to_string(),
    );
    if status == StatusCode::CREATED
        && let Ok(location) =
            header::HeaderValue::from_str(&format!("/api/v1/profiles/{}", profile.id))
    {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

/// A trimmed name of 1..=100 characters without control characters.
fn check_name(raw: &str) -> Result<String, &'static str> {
    let name = raw.trim();
    let chars = name.chars().count();
    if chars == 0 || chars > profiles::MAX_NAME_CHARS {
        Err("name must be 1 to 100 characters")
    } else if name.chars().any(char::is_control) {
        Err("name must not contain control characters")
    } else {
        Ok(name.to_owned())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileBody {
    name: String,
    data: ProfileData,
}

/// Parse and validate `{"name","data"}`; the data is returned normalised (defaults filled in).
fn parse_body(body: &[u8]) -> Result<(String, serde_json::Value), Box<Response>> {
    let parsed: ProfileBody =
        serde_json::from_slice(body).map_err(|e| invalid(&format!("malformed request: {e}")))?;
    let name = check_name(&parsed.name).map_err(invalid)?;
    parsed.data.validate().map_err(|e| invalid(&e))?;
    let data = serde_json::to_value(&parsed.data).map_err(|e| internal(&e))?;
    Ok((name, data))
}

async fn list_profiles(State(state): State<ProfilesState>) -> Response {
    let result = async {
        let all = profiles::list(pool(&state)?)
            .await
            .map_err(|e| internal(&e))?;
        let all: Vec<_> = all.iter().map(profile_json).collect();
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            serde_json::json!({"v": 1, "profiles": all}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn get_profile(State(state): State<ProfilesState>, Path(id): Path<String>) -> Response {
    let result = async {
        let id = profile_id(&id)?;
        let found = profiles::get(pool(&state)?, &id)
            .await
            .map_err(|e| internal(&e))?
            .ok_or_else(not_found)?;
        Ok::<_, Box<Response>>(profile_response(StatusCode::OK, &found))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn create_profile(
    State(state): State<ProfilesState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let (name, data) = parse_body(&body)?;
        let created = profiles::create(pool(&state)?, &name, &data)
            .await
            .map_err(write_failure)?;
        Ok::<_, Box<Response>>(profile_response(StatusCode::CREATED, &created))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn update_profile(
    State(state): State<ProfilesState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let id = profile_id(&id)?;
        let (name, data) = parse_body(&body)?;
        let updated = profiles::update(pool(&state)?, &id, &name, &data)
            .await
            .map_err(write_failure)?
            .ok_or_else(not_found)?;
        Ok::<_, Box<Response>>(profile_response(StatusCode::OK, &updated))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn delete_profile(
    State(state): State<ProfilesState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, false)?;
        let id = profile_id(&id)?;
        if !profiles::delete(pool(&state)?, &id)
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveBody {
    name: String,
}

/// Snapshot the device's current state (asked from the daemon, not from KV) as a new profile.
async fn save_profile(
    State(state): State<ProfilesState>,
    Path(device): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let parsed: SaveBody = serde_json::from_slice(&body)
            .map_err(|e| invalid(&format!("malformed request: {e}")))?;
        let name = check_name(&parsed.name).map_err(invalid)?;
        let pool = pool(&state)?;
        let device_state = ctl_state(&state.devices, &device).await?;
        let data = ProfileData::from_state(&device_state).map_err(|e| {
            tracing::warn!(error = %e, "profile save: unusable device state");
            Box::new(failure(
                StatusCode::BAD_GATEWAY,
                "bus",
                "unusable device state",
            ))
        })?;
        let data = serde_json::to_value(&data).map_err(|e| internal(&e))?;
        let created = profiles::create(pool, &name, &data)
            .await
            .map_err(write_failure)?;
        Ok::<_, Box<Response>>(profile_response(StatusCode::CREATED, &created))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadBody {
    id: String,
}

/// Apply a stored profile to the device; the daemon's reply (the new state, or the reason it
/// refused) is passed through.
async fn load_profile(
    State(state): State<ProfilesState>,
    Path(device): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let parsed: LoadBody = serde_json::from_slice(&body)
            .map_err(|e| invalid(&format!("malformed request: {e}")))?;
        let subject =
            subjects::ctl_profile(&device, "load").map_err(|_| invalid("invalid device id"))?;
        let id = profile_id(&parsed.id)?;
        let stored = profiles::get(pool(&state)?, &id)
            .await
            .map_err(|e| internal(&e))?
            .ok_or_else(not_found)?;
        let payload = serde_json::json!({"v": 1, "profile": stored.data})
            .to_string()
            .into_bytes();
        let reply = call_daemon(&state.devices, subject, payload).await?;
        Ok::<_, Box<Response>>(json(StatusCode::OK, reply))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::OriginPolicy;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn call(request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let devices = DevicesState::new(None, OriginPolicy::default());
        let app = router(ProfilesState::new(devices, None));
        let response = app.oneshot(request).await.expect("infallible");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    fn req(method: &str, path: &str, origin: Option<&str>, body: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "sdr.local:8073");
        if !body.is_empty() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        builder.body(Body::from(body.to_owned())).expect("request")
    }

    const OK_DATA: &str = r#"{"v":1,"center_hz":1,"sample_rate":2}"#;

    #[tokio::test]
    async fn mutations_check_origin_and_body_before_the_database() {
        let body = format!(r#"{{"name":"a","data":{OK_DATA}}}"#);
        let (status, _) = call(req(
            "POST",
            "/api/v1/profiles",
            Some("https://evil.example"),
            &body,
        ))
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        for bad in [
            "",
            "{}",
            r#"{"name":"","data":{"v":1,"center_hz":1,"sample_rate":2}}"#,
            r#"{"name":"a","data":{"v":2,"center_hz":1,"sample_rate":2}}"#,
            r#"{"name":"a","data":{"v":1,"center_hz":1,"sample_rate":2},"x":1}"#,
            r#"{"name":"bad\u0007","data":{"v":1,"center_hz":1,"sample_rate":2}}"#,
        ] {
            let mut request = req("POST", "/api/v1/profiles", None, bad);
            if bad.is_empty() {
                request.headers_mut().insert(
                    header::CONTENT_TYPE,
                    "application/json".parse().expect("header"),
                );
            }
            let (status, body) = call(request).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
            assert_eq!(body["code"], "invalid", "{bad}");
        }
        // Valid input only fails for lack of a database.
        let (status, body) = call(req("POST", "/api/v1/profiles", None, &body)).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, Some("unavailable"))
        );
    }

    #[tokio::test]
    async fn bad_ids_are_not_found_and_reads_need_a_database() {
        let (status, body) = call(req("GET", "/api/v1/profiles/not-a-uuid", None, "")).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::NOT_FOUND, Some("not_found"))
        );
        let (status, _) = call(req("DELETE", "/api/v1/profiles/123", None, "")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(req("GET", "/api/v1/profiles", None, "")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let (status, _) = call(req("GET", &format!("/api/v1/profiles/{id}"), None, "")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn device_actions_validate_their_input() {
        let (status, _) = call(req(
            "POST",
            "/api/v1/devices/rtl0/profile/save",
            None,
            r#"{"name":" "}"#,
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(req(
            "POST",
            "/api/v1/devices/rtl0/profile/load",
            None,
            r#"{"id":"x","y":1}"#,
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(req(
            "POST",
            "/api/v1/devices/rtl0/profile/load",
            None,
            r#"{"id":"x"}"#,
        ))
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn names_are_trimmed_and_limited() {
        assert_eq!(check_name("  Airband  ").as_deref(), Ok("Airband"));
        assert!(check_name("   ").is_err());
        assert!(check_name(&"x".repeat(101)).is_err());
        assert!(check_name(&"ä".repeat(100)).is_ok());
        assert!(check_name("a\nb").is_err());
    }
}
