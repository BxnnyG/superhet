//! Static files of plugin web panels (`[ui]` in a plugin manifest, ADR-0022,
//! `docs/plugins-ui-panels.md`).
//!
//! `GET /api/v1/plugin-ui/{plugin_id}/{path}` asks the plugin host for one file below the plugin's
//! UI root and serves it with a locked-down response: a CSP with `sandbox allow-scripts` (so the
//! page gets an opaque origin even when opened directly, and can load only its own files, run
//! scripts and nothing else - no network, no frames, no forms), `nosniff`, no referrer.
//!
//! The route is **public** on purpose, like the static web UI: a sandboxed frame has an opaque
//! origin, its sub-resource requests never carry the `SameSite=Strict` session cookie, and a panel
//! must never be able to act with the viewer's session. The files are plugin code that an admin
//! installed; plugins must not keep secrets in their UI directory. The web app decides who sees a
//! panel (`GET /api/v1/plugin-panels`, listeners and guests with decoder access).

use std::time::Duration;

use async_nats::client::RequestErrorKind;
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
    routing::get,
};
use serde_json::{Value, json};
use superhet_bus::subjects;

use crate::devices::{DevicesState, connected, failure};

/// Wait for the plugin host (it only reads a file).
const HOST_TIMEOUT: Duration = Duration::from_secs(5);

/// What every panel response carries. `sandbox allow-scripts` without `allow-same-origin` gives
/// the document a unique origin even if it is opened outside a frame; `connect-src 'none'` and
/// `form-action 'none'` close the remaining ways to talk to the server; `frame-ancestors 'self'`
/// lets only this web app embed it.
pub const PANEL_CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'self'; \
style-src 'self'; img-src 'self' data:; font-src 'self'; media-src 'none'; connect-src 'none'; \
frame-src 'none'; object-src 'none'; worker-src 'none'; manifest-src 'none'; \
form-action 'none'; base-uri 'none'; frame-ancestors 'self'";

/// Public routes (outside the session middleware).
pub fn router(state: DevicesState) -> Router {
    Router::new()
        .route("/api/v1/plugin-ui/{plugin_id}/{*path}", get(asset))
        .with_state(state)
}

/// Plugin ids and asset paths as the plugin protocol defines them (`superhet-plugin-protocol`
/// `manifest::is_valid_plugin_id` and `ui::valid_asset_path`; a test pins the two together). The
/// host checks again, this keeps junk off the bus.
fn valid_plugin_id(id: &str) -> bool {
    if id.len() > 128 {
        return false;
    }
    let mut segments = 0;
    for segment in id.split('.') {
        segments += 1;
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first.is_ascii_lowercase() || first.is_ascii_digit())
            || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return false;
        }
    }
    segments >= 3
}

fn valid_asset_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 200 {
        return false;
    }
    let mut segments = 0;
    for segment in path.split('/') {
        segments += 1;
        if segments > 8
            || segment.is_empty()
            || segment.starts_with('.')
            || !segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return false;
        }
    }
    true
}

/// The reply of the plugin host: header line, newline, bytes (`MediaReplyHeader`).
fn split_reply(payload: &[u8]) -> Option<(String, &[u8])> {
    let split = payload.iter().position(|&b| b == b'\n')?;
    let header: Value = serde_json::from_slice(&payload[..split]).ok()?;
    (header["ok"] == true).then_some(())?;
    Some((header["mime"].as_str()?.to_owned(), &payload[split + 1..]))
}

/// Content types a panel response may carry; the host only sends these, anything else is refused
/// here as well so a confused host cannot turn a file into something the browser renders.
const SERVED_MIME: &[&str] = &[
    "text/html; charset=utf-8",
    "text/javascript; charset=utf-8",
    "text/css; charset=utf-8",
    "application/json",
    "text/plain; charset=utf-8",
    "image/svg+xml",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "font/woff2",
];

