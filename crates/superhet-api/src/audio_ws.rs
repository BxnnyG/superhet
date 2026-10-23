//! WebSocket audio gateway: `GET /api/v1/ws/audio?dev=<id>`.
//!
//! Protocol: see `docs/protocol-audio.md`. Binary server frames are `AudioFrame` v1 messages
//! exactly as published by sdrd; the client picks the VFOs with a JSON `subscribe` text message.

use std::collections::BTreeSet;
use std::sync::Arc;

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
use tokio::sync::Semaphore;

use crate::audio_hub::{AudioHub, HubError, Session};
use crate::origin::OriginPolicy;
use crate::ws::send;

/// Route path of the audio gateway.
pub const PATH: &str = "/api/v1/ws/audio";
/// Maximum size of a client control message.
const MAX_CONTROL_BYTES: usize = 1024;
/// Maximum number of VFOs one client may listen to (the daemon runs at most this many).
pub const MAX_VFOS: usize = 8;
/// Maximum concurrent audio clients.
const MAX_CLIENTS: usize = 32;

/// Shared state of the audio route.
#[derive(Clone)]
pub struct AudioState {
    hub: Arc<AudioHub>,
    origins: Arc<OriginPolicy>,
    slots: Arc<Semaphore>,
}

impl AudioState {
    /// Create the state from a hub and an origin policy.
    #[must_use]
    pub fn new(hub: Arc<AudioHub>, origins: OriginPolicy) -> Self {
        Self {
            hub,
            origins: Arc::new(origins),
            slots: Arc::new(Semaphore::new(MAX_CLIENTS)),
        }
    }
}

/// Routes of the gateway, behind the authentication hook.
pub fn router(state: AudioState) -> Router {
    Router::new().route(PATH, get(audio_ws)).with_state(state)
}

#[derive(Debug, Deserialize)]
struct AudioQuery {
    dev: String,
}

/// The client's only message: replaces the set of subscribed VFOs.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Subscribe {
    #[serde(rename = "type")]
    _kind: String,
    v: Option<u32>,
    vfos: Vec<u32>,
}

/// Parse a client message into the wanted VFO set (duplicates collapse, empty = none).
///
/// # Errors
/// A static reason for a message that is not a valid `subscribe`.
pub fn parse_subscribe(text: &str) -> Result<BTreeSet<u32>, &'static str> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "message is not valid JSON")?;
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("subscribe") => {}
        _ => return Err("unknown message type"),
    }
    let message: Subscribe = serde_json::from_value(value).map_err(
        |_| "malformed subscribe message (expected {\"type\":\"subscribe\",\"vfos\":[ids]})",
    )?;
    if message.v.is_some_and(|v| v != 1) {
        return Err("unsupported protocol version");
    }
    let vfos: BTreeSet<u32> = message.vfos.into_iter().collect();
    if vfos.contains(&0) {
        return Err("invalid vfo id");
    }
    if vfos.len() > MAX_VFOS {
        return Err("too many vfos (at most 8)");
    }
    Ok(vfos)
}

fn error_text(reason: &str) -> Message {
    Message::text(serde_json::json!({"v": 1, "type": "error", "error": reason}).to_string())
}

