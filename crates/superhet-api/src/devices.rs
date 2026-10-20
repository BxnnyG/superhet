//! Device state, tuning and VFOs over REST: `GET /api/v1/devices/{dev}/state`,
//! `POST /api/v1/devices/{dev}/tune` and `/api/v1/devices/{dev}/vfos[/{id}]`. Protocol:
//! `docs/api-devices.md`.
//!
//! The API is a thin edge here: tune and VFO bodies are forwarded as `ctl.*` requests to the
//! daemon owning the device (which validates authoritatively) and its reply is passed through;
//! state (and with it the VFO list) is read from the JetStream KV bucket `STATE`, falling back to
//! a `ctl.state` request.

use std::sync::Arc;
use std::time::Duration;

use async_nats::{Client, RequestErrorKind, connection::State as Connection, jetstream};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use superhet_bus::subjects;

use crate::origin::OriginPolicy;

/// Largest accepted request body.
pub const MAX_BODY_BYTES: usize = 4096;
/// How long a bus request may take before the client gets a 504.
const BUS_TIMEOUT: Duration = Duration::from_secs(2);

/// Shared state of the device routes.
#[derive(Clone)]
pub struct DevicesState {
    nats: Option<Client>,
    origins: Arc<OriginPolicy>,
}

impl DevicesState {
    /// Create the state from the bus client (if configured) and the origin policy.
    #[must_use]
    pub fn new(nats: Option<Client>, origins: OriginPolicy) -> Self {
        Self {
            nats,
            origins: Arc::new(origins),
        }
    }
}

/// Routes of the device API, behind the authentication hook.
pub fn router(state: DevicesState) -> Router {
    Router::new()
        .route("/api/v1/devices/{dev}/state", get(get_state))
        .route(
            "/api/v1/devices/{dev}/tune",
            post(post_tune).layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .route(
            "/api/v1/devices/{dev}/vfos",
            get(list_vfos)
                .post(post_vfo)
                .layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .route(
            "/api/v1/devices/{dev}/vfos/{id}",
            axum::routing::patch(patch_vfo)
                .delete(delete_vfo)
                .layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .with_state(state)
}

fn json(status: StatusCode, body: impl Into<Body>) -> Response {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    response
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    json(
        status,
        serde_json::json!({"v": 1, "ok": false, "code": code, "error": message}).to_string(),
    )
}

/// Why a bus request produced no usable reply.
enum BusFailure {
    /// No connection to the bus.
    Offline,
    /// Nobody serves the subject (unknown device or daemon down).
    NoResponders,
    /// The daemon did not answer in time.
    Timeout,
    /// Any other transport error.
    Other,
}

impl BusFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Offline => failure(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "no bus"),
            Self::NoResponders => failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "no such device, or its daemon is not running",
            ),
            Self::Timeout => failure(
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                "the device daemon did not answer in time",
            ),
            Self::Other => failure(StatusCode::BAD_GATEWAY, "bus", "bus request failed"),
        }
    }
}

fn connected(nats: Option<&Client>) -> Result<&Client, BusFailure> {
    match nats {
        Some(client) if client.connection_state() == Connection::Connected => Ok(client),
        _ => Err(BusFailure::Offline),
    }
}

async fn ctl_request(
    client: &Client,
    subject: String,
    payload: Vec<u8>,
) -> Result<Bytes, BusFailure> {
    match tokio::time::timeout(BUS_TIMEOUT, client.request(subject, payload.into())).await {
        Err(_) => Err(BusFailure::Timeout),
        Ok(Ok(message)) => Ok(message.payload),
        Ok(Err(e)) => Err(match e.kind() {
            RequestErrorKind::NoResponders => BusFailure::NoResponders,
            RequestErrorKind::TimedOut => BusFailure::Timeout,
            _ => {
                tracing::warn!(error = %e, "device ctl request failed");
                BusFailure::Other
            }
        }),
    }
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}

/// Parse a request body into a JSON object, `"v"` defaulting to 1.
fn prepare_object(body: &[u8]) -> Result<serde_json::Map<String, serde_json::Value>, &'static str> {
    let serde_json::Value::Object(mut object) =
        serde_json::from_slice(body).map_err(|_| "body is not valid JSON")?
    else {
        return Err("body must be a JSON object");
    };
    match object.get("v") {
        None => {
            object.insert("v".into(), 1.into());
        }
        Some(v) if v.as_u64() == Some(1) => {}
        Some(_) => return Err("unsupported protocol version"),
    }
    Ok(object)
}

/// Parse a tune body into the bus request: a JSON object, `"v"` defaulting to 1.
fn prepare_tune(body: &[u8]) -> Result<Vec<u8>, &'static str> {
    Ok(serde_json::Value::Object(prepare_object(body)?)
        .to_string()
        .into_bytes())
}

