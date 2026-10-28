//! Plugin administration over REST (`docs/api-plugins.md`): installed plugins and running
//! instances. The API is a thin, audited edge in front of the plugin host's bus control API
//! (`sh.v1.plugin.ctl.<cmd>`, `docs/plugin-contract.md`).
//!
//! Who may do what is decided in `access::RULES`: installing and removing plugins is `admin`,
//! listing plugins and starting, stopping and restarting instances is `operator` or better.
//! Every change is written to the audit log (`plugin.install`, `plugin.remove`, `plugin.start`,
//! `plugin.stop`, `plugin.restart`).

use std::time::Duration;

use async_nats::{Client, client::RequestErrorKind};
use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;
use superhet_bus::subjects;

use crate::access::Principal;
use crate::devices::{DevicesState, connected, failure, guard_mutation, json};

/// Request bodies are small JSON documents.
const MAX_BODY_BYTES: usize = 16 * 1024;
/// `stop` waits for the plugin to end (its grace period is 5 s).
const HOST_TIMEOUT: Duration = Duration::from_secs(12);

/// Routes, behind the access middleware.
pub fn router(state: DevicesState) -> Router {
    let limit = axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES);
    Router::new()
        .route(
            "/api/v1/plugins",
            get(list_plugins).post(install_plugin).layer(limit),
        )
        .route("/api/v1/plugins/{id}", axum::routing::delete(remove_plugin))
        .route(
            "/api/v1/plugin-instances",
            get(list_instances).post(start_instance).layer(limit),
        )
        .route("/api/v1/plugin-instances/{id}", get(get_instance))
        .route("/api/v1/plugin-instances/{id}/stop", post(stop_instance))
        .route(
            "/api/v1/plugin-instances/{id}/restart",
            post(restart_instance),
        )
        .route("/api/v1/plugin-instances/{id}/status", get(instance_status))
        .route(
            "/api/v1/plugin-instances/{id}/media/{key}",
            get(instance_media),
        )
        .route(
            "/api/v1/plugin-instances/{id}/control",
            post(instance_control).layer(limit),
        )
        .route(
            "/api/v1/plugin-instances/{id}/reconfigure",
            post(reconfigure_instance).layer(limit),
        )
        .with_state(state)
}

/// Status of a plugin host failure reply, by its `code`.
fn status_of(code: &str) -> StatusCode {
    match code {
        "invalid" | "unsupported" => StatusCode::BAD_REQUEST,
        "not_found" => StatusCode::NOT_FOUND,
        "conflict" => StatusCode::CONFLICT,
        _ => StatusCode::BAD_GATEWAY,
    }
}

/// Send `body` to the plugin host and return its success reply, or the response to give.
async fn host_call(client: &Client, cmd: &str, mut body: Value) -> Result<Value, Box<Response>> {
    let Ok(subject) = subjects::plugin_ctl(cmd) else {
        return Err(Box::new(failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "invalid command",
        )));
    };
    body["v"] = json!(1);
    let payload = serde_json::to_vec(&body).unwrap_or_default();
    let reply =
        match tokio::time::timeout(HOST_TIMEOUT, client.request(subject, payload.into())).await {
            Err(_) => {
                return Err(Box::new(failure(
                    StatusCode::GATEWAY_TIMEOUT,
                    "timeout",
                    "the plugin host did not answer in time",
                )));
            }
            Ok(Err(err)) => {
                return Err(Box::new(match err.kind() {
                    RequestErrorKind::NoResponders => failure(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "the plugin host is not running",
                    ),
                    _ => {
                        tracing::warn!(%err, "plugin host request failed");
                        failure(StatusCode::BAD_GATEWAY, "bus", "bus request failed")
                    }
                }));
            }
            Ok(Ok(message)) => message.payload,
        };
    match serde_json::from_slice::<Value>(&reply) {
        Ok(value) if value["ok"] == true => Ok(value),
        Ok(value) => Err(Box::new(json(
            status_of(value["code"].as_str().unwrap_or("")),
            reply,
        ))),
        Err(_) => Err(Box::new(failure(
            StatusCode::BAD_GATEWAY,
            "bus",
            "malformed plugin host reply",
        ))),
    }
}

fn reply(status: StatusCode, value: &Value) -> Response {
    json(status, Body::from(value.to_string()))
}

fn bad(message: &str) -> Box<Response> {
    Box::new(failure(StatusCode::BAD_REQUEST, "invalid", message))
}

