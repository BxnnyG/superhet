//! `GET /api/v1/share/{token}/ws/audio`: the audio socket of a share link.
//!
//! Same frames as `/api/v1/ws/audio` (`docs/protocol-audio.md`), but the server decides what may
//! be heard. On a fixed link every connection gets a VFO of its own, created when the socket
//! opens and deleted when it closes; on a link that follows the operator the client subscribes
//! to the operator's VFOs it learned from `state`. Besides `subscribe` the socket understands
//! `{"type":"tune","freq_hz":N}`, honoured only inside the link's tuning range.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Extension,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use superhet_bus::{audit::Outcome, subjects};

use super::links::{Feature, Link, TuneRefusal, Tuning};
use super::live::Permit;
use super::{Grant, ShareState, audio_refusal};
use crate::audio_hub::{HubError, Session};
use crate::audit::Who;
use crate::devices::{call_daemon, ctl_state};
use crate::registry::ConnGuard;
use crate::ws::send;

/// Largest client message.
const MAX_CONTROL_BYTES: usize = 512;
/// Retunes closer together than this are refused (a slider drag sends many).
const MIN_TUNE_INTERVAL: Duration = Duration::from_millis(100);

pub(super) async fn audio_ws(
    State(state): State<ShareState>,
    Extension(grant): Extension<Grant>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    if !state.0.devices.origins.allows(&headers) {
        tracing::warn!("share audio ws: origin rejected");
        return Err(StatusCode::FORBIDDEN);
    }
    let permit = grant
        .permit
        .clone()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let link = Arc::clone(&grant.link);
    let session = state.0.audio_hub.session(&link.device).map_err(|err| {
        tracing::warn!(%err, "share audio ws: session refused");
        match err {
            HubError::InvalidDevice => StatusCode::BAD_REQUEST,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }
    })?;
    let who = ShareState::who(&grant.client, Some(&link), Some(permit.key()));
    let conn = state
        .auth()
        .registry()
        .connect(permit.key(), &who, "audio", &link.device)
        .await;
    Ok(upgrade
        .max_message_size(MAX_CONTROL_BYTES)
        .max_frame_size(MAX_CONTROL_BYTES)
        .on_upgrade(move |socket| async move {
            let _client = crate::metrics::track_client("audio");
            serve(socket, state, link, session, conn, who, permit).await;
        }))
}

fn text(value: &Value) -> Message {
    Message::text(value.to_string())
}

fn error_text(reason: &str) -> Message {
    text(&json!({"v": 1, "type": "error", "error": reason}))
}

/// Create the listener's VFO on the daemon; returns its id.
async fn create_vfo(
    state: &ShareState,
    device: &str,
    owner: &str,
    freq_hz: u64,
    mode: &str,
) -> Result<u32, &'static str> {
    let subject = subjects::ctl_vfo(device, "create").map_err(|_| "invalid device")?;
    let payload = json!({"v": 1, "freq_hz": freq_hz, "mode": mode, "owner": owner})
        .to_string()
        .into_bytes();
    let reply = call_daemon(&state.0.devices, subject, payload)
        .await
        .map_err(|_| "the receiver is busy")?;
    serde_json::from_slice::<Value>(&reply)
        .ok()
        .and_then(|v| v["vfo"]["id"].as_u64())
        .and_then(|id| u32::try_from(id).ok())
        .ok_or("the receiver is busy")
}

async fn vfo_call(state: &ShareState, device: &str, action: &str, body: Value) -> bool {
    let Ok(subject) = subjects::ctl_vfo(device, action) else {
        return false;
    };
    call_daemon(&state.0.devices, subject, body.to_string().into_bytes())
        .await
        .is_ok()
}

/// Whether the connection may hear `wanted`; follows the operator's VFOs from a fresh state.
async fn may_listen(
    state: &ShareState,
    link: &Link,
    own: Option<u32>,
    wanted: &BTreeSet<u32>,
) -> Result<(), &'static str> {
    let needs_state = link.tuning == Tuning::Follow
        && wanted
            .iter()
            .any(|id| !superhet_bus::audio::is_plugin_vfo(*id));
    let doc = if needs_state {
        Some(
            ctl_state(&state.0.devices, &link.device)
                .await
                .map_err(|_| "audio stream unavailable")?,
        )
    } else {
        None
    };
    let operators = state.operators().await;
    audio_refusal(link, doc.as_ref(), &operators, own, wanted)
}

struct Conn<'a> {
    state: &'a ShareState,
    link: &'a Link,
    who: &'a Who,
    own: Option<u32>,
    last_tune: Option<Instant>,
}