async fn audio_ws(
    State(state): State<AudioState>,
    Query(query): Query<AudioQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<axum::response::Response, StatusCode> {
    if !state.origins.allows(&headers) {
        tracing::warn!("audio ws: origin rejected");
        return Err(StatusCode::FORBIDDEN);
    }
    let permit = Arc::clone(&state.slots)
        .try_acquire_owned()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let session = state.hub.session(&query.dev).map_err(|err| {
        tracing::warn!(%err, "audio ws: session refused");
        match err {
            HubError::InvalidDevice => StatusCode::BAD_REQUEST,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }
    })?;
    Ok(upgrade
        .max_message_size(MAX_CONTROL_BYTES)
        .max_frame_size(MAX_CONTROL_BYTES)
        .on_upgrade(move |socket| async move {
            let dropped = serve_client(socket, session).await;
            if dropped > 0 {
                tracing::info!(dropped, "audio client disconnected after dropped frames");
            }
            drop(permit);
        }))
}

/// Serve one client until it leaves; returns the number of frames dropped for it.
async fn serve_client(mut socket: WebSocket, mut session: Session) -> u64 {
    let queue = Arc::clone(session.queue());
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(text))) => {
                    let reply = match parse_subscribe(text.as_str()) {
                        Ok(wanted) => match session.set(&wanted).await {
                            Ok(()) => Message::text(
                                serde_json::json!({
                                    "v": 1,
                                    "type": "ack",
                                    "vfos": session.vfos(),
                                })
                                .to_string(),
                            ),
                            Err(err) => {
                                tracing::warn!(%err, "audio ws: subscribe failed");
                                error_text("audio stream unavailable")
                            }
                        },
                        Err(reason) => error_text(reason),
                    };
                    if !send(&mut socket, reply).await {
                        break;
                    }
                }
                // Pings are answered by the transport; binary input is not part of the protocol.
                Some(Ok(_)) => {}
            },
            item = queue.pop() => {
                let message = if item.text {
                    match String::from_utf8(item.data.to_vec()) {
                        Ok(text) => Message::text(text),
                        Err(_) => continue,
                    }
                } else {
                    Message::Binary(item.data)
                };
                if !send(&mut socket, message).await {
                    break;
                }
            }
        }
    }
    let dropped = queue.dropped();
    session.close().await;
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header};
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

    fn set(vfos: &[u32]) -> BTreeSet<u32> {
        vfos.iter().copied().collect()
    }

    #[test]
    fn parses_subscribe_messages() {
        assert_eq!(
            parse_subscribe(r#"{"type":"subscribe","vfos":[2,1,2]}"#),
            Ok(set(&[1, 2]))
        );
        assert_eq!(
            parse_subscribe(r#"{"type":"subscribe","v":1,"vfos":[]}"#),
            Ok(set(&[]))
        );
        let eight = r#"{"type":"subscribe","vfos":[1,2,3,4,5,6,7,8]}"#;
        assert_eq!(parse_subscribe(eight).map(|s| s.len()), Ok(8));
        // Duplicates do not count towards the limit.
        let dup = r#"{"type":"subscribe","vfos":[1,1,1,1,1,1,1,1,1,1]}"#;
        assert_eq!(parse_subscribe(dup), Ok(set(&[1])));
    }

    #[test]
    fn rejects_invalid_subscribe_messages() {
        for (text, reason) in [
            ("", "message is not valid JSON"),
            ("nope", "message is not valid JSON"),
            ("[]", "unknown message type"),
            (r#"{"vfos":[1]}"#, "unknown message type"),
            (r#"{"type":"view","vfos":[1]}"#, "unknown message type"),
            (r#"{"type":5,"vfos":[1]}"#, "unknown message type"),
            (
                r#"{"type":"subscribe","v":2,"vfos":[1]}"#,
                "unsupported protocol version",
            ),
            (r#"{"type":"subscribe","vfos":[0]}"#, "invalid vfo id"),
            (
                r#"{"type":"subscribe","vfos":[1,2,3,4,5,6,7,8,9]}"#,
                "too many vfos (at most 8)",
            ),
        ] {
            assert_eq!(parse_subscribe(text), Err(reason), "{text}");
        }
        for text in [
            r#"{"type":"subscribe"}"#,
            r#"{"type":"subscribe","vfos":"1"}"#,
            r#"{"type":"subscribe","vfos":[-1]}"#,
            r#"{"type":"subscribe","vfos":[1.5]}"#,
            r#"{"type":"subscribe","vfos":[4294967296]}"#,
            r#"{"type":"subscribe","vfos":[1],"extra":true}"#,
        ] {
            assert!(
                parse_subscribe(text).is_err_and(|e| e.starts_with("malformed")),
                "{text}"
            );
        }
    }

    struct Server {
        addr: std::net::SocketAddr,
        hub: Arc<AudioHub>,
    }

    async fn start(hub: AudioHub, origins: OriginPolicy) -> Server {
        let hub = Arc::new(hub);
        let app = router(AudioState::new(Arc::clone(&hub), origins));
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

    async fn next_text(client: &mut Client) -> String {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), client.next()).await {
                Ok(Some(Ok(tungstenite::Message::Text(t)))) => return t.to_string(),
                Ok(Some(Ok(_))) => {}
                other => panic!("expected text, got {other:?}"),
            }
        }
    }

    async fn next_binary(client: &mut Client) -> Vec<u8> {
        match tokio::time::timeout(Duration::from_secs(5), client.next()).await {
            Ok(Some(Ok(tungstenite::Message::Binary(b)))) => b.to_vec(),
            other => panic!("expected binary, got {other:?}"),
        }
    }

    /// 16 bytes standing in for a frame: VFO id at offset 12, tag in byte 0.
    fn raw(vfo: u32, tag: u8) -> Vec<u8> {
        let mut data = vec![0u8; 16];
        data[0] = tag;
        data[12..16].copy_from_slice(&vfo.to_le_bytes());
        data
    }

    async fn subscribe(client: &mut Client, text: &str) -> serde_json::Value {
        client
            .send(tungstenite::Message::text(text))
            .await
            .expect("send");
        serde_json::from_str(&next_text(client).await).expect("json reply")
    }

    #[tokio::test]
    async fn forwards_only_subscribed_vfos_unchanged() {
        let server = start(AudioHub::local(), OriginPolicy::default()).await;
        let mut client = connect(&server, "?dev=dev0", None).await.expect("connect");
        // Nothing subscribed yet: frames of any VFO are not delivered.
        assert_eq!(
            subscribe(&mut client, r#"{"type":"subscribe","vfos":[1,2]}"#).await,
            serde_json::json!({"v":1,"type":"ack","vfos":[1,2]})
        );
        server.hub.publish_local("dev0", 1, raw(1, 10)).await;
        server.hub.publish_local("dev0", 3, raw(3, 30)).await;
        server.hub.publish_local("dev0", 2, raw(2, 20)).await;
        assert_eq!(next_binary(&mut client).await, raw(1, 10));
        assert_eq!(next_binary(&mut client).await, raw(2, 20));

        // Replacing the set stops VFO 1 and starts VFO 3.
        assert_eq!(
            subscribe(&mut client, r#"{"type":"subscribe","vfos":[3,2]}"#).await,
            serde_json::json!({"v":1,"type":"ack","vfos":[2,3]})
        );
        server.hub.publish_local("dev0", 1, raw(1, 11)).await;
        server.hub.publish_local("dev0", 3, raw(3, 31)).await;
        assert_eq!(next_binary(&mut client).await, raw(3, 31));
        assert_eq!(server.hub.stream_count().await, 2);

        // Empty set = none; the streams are released.
        assert_eq!(
            subscribe(&mut client, r#"{"type":"subscribe","vfos":[]}"#).await,
            serde_json::json!({"v":1,"type":"ack","vfos":[]})
        );
        assert_eq!(server.hub.stream_count().await, 0);
    }

    #[tokio::test]
    async fn rds_state_arrives_as_text_between_the_audio_frames() {
        let server = start(AudioHub::local(), OriginPolicy::default()).await;
        let mut client = connect(&server, "?dev=dev0", None).await.expect("connect");
        subscribe(&mut client, r#"{"type":"subscribe","vfos":[1]}"#).await;
        let rds = superhet_bus::rds::RdsMessage {
            synced: true,
            ps: Some("SUPERHET".into()),
            ..superhet_bus::rds::RdsMessage::empty(1)
        }
        .encode();
        server.hub.publish_local("dev0", 1, raw(1, 10)).await;
        server.hub.publish_local_rds("dev0", 1, &rds).await;
        // Invalid, or for another VFO: dropped.
        server.hub.publish_local_rds("dev0", 1, b"nope").await;
        let foreign = superhet_bus::rds::RdsMessage::empty(2).encode();
        server.hub.publish_local_rds("dev0", 1, &foreign).await;
        server.hub.publish_local("dev0", 1, raw(1, 11)).await;
        assert_eq!(next_binary(&mut client).await, raw(1, 10));
        let text = next_text(&mut client).await;
        assert_eq!(text.as_bytes(), rds.as_slice());
        assert_eq!(next_binary(&mut client).await, raw(1, 11));
    }

    #[tokio::test]
    async fn invalid_messages_get_an_error_and_keep_the_connection() {
        let server = start(AudioHub::local(), OriginPolicy::default()).await;
        let mut client = connect(&server, "?dev=dev0", None).await.expect("connect");
        assert_eq!(
            subscribe(
                &mut client,
                r#"{"type":"subscribe","vfos":[1,2,3,4,5,6,7,8,9]}"#
            )
            .await,
            serde_json::json!({"v":1,"type":"error","error":"too many vfos (at most 8)"})
        );
        let reply = subscribe(&mut client, "nope").await;
        assert_eq!(reply["type"], "error");
        // Still usable.
        let reply = subscribe(&mut client, r#"{"type":"subscribe","vfos":[1]}"#).await;
        assert_eq!(reply["type"], "ack");
        // An oversized message closes the connection (limit enforced by the transport).
        let big = format!(
            r#"{{"type":"subscribe","vfos":[1],"x":"{}"}}"#,
            "a".repeat(2048)
        );
        let _ = client.send(tungstenite::Message::text(big)).await;
        loop {
            match tokio::time::timeout(Duration::from_secs(5), client.next()).await {
                Ok(Some(Ok(tungstenite::Message::Text(_)))) => panic!("oversized message answered"),
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(_)) | None) => break,
                Err(_) => panic!("connection stayed open"),
            }
        }
    }

    #[tokio::test]
    async fn rejects_bad_origin_device_and_missing_bus() {
        let server = start(AudioHub::local(), OriginPolicy::default()).await;
        let err = connect(&server, "?dev=dev0", Some("https://evil.example"))
            .await
            .expect_err("origin");
        assert_eq!(status_of(err), 403);
        let err = connect(&server, "?dev=a.b", None).await.expect_err("dev");
        assert_eq!(status_of(err), 400);
        let err = connect(&server, "", None).await.expect_err("missing dev");
        assert_eq!(status_of(err), 400);

        let server = start(AudioHub::new(None), OriginPolicy::default()).await;
        let err = connect(&server, "?dev=dev0", None).await.expect_err("bus");
        assert_eq!(status_of(err), 503);
    }

    #[tokio::test]
    async fn disconnect_releases_the_streams() {
        let server = start(AudioHub::local(), OriginPolicy::default()).await;
        let mut client = connect(&server, "?dev=dev0", None).await.expect("connect");
        subscribe(&mut client, r#"{"type":"subscribe","vfos":[1,2]}"#).await;
        assert_eq!(server.hub.stream_count().await, 2);
        drop(client);
        for _ in 0..100 {
            if server.hub.stream_count().await == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("streams were not released");
    }
}
