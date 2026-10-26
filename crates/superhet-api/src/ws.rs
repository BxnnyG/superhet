//! WebSocket spectrum gateway: `GET /api/v1/ws/spectrum?dev=<id>`.
//!
//! Protocol: see `docs/protocol-ws.md`. Binary server frames are spectrum frames in the
//! `superhet-bus` wire format v1, cropped and decimated to the client's view; text frames
//! carry JSON control messages and acknowledgements.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::Deserialize;
use tokio::sync::{Semaphore, broadcast};
use tokio::time::Instant;

use crate::hub::{Hub, HubError, Streams};
use crate::origin::OriginPolicy;
use crate::view::{View, covers, crop_decimate, parse_control};
use superhet_bus::spectrum::SpectrumFrame;

/// Route path of the spectrum gateway.
pub const PATH: &str = "/api/v1/ws/spectrum";
/// Maximum size of a client control message.
const MAX_CONTROL_BYTES: usize = 4096;
/// Maximum concurrent spectrum clients.
const MAX_CLIENTS: usize = 32;
/// A client that cannot take a frame within this time is disconnected.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// A changed view is passed on to the daemon after it stayed unchanged this long (a wheel
/// zoom sends many; only the last one needs a new slice).
const ZOOM_DEBOUNCE: Duration = Duration::from_millis(100);
/// Zoom frames count as flowing for this long after the last one that covered the view; the
/// full-band frames are held back meanwhile, so a client never gets both for one view.
const ZOOM_FLOW: Duration = Duration::from_millis(400);
/// Identifies a spectrum session towards the zoom lease.
static SESSION_IDS: AtomicU64 = AtomicU64::new(1);

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
    let streams = state.hub.subscribe(&device).await.map_err(|err| {
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
            let _client = crate::metrics::track_client("spectrum");
            serve_client(socket, streams, &hub, &device).await;
            // The receivers were moved into `serve_client` and are dropped by now.
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

/// What one spectrum session knows about the frames it is fed.
struct Session {
    view: View,
    last_sent: Option<Instant>,
    /// Bandwidth of the newest full-band frame (0 until one arrived).
    full_rate: u32,
    /// Newest zoom frame and when it arrived.
    last_zoom: Option<(Arc<SpectrumFrame>, Instant)>,
    /// Slice last reported to the hub.
    leased: Option<(u64, u32)>,
    /// When to report the slice for the current view (debounce).
    zoom_due: Option<Instant>,
}

impl Session {
    fn new() -> Self {
        Self {
            view: View::default(),
            last_sent: None,
            full_rate: 0,
            last_zoom: None,
            leased: None,
            zoom_due: None,
        }
    }

    /// Zoom frames that cover the view arrived recently: they serve this view.
    fn zoom_flowing(&self) -> bool {
        self.last_zoom
            .as_ref()
            .is_some_and(|(frame, at)| at.elapsed() < ZOOM_FLOW && covers(frame, &self.view))
    }

    fn rate_limited(&self) -> bool {
        self.last_sent
            .is_some_and(|t| t.elapsed() < self.view.min_interval)
    }
}

async fn serve_client(mut socket: WebSocket, streams: Streams, hub: &Hub, device: &str) {
    let Streams {
        full: mut frames,
        zoom: mut zoom_frames,
    } = streams;
    let id = SESSION_IDS.fetch_add(1, Ordering::Relaxed);
    let mut s = Session::new();
    loop {
        let due = s.zoom_due;
        tokio::select! {
            incoming = socket.recv() => match incoming {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(text))) => {
                    let reply = match parse_control(text.as_str()) {
                        Ok(new_view) => {
                            s.view = new_view;
                            s.last_sent = None;
                            s.zoom_due = Some(Instant::now() + ZOOM_DEBOUNCE);
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
            () = async { tokio::time::sleep_until(due.unwrap_or_else(Instant::now)).await },
                if due.is_some() =>
            {
                s.zoom_due = None;
                let wanted = s.view.zoom_request(s.full_rate);
                if wanted != s.leased {
                    s.leased = wanted;
                    hub.zoom_set(device, id, wanted).await;
                }
            }
            frame = frames.recv() => match frame {
                Ok(frame) => {
                    if frame.sample_rate != s.full_rate {
                        // The band became known or changed: the slice to ask for may differ.
                        s.full_rate = frame.sample_rate;
                        s.zoom_due.get_or_insert_with(Instant::now);
                    }
                    if s.rate_limited() || s.zoom_flowing() {
                        continue;
                    }
                    let Some(out) = crop_decimate(&frame, &s.view) else { continue };
                    if !send(&mut socket, Message::Binary(out.encode().into())).await {
                        break;
                    }
                    s.last_sent = Some(Instant::now());
                }
                // This client was too slow: the oldest frames are gone, carry on with fresh ones.
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    crate::metrics::spectrum_frames_missed(missed);
                    tracing::debug!(missed, "spectrum client lagging, frames dropped");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            frame = zoom_frames.recv() => match frame {
                Ok(frame) => {
                    if !covers(&frame, &s.view) {
                        continue;
                    }
                    s.last_zoom = Some((Arc::clone(&frame), Instant::now()));
                    if s.rate_limited() {
                        continue;
                    }
                    let Some(out) = crop_decimate(&frame, &s.view) else { continue };
                    if !send(&mut socket, Message::Binary(out.encode().into())).await {
                        break;
                    }
                    s.last_sent = Some(Instant::now());
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    crate::metrics::spectrum_frames_missed(missed);
                    tracing::debug!(missed, "zoom client lagging, frames dropped");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    if s.leased.is_some() {
        hub.zoom_set(device, id, None).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header};
    use futures_util::{SinkExt, StreamExt};
    use superhet_bus::zoom::ZoomCommand;
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

    /// A zoom frame: 250 kHz over 2000 bins (125 Hz each) around 100 MHz.
    fn zoom_frame() -> SpectrumFrame {
        let mut db = vec![-90.0f32; 2000];
        db[1000] = -10.0;
        SpectrumFrame::quantize(100_000_000, 250_000, 8, -100.0, 0.0, &db)
    }

    /// Next binary frame within `ms`, if any.
    async fn try_binary(client: &mut Client, ms: u64) -> Option<SpectrumFrame> {
        match tokio::time::timeout(Duration::from_millis(ms), client.next()).await {
            Ok(Some(Ok(tungstenite::Message::Binary(data)))) => SpectrumFrame::decode(&data).ok(),
            _ => None,
        }
    }

    #[tokio::test]
    async fn narrow_views_lease_a_slice_and_are_served_from_zoom_frames() {
        let server = start(Hub::local(), OriginPolicy::default()).await;
        let mut client = connect(&server, "?dev=dev0", None).await.expect("connect");
        // The full-band frame tells the session how wide the band is.
        next_binary(&server, &mut client).await;
        let view = r#"{"type":"view","center_hz":100000000,"span_hz":200000,"bins":4096}"#;
        client
            .send(tungstenite::Message::text(view))
            .await
            .expect("send");
        assert_eq!(next_text(&mut client).await, r#"{"v":1,"type":"ack"}"#);

        // After the debounce the slice is leased (exactly once).
        let mut leased = false;
        for _ in 0..50 {
            server.hub.publish_local("dev0", test_frame()).await;
            let _ = try_binary(&mut client, 50).await;
            if !server.hub.zoom_commands().is_empty() {
                leased = true;
                break;
            }
        }
        assert!(leased, "no zoom request");
        assert_eq!(
            server.hub.zoom_commands(),
            vec![(
                "dev0".to_owned(),
                ZoomCommand::View {
                    center_hz: 100_000_000,
                    span_hz: 200_000
                }
            )]
        );

        // Zoom frames now carry the view at 125 Hz per bin; full-band frames in between are
        // held back (they would only give 200 bins).
        server.hub.publish_local_zoom("dev0", zoom_frame()).await;
        let mut got = Vec::new();
        for _ in 0..6 {
            server.hub.publish_local_zoom("dev0", zoom_frame()).await;
            server.hub.publish_local("dev0", test_frame()).await;
            while let Some(f) = try_binary(&mut client, 60).await {
                got.push(f);
            }
        }
        // Until the first zoom frame was seen a full-band frame may still slip through.
        let first = got
            .iter()
            .position(|f| f.bins.len() > 1500)
            .expect("zoom frame");
        for f in &got[first..] {
            assert!(f.bins.len() > 1500, "{} bins", f.bins.len());
            assert!(f.sample_rate >= 200_000 && f.sample_rate <= 200_250);
            assert_eq!(f.timestamp_us, 8);
        }

        // Leaving the zoomed view returns the lease.
        let view = r#"{"type":"view","bins":4096}"#;
        client
            .send(tungstenite::Message::text(view))
            .await
            .expect("send");
        let mut returned = false;
        for _ in 0..50 {
            server.hub.publish_local("dev0", test_frame()).await;
            let _ = try_binary(&mut client, 50).await;
            if server.hub.zoom_commands().len() == 2 {
                returned = true;
                break;
            }
        }
        assert!(returned, "slice not released");
        assert_eq!(
            server.hub.zoom_commands().last().map(|(_, c)| *c),
            Some(ZoomCommand::Off)
        );
    }

    #[tokio::test]
    async fn zoom_frames_that_miss_the_view_are_ignored() {
        let server = start(Hub::local(), OriginPolicy::default()).await;
        let mut client = connect(&server, "?dev=dev0", None).await.expect("connect");
        next_binary(&server, &mut client).await;
        // View 99.60 .. 99.65 MHz; the zoom frame covers 99.875 .. 100.125 MHz.
        let view = r#"{"type":"view","start_hz":99600000,"end_hz":99650000,"bins":4096}"#;
        client
            .send(tungstenite::Message::text(view))
            .await
            .expect("send");
        assert_eq!(next_text(&mut client).await, r#"{"v":1,"type":"ack"}"#);
        for _ in 0..6 {
            server.hub.publish_local_zoom("dev0", zoom_frame()).await;
            server.hub.publish_local("dev0", test_frame()).await;
            while let Some(f) = try_binary(&mut client, 60).await {
                // Only the full-band frame fits: 1 kHz bins over the 50 kHz view.
                assert!(f.bins.len() <= 52, "{} bins", f.bins.len());
            }
        }
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