impl Conn<'_> {
    async fn handle(&mut self, session: &mut Session, raw: &str) -> Message {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return error_text("message is not valid JSON");
        };
        match value.get("type").and_then(Value::as_str) {
            Some("subscribe") => match crate::audio_ws::parse_subscribe(raw) {
                Err(reason) => error_text(reason),
                Ok(wanted) => match may_listen(self.state, self.link, self.own, &wanted).await {
                    Err(reason) => error_text(reason),
                    Ok(()) => match session.set(&wanted).await {
                        Ok(()) => text(&json!({"v": 1, "type": "ack", "vfos": session.vfos()})),
                        Err(err) => {
                            tracing::warn!(%err, "share audio ws: subscribe failed");
                            error_text("audio stream unavailable")
                        }
                    },
                },
            },
            Some("tune") => self.tune(&value).await,
            _ => error_text("unknown message type"),
        }
    }

    async fn tune(&mut self, value: &Value) -> Message {
        let Some(freq) = value.get("freq_hz").and_then(Value::as_u64) else {
            return error_text(
                "malformed tune message (expected {\"type\":\"tune\",\"freq_hz\":N})",
            );
        };
        if self
            .last_tune
            .is_some_and(|t| t.elapsed() < MIN_TUNE_INTERVAL)
        {
            return error_text("tuning too fast");
        }
        self.last_tune = Some(Instant::now());
        let audit = |outcome: Outcome| {
            self.state.auth().audit().log_coalesced(
                self.who,
                "share.tune",
                Some(&self.link.device),
                outcome,
                json!({"link": self.link.id, "freq_hz": freq}),
            );
        };
        match self.link.check_tune(freq) {
            Err(TuneRefusal::NotAllowed) => {
                audit(Outcome::Denied);
                error_text("tuning is not allowed on this link")
            }
            Err(TuneRefusal::OutOfRange) => {
                audit(Outcome::Denied);
                error_text("frequency outside the range of this link")
            }
            Ok(()) => {
                let Some(id) = self.own else {
                    return error_text("tuning is not allowed on this link");
                };
                let body = json!({"v": 1, "id": id, "freq_hz": freq});
                if vfo_call(self.state, &self.link.device, "update", body).await {
                    audit(Outcome::Ok);
                    text(&json!({"v": 1, "type": "tuned", "freq_hz": freq}))
                } else {
                    audit(Outcome::Error);
                    error_text("the receiver could not retune")
                }
            }
        }
    }
}

/// The greeting: what this connection may do.
fn hello(link: &Link, own: Option<u32>, freq_hz: Option<u64>) -> Message {
    text(&json!({
        "v": 1,
        "type": "hello",
        "vfo": own,
        "freq_hz": freq_hz,
        "tune": link.tune_range().filter(|_| link.allows(Feature::Audio))
            .map(|(lo, hi)| json!({"min_hz": lo, "max_hz": hi})),
    }))
}

async fn serve(
    mut socket: WebSocket,
    state: ShareState,
    link: Arc<Link>,
    mut session: Session,
    mut kill: ConnGuard,
    who: Who,
    permit: Arc<Permit>,
) {
    let owner = permit.key().replacen("share:", "s:", 1);
    let mut own = None;
    let mut start_freq = None;
    if let Tuning::Fixed { freq_hz, mode, .. } = &link.tuning
        && link.allows(Feature::Audio)
    {
        // Announce the owner before the VFO exists, so the upkeep never sweeps it as an orphan.
        state.live().hold_owner(&owner);
        match create_vfo(&state, &link.device, &owner, *freq_hz, mode).await {
            Ok(id) => {
                own = Some(id);
                start_freq = Some(*freq_hz);
            }
            Err(reason) => {
                state.live().release_owner(&owner);
                let _ = send(&mut socket, error_text(reason)).await;
                session.close().await;
                return;
            }
        }
    }
    let greeting = hello(&link, own, start_freq);
    let mut alive = send(&mut socket, greeting).await;
    if alive && let Some(id) = own {
        let wanted = BTreeSet::from([id]);
        alive = session.set(&wanted).await.is_ok();
    }
    let queue = Arc::clone(session.queue());
    let mut conn = Conn {
        state: &state,
        link: &link,
        who: &who,
        own,
        last_tune: None,
    };
    loop {
        if !alive {
            break;
        }
        tokio::select! {
            () = kill.killed() => break,
            incoming = socket.recv() => match incoming {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(raw))) => {
                    let reply = conn.handle(&mut session, raw.as_str()).await;
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
                        Ok(t) => Message::text(t),
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
    session.close().await;
    if let Some(id) = own {
        let _ = vfo_call(&state, &link.device, "delete", json!({"v": 1, "id": id})).await;
    }
    state.live().release_owner(&owner);
    drop(permit);
}
