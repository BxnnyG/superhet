//! Superhet API service (HTTP edge). Port 8073 by default.

mod audio_hub;
mod audio_ws;
mod auth;
mod devices;
mod hub;
mod origin;
mod view;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, extract::State, http::StatusCode, routing::get};
use tracing_subscriber::EnvFilter;

/// Default listen address; override with `SUPERHET_LISTEN`.
const DEFAULT_LISTEN: &str = "0.0.0.0:8073";

/// Optional backends probed by `/readyz`. Unconfigured backends are skipped.
#[derive(Clone, Default)]
struct Deps {
    db: Option<superhet_db::PgPool>,
    nats: Option<async_nats::Client>,
}

/// Everything except the health probes and `/api/v1/auth/*` requires a valid session; routes
/// added to `protected` are covered automatically.
fn app(
    deps: Deps,
    auth: auth::AuthState,
    ws: ws::WsState,
    audio: audio_ws::AudioState,
    devices: devices::DevicesState,
) -> Router {
    let protected = Router::new()
        .merge(ws::router(ws))
        .merge(audio_ws::router(audio))
        .merge(devices::router(devices));
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(deps)
        .merge(auth::router(auth.clone()))
        .merge(auth::protect(protected, auth))
}

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz(State(deps): State<Deps>) -> (StatusCode, String) {
    let mut failures = Vec::new();
    if let Some(db) = &deps.db
        && let Err(err) = superhet_db::ping(db).await
    {
        tracing::warn!(%err, "readyz: database not ready");
        failures.push("database");
    }
    if let Some(nats) = &deps.nats
        && nats.connection_state() != async_nats::connection::State::Connected
    {
        tracing::warn!("readyz: nats not connected");
        failures.push("nats");
    }
    if failures.is_empty() {
        (StatusCode::OK, "ok".to_owned())
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("not ready: {}", failures.join(", ")),
        )
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let db = superhet_db::connect_from_env().await?;
    if let Some(pool) = &db {
        superhet_db::migrate(pool).await?;
        tracing::info!("database ready, migrations applied");
    }
    // Connects in the background: the API starts without the bus and /readyz reports it.
    let nats = superhet_bus::connect_background_from_env("superhet-api").await?;
    if nats.is_some() {
        tracing::info!("nats client started (connecting in the background)");
    }
    let origins = origin::OriginPolicy::from_env();
    let auth = auth::AuthState::new(db.clone(), origins.clone(), auth::AuthConfig::from_env()?)?;
    auth.prepare_setup().await?;
    auth.spawn_maintenance();
    let ws = ws::WsState::new(Arc::new(hub::Hub::new(nats.clone())), origins.clone());
    let audio = audio_ws::AudioState::new(
        Arc::new(audio_hub::AudioHub::new(nats.clone())),
        origins.clone(),
    );
    let devices = devices::DevicesState::new(nats.clone(), origins);

    let listen = std::env::var("SUPERHET_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.to_owned());
    let addr: SocketAddr = listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "superhet-api listening");
    let app = app(Deps { db, nats }, auth, ws, audio, devices);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        tracing::warn!("failed to install ctrl-c handler");
        std::future::pending::<()>().await;
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get_path(path: &str) -> (StatusCode, Vec<u8>) {
        let ws = ws::WsState::new(
            Arc::new(hub::Hub::new(None)),
            origin::OriginPolicy::default(),
        );
        let audio = audio_ws::AudioState::new(
            Arc::new(audio_hub::AudioHub::new(None)),
            origin::OriginPolicy::default(),
        );
        let devices = devices::DevicesState::new(None, origin::OriginPolicy::default());
        let auth = auth::AuthState::new(
            None,
            origin::OriginPolicy::default(),
            auth::AuthConfig::default(),
        )
        .expect("auth state");
        let response = app(Deps::default(), auth, ws, audio, devices)
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("infallible");
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (status, body.to_vec())
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let (status, body) = get_path("/healthz").await;
        assert!(status.is_success());
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn readyz_is_ok_without_configured_backends() {
        let (status, body) = get_path("/readyz").await;
        assert!(status.is_success());
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn protected_routes_and_unknown_paths_answer_401_without_a_session() {
        for path in [
            "/api/v1/devices/rtl0/state",
            "/api/v1/ws/spectrum?dev=rtl0",
            "/api/v1/ws/audio?dev=rtl0",
            "/api/v1/does/not/exist",
            "/",
        ] {
            let (status, body) = get_path(path).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
            assert_eq!(&body[..], br#"{"error":"unauthenticated"}"#, "{path}");
        }
    }

    #[tokio::test]
    async fn auth_endpoints_are_public_but_need_a_database() {
        let (status, body) = get_path("/api/v1/auth/status").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(&body[..], br#"{"error":"unavailable"}"#);
    }
}