/// Parse a JSON object body (`"v"` is set by [`host_call`]).
fn object(body: &Bytes) -> Result<serde_json::Map<String, Value>, Box<Response>> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(bad("the body must be a JSON object")),
    }
}

/// A plugin or instance id from the URL: short and without anything but id characters.
fn check_id(raw: &str) -> Result<(), Box<Response>> {
    if !raw.is_empty()
        && raw.len() <= 128
        && raw.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_')
        })
    {
        Ok(())
    } else {
        Err(bad("invalid id"))
    }
}

/// The audit outcome of a host call. (`Response` is not `Sync`, so the result itself must not be
/// held by reference across an await.)
fn outcome_of(result: &Result<Value, Box<Response>>) -> Outcome {
    match result {
        Ok(_) => Outcome::Ok,
        Err(response)
            if matches!(
                response.status(),
                StatusCode::CONFLICT | StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
            ) =>
        {
            Outcome::Denied
        }
        Err(_) => Outcome::Error,
    }
}

/// Record a change in the audit log.
async fn audit(
    state: &DevicesState,
    principal: &Principal,
    action: &str,
    target: &str,
    outcome: Outcome,
    details: Value,
) {
    state
        .auth
        .audit()
        .log(&principal.who(), action, Some(target), outcome, details)
        .await;
}

async fn list_plugins(State(state): State<DevicesState>) -> Response {
    simple_get(&state, "list").await
}

async fn list_instances(State(state): State<DevicesState>) -> Response {
    simple_get(&state, "instances").await
}

async fn simple_get(state: &DevicesState, cmd: &str) -> Response {
    let client = match connected(state.nats.as_ref()) {
        Ok(client) => client,
        Err(err) => return err.into_response(),
    };
    match host_call(client, cmd, json!({})).await {
        Ok(value) => reply(StatusCode::OK, &value),
        Err(response) => *response,
    }
}

async fn get_instance(State(state): State<DevicesState>, Path(id): Path<String>) -> Response {
    if let Err(response) = check_id(&id) {
        return *response;
    }
    let client = match connected(state.nats.as_ref()) {
        Ok(client) => client,
        Err(err) => return err.into_response(),
    };
    match host_call(client, "get", json!({"id": id})).await {
        Ok(value) => reply(StatusCode::OK, &value),
        Err(response) => *response,
    }
}

/// `POST /api/v1/plugins` `{"path": "/dir/with/plugin.toml"}` – install from a path on the
/// plugin host (admin).
async fn install_plugin(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state, &headers, true)?;
        let map = object(&body)?;
        let path = map
            .get("path")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty() && p.len() <= 4096)
            .ok_or_else(|| bad("\"path\" (a directory on the plugin host) is required"))?
            .to_owned();
        if map.len() != 1 {
            return Err(bad("only \"path\" is accepted"));
        }
        let client = connected(state.nats.as_ref()).map_err(|e| Box::new(e.into_response()))?;
        let result = host_call(client, "install", json!({"path": path})).await;
        let target = result
            .as_ref()
            .ok()
            .and_then(|v| v["plugin"]["manifest"]["plugin"]["id"].as_str())
            .unwrap_or(&path)
            .to_owned();
        let version = result.as_ref().ok().and_then(|v| {
            v["plugin"]["manifest"]["plugin"]["version"]
                .as_str()
                .map(str::to_owned)
        });
        let outcome = outcome_of(&result);
        audit(
            &state,
            &principal,
            "plugin.install",
            &target,
            outcome,
            json!({"path": path, "version": version}),
        )
        .await;
        result
    }
    .await;
    match result {
        Ok(value) => reply(StatusCode::CREATED, &value),
        Err(response) => *response,
    }
}

/// `DELETE /api/v1/plugins/{id}` – remove every version of an installed plugin (admin); refused
/// while instances of it exist.
async fn remove_plugin(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        guard_mutation(&state, &headers, false)?;
        check_id(&id)?;
        let client = connected(state.nats.as_ref()).map_err(|e| Box::new(e.into_response()))?;
        let result = host_call(client, "remove", json!({"plugin_id": id})).await;
        let outcome = outcome_of(&result);
        audit(
            &state,
            &principal,
            "plugin.remove",
            &id,
            outcome,
            result
                .as_ref()
                .map_or_else(|_| json!({}), |v| json!({"versions": v["versions"]})),
        )
        .await;
        result
    }
    .await;
    match result {
        Ok(_) => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            response
        }
        Err(response) => *response,
    }
}

