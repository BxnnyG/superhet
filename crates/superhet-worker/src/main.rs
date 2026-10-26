//! Superhet worker (background jobs and event consumers). Port 8076 by default (health only).
//!
//! Today it records the decoder history (DTMF bursts, tone changes, RDS changes) into PostgreSQL
//! (`docs/api-decoder-events.md`).

mod consumer;
mod recorder;

use std::net::SocketAddr;

use axum::{Router, routing::get};
use tracing_subscriber::EnvFilter;

/// Default listen address; override with `SUPERHET_LISTEN`.
const DEFAULT_LISTEN: &str = "0.0.0.0:8076";

fn app() -> Router {
    Router::new().route("/healthz", get(healthz))
}

async fn healthz() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let retention =
        consumer::retention_days(std::env::var(consumer::ENV_RETENTION_DAYS).ok().as_deref())?;
    let db = superhet_db::connect_from_env().await?;
    if let Some(pool) = &db {
        superhet_db::migrate(pool).await?;
        tracing::info!("database ready, migrations applied");
    }
    let nats = superhet_bus::connect_background_from_env("superhet-worker").await?;

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = stop_tx.send(true);
    });

    let recorder = match (db, nats) {
        (Some(pool), Some(client)) => {
            let mut stop = stop_rx.clone();
            Some(tokio::spawn(async move {
                let wait = async move {
                    let _ = stop.wait_for(|stopped| *stopped).await;
                };
                if let Err(err) = consumer::run(pool, client, retention, wait).await {
                    tracing::error!(%err, "decoder history recorder stopped");
                }
            }))
        }
        _ => {
            tracing::warn!(
                "decoder history disabled: both SUPERHET_DATABASE_URL and SUPERHET_NATS_URL are required"
            );
            None
        }
    };

    let listen = std::env::var("SUPERHET_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.to_owned());
    let addr: SocketAddr = listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "superhet-worker listening");
    let mut stop = stop_rx;
    axum::serve(listener, app())
        .with_graceful_shutdown(async move {
            let _ = stop.wait_for(|stopped| *stopped).await;
        })
        .await?;
    if let Some(task) = recorder {
        let _ = task.await;
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            tracing::info!("shutting down");
            return;
        }
    }
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

    #[tokio::test]
    async fn healthz_returns_ok() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
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
        assert_eq!(&body[..], b"ok");
    }
}
