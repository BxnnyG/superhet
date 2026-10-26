//! Device state, tuning, VFOs and the tuning lock over REST: `GET /api/v1/devices/{dev}/state`,
//! `POST /api/v1/devices/{dev}/tune`, `/api/v1/devices/{dev}/vfos[/{id}]` and
//! `POST|DELETE /api/v1/devices/{dev}/lock`. Protocol: `docs/api-devices.md`.
//!
//! The API is a thin edge here: tune and VFO bodies are forwarded as `ctl.*` requests to the
//! daemon owning the device (which validates authoritatively) and its reply is passed through;
//! state (and with it the VFO list) is read from the JetStream KV bucket `STATE`, falling back to
//! a `ctl.state` request.

use std::sync::Arc;
use std::time::Duration;

use async_nats::{Client, RequestErrorKind, connection::State as Connection, jetstream};
use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use serde_json::{Map, Value, json};
use superhet_bus::{audit::Outcome, subjects};
use superhet_db::users::Role;

use crate::access::{Principal, view_state};
use crate::auth::AuthState;
use crate::origin::OriginPolicy;

/// Largest accepted request body.
pub const MAX_BODY_BYTES: usize = 4096;
/// How long a bus request may take before the client gets a 504.
const BUS_TIMEOUT: Duration = Duration::from_secs(2);

/// Shared state of the device routes.
#[derive(Clone)]
pub struct DevicesState {
    pub(crate) nats: Option<Client>,
    pub(crate) origins: Arc<OriginPolicy>,
    pub(crate) auth: AuthState,
}

impl DevicesState {
    /// Create the state from the bus client (if configured), the origin policy and the auth
    /// layer (audit log, settings, live registry).
    #[must_use]
    pub fn new(nats: Option<Client>, origins: OriginPolicy, auth: AuthState) -> Self {
        Self {
            nats,
            origins: Arc::new(origins),
            auth,
        }
    }
}