/// `POST /api/v1/plugin-instances` `{"plugin_id", "device"?, "vfo"?, "freq_hz"?, "config"?}` –
/// start an instance (operator).
async fn start_instance(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state, &headers, true)?;
        let map = object(&body)?;
        let Some(plugin_id) = map.get("plugin_id").and_then(Value::as_str) else {
            return Err(bad("\"plugin_id\" is required"));
        };
        check_id(plugin_id)?;
        for key in map.keys() {
            if !["plugin_id", "device", "vfo", "freq_hz", "config"].contains(&key.as_str()) {
                return Err(bad(&format!("unknown field {key:?}")));
            }
        }
        // Device ids become subject tokens; reject anything else before it reaches the bus.
        if let Some(device) = map.get("device")
            && !device.is_null()
            && !device
                .as_str()
                .is_some_and(|d| subjects::ctl(d, "x").is_ok() && d.len() <= 64)
        {
            return Err(bad("invalid device"));
        }
        let plugin_id = plugin_id.to_owned();
        let client = connected(state.nats.as_ref()).map_err(|e| Box::new(e.into_response()))?;
        let result = host_call(client, "start", Value::Object(map.clone())).await;
        let target = result
            .as_ref()
            .ok()
            .and_then(|v| v["instance"]["id"].as_str())
            .unwrap_or(&plugin_id)
            .to_owned();
        let outcome = outcome_of(&result);
        audit(
            &state,
            &principal,
            "plugin.start",
            &target,
            outcome,
            json!({
                "plugin_id": plugin_id,
                "device": map.get("device"),
                "vfo": map.get("vfo"),
                "freq_hz": map.get("freq_hz"),
            }),
        )
        .await;
        result
    }
    .await;
    match result {
        Ok(value) => reply(StatusCode::CREATED, &value),
        Err(response) => *response,
    }
}

/// `GET /api/v1/plugin-instances/{id}/status` – what the framed plugin reports about itself
/// (service list, now playing, signal quality, ...), plus the instance's plugin id, device,
/// frequency, state and audio stream, so a listener can tell what holds the device without
/// being allowed to list instances.
async fn instance_status(State(state): State<DevicesState>, Path(id): Path<String>) -> Response {
    if let Err(response) = check_id(&id) {
        return *response;
    }
    let client = match connected(state.nats.as_ref()) {
        Ok(client) => client,
        Err(err) => return err.into_response(),
    };
    match host_call(client, "status", json!({"id": id})).await {
        Ok(value) => reply(StatusCode::OK, &value),
        Err(response) => *response,
    }
}

/// A media key as the plugin protocol defines it: 1..=64 characters of `[a-z0-9._-]`.
fn valid_media_key(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// Header of a successful `media` reply from the plugin host.
struct MediaHead {
    mime: String,
    revision: u64,
}

/// Split a `media` reply: one line of JSON (`ok`, `mime`, `revision`), a newline, the bytes.
/// `None` for a failure reply (plain JSON without newline) or garbage.
fn split_media_reply(payload: &[u8]) -> Option<(MediaHead, &[u8])> {
    let split = payload.iter().position(|&b| b == b'\n')?;
    let header: Value = serde_json::from_slice(&payload[..split]).ok()?;
    (header["ok"] == true).then_some(())?;
    Some((
        MediaHead {
            mime: header["mime"].as_str()?.to_owned(),
            revision: header["revision"].as_u64()?,
        },
        &payload[split + 1..],
    ))
}

/// MIME types served as such; anything else is delivered as an opaque download.
const SERVED_MIME: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

/// `GET /api/v1/plugin-instances/{id}/media/{key}` – the latest binary object a plugin stored
/// under `key` (a slideshow image). The bytes are plugin-supplied, so only image types are
/// labelled as such and `nosniff` is set.
async fn instance_media(
    State(state): State<DevicesState>,
    Path((id, key)): Path<(String, String)>,
) -> Response {
    let check = check_id(&id).and_then(|()| {
        if valid_media_key(&key) {
            Ok(())
        } else {
            Err(bad("invalid key"))
        }
    });
    if let Err(response) = check {
        return *response;
    }
    let client = match connected(state.nats.as_ref()) {
        Ok(client) => client,
        Err(err) => return err.into_response(),
    };
    let Ok(subject) = subjects::plugin_ctl("media") else {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "invalid command",
        );
    };
    let payload = json!({"v": 1, "id": id, "key": key}).to_string();
    let message = match tokio::time::timeout(
        HOST_TIMEOUT,
        client.request(subject, payload.into_bytes().into()),
    )
    .await
    {
        Ok(Ok(message)) => message,
        Ok(Err(err)) if matches!(err.kind(), RequestErrorKind::NoResponders) => {
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "the plugin host is not running",
            );
        }
        Ok(Err(_)) => return failure(StatusCode::BAD_GATEWAY, "bus", "bus request failed"),
        Err(_) => {
            return failure(
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                "the plugin host did not answer in time",
            );
        }
    };
    let Some((header, bytes)) = split_media_reply(&message.payload) else {
        // A failure reply is JSON with a `code`.
        return match serde_json::from_slice::<Value>(&message.payload) {
            Ok(value) => json(
                status_of(value["code"].as_str().unwrap_or("")),
                message.payload.clone(),
            ),
            Err(_) => failure(
                StatusCode::BAD_GATEWAY,
                "bus",
                "malformed plugin host reply",
            ),
        };
    };
    let mime = if SERVED_MIME.contains(&header.mime.as_str()) {
        header.mime.as_str()
    } else {
        "application/octet-stream"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime)
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "private, no-cache")
        .header("ETag", format!("\"{}\"", header.revision))
        .body(Body::from(bytes.to_vec()))
        .unwrap_or_else(|_| failure(StatusCode::INTERNAL_SERVER_ERROR, "internal", "response"))
}

