//! Prometheus metrics of the API (`GET /metrics`).
//!
//! The only published port of Superhet (8073) carries the session-protected API and the public
//! web UI, so `/metrics` is deliberately **not** served there. It is served on a separate
//! listener that is off unless `SUPERHET_METRICS_LISTEN` is set (e.g. `0.0.0.0:8077` inside the
//! compose network, which does not publish it); see `docs/perf.md`.
//!
//! Counters are process globals bumped with relaxed atomics from the WebSocket handlers.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::{Router, http::header, response::IntoResponse, routing::get};
use superhet_bus::metrics::{CONTENT_TYPE, Exposition, Kind};

/// Environment variable with the address of the metrics listener; unset = no metrics server.
pub const ENV_LISTEN: &str = "SUPERHET_METRICS_LISTEN";

/// WebSocket endpoints, in label order.
const KINDS: [&str; 4] = ["spectrum", "audio", "state", "events"];

static CLIENTS: [AtomicU64; KINDS.len()] = [const { AtomicU64::new(0) }; KINDS.len()];
static SPECTRUM_FRAMES_MISSED: AtomicU64 = AtomicU64::new(0);
static AUDIO_FRAMES_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Counts a connected WebSocket client until dropped.
#[derive(Debug)]
pub struct ClientGuard(usize);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        CLIENTS[self.0].fetch_sub(1, Ordering::Relaxed);
    }
}

/// Registers a connected client of `kind` (`spectrum`, `audio`, `state` or `events`).
#[must_use]
pub fn track_client(kind: &str) -> ClientGuard {
    // An unknown kind is a programming error; count it as the last one rather than panic.
    let index = KINDS
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(KINDS.len() - 1);
    CLIENTS[index].fetch_add(1, Ordering::Relaxed);
    ClientGuard(index)
}

/// Counts spectrum frames a slow client missed.
pub fn spectrum_frames_missed(count: u64) {
    SPECTRUM_FRAMES_MISSED.fetch_add(count, Ordering::Relaxed);
}

/// Counts audio frames dropped for slow clients.
pub fn audio_frames_dropped(count: u64) {
    AUDIO_FRAMES_DROPPED.fetch_add(count, Ordering::Relaxed);
}

/// The text exposition.
#[must_use]
pub fn render() -> String {
    let mut e = Exposition::new();
    e.family(
        "superhet_api_ws_clients",
        "Connected WebSocket clients, by endpoint.",
        Kind::Gauge,
    );
    for (i, kind) in KINDS.iter().enumerate() {
        e.sample_u64(
            "superhet_api_ws_clients",
            &[("endpoint", kind)],
            CLIENTS[i].load(Ordering::Relaxed),
        );
    }
    e.single_u64(
        "superhet_api_spectrum_frames_missed_total",
        "Spectrum frames that slow WebSocket clients missed (they carry on with fresh ones).",
        Kind::Counter,
        SPECTRUM_FRAMES_MISSED.load(Ordering::Relaxed),
    );
    e.single_u64(
        "superhet_api_audio_frames_dropped_total",
        "Audio frames dropped because a WebSocket client was too slow.",
        Kind::Counter,
        AUDIO_FRAMES_DROPPED.load(Ordering::Relaxed),
    );
    e.finish()
}

/// Router with `/metrics` only; served on the separate listener, never on the public port.
pub fn router() -> Router {
    Router::new().route("/metrics", get(metrics_text))
}

async fn metrics_text() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, CONTENT_TYPE)], render())
}

/// Parses `SUPERHET_METRICS_LISTEN`; `Ok(None)` when unset or empty.
///
/// # Errors
/// The value is not a socket address.
pub fn listen_addr(value: Option<&str>) -> Result<Option<SocketAddr>, std::net::AddrParseError> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(addr) => addr.parse().map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[test]
    fn client_guard_counts_up_and_down() {
        let before = CLIENTS[2].load(Ordering::Relaxed);
        let guard = track_client("state");
        assert_eq!(CLIENTS[2].load(Ordering::Relaxed), before + 1);
        drop(guard);
        assert_eq!(CLIENTS[2].load(Ordering::Relaxed), before);
    }

    #[test]
    fn listen_address_is_optional() {
        assert_eq!(listen_addr(None), Ok(None));
        assert_eq!(listen_addr(Some(" ")), Ok(None));
        assert!(
            listen_addr(Some("127.0.0.1:8077"))
                .expect("valid")
                .is_some()
        );
        assert!(listen_addr(Some("nonsense")).is_err());
    }

    #[tokio::test]
    async fn metrics_route_serves_the_exposition() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("infallible");
        assert!(response.status().is_success());
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("superhet_api_ws_clients{endpoint=\"spectrum\"}"));
    }
}
