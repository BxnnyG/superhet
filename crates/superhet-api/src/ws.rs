//! WebSocket spectrum gateway: `GET /api/v1/ws/spectrum?dev=<id>`.
//!
//! Protocol: see `docs/protocol-ws.md`. Binary server frames are spectrum frames in the
//! `superhet-bus` wire format v1, cropped and decimated to the client's view; text frames
//! carry JSON control messages and acknowledgements.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    middleware,
    routing::get,
};
use serde::Deserialize;
use tokio::sync::{Semaphore, broadcast};
use tokio::time::Instant;

use crate::auth;
use crate::hub::{Hub, HubError};
use crate::origin::OriginPolicy;
use crate::view::{View, crop_decimate, parse_control};

/// Route path of the spectrum gateway.
pub const PATH: &str = "/api/v1/ws/spectrum";
/// Maximum size of a client control message.
const MAX_CONTROL_BYTES: usize = 4096;
/// Maximum concurrent spectrum clients.
const MAX_CLIENTS: usize = 32;
/// A client that cannot take a frame within this time is disconnected.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// Shared state of the WebSocket routes.
#[derive(Clone)]
pub struct WsState {
    hub: Arc<Hub>,
    origins: Arc<OriginPolicy>,
    slots: Arc<Semaphore>,
}

impl WsState {
    /// Create the state from a hub and an origin policy.
    #[must_use]
    pub fn new(hub: Arc<Hub>, origins: OriginPolicy) -> Self {
        Self {
            hub,
            origins: Arc::new(origins),
            slots: Arc::new(Semaphore::new(MAX_CLIENTS)),
        }
    }
}

/// Routes of the gateway, behind the authentication hook.
pub fn router(state: WsState) -> Router {
    Router::new()
        .route(PATH, get(spectrum_ws))
        .route_layer(middleware::from_fn(auth::require_session))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct SpectrumQuery {
    dev: String,
}

async fn spectrum_ws(
    State(state): State<WsState>,
    Query(query): Query<SpectrumQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<axum::response::Response, StatusCode> {
    if !state.origins.allows(&headers) {
        tracing::warn!("spectrum ws: origin rejected");
        return Err(StatusCode::FORBIDDEN);
    }
    let permit = Arc::clone(&state.slots)
        .try_acquire_owned()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let device = query.dev;
    let frames = state.hub.subscribe(&device).await.map_err(|err| {
        tracing::warn!(%err, "spectrum ws: subscribe failed");
        match err {
            HubError::InvalidDevice => StatusCode::BAD_REQUEST,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }
    })?;
    let hub = Arc::clone(&state.hub);
    Ok(upgrade
        .max_message_size(MAX_CONTROL_BYTES)
        .max_frame_size(MAX_CONTROL_BYTES)
        .on_upgrade(move |socket| async move {
            serve_client(socket, frames).await;
            // The receiver was moved into `serve_client` and is dropped by now.
            hub.release(&device).await;
            drop(permit);
        }))
}

/// Send with a deadline; `false` means the client is gone or too slow.
pub(crate) async fn send(socket: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, socket.send(message)).await,
        Ok(Ok(()))
    )
}