/// `POST /api/v1/plugin-instances/{id}/control` – a JSON object for the plugin (select a DAB
/// service, start a scan; the vocabulary is the plugin's) (operator).
async fn instance_control(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state, &headers, true)?;
        check_id(&id)?;
        let message = object(&body)?;
        let client = connected(state.nats.as_ref()).map_err(|e| Box::new(e.into_response()))?;
        let result = host_call(
            client,
            "control",
            json!({"id": id, "message": Value::Object(message.clone())}),
        )
        .await;
        let outcome = outcome_of(&result);
        audit(
            &state,
            &principal,
            "plugin.control",
            &id,
            outcome,
            json!({"message": Value::Object(message)}),
        )
        .await;
        result
    }
    .await;
    match result {
        Ok(value) => reply(StatusCode::OK, &value),
        Err(response) => *response,
    }
}

/// `POST /api/v1/plugin-instances/{id}/stop` and `/restart` (operator).
async fn stop_instance(
    state: State<DevicesState>,
    principal: Extension<Principal>,
    id: Path<String>,
    headers: HeaderMap,
) -> Response {
    change_instance(state, principal, id, headers, "stop").await
}

async fn restart_instance(
    state: State<DevicesState>,
    principal: Extension<Principal>,
    id: Path<String>,
    headers: HeaderMap,
) -> Response {
    change_instance(state, principal, id, headers, "restart").await
}

/// `POST /api/v1/plugin-instances/{id}/reconfigure` `{"config": {...}}` – replace the
/// configuration and restart the process with it (operator).
async fn reconfigure_instance(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state, &headers, true)?;
        check_id(&id)?;
        let map = object(&body)?;
        for key in map.keys() {
            if key != "config" {
                return Err(bad(&format!("unknown field {key:?}")));
            }
        }
        let config = map.get("config").cloned().unwrap_or(Value::Null);
        let client = connected(state.nats.as_ref()).map_err(|e| Box::new(e.into_response()))?;
        let result = host_call(client, "reconfigure", json!({"id": id, "config": config})).await;
        let outcome = outcome_of(&result);
        audit(
            &state,
            &principal,
            "plugin.reconfigure",
            &id,
            outcome,
            result.as_ref().map_or_else(
                |_| json!({}),
                |v| json!({"plugin_id": v["instance"]["plugin_id"]}),
            ),
        )
        .await;
        result
    }
    .await;
    match result {
        Ok(value) => reply(StatusCode::OK, &value),
        Err(response) => *response,
    }
}