/// Parse a VFO update body and bind it to the id of the URL (a differing `"id"` is an error).
fn prepare_vfo_update(body: &[u8], id: u32) -> Result<Vec<u8>, &'static str> {
    let mut object = prepare_object(body)?;
    match object.get("id") {
        None => {}
        Some(v) if v.as_u64() == Some(u64::from(id)) => {}
        Some(_) => return Err("body id does not match the URL"),
    }
    object.insert("id".into(), id.into());
    Ok(serde_json::Value::Object(object).to_string().into_bytes())
}

/// The VFO id of a URL segment.
fn parse_vfo_id(raw: &str) -> Result<u32, &'static str> {
    raw.parse().map_err(|_| "invalid VFO id")
}

/// CSRF and content-type checks shared by all mutating routes. Cookies are sent along with
/// cross-site requests, so the Origin check is what keeps a foreign page from controlling the
/// receiver.
fn guard_mutation(
    state: &DevicesState,
    headers: &HeaderMap,
    has_body: bool,
) -> Result<(), Box<Response>> {
    if !state.origins.allows(headers) {
        tracing::warn!("device control: origin rejected");
        return Err(Box::new(failure(
            StatusCode::FORBIDDEN,
            "forbidden",
            "origin not allowed",
        )));
    }
    if has_body && !is_json_content_type(headers) {
        return Err(Box::new(failure(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid",
            "content type must be application/json",
        )));
    }
    Ok(())
}

/// Send a control request to the daemon and return its reply if it reports success; a failure
/// reply or a bus problem becomes the response for the client (`invalid`/`unsupported` are 400, `not_found` 404).
async fn call_daemon(
    state: &DevicesState,
    subject: String,
    payload: Vec<u8>,
) -> Result<Bytes, Box<Response>> {
    let client = connected(state.nats.as_ref()).map_err(|e| Box::new(e.into_response()))?;
    let reply = ctl_request(client, subject, payload)
        .await
        .map_err(|e| Box::new(e.into_response()))?;
    match serde_json::from_slice::<serde_json::Value>(&reply) {
        Ok(value) if value["ok"] == true => Ok(reply),
        Ok(value) => Err(Box::new(json(
            match value["code"].as_str() {
                Some("invalid" | "unsupported") => StatusCode::BAD_REQUEST,
                Some("not_found") => StatusCode::NOT_FOUND,
                _ => StatusCode::BAD_GATEWAY,
            },
            reply,
        ))),
        Err(_) => Err(Box::new(failure(
            StatusCode::BAD_GATEWAY,
            "bus",
            "malformed daemon reply",
        ))),
    }
}

async fn post_tune(
    State(state): State<DevicesState>,
    Path(device): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = guard_mutation(&state, &headers, true) {
        return *response;
    }
    let Ok(subject) = subjects::ctl(&device, "tune") else {
        return failure(StatusCode::BAD_REQUEST, "invalid", "invalid device id");
    };
    let payload = match prepare_tune(&body) {
        Ok(payload) => payload,
        Err(message) => return failure(StatusCode::BAD_REQUEST, "invalid", message),
    };
    match call_daemon(&state, subject, payload).await {
        Ok(reply) => json(StatusCode::OK, reply),
        Err(response) => *response,
    }
}

