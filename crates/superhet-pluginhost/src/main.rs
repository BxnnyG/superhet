//! Superhet plugin host (supervises out-of-process plugins). Port 8075 by default (health only).
//!
//! Plugins live in `SUPERHET_PLUGIN_DIR` (default `/data/plugins`). The control API is served on
//! the bus (`sh.v1.plugin.ctl.<cmd>`, `docs/plugin-contract.md`). With `SUPERHET_DATABASE_URL`
//! started instances are stored and come back after a restart.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{Router, routing::get};
use superhet_pluginhost::ctl::{self, Host, QueueSink};
use superhet_pluginhost::inputs::BusInputs;
use superhet_pluginhost::registry::Registry;
use superhet_pluginhost::store::InstanceStore;
use superhet_pluginhost::supervisor::{NullSink, Policy, Supervisor};
use tracing_subscriber::EnvFilter;

/// Default listen address; override with `SUPERHET_LISTEN`.
const DEFAULT_LISTEN: &str = "0.0.0.0:8075";
/// Default plugin directory; override with `SUPERHET_PLUGIN_DIR`.
const DEFAULT_PLUGIN_DIR: &str = "/data/plugins";
/// Events buffered between the supervisor and the bus publisher.
const EVENT_QUEUE: usize = 4096;

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

    let plugin_dir = PathBuf::from(
        std::env::var("SUPERHET_PLUGIN_DIR").unwrap_or_else(|_| DEFAULT_PLUGIN_DIR.to_owned()),
    );
    let registry = Arc::new(Registry::open(&plugin_dir)?);
    tracing::info!(dir = %plugin_dir.display(), plugins = registry.list().len(), "plugin registry loaded");

    let db = superhet_db::connect_from_env().await?;
    if let Some(pool) = &db {
        superhet_db::migrate(pool).await?;
        tracing::info!("database ready, instances are persistent");
    } else {
        tracing::warn!("SUPERHET_DATABASE_URL is not set: instances do not survive a restart");
    }
    let store = InstanceStore::new(db);

    let nats = superhet_bus::connect_background_from_env("superhet-pluginhost").await?;
    let (supervisor, tasks) = match nats {
        Some(client) => {
            let (sink, events) = QueueSink::new(EVENT_QUEUE);
            let supervisor = Supervisor::with_inputs(
                Policy::default(),
                Arc::new(sink),
                Arc::new(BusInputs::new(client.clone())),
            );
            let host = Host {
                registry: Arc::clone(&registry),
                supervisor: supervisor.clone(),
                store,
            };
            let tasks = vec![
                tokio::spawn(ctl::ensure_events_stream(client.clone())),
                tokio::spawn({
                    let host = host.clone();
                    async move { ctl::restore(&host).await }
                }),
                tokio::spawn(ctl::publish_events(client.clone(), events)),
                tokio::spawn(ctl::publish_state(client.clone(), supervisor.clone())),
                tokio::spawn({
                    let client = client.clone();
                    async move {
                        if let Err(err) = ctl::serve(client, host).await {
                            tracing::error!(%err, "control API stopped");
                        }
                    }
                }),
            ];
            (supervisor, tasks)
        }
        None => {
            tracing::warn!("SUPERHET_NATS_URL is not set: the control API is unavailable");
            (
                Supervisor::new(Policy::default(), Arc::new(NullSink)),
                Vec::new(),
            )
        }
    };

    let listen = std::env::var("SUPERHET_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.to_owned());
    let addr: SocketAddr = listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "superhet-pluginhost listening");
    axum::serve(listener, app())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    supervisor.shutdown().await;
    for task in tasks {
        task.abort();
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