async fn change_instance(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    cmd: &str,
) -> Response {
    let result = async {
        guard_mutation(&state, &headers, false)?;
        check_id(&id)?;
        let client = connected(state.nats.as_ref()).map_err(|e| Box::new(e.into_response()))?;
        let result = host_call(client, cmd, json!({"id": id})).await;
        let details = result.as_ref().map_or_else(
            |_| json!({}),
            |v| json!({"plugin_id": v["instance"]["plugin_id"]}),
        );
        let outcome = outcome_of(&result);
        audit(
            &state,
            &principal,
            &format!("plugin.{cmd}"),
            &id,
            outcome,
            details,
        )
        .await;
        result
    }
    .await;
    match result {
        Ok(value) => reply(StatusCode::OK, &value),
        Err(response) => *response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn call(method: &str, path: &str, body: &str) -> (StatusCode, Value) {
        let app = crate::access::as_test_principal(router(DevicesState::for_tests()));
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .expect("request");
        let response = app.oneshot(request).await.expect("infallible");
        let status = response.status();
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .expect("body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn input_is_validated_before_the_bus() {
        for (method, path, body, want) in [
            ("POST", "/api/v1/plugins", "[]", StatusCode::BAD_REQUEST),
            ("POST", "/api/v1/plugins", "{}", StatusCode::BAD_REQUEST),
            (
                "POST",
                "/api/v1/plugins",
                r#"{"path":"/x","extra":1}"#,
                StatusCode::BAD_REQUEST,
            ),
            (
                "DELETE",
                "/api/v1/plugins/Bad%20Id",
                "",
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/api/v1/plugin-instances",
                "{}",
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/api/v1/plugin-instances",
                r#"{"plugin_id":"a.b.c","x":1}"#,
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/api/v1/plugin-instances",
                r#"{"plugin_id":"a.b.c","device":"a.b"}"#,
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/api/v1/plugin-instances/BAD/stop",
                "",
                StatusCode::BAD_REQUEST,
            ),
            (
                "GET",
                "/api/v1/plugin-instances/BAD",
                "",
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let (status, reply) = call(method, path, body).await;
            assert_eq!(status, want, "{method} {path} {body}: {reply}");
            assert_eq!(reply["code"], "invalid", "{method} {path}");
        }
    }

    #[tokio::test]
    async fn without_a_bus_everything_is_unavailable() {
        for (method, path, body) in [
            ("GET", "/api/v1/plugins", ""),
            ("GET", "/api/v1/plugin-instances", ""),
            ("GET", "/api/v1/plugin-instances/i-1", ""),
            ("POST", "/api/v1/plugins", r#"{"path":"/x"}"#),
            ("DELETE", "/api/v1/plugins/a.b.c", ""),
            (
                "POST",
                "/api/v1/plugin-instances",
                r#"{"plugin_id":"a.b.c"}"#,
            ),
            ("POST", "/api/v1/plugin-instances/i-1/stop", ""),
            ("POST", "/api/v1/plugin-instances/i-1/restart", ""),
            ("GET", "/api/v1/plugin-instances/i-1/status", ""),
            (
                "POST",
                "/api/v1/plugin-instances/i-1/control",
                r#"{"select":"c224"}"#,
            ),
        ] {
            let (status, reply) = call(method, path, body).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{method} {path}");
            assert_eq!(reply["code"], "unavailable");
        }
    }

    #[test]
    fn media_replies_and_keys_are_parsed_strictly() {
        let mut ok =
            br#"{"v":1,"ok":true,"key":"slide.a","mime":"image/png","revision":4}"#.to_vec();
        ok.extend_from_slice(b"\n\x89PNG\n\x00");
        let (head, bytes) = split_media_reply(&ok).expect("reply");
        assert_eq!((head.mime.as_str(), head.revision), ("image/png", 4));
        assert_eq!(bytes, b"\x89PNG\n\x00", "bytes may contain newlines");
        let failure = br#"{"v":1,"ok":false,"code":"not_found","error":"no"}"#;
        assert!(split_media_reply(failure).is_none());
        assert!(split_media_reply(b"{\"ok\":true}\nx").is_none());
        assert!(valid_media_key("slide.c224") && valid_media_key("a_b-1"));
        assert!(!valid_media_key("") && !valid_media_key("../x") && !valid_media_key("A"));
        assert!(!valid_media_key(&"a".repeat(65)));
    }

    #[test]
    fn host_codes_map_to_statuses() {
        assert_eq!(status_of("invalid"), StatusCode::BAD_REQUEST);
        assert_eq!(status_of("unsupported"), StatusCode::BAD_REQUEST);
        assert_eq!(status_of("not_found"), StatusCode::NOT_FOUND);
        assert_eq!(status_of("conflict"), StatusCode::CONFLICT);
        assert_eq!(status_of("internal"), StatusCode::BAD_GATEWAY);
    }
}