async fn post_vfo(
    State(state): State<DevicesState>,
    Path(device): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = guard_mutation(&state, &headers, true) {
        return *response;
    }
    let Ok(subject) = subjects::ctl_vfo(&device, "create") else {
        return failure(StatusCode::BAD_REQUEST, "invalid", "invalid device id");
    };
    let payload = match prepare_tune(&body) {
        Ok(payload) => payload,
        Err(message) => return failure(StatusCode::BAD_REQUEST, "invalid", message),
    };
    let reply = match call_daemon(&state, subject, payload).await {
        Ok(reply) => reply,
        Err(response) => return *response,
    };
    let mut response = json(StatusCode::CREATED, reply.clone());
    if let Some(id) = serde_json::from_slice::<serde_json::Value>(&reply)
        .ok()
        .and_then(|v| v["vfo"]["id"].as_u64())
        && let Ok(location) =
            header::HeaderValue::from_str(&format!("/api/v1/devices/{device}/vfos/{id}"))
    {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

async fn patch_vfo(
    State(state): State<DevicesState>,
    Path((device, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = guard_mutation(&state, &headers, true) {
        return *response;
    }
    let Ok(subject) = subjects::ctl_vfo(&device, "update") else {
        return failure(StatusCode::BAD_REQUEST, "invalid", "invalid device id");
    };
    let payload = match parse_vfo_id(&id).and_then(|id| prepare_vfo_update(&body, id)) {
        Ok(payload) => payload,
        Err(message) => return failure(StatusCode::BAD_REQUEST, "invalid", message),
    };
    match call_daemon(&state, subject, payload).await {
        Ok(reply) => json(StatusCode::OK, reply),
        Err(response) => *response,
    }
}

async fn delete_vfo(
    State(state): State<DevicesState>,
    Path((device, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = guard_mutation(&state, &headers, false) {
        return *response;
    }
    let Ok(subject) = subjects::ctl_vfo(&device, "delete") else {
        return failure(StatusCode::BAD_REQUEST, "invalid", "invalid device id");
    };
    let id = match parse_vfo_id(&id) {
        Ok(id) => id,
        Err(message) => return failure(StatusCode::BAD_REQUEST, "invalid", message),
    };
    let payload = serde_json::json!({"v": 1, "id": id})
        .to_string()
        .into_bytes();
    match call_daemon(&state, subject, payload).await {
        Ok(_) => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            response
        }
        Err(response) => *response,
    }
}

/// The state object from a stored KV value or a `ctl.state` reply.
async fn read_kv(client: &Client, key: &str) -> Option<Bytes> {
    let lookup = async {
        let store = jetstream::new(client.clone())
            .get_key_value(subjects::STATE_BUCKET)
            .await
            .ok()?;
        store.get(key).await.ok().flatten()
    };
    tokio::time::timeout(BUS_TIMEOUT, lookup)
        .await
        .ok()
        .flatten()
}

/// The state object of a device as JSON bytes: KV first, then a `ctl.state` request.
async fn load_state(state: &DevicesState, device: &str) -> Result<Bytes, Box<Response>> {
    let (Ok(key), Ok(subject)) = (subjects::state_key(device), subjects::ctl(device, "state"))
    else {
        return Err(Box::new(failure(
            StatusCode::BAD_REQUEST,
            "invalid",
            "invalid device id",
        )));
    };
    let client = connected(state.nats.as_ref()).map_err(|e| Box::new(e.into_response()))?;
    if let Some(stored) = read_kv(client, &key).await {
        return Ok(stored);
    }
    let reply = ctl_request(client, subject, Vec::new())
        .await
        .map_err(|e| Box::new(e.into_response()))?;
    match serde_json::from_slice::<serde_json::Value>(&reply) {
        Ok(value) if value["ok"] == true && value["state"].is_object() => {
            Ok(value["state"].to_string().into())
        }
        _ => Err(Box::new(failure(
            StatusCode::BAD_GATEWAY,
            "bus",
            "malformed daemon reply",
        ))),
    }
}

async fn get_state(State(state): State<DevicesState>, Path(device): Path<String>) -> Response {
    match load_state(&state, &device).await {
        Ok(stored) => json(StatusCode::OK, stored),
        Err(response) => *response,
    }
}

async fn list_vfos(State(state): State<DevicesState>, Path(device): Path<String>) -> Response {
    let stored = match load_state(&state, &device).await {
        Ok(stored) => stored,
        Err(response) => return *response,
    };
    match serde_json::from_slice::<serde_json::Value>(&stored) {
        Ok(value) if value.is_object() => {
            let vfos = value
                .get("vfos")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            json(
                StatusCode::OK,
                serde_json::json!({"v": 1, "vfos": vfos}).to_string(),
            )
        }
        _ => failure(StatusCode::BAD_GATEWAY, "bus", "malformed device state"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn call(request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let app = router(DevicesState::new(None, OriginPolicy::default()));
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

    fn post_req(
        path: &str,
        content_type: Option<&str>,
        origin: Option<&str>,
        body: &str,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::HOST, "sdr.local:8073");
        if let Some(ct) = content_type {
            builder = builder.header(header::CONTENT_TYPE, ct);
        }
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        builder.body(Body::from(body.to_owned())).expect("request")
    }

    const TUNE: &str = "/api/v1/devices/rtl0/tune";

    #[tokio::test]
    async fn tune_rejects_foreign_origin_before_anything_else() {
        let (status, body) = call(post_req(
            TUNE,
            Some("application/json"),
            Some("https://evil.example"),
            r#"{"center_hz":1}"#,
        ))
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn tune_requires_json_content_type() {
        for ct in [
            None,
            Some("text/plain"),
            Some("application/x-www-form-urlencoded"),
        ] {
            let (status, _) = call(post_req(TUNE, ct, None, r#"{"center_hz":1}"#)).await;
            assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{ct:?}");
        }
        let (status, _) = call(post_req(
            TUNE,
            Some("Application/JSON; charset=utf-8"),
            Some("https://sdr.local:8073"),
            r#"{"center_hz":1}"#,
        ))
        .await;
        // Passes the checks and only fails for lack of a bus.
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn tune_validates_device_and_body() {
        let json_ct = Some("application/json");
        let (status, _) = call(post_req("/api/v1/devices/a*b/tune", json_ct, None, "{}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        for body in ["", "nope", "[]", "5", r#"{"v":2}"#, r#"{"v":"1"}"#] {
            let (status, _) = call(post_req(TUNE, json_ct, None, body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        }
    }

    #[tokio::test]
    async fn tune_body_is_limited() {
        let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(MAX_BODY_BYTES));
        let (status, _) = call(post_req(TUNE, Some("application/json"), None, &big)).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn without_bus_both_routes_are_unavailable() {
        let (status, _) = call(post_req(
            TUNE,
            Some("application/json"),
            None,
            r#"{"ppm":1}"#,
        ))
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let get = Request::builder()
            .uri("/api/v1/devices/rtl0/state")
            .body(Body::empty())
            .expect("request");
        let (status, _) = call(get).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let bad = Request::builder()
            .uri("/api/v1/devices/a.b/state")
            .body(Body::empty())
            .expect("request");
        let (status, _) = call(bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    const VFOS: &str = "/api/v1/devices/rtl0/vfos";

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

    #[tokio::test]
    async fn vfo_mutations_reject_foreign_origin() {
        let evil = Some("https://evil.example");
        for (method, path, body) in [
            ("POST", VFOS.to_owned(), r#"{"freq_hz":1,"mode":"nfm"}"#),
            ("PATCH", format!("{VFOS}/1"), r#"{"freq_hz":2}"#),
            ("DELETE", format!("{VFOS}/1"), ""),
        ] {
            let (status, resp) = call(req(method, &path, evil, body)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method}");
            assert_eq!(resp["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn vfo_bodies_need_json_and_stay_small() {
        for method in ["POST", "PATCH"] {
            let path = if method == "POST" {
                VFOS.to_owned()
            } else {
                format!("{VFOS}/1")
            };
            let (status, _) = call(post_req_method(method, &path, Some("text/plain"), "{}")).await;
            assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{method}");
            let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(MAX_BODY_BYTES));
            let (status, _) = call(post_req_method(
                method,
                &path,
                Some("application/json"),
                &big,
            ))
            .await;
            assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{method}");
        }
    }

    fn post_req_method(
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: &str,
    ) -> Request<Body> {
        let mut request = post_req(path, content_type, None, body);
        *request.method_mut() = method.parse().expect("method");
        request
    }

    #[tokio::test]
    async fn vfo_routes_validate_input_before_the_bus() {
        let json_ct = Some("application/json");
        // Bad device id, bad VFO id and bad bodies are 400; a valid request fails on the bus.
        let (status, _) = call(post_req_method(
            "POST",
            "/api/v1/devices/a*b/vfos",
            json_ct,
            "{}",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        for id in ["x", "-1", "99999999999"] {
            let (status, _) = call(req("DELETE", &format!("{VFOS}/{id}"), None, "")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{id}");
        }
        for body in ["nope", "[]", r#"{"v":2}"#, r#"{"id":2}"#] {
            let (status, _) = call(post_req_method(
                "PATCH",
                &format!("{VFOS}/1"),
                json_ct,
                body,
            ))
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        }
        let (status, _) = call(post_req_method(
            "PATCH",
            &format!("{VFOS}/1"),
            json_ct,
            r#"{"id":1,"freq_hz":5}"#,
        ))
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (status, _) = call(req("DELETE", &format!("{VFOS}/1"), None, "")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (status, _) = call(req("GET", VFOS, None, "")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (status, _) = call(req("GET", "/api/v1/devices/a.b/vfos", None, "")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn prepare_vfo_update_binds_the_url_id() {
        let out = prepare_vfo_update(br#"{"freq_hz":5}"#, 3).expect("valid");
        let value: serde_json::Value = serde_json::from_slice(&out).expect("json");
        assert_eq!(value, serde_json::json!({"v":1,"id":3,"freq_hz":5}));
        assert!(prepare_vfo_update(br#"{"id":3}"#, 3).is_ok());
        assert!(prepare_vfo_update(br#"{"id":4}"#, 3).is_err());
        assert!(prepare_vfo_update(br#"{"id":"3"}"#, 3).is_err());
        assert_eq!(parse_vfo_id("12"), Ok(12));
        assert!(parse_vfo_id("1a").is_err());
    }

    #[test]
    fn prepare_tune_defaults_the_version() {
        let out = prepare_tune(br#"{"center_hz":5}"#).expect("valid");
        let value: serde_json::Value = serde_json::from_slice(&out).expect("json");
        assert_eq!(value["v"], 1);
        assert_eq!(value["center_hz"], 5);
        assert!(prepare_tune(br#"{"v":1,"ppm":1}"#).is_ok());
    }
}
