//! Superhet API service (HTTP edge). Port 8073 by default.

mod audio_hub;
mod audio_ws;
mod auth;
mod bookmarks;
mod devices;
mod hub;
mod origin;
mod profiles;
mod state_ws;
mod view;
mod web;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, extract::State, http::StatusCode, middleware, routing::get};
use tracing_subscriber::EnvFilter;

/// Default listen address; override with `SUPERHET_LISTEN`.
const DEFAULT_LISTEN: &str = "0.0.0.0:8073";

/// Optional backends probed by `/readyz`. Unconfigured backends are skipped.
#[derive(Clone, Default)]
struct Deps {
    db: Option<superhet_db::PgPool>,
    nats: Option<async_nats::Client>,
}

/// Everything below `/api/v1` except `/api/v1/auth/*` requires a valid session (and so do
/// unknown API paths); routes added to `protected` are covered automatically. The health probes
/// and the static web UI (all other GET/HEAD paths) are public.
fn app(
    web: web::Web,
    deps: Deps,
    auth: auth::AuthState,
    ws: ws::WsState,
    audio: audio_ws::AudioState,
    devices: devices::DevicesState,
) -> Router {
    let protected = Router::new()
        .merge(ws::router(ws))
        .merge(audio_ws::router(audio))
        .merge(state_ws::router(state_ws::StateWsState::new(
            devices.clone(),
        )))
        .merge(bookmarks::router(bookmarks::BookmarksState::new(
            devices.clone(),
            deps.db.clone(),
        )))
        .merge(profiles::router(profiles::ProfilesState::new(
            devices.clone(),
            deps.db.clone(),
        )))
        .merge(devices::router(devices));
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(deps)
        .merge(auth::router(auth.clone()))
        .merge(auth::protect(protected, auth))
        .layer(middleware::from_fn_with_state(web, web::layer))
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
    let web = web::Web::from_env();
    let app = app(web, Deps { db, nats }, auth, ws, audio, devices);
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
    use std::path::Path;
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
        let response = app(
            web::Web::new(Path::new("/nonexistent")),
            Deps::default(),
            auth,
            ws,
            audio,
            devices,
        )
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
            "/api/v1/ws/state?dev=rtl0",
            "/api/v1/profiles",
            "/api/v1/bookmarks",
            "/api/v1/devices/rtl0/lock",
            "/api/v1/does/not/exist",
        ] {
            let (status, body) = get_path(path).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
            assert_eq!(&body[..], br#"{"error":"unauthenticated"}"#, "{path}");
        }
    }

    #[tokio::test]
    async fn missing_web_dir_answers_a_plain_404_hint_for_ui_paths() {
        for path in ["/", "/some/route"] {
            let (status, body) = get_path(path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert!(String::from_utf8_lossy(&body).contains("SUPERHET_WEB_DIR"));
        }
    }

    #[tokio::test]
    async fn auth_endpoints_are_public_but_need_a_database() {
        let (status, body) = get_path("/api/v1/auth/status").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(&body[..], br#"{"error":"unavailable"}"#);
    }
}
