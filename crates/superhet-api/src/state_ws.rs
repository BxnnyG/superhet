//! WebSocket state feed: `GET /api/v1/ws/state?dev=<id>`.
//!
//! Forwards every state document the daemon publishes on `sh.v1.dev.<dev>.state` (tuning, VFOs,
//! locks, ... see `docs/api-devices.md`) so the browser never has to poll. The current state is
//! sent first. Protocol: `docs/protocol-ws.md`.

use std::sync::Arc;

use async_nats::{Client, Subscriber, connection::State as Connection};
use axum::{
    Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
};
use futures_util::StreamExt;
use serde::Deserialize;
use superhet_bus::subjects;
use tokio::sync::Semaphore;

use crate::devices::{DevicesState, ctl_state, read_kv};
use crate::ws::send;

/// Route path of the state feed.
pub const PATH: &str = "/api/v1/ws/state";
/// Maximum size of a client message (the feed ignores them, this only bounds memory).
const MAX_CLIENT_BYTES: usize = 1024;
/// Maximum concurrent state clients.
const MAX_CLIENTS: usize = 32;

/// Shared state of the state feed.
#[derive(Clone)]
pub struct StateWsState {
    devices: DevicesState,
    slots: Arc<Semaphore>,
}

impl StateWsState {
    /// Create the state from the device routes' state (bus, origin policy).
    #[must_use]
    pub fn new(devices: DevicesState) -> Self {
        Self {
            devices,
            slots: Arc::new(Semaphore::new(MAX_CLIENTS)),
        }
    }
}

/// Routes of the feed, behind the authentication hook.
pub fn router(state: StateWsState) -> Router {
    Router::new().route(PATH, get(state_ws)).with_state(state)
}

#[derive(Debug, Deserialize)]
struct StateQuery {
    dev: String,
}

async fn state_ws(
    State(state): State<StateWsState>,
    Query(query): Query<StateQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    if !state.devices.origins.allows(&headers) {
        tracing::warn!("state ws: origin rejected");
        return Err(StatusCode::FORBIDDEN);
    }
    let subject = subjects::state(&query.dev).map_err(|_| StatusCode::BAD_REQUEST)?;
    let client = match &state.devices.nats {
        Some(client) if client.connection_state() == Connection::Connected => client.clone(),
        _ => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    let permit = Arc::clone(&state.slots)
        .try_acquire_owned()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // Subscribe before reading the current state, so no change falls between the two.
    let updates = client.subscribe(subject).await.map_err(|err| {
        tracing::warn!(%err, "state ws: subscribe failed");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    let device = query.dev;
    Ok(upgrade
        .max_message_size(MAX_CLIENT_BYTES)
        .max_frame_size(MAX_CLIENT_BYTES)
        .on_upgrade(move |socket| async move {
            let initial = current_state(&state.devices, &client, &device).await;
            serve_client(socket, updates, initial).await;
            drop(permit);
        }))
}

/// The state to start with: from the daemon, else the last known one from KV.
async fn current_state(devices: &DevicesState, client: &Client, device: &str) -> Option<String> {
    if let Ok(value) = ctl_state(devices, device).await {
        return Some(value.to_string());
    }
    let key = subjects::state_key(device).ok()?;
    let stored = read_kv(client, &key).await?;
    String::from_utf8(stored.to_vec()).ok()
}

/// `{"v":1,"type":"state","state":<doc>}` for a document that is a JSON object.
fn envelope(doc: &str) -> Option<String> {
    let is_object = serde_json::from_str::<serde_json::Value>(doc)
        .ok()
        .is_some_and(|v| v.is_object());
    is_object.then(|| format!(r#"{{"v":1,"type":"state","state":{doc}}}"#))
}

async fn serve_client(mut socket: WebSocket, mut updates: Subscriber, initial: Option<String>) {
    if let Some(frame) = initial.as_deref().and_then(envelope)
        && !send(&mut socket, Message::text(frame)).await
    {
        return;
    }
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                // The feed is one-way; anything the client sends is ignored.
                Some(Ok(_)) => {}
            },
            update = updates.next() => {
                let Some(update) = update else { break };
                let Some(frame) = std::str::from_utf8(&update.payload).ok().and_then(envelope) else {
                    continue;
                };
                if !send(&mut socket, Message::text(frame)).await {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_only_json_objects() {
        assert_eq!(
            envelope(r#"{"a":1}"#).as_deref(),
            Some(r#"{"v":1,"type":"state","state":{"a":1}}"#)
        );
        for bad in ["", "[]", "1", "not json", r#"{"a":"#] {
            assert_eq!(envelope(bad), None, "{bad}");
        }
    }
}