async fn serve_client(
    mut socket: WebSocket,
    mut frames: broadcast::Receiver<Arc<superhet_bus::spectrum::SpectrumFrame>>,
) {
    let mut view = View::default();
    let mut last_sent: Option<Instant> = None;
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(text))) => {
                    let reply = match parse_control(text.as_str()) {
                        Ok(new_view) => {
                            view = new_view;
                            last_sent = None;
                            r#"{"v":1,"type":"ack"}"#.to_owned()
                        }
                        Err(err) => format!(r#"{{"v":1,"type":"error","error":"{err}"}}"#),
                    };
                    if !send(&mut socket, Message::text(reply)).await {
                        break;
                    }
                }
                // Pings are answered by the transport; binary input is not part of the protocol.
                Some(Ok(_)) => {}
            },
            frame = frames.recv() => match frame {
                Ok(frame) => {
                    if last_sent.is_some_and(|t| t.elapsed() < view.min_interval) {
                        continue;
                    }
                    let Some(out) = crop_decimate(&frame, &view) else { continue };
                    if !send(&mut socket, Message::Binary(out.encode().into())).await {
                        break;
                    }
                    last_sent = Some(Instant::now());
                }
                // This client was too slow: the oldest frames are gone, carry on with fresh ones.
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::debug!(missed, "spectrum client lagging, frames dropped");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header};
    use futures_util::{SinkExt, StreamExt};
    use superhet_bus::spectrum::SpectrumFrame;
    use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

    struct Server {
        addr: std::net::SocketAddr,
        hub: Arc<Hub>,
    }

    async fn start(hub: Hub, origins: OriginPolicy) -> Server {
        let hub = Arc::new(hub);
        let app = router(WsState::new(Arc::clone(&hub), origins));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Server { addr, hub }
    }

    type Client = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn connect(
        server: &Server,
        query: &str,
        origin: Option<&str>,
    ) -> Result<Client, tungstenite::Error> {
        let mut req = format!("ws://{}{PATH}{query}", server.addr)
            .into_client_request()
            .expect("request");
        if let Some(origin) = origin {
            req.headers_mut().insert(
                header::ORIGIN,
                HeaderValue::from_str(origin).expect("header"),
            );
        }
        tokio_tungstenite::connect_async(req).await.map(|(c, _)| c)
    }

    fn status_of(err: tungstenite::Error) -> u16 {
        match err {
            tungstenite::Error::Http(resp) => resp.status().as_u16(),
            other => panic!("expected http error, got {other:?}"),
        }
    }

    fn test_frame() -> SpectrumFrame {
        // 1000 bins over 1 MHz around 100 MHz with one narrow peak at bin 501.
        let mut db = vec![-90.0f32; 1000];
        db[501] = -10.0;
        SpectrumFrame::quantize(100_000_000, 1_000_000, 7, -100.0, 0.0, &db)
    }

    /// Publish until a binary frame arrives (the client may subscribe a moment later).
    async fn next_binary(server: &Server, client: &mut Client) -> SpectrumFrame {
        for _ in 0..100 {
            server.hub.publish_local("dev0", test_frame()).await;
            if let Ok(Some(Ok(tungstenite::Message::Binary(data)))) =
                tokio::time::timeout(Duration::from_millis(100), client.next()).await
            {
                return SpectrumFrame::decode(&data).expect("valid frame");
            }
        }
        panic!("no frame received");
    }

    async fn next_text(client: &mut Client) -> String {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), client.next()).await {
                Ok(Some(Ok(tungstenite::Message::Text(t)))) => return t.to_string(),
                Ok(Some(Ok(_))) => {}
                other => panic!("expected text, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn streams_cropped_and_decimated_frames() {
        let server = start(Hub::local(), OriginPolicy::default()).await;
        let mut client = connect(&server, "?dev=dev0", None).await.expect("connect");

        // Default view: full frame at the default resolution (1000 bins pass through).
        let full = next_binary(&server, &mut client).await;
        assert_eq!(full.bins.len(), 1000);
        assert_eq!(full.sample_rate, 1_000_000);

        let view = r#"{"type":"view","v":1,"center_hz":100000000,"span_hz":1000000,"bins":100}"#;
        client
            .send(tungstenite::Message::text(view))
            .await
            .expect("send");
        assert_eq!(next_text(&mut client).await, r#"{"v":1,"type":"ack"}"#);
        // Drain frames still in flight from the old view.
        let mut out = next_binary(&server, &mut client).await;
        while out.bins.len() != 100 {
            out = next_binary(&server, &mut client).await;
        }
        assert_eq!(
            out.bins.iter().copied().max(),
            full.bins.iter().copied().max()
        );
        assert_eq!(out.center_hz, 100_000_000);
        assert_eq!(out.timestamp_us, 7);

        let view = r#"{"type":"view","start_hz":99900000,"end_hz":100100000,"bins":4096}"#;
        client
            .send(tungstenite::Message::text(view))
            .await
            .expect("send");
        assert_eq!(next_text(&mut client).await, r#"{"v":1,"type":"ack"}"#);
        let mut out = next_binary(&server, &mut client).await;
        while out.bins.len() != 200 {
            out = next_binary(&server, &mut client).await;
        }
        assert_eq!(out.sample_rate, 200_000);
        assert_eq!(out.center_hz, 100_000_000);
    }

    #[tokio::test]
    async fn invalid_control_message_gets_an_error_and_keeps_the_connection() {
        let server = start(Hub::local(), OriginPolicy::default()).await;
        let mut client = connect(&server, "?dev=dev0", None).await.expect("connect");
        client
            .send(tungstenite::Message::text(
                r#"{"type":"view","start_hz":5}"#,
            ))
            .await
            .expect("send");
        assert_eq!(
            next_text(&mut client).await,
            r#"{"v":1,"type":"error","error":"invalid span"}"#
        );
        let frame = next_binary(&server, &mut client).await;
        assert_eq!(frame.bins.len(), 1000);
    }

    #[tokio::test]
    async fn rejects_bad_origin_device_and_missing_bus() {
        let server = start(Hub::local(), OriginPolicy::default()).await;
        let err = connect(&server, "?dev=dev0", Some("https://evil.example"))
            .await
            .expect_err("origin");
        assert_eq!(status_of(err), 403);
        let err = connect(&server, "?dev=a.b", None).await.expect_err("dev");
        assert_eq!(status_of(err), 400);
        let err = connect(&server, "", None).await.expect_err("missing dev");
        assert_eq!(status_of(err), 400);

        let server = start(Hub::new(None), OriginPolicy::default()).await;
        let err = connect(&server, "?dev=dev0", None).await.expect_err("bus");
        assert_eq!(status_of(err), 503);
    }

    #[tokio::test]
    async fn disconnect_releases_the_device() {
        let server = start(Hub::local(), OriginPolicy::default()).await;
        let mut client = connect(&server, "?dev=dev0", None).await.expect("connect");
        next_binary(&server, &mut client).await;
        drop(client);
        // The server notices the close asynchronously.
        for _ in 0..100 {
            if server.hub.device_count().await == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("device was not released");
    }
}