#[cfg(test)]
impl DevicesState {
    /// A state without bus and database for unit tests of single routes.
    #[must_use]
    pub fn for_tests() -> Self {
        Self::new(None, OriginPolicy::default(), AuthState::for_tests())
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
            "/api/v1/devices/{dev}/lock",
            post(post_lock)
                .delete(delete_lock)
                .layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
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

pub(crate) fn json(status: StatusCode, body: impl Into<Body>) -> Response {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    response
}

pub(crate) fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    json(
        status,
        serde_json::json!({"v": 1, "ok": false, "code": code, "error": message}).to_string(),
    )
}

/// Why a bus request produced no usable reply.
pub(crate) enum BusFailure {
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
    pub(crate) fn into_response(self) -> Response {
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

pub(crate) fn connected(nats: Option<&Client>) -> Result<&Client, BusFailure> {
    match nats {
        Some(client) if client.connection_state() == Connection::Connected => Ok(client),
        _ => Err(BusFailure::Offline),
    }
}

pub(crate) async fn ctl_request(
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
#[cfg(test)]
fn prepare_tune(body: &[u8]) -> Result<Vec<u8>, &'static str> {
    Ok(serde_json::Value::Object(prepare_object(body)?)
        .to_string()
        .into_bytes())
}

/// Parse a VFO update body and bind it to the id of the URL (a differing `"id"` is an error).
fn prepare_vfo_patch(body: &[u8], id: u32) -> Result<Map<String, Value>, &'static str> {
    let mut object = prepare_object(body)?;
    match object.get("id") {
        None => {}
        Some(v) if v.as_u64() == Some(u64::from(id)) => {}
        Some(_) => return Err("body id does not match the URL"),
    }
    object.insert("id".into(), id.into());
    Ok(object)
}

#[cfg(test)]
fn prepare_vfo_update(body: &[u8], id: u32) -> Result<Vec<u8>, &'static str> {
    Ok(Value::Object(prepare_vfo_patch(body, id)?)
        .to_string()
        .into_bytes())
}

fn forbidden(code: &str, message: &str) -> Response {
    failure(StatusCode::FORBIDDEN, code, message)
}

/// The VFO objects of a state document.
fn vfos_of(doc: &Value) -> &[Value] {
    doc.get("vfos")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The most VFOs `principal` may own at once (`0` = no limit beyond the daemon's cap).
async fn vfo_limit(state: &DevicesState, principal: &Principal) -> u32 {
    if principal.is_guest() {
        return state
            .auth
            .settings()
            .guest_policy()
            .await
            .max_vfos_per_guest;
    }
    let limits = state.auth.settings().limits().await;
    match principal.role {
        Role::Listener => limits.listener_max_vfos,
        Role::Operator => limits.operator_max_vfos,
        Role::Admin => 0,
    }
}

/// Guest policy for a requested mode / frequency (`None` fields are not requested).
async fn check_guest_request(
    state: &DevicesState,
    principal: &Principal,
    object: &Map<String, Value>,
) -> Result<(), Box<Response>> {
    if !principal.is_guest() {
        return Ok(());
    }
    let policy = state.auth.settings().guest_policy().await;
    if let Some(mode) = object.get("mode").and_then(Value::as_str)
        && !policy.allows_mode(mode)
    {
        return Err(Box::new(forbidden(
            "mode_not_allowed",
            "this mode is not allowed for guests",
        )));
    }
    if let Some(freq) = object.get("freq_hz").and_then(Value::as_u64)
        && !policy.allows_freq(freq)
    {
        return Err(Box::new(forbidden(
            "frequency_not_allowed",
            "this frequency is outside the range allowed for guests",
        )));
    }
    Ok(())
}

/// Role, guest policy and quota checks before a VFO is created. Must run under the VFO gate.
async fn check_vfo_create(
    state: &DevicesState,
    principal: &Principal,
    device: &str,
    object: &Map<String, Value>,
) -> Result<(), Box<Response>> {
    check_guest_request(state, principal, object).await?;
    let limit = vfo_limit(state, principal).await;
    let only_without_lock =
        principal.is_guest() && state.auth.settings().guest_policy().await.only_without_lock;
    if limit == 0 && !only_without_lock {
        return Ok(());
    }
    let doc = ctl_state(state, device).await?;
    if only_without_lock
        && doc
            .get("locks")
            .and_then(Value::as_array)
            .is_some_and(|l| !l.is_empty())
    {
        return Err(Box::new(forbidden(
            "locked",
            "guests cannot create VFOs while the tuning is locked",
        )));
    }
    let me = principal.owner_id();
    let owned = vfos_of(&doc)
        .iter()
        .filter(|v| v.get("owner").and_then(Value::as_str) == Some(me.as_str()))
        .count();
    if limit > 0 && owned >= limit as usize {
        return Err(Box::new(forbidden(
            "vfo_limit",
            &format!("at most {limit} VFOs per client"),
        )));
    }
    Ok(())
}

/// Ownership and guest policy before a VFO is changed or deleted.
async fn check_vfo_modify(
    state: &DevicesState,
    principal: &Principal,
    device: &str,
    id: u32,
    patch: Option<&Map<String, Value>>,
) -> Result<(), Box<Response>> {
    if principal.sees_all_vfos() {
        return Ok(());
    }
    let doc = ctl_state(state, device).await?;
    let me = principal.owner_id();
    let Some(vfo) = vfos_of(&doc)
        .iter()
        .find(|v| v.get("id").and_then(Value::as_u64) == Some(u64::from(id)))
    else {
        return Err(Box::new(failure(
            StatusCode::NOT_FOUND,
            "not_found",
            &format!("no VFO with id {id}"),
        )));
    };
    if vfo.get("owner").and_then(Value::as_str) != Some(me.as_str()) {
        return Err(Box::new(forbidden(
            "not_owner",
            "this VFO belongs to somebody else",
        )));
    }
    if let Some(patch) = patch {
        check_guest_request(state, principal, patch).await?;
    }
    Ok(())
}

/// Audit a refused or failed mutation.
fn audit_refusal(
    state: &DevicesState,
    principal: &Principal,
    action: &str,
    target: &str,
    response: &Response,
) {
    let outcome = if matches!(
        response.status(),
        StatusCode::FORBIDDEN | StatusCode::CONFLICT | StatusCode::NOT_FOUND
    ) {
        Outcome::Denied
    } else {
        Outcome::Error
    };
    state.auth.audit().log_coalesced(
        &principal.who(),
        action,
        Some(target),
        outcome,
        json!({"status": response.status().as_u16()}),
    );
}

/// The VFO id of a URL segment.
fn parse_vfo_id(raw: &str) -> Result<u32, &'static str> {
    raw.parse().map_err(|_| "invalid VFO id")
}

/// CSRF and content-type checks shared by all mutating routes. Cookies are sent along with
/// cross-site requests, so the Origin check is what keeps a foreign page from controlling the
/// receiver.
pub(crate) fn guard_mutation(
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
/// reply or a bus problem becomes the response for the client (`invalid`/`unsupported` are 400,
/// `not_found` 404, `locked` 409).
pub(crate) async fn call_daemon(
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
                Some("locked") => StatusCode::CONFLICT,
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
    Extension(principal): Extension<Principal>,
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
    let object = match prepare_object(&body) {
        Ok(object) => object,
        Err(message) => return failure(StatusCode::BAD_REQUEST, "invalid", message),
    };
    state.auth.registry().note_device(&device);
    let payload = Value::Object(object.clone()).to_string().into_bytes();
    match call_daemon(&state, subject, payload).await {
        Ok(reply) => {
            let mut details = object;
            details.remove("v");
            state.auth.audit().log_coalesced(
                &principal.who(),
                "device.tune",
                Some(&device),
                Outcome::Ok,
                Value::Object(details),
            );
            json(StatusCode::OK, reply)
        }
        Err(response) => {
            audit_refusal(&state, &principal, "device.tune", &device, &response);
            *response
        }
    }
}

/// `POST` takes the user lock, `DELETE` releases it; both are idempotent and reply with the new
/// state like `/tune`.
async fn set_lock(
    state: &DevicesState,
    principal: &Principal,
    device: &str,
    headers: &HeaderMap,
    acquire: bool,
    force: bool,
) -> Response {
    if let Err(response) = guard_mutation(state, headers, false) {
        return *response;
    }
    let Ok(subject) = subjects::ctl_lock(device, if acquire { "acquire" } else { "release" })
    else {
        return failure(StatusCode::BAD_REQUEST, "invalid", "invalid device id");
    };
    if force && (acquire || principal.role < Role::Admin) {
        return forbidden("forbidden", "only an admin may release the locks of others");
    }
    let payload = if force {
        json!({"v": 1, "force": true})
    } else if acquire {
        json!({"v": 1, "holder": principal.owner_id(), "name": principal.name})
    } else {
        json!({"v": 1, "holder": principal.owner_id()})
    };
    state.auth.registry().note_device(device);
    let action = if acquire {
        "device.lock"
    } else {
        "device.unlock"
    };
    match call_daemon(state, subject, payload.to_string().into_bytes()).await {
        Ok(reply) => {
            state
                .auth
                .audit()
                .log(
                    &principal.who(),
                    action,
                    Some(device),
                    Outcome::Ok,
                    json!({"force": force}),
                )
                .await;
            json(StatusCode::OK, reply)
        }
        Err(response) => {
            audit_refusal(state, principal, action, device, &response);
            *response
        }
    }
}

#[derive(serde::Deserialize)]
struct LockQuery {
    #[serde(default)]
    force: bool,
}

async fn post_lock(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    Path(device): Path<String>,
    headers: HeaderMap,
) -> Response {
    set_lock(&state, &principal, &device, &headers, true, false).await
}

async fn delete_lock(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    Path(device): Path<String>,
    Query(query): Query<LockQuery>,
    headers: HeaderMap,
) -> Response {
    set_lock(&state, &principal, &device, &headers, false, query.force).await
}

async fn post_vfo(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
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
    let mut object = match prepare_object(&body) {
        Ok(object) => object,
        Err(message) => return failure(StatusCode::BAD_REQUEST, "invalid", message),
    };
    // The owner is always the caller; a client-supplied value is discarded.
    object.insert("owner".into(), principal.owner_id().into());
    state.auth.registry().note_device(&device);
    let target = format!("{device}/new");
    // Count-then-create must not interleave with another create.
    let _gate = state.auth.registry().vfo_gate().await;
    if let Err(response) = check_vfo_create(&state, &principal, &device, &object).await {
        audit_refusal(&state, &principal, "vfo.create", &target, &response);
        return *response;
    }
    let payload = Value::Object(object.clone()).to_string().into_bytes();
    let reply = match call_daemon(&state, subject, payload).await {
        Ok(reply) => reply,
        Err(response) => {
            audit_refusal(&state, &principal, "vfo.create", &target, &response);
            return *response;
        }
    };
    let created = serde_json::from_slice::<Value>(&reply)
        .ok()
        .and_then(|v| v["vfo"]["id"].as_u64());
    state
        .auth
        .audit()
        .log(
            &principal.who(),
            "vfo.create",
            Some(&format!("{device}/{}", created.unwrap_or(0))),
            Outcome::Ok,
            json!({"freq_hz": object.get("freq_hz"), "mode": object.get("mode")}),
        )
        .await;
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
    Extension(principal): Extension<Principal>,
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
    let (vfo_id, patch) = match parse_vfo_id(&id)
        .and_then(|vfo_id| prepare_vfo_patch(&body, vfo_id).map(|patch| (vfo_id, patch)))
    {
        Ok(parsed) => parsed,
        Err(message) => return failure(StatusCode::BAD_REQUEST, "invalid", message),
    };
    let target = format!("{device}/{vfo_id}");
    if let Err(response) = check_vfo_modify(&state, &principal, &device, vfo_id, Some(&patch)).await
    {
        audit_refusal(&state, &principal, "vfo.update", &target, &response);
        return *response;
    }
    let payload = Value::Object(patch.clone()).to_string().into_bytes();
    match call_daemon(&state, subject, payload).await {
        Ok(reply) => {
            let mut details = patch;
            details.remove("v");
            details.remove("id");
            state.auth.audit().log_coalesced(
                &principal.who(),
                "vfo.update",
                Some(&target),
                Outcome::Ok,
                Value::Object(details),
            );
            json(StatusCode::OK, reply)
        }
        Err(response) => {
            audit_refusal(&state, &principal, "vfo.update", &target, &response);
            *response
        }
    }
}

async fn delete_vfo(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
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
    let target = format!("{device}/{id}");
    if let Err(response) = check_vfo_modify(&state, &principal, &device, id, None).await {
        audit_refusal(&state, &principal, "vfo.delete", &target, &response);
        return *response;
    }
    let payload = serde_json::json!({"v": 1, "id": id})
        .to_string()
        .into_bytes();
    match call_daemon(&state, subject, payload).await {
        Ok(_) => {
            state
                .auth
                .audit()
                .log(
                    &principal.who(),
                    "vfo.delete",
                    Some(&target),
                    Outcome::Ok,
                    json!({}),
                )
                .await;
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            response
        }
        Err(response) => {
            audit_refusal(&state, &principal, "vfo.delete", &target, &response);
            *response
        }
    }
}

/// The state object from a stored KV value or a `ctl.state` reply.
pub(crate) async fn read_kv(client: &Client, key: &str) -> Option<Bytes> {
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

/// The state object straight from the daemon (`ctl.state`), never from KV.
pub(crate) async fn ctl_state(
    state: &DevicesState,
    device: &str,
) -> Result<serde_json::Value, Box<Response>> {
    let Ok(subject) = subjects::ctl(device, "state") else {
        return Err(Box::new(failure(
            StatusCode::BAD_REQUEST,
            "invalid",
            "invalid device id",
        )));
    };
    let reply = call_daemon(state, subject, Vec::new()).await?;
    match serde_json::from_slice::<serde_json::Value>(&reply) {
        Ok(mut value) if value["state"].is_object() => Ok(value["state"].take()),
        _ => Err(Box::new(failure(
            StatusCode::BAD_GATEWAY,
            "bus",
            "malformed daemon reply",
        ))),
    }
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

async fn get_state(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    Path(device): Path<String>,
) -> Response {
    state.auth.registry().note_device(&device);
    let stored = match load_state(&state, &device).await {
        Ok(stored) => stored,
        Err(response) => return *response,
    };
    match serde_json::from_slice::<Value>(&stored) {
        Ok(mut doc) if doc.is_object() => {
            view_state(&mut doc, &principal);
            json(StatusCode::OK, doc.to_string())
        }
        _ => failure(StatusCode::BAD_GATEWAY, "bus", "malformed device state"),
    }
}

async fn list_vfos(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    Path(device): Path<String>,
) -> Response {
    let stored = match load_state(&state, &device).await {
        Ok(stored) => stored,
        Err(response) => return *response,
    };
    match serde_json::from_slice::<serde_json::Value>(&stored) {
        Ok(mut value) if value.is_object() => {
            view_state(&mut value, &principal);
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
        let app = crate::access::as_test_principal(router(DevicesState::for_tests()));
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