/// The response for one file: the locked-down headers whatever the type.
fn asset_response(mime: &str, bytes: Vec<u8>) -> Response {
    let Some(mime) = SERVED_MIME.iter().find(|m| **m == mime) else {
        return failure(StatusCode::NOT_FOUND, "not_found", "no such asset");
    };
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(PANEL_CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        "Permissions-Policy",
        HeaderValue::from_static(
            "camera=(), microphone=(), geolocation=(), payment=(), usb=(), serial=(), hid=(), \
             clipboard-read=(), clipboard-write=(), display-capture=()",
        ),
    );
    // The frame's origin is opaque: module scripts and fonts are fetched with CORS, so the
    // (public, credential-free) files must allow any origin.
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        "Cross-Origin-Resource-Policy",
        HeaderValue::from_static("cross-origin"),
    );
    response
}

/// `GET /api/v1/plugin-ui/{plugin_id}/{path}`.
async fn asset(
    State(state): State<DevicesState>,
    Path((plugin_id, path)): Path<(String, String)>,
) -> Response {
    if !valid_plugin_id(&plugin_id) || !valid_asset_path(&path) {
        return failure(
            StatusCode::BAD_REQUEST,
            "invalid",
            "invalid plugin id or path",
        );
    }
    let client = match connected(state.nats.as_ref()) {
        Ok(client) => client,
        Err(err) => return err.into_response(),
    };
    let Ok(subject) = subjects::plugin_ctl("asset") else {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "invalid command",
        );
    };
    let payload = json!({"v": 1, "plugin_id": plugin_id, "path": path}).to_string();
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
    if let Some((mime, bytes)) = split_reply(&message.payload) {
        return asset_response(&mime, bytes.to_vec());
    }
    // A failure reply is JSON with a `code`; the client only learns "not found" or "invalid".
    let code = serde_json::from_slice::<Value>(&message.payload)
        .ok()
        .and_then(|v| v["code"].as_str().map(str::to_owned));
    match code.as_deref() {
        Some("not_found") => failure(StatusCode::NOT_FOUND, "not_found", "no such asset"),
        Some("invalid") => failure(StatusCode::BAD_REQUEST, "invalid", "invalid asset request"),
        _ => failure(
            StatusCode::BAD_GATEWAY,
            "bus",
            "malformed plugin host reply",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get_status(path: &str) -> (StatusCode, String) {
        let app = router(DevicesState::for_tests());
        let response = app
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn traversal_and_junk_are_refused_before_the_bus() {
        let id = "org.superhet.example.echo";
        for path in [
            "../plugin.toml",
            "ui/../../plugin.toml",
            "%2e%2e/plugin.toml",
            "%2e%2e%2fplugin.toml",
            "..%2fplugin.toml",
            "a/%2e%2e/b.js",
            ".hidden.js",
            "a//b.js",
            "a%5cb.js",
            "a%00.js",
            "%2fetc%2fpasswd",
            "a%20b.js",
            "%c3%a4.js",
            "1/2/3/4/5/6/7/8/9.js",
        ] {
            let (status, body) = get_status(&format!("/api/v1/plugin-ui/{id}/{path}")).await;
            // 400 from the validation; never a 503 (that would mean the request reached the bus
            // check) and never a file.
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
        }
        for bad_id in ["Bad.Id.x", "a.b", "..%2f..%2fx.y.z", "a.b.c%2f..", "-a.b.c"] {
            let (status, _) = get_status(&format!("/api/v1/plugin-ui/{bad_id}/index.html")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_id}");
        }
    }

    #[tokio::test]
    async fn a_clean_request_needs_the_plugin_host() {
        let (status, body) =
            get_status("/api/v1/plugin-ui/org.superhet.example.echo/index.html").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        // No directory listing or empty path.
        let (status, _) = get_status("/api/v1/plugin-ui/org.superhet.example.echo/").await;
        assert_ne!(status, StatusCode::OK);
    }

    #[test]
    fn every_response_is_sandboxed_and_cookie_free() {
        let response = asset_response("text/html; charset=utf-8", b"<p>x</p>".to_vec());
        let h = response.headers();
        let csp = h[header::CONTENT_SECURITY_POLICY].to_str().expect("ascii");
        for needed in [
            "sandbox allow-scripts;",
            "default-src 'none'",
            "script-src 'self'",
            "connect-src 'none'",
            "form-action 'none'",
            "frame-src 'none'",
            "base-uri 'none'",
            "frame-ancestors 'self'",
        ] {
            assert!(csp.contains(needed), "{needed} missing in {csp}");
        }
        // Never `allow-same-origin` (that would hand the page the viewer's session), no
        // top navigation, popups, forms or unsafe-inline.
        for forbidden in [
            "allow-same-origin",
            "allow-top-navigation",
            "allow-popups",
            "allow-forms",
            "unsafe-inline",
            "unsafe-eval",
            "http:",
            "https:",
            "*",
        ] {
            assert!(!csp.contains(forbidden), "{forbidden} in {csp}");
        }
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(h[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert!(h.get(header::SET_COOKIE).is_none());
        assert!(h.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
    }

    #[test]
    fn only_known_content_types_are_labelled() {
        for mime in [
            "text/javascript; charset=utf-8",
            "text/css; charset=utf-8",
            "image/svg+xml",
            "application/json",
            "font/woff2",
        ] {
            let response = asset_response(mime, vec![1]);
            assert_eq!(response.status(), StatusCode::OK, "{mime}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], mime);
            // The CSP rides on every type, SVG included.
            assert!(
                response
                    .headers()
                    .contains_key(header::CONTENT_SECURITY_POLICY)
            );
        }
        for mime in [
            "application/octet-stream",
            "text/html",
            "application/wasm",
            "",
        ] {
            assert_eq!(
                asset_response(mime, vec![1]).status(),
                StatusCode::NOT_FOUND,
                "{mime}"
            );
        }
    }

    #[test]
    fn host_replies_are_split_strictly() {
        let ok = b"{\"v\":1,\"ok\":true,\"key\":\"a.js\",\"mime\":\"text/javascript; charset=utf-8\",\"revision\":0}\nbody";
        let (mime, bytes) = split_reply(ok).expect("reply");
        assert_eq!(mime, "text/javascript; charset=utf-8");
        assert_eq!(bytes, b"body");
        assert!(split_reply(b"{\"ok\":false,\"code\":\"not_found\"}").is_none());
        assert!(split_reply(b"{\"ok\":true}\nx").is_none());
        assert!(split_reply(b"garbage\nx").is_none());
    }

    /// The api keeps its own copies of the protocol's validators (it does not link the plugin
    /// protocol crate); they must not drift apart.
    #[test]
    fn the_validators_agree_with_the_plugin_protocol() {
        use superhet_plugin_protocol::manifest::is_valid_plugin_id;
        use superhet_plugin_protocol::ui;
        let paths = [
            "index.html",
            "js/app.js",
            "../x",
            "a/../b",
            "a//b",
            ".x",
            "a/.x",
            "",
            "/abs",
            "a\\b",
            "a b",
            "ä.js",
            "a%2fb",
            "1/2/3/4/5/6/7/8",
            "1/2/3/4/5/6/7/8/9",
            "x\0y",
        ];
        for path in paths {
            assert_eq!(
                valid_asset_path(path),
                ui::valid_asset_path(path),
                "{path:?}"
            );
        }
        assert_eq!(
            valid_asset_path(&"a".repeat(200)),
            ui::valid_asset_path(&"a".repeat(200))
        );
        assert_eq!(
            valid_asset_path(&"a".repeat(201)),
            ui::valid_asset_path(&"a".repeat(201))
        );
        for id in [
            "org.superhet.example.echo",
            "a.b",
            "a.b.c",
            "A.b.c",
            "a..c.d",
            "-a.b.c",
            "a.b.c-",
            "a.b.c/..",
            "",
        ] {
            assert_eq!(valid_plugin_id(id), is_valid_plugin_id(id), "{id:?}");
        }
        // Every type the protocol serves is one the response accepts.
        for name in [
            "a.html", "a.js", "a.mjs", "a.css", "a.json", "a.txt", "a.svg", "a.png", "a.jpg",
            "a.jpeg", "a.gif", "a.webp", "a.woff2",
        ] {
            let mime = ui::mime_for(name).expect(name);
            assert!(SERVED_MIME.contains(&mime), "{mime}");
        }
    }
}
