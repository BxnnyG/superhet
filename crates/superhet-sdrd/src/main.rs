//! Superhet SDR daemon (device and DSP, one instance per SDR). Port 8074 by default.
//!
//! With `SUPERHET_DEVICE` set (e.g. `file:/data/capture.cu8?format=cu8&rate=2048000&freq=100000000`)
//! and NATS configured, spectrum frames are published on `sh.v1.dev.<id>.spectrum`, control
//! requests are served on `sh.v1.dev.<id>.ctl.*` (including VFOs, whose Opus audio goes to
//! `sh.v1.dev.<id>.audio.<vfo>`) and the device state is kept in the KV bucket
//! `STATE` (see `docs/api-devices.md`). While the gateway leases a slice (`ctl.spectrum.zoom`), a
//! zoom spectrum with real resolution is published on `sh.v1.dev.<id>.spectrum.zoom`.

mod audio;
mod config;
mod control;
mod controller;
mod lock;
mod pipeline;
mod publish;
mod service;
mod vfo;
mod window;
mod zoom;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{
    Router,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use tracing_subscriber::EnvFilter;

/// Default listen address; override with `SUPERHET_LISTEN`.
const DEFAULT_LISTEN: &str = "0.0.0.0:8074";

/// What `/readyz` looks at.
#[derive(Clone, Default)]
struct Deps {
    /// A device is configured, so the stream and the bus must both be up.
    device_expected: bool,
    /// True while the DSP pipeline processes the device stream.
    pipeline_running: Option<Arc<AtomicBool>>,
    /// Bus client (connecting in the background).
    nats: Option<async_nats::Client>,
}

fn app(deps: Deps) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(deps)
}

async fn healthz() -> &'static str {
    "ok"
}

/// 200 `ok` when ready (also without a configured device: health-only mode); otherwise 503
/// with a small JSON body naming the first missing prerequisite.
async fn readyz(State(deps): State<Deps>) -> impl IntoResponse {
    if !deps.device_expected {
        return (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain")], "ok");
    }
    let reason = if !deps
        .pipeline_running
        .as_ref()
        .is_some_and(|r| r.load(Ordering::Acquire))
    {
        Some(r#"{"ready":false,"reason":"device stream not running"}"#)
    } else if !deps
        .nats
        .as_ref()
        .is_some_and(|c| c.connection_state() == async_nats::connection::State::Connected)
    {
        Some(r#"{"ready":false,"reason":"nats not connected"}"#)
    } else {
        None
    };
    match reason {
        None => (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain")], "ok"),
        Some(body) => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            body,
        ),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = config::Config::from_env()?;
    // Keep the running parts alive for the lifetime of the process.
    let (_running, control_service, deps) = start_pipeline(&config).await?;

    let listen = std::env::var("SUPERHET_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.to_owned());
    let addr: SocketAddr = listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "superhet-sdrd listening");
    axum::serve(listener, app(deps))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    if let Some(service) = control_service {
        service.shutdown().await;
    }
    Ok(())
}

/// The device pipeline and the VFO audio thread; dropped in field order (device first).
struct Running {
    _pipeline: pipeline::Pipeline,
    _audio: audio::AudioEngine,
}

/// Open the device and start the pipelines. NATS connects in the background, so a missing or
/// restarting server never prevents start-up; frames are dropped until it is reachable.
async fn start_pipeline(
    config: &config::Config,
) -> Result<(Option<Running>, Option<service::Service>, Deps), Box<dyn std::error::Error>> {
    let Some(spec) = &config.device_spec else {
        tracing::warn!("SUPERHET_DEVICE not set; running health-only");
        return Ok((None, None, Deps::default()));
    };
    let mut deps = Deps {
        device_expected: true,
        ..Deps::default()
    };
    let Some(client) = superhet_bus::connect_background_from_env("superhet-sdrd").await? else {
        tracing::warn!("SUPERHET_NATS_URL not set; spectrum pipeline disabled");
        return Ok((None, None, deps));
    };
    deps.nats = Some(client.clone());
    let subject = superhet_bus::subjects::spectrum(&config.device_id)?;
    let device = superhet_device::open_from_spec(spec)?;
    tracing::info!(device = device.name(), %subject, "starting spectrum pipeline");
    let vfos = Arc::new(vfo::VfoShared::default());
    let audio_sink = publish::spawn_audio_publisher(client.clone(), &config.device_id)?;
    let rds_sink = publish::spawn_rds_publisher(client.clone(), &config.device_id)?;
    let audio = audio::AudioEngine::start_with_rds(Arc::clone(&vfos), audio_sink, rds_sink)?;
    let sink = publish::spawn_spectrum_publisher(client.clone(), subject);
    let zoom_handle = zoom::ZoomHandle::default();
    let zoom_sink = publish::spawn_spectrum_publisher(
        client.clone(),
        superhet_bus::subjects::spectrum_zoom(&config.device_id)?,
    );
    let zoom_engine =
        zoom::ZoomEngine::new(zoom_handle.clone(), config.spectrum.clone(), zoom_sink);
    let pipeline = pipeline::Pipeline::start(
        device,
        &config.spectrum,
        sink,
        Some(audio.tap()),
        Some(zoom_engine),
    )?;
    deps.pipeline_running = Some(pipeline.running_flag());
    let service = service::start(
        client,
        config.device_id.clone(),
        pipeline.control(),
        vfos,
        zoom_handle,
    )
    .await?;
    let running = Running {
        _pipeline: pipeline,
        _audio: audio,
    };
    Ok((Some(running), Some(service), deps))
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

    async fn get_path(deps: Deps, path: &str) -> (StatusCode, Vec<u8>) {
        let response = app(deps)
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
        let (status, body) = get_path(Deps::default(), "/healthz").await;
        assert!(status.is_success());
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn readyz_is_ok_without_device() {
        let (status, body) = get_path(Deps::default(), "/readyz").await;
        assert!(status.is_success());
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn readyz_is_unavailable_while_pipeline_is_down() {
        let deps = Deps {
            device_expected: true,
            pipeline_running: Some(Arc::new(AtomicBool::new(false))),
            nats: None,
        };
        let (status, body) = get_path(deps, "/readyz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(String::from_utf8_lossy(&body).contains("device stream not running"));
    }

    #[tokio::test]
    async fn readyz_is_unavailable_while_nats_is_down() {
        // Port 1 is never listening; the client keeps retrying in the background.
        let config = superhet_bus::BusConfig {
            url: "nats://127.0.0.1:1".into(),
            auth: superhet_bus::Auth::None,
            client_name: "sdrd-test".into(),
        };
        let client = superhet_bus::connect_background(&config)
            .await
            .expect("background connect returns immediately");
        let deps = Deps {
            device_expected: true,
            pipeline_running: Some(Arc::new(AtomicBool::new(true))),
            nats: Some(client),
        };
        let (status, body) = get_path(deps, "/readyz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(String::from_utf8_lossy(&body).contains("nats not connected"));
    }
}
