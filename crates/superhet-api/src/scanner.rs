//! Scanner control over REST: `POST /api/v1/devices/{dev}/scanner/{action}` with `action` one of
//! `start`, `stop`, `pause`, `resume`, `skip`, `lockout`, `unlock`. Protocol: `docs/scanner.md`.
//!
//! A thin edge like the tune route: the body is forwarded as `ctl.scanner.<action>` to the daemon
//! owning the device, which validates it and answers with the new device state (the scanner's
//! state is its `scanner` object). Operators and admins only; the state itself is readable by
//! everybody who can read the device state.
//!
//! `start` also takes `record_hits` (boolean, handled here, not by the daemon): while the scan
//! runs, every hit is recorded as a squelch-gated audio recording (`recorder.rs`, ADR-0028).

use std::sync::Arc;

use axum::{
    Extension, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use serde_json::{Map, Value, json};
use superhet_bus::{audit::Outcome, subjects};

use crate::access::Principal;
use crate::devices::{DevicesState, call_daemon, failure, guard_mutation, json, viewed};
use crate::recorder::{Manager, Owner};

/// Largest accepted body: a channel list of up to 2000 entries.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// The commands the daemon knows.
const ACTIONS: [&str; 7] = [
    "start", "stop", "pause", "resume", "skip", "lockout", "unlock",
];

/// Routes of the scanner API, behind the authentication hook.
pub fn router(state: DevicesState, recorder: Arc<Manager>) -> Router {
    Router::new()
        .route(
            "/api/v1/devices/{dev}/scanner/{action}",
            post(post_scanner).layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .layer(Extension(recorder))
        .with_state(state)
}

/// The body as the bus request: a JSON object (an empty body is `{}`), `"v"` defaulting to 1.
fn prepare(body: &[u8]) -> Result<Map<String, Value>, &'static str> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Map::from_iter([("v".to_owned(), json!(1))]));
    }
    let Value::Object(mut object) =
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

async fn post_scanner(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
    Extension(recorder): Extension<Arc<Manager>>,
    Path((device, action)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = guard_mutation(&state, &headers, !body.is_empty()) {
        return *response;
    }
    if !ACTIONS.contains(&action.as_str()) {
        return failure(StatusCode::NOT_FOUND, "not_found", "unknown scanner action");
    }
    let Ok(subject) = subjects::ctl_scanner(&device, &action) else {
        return failure(StatusCode::BAD_REQUEST, "invalid", "invalid device id");
    };
    let mut object = match prepare(&body) {
        Ok(object) => object,
        Err(message) => return failure(StatusCode::BAD_REQUEST, "invalid", message),
    };
    let record_hits = match object.remove("record_hits") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(on)) => on,
        Some(_) => {
            return failure(
                StatusCode::BAD_REQUEST,
                "invalid",
                "record_hits must be a boolean",
            );
        }
    };
    state.auth.registry().note_device(&device);
    let payload = Value::Object(object.clone()).to_string().into_bytes();
    let audit_action = format!("device.scanner.{action}");
    match call_daemon(&state, subject, payload).await {
        Ok(reply) => {
            match action.as_str() {
                "start" if record_hits => recorder.enable_hit_recording(
                    &device,
                    Owner {
                        name: principal.name.clone(),
                        id: principal.owner_id(),
                    },
                    principal.who(),
                ),
                "start" | "stop" => recorder.disable_hit_recording(&device).await,
                _ => {}
            }
            // Never log a whole channel list: the action and what it addressed are enough.
            let details = match action.as_str() {
                "start" => {
                    json!({"source": object.get("source").and_then(|s| s.get("kind")), "record_hits": record_hits})
                }
                "lockout" | "unlock" => json!({"freq_hz": object.get("freq_hz")}),
                _ => json!({}),
            };
            state
                .auth
                .audit()
                .log(
                    &principal.who(),
                    &audit_action,
                    Some(&device),
                    Outcome::Ok,
                    details,
                )
                .await;
            json(StatusCode::OK, viewed(reply, &principal))
        }
        Err(response) => {
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
                &audit_action,
                Some(&device),
                outcome,
                json!({"status": response.status().as_u16()}),
            );
            *response
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    async fn call(request: Request<Body>) -> (StatusCode, Value) {
        let router = router(
            DevicesState::for_tests(),
            Manager::for_tests(std::env::temp_dir().join("superhet-scanner-test")),
        )
        .layer(Extension(Principal::test(
            superhet_db::users::Role::Operator,
        )));
        let response = router.oneshot(request).await.expect("infallible");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn post_to(path: &str, origin: Option<&str>, body: &str) -> Request<Body> {
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(origin) = origin {
            request = request.header("origin", origin).header("host", "sh.test");
        }
        request.body(Body::from(body.to_owned())).expect("request")
    }

    #[test]
    fn empty_and_object_bodies_get_a_version() {
        assert_eq!(prepare(b"").expect("empty")["v"], 1);
        assert_eq!(prepare(b" \n").expect("blank")["v"], 1);
        assert_eq!(prepare(br#"{"freq_hz":5}"#).expect("object")["v"], 1);
        assert!(prepare(b"[]").is_err());
        assert!(prepare(b"nope").is_err());
        assert!(prepare(br#"{"v":2}"#).is_err());
    }

    #[tokio::test]
    async fn rejects_foreign_origins_unknown_actions_and_bad_bodies() {
        let (status, _) = call(post_to(
            "/api/v1/devices/dev0/scanner/stop",
            Some("https://evil.example"),
            "{}",
        ))
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = call(post_to("/api/v1/devices/dev0/scanner/format", None, "{}")).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::NOT_FOUND, Some("not_found"))
        );
        let (status, _) = call(post_to("/api/v1/devices/dev0/scanner/start", None, "[1]")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // Valid, but nobody serves the bus here.
        let (status, _) = call(post_to("/api/v1/devices/dev0/scanner/stop", None, "")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
}
