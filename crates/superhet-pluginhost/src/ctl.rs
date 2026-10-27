//! The plugin host's control API on the bus (`sh.v1.plugin.ctl.<cmd>`), the instance state feed
//! (`sh.v1.plugin.state`) and the event publisher (`sh.v1.events.<plugin>.<type>`).
//!
//! [`handle`] is the transport independent request handler (JSON in, JSON out); [`serve`] wires
//! it to NATS. Who may call it is decided by the API gateway (admin for install, operator or
//! admin for start/stop/restart, ARCHITECTURE section 6.7), the host trusts its bus peers.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::Value;
use superhet_bus::subjects;
use superhet_plugin_protocol::PROTOCOL_VERSION;
use superhet_plugin_protocol::ctl::{
    ControlRequest, ErrorReply, InstallRequest, InstanceRef, InstanceReply, InstancesReply,
    MediaReplyHeader, MediaRequest, OkReply, PluginInfo, PluginReply, PluginsReply, RemoveRequest,
    RemovedReply, StartRequest, StatusReply, cmd, code,
};
use superhet_plugin_protocol::event::HostEvent;
use tokio::sync::mpsc;

use crate::registry::{Registry, RegistryError};
use crate::store::InstanceStore;
use crate::supervisor::{EventSink, StartError, StartSpec, Supervisor};

/// Everything a request needs.
#[derive(Clone)]
pub struct Host {
    /// Installed plugins.
    pub registry: Arc<Registry>,
    /// Running instances.
    pub supervisor: Supervisor,
    /// Persistent record of the started instances.
    pub store: InstanceStore,
}

/// Start every stored instance again (host start-up). Instances whose plugin is gone or whose
/// binding is refused stay in the store and are reported.
pub async fn restore(host: &Host) {
    for stored in host.store.load().await {
        let Some(package) = host.registry.get(&stored.plugin_id) else {
            tracing::warn!(instance = %stored.id, plugin = %stored.plugin_id, "stored instance: plugin is not installed");
            continue;
        };
        match host
            .supervisor
            .start_as(Some(stored.id.clone()), package, stored.spec)
        {
            Ok(info) => {
                tracing::info!(instance = %info.id, plugin = %info.plugin_id, "restored plugin instance");
            }
            Err(err) => {
                tracing::warn!(instance = %stored.id, plugin = %stored.plugin_id, %err, "stored instance could not be restored");
            }
        }
    }
}

fn ok_json<T: serde::Serialize>(reply: &T) -> Value {
    serde_json::to_value(reply)
        .unwrap_or_else(|_| error(code::INTERNAL, "reply could not be encoded"))
}

fn error(code: &str, message: impl Into<String>) -> Value {
    serde_json::to_value(ErrorReply::new(code, message)).unwrap_or(Value::Null)
}

fn parse<T: DeserializeOwned>(payload: &[u8]) -> Result<T, Value> {
    let value: Value = serde_json::from_slice(payload)
        .map_err(|err| error(code::INVALID, format!("body is not JSON: {err}")))?;
    if value.get("v").and_then(Value::as_u64) != Some(u64::from(PROTOCOL_VERSION)) {
        return Err(error(code::INVALID, "\"v\":1 is required"));
    }
    serde_json::from_value(value).map_err(|err| error(code::INVALID, err.to_string()))
}

fn start_error(err: &StartError) -> Value {
    let class = match err {
        StartError::Unsupported(_) => code::UNSUPPORTED,
        StartError::Conflict(_) => code::CONFLICT,
        StartError::Invalid(_) => code::INVALID,
        StartError::NotFound => code::NOT_FOUND,
    };
    error(class, err.to_string())
}

/// Handle one request and encode the reply: JSON for every command, except that a successful
/// `media` reply is `MediaReplyHeader` JSON, a newline and the object bytes.
pub async fn handle_bytes(host: &Host, command: &str, payload: &[u8]) -> Vec<u8> {
    if command != cmd::MEDIA {
        return serde_json::to_vec(&handle(host, command, payload).await).unwrap_or_default();
    }
    let reply = match parse::<MediaRequest>(payload) {
        Err(reply) => reply,
        Ok(req) => match host.supervisor.media(&req.id, &req.key) {
            Some((mime, revision, bytes)) => {
                return MediaReplyHeader {
                    v: PROTOCOL_VERSION,
                    ok: true,
                    key: req.key,
                    mime,
                    revision,
                }
                .encode(&bytes);
            }
            None if host.supervisor.get(&req.id).is_none() => start_error(&StartError::NotFound),
            None => error(code::NOT_FOUND, "no such media object"),
        },
    };
    serde_json::to_vec(&reply).unwrap_or_default()
}

/// Handle one request; the reply is always a JSON object with `ok`.
pub async fn handle(host: &Host, command: &str, payload: &[u8]) -> Value {
    match handle_inner(host, command, payload).await {
        Ok(reply) | Err(reply) => reply,
    }
}

async fn handle_inner(host: &Host, command: &str, payload: &[u8]) -> Result<Value, Value> {
    let instance = |info| {
        ok_json(&InstanceReply {
            v: PROTOCOL_VERSION,
            ok: true,
            instance: info,
        })
    };
    match command {
        cmd::LIST => Ok(ok_json(&PluginsReply {
            v: PROTOCOL_VERSION,
            ok: true,
            plugins: host
                .registry
                .list()
                .iter()
                .map(|p| PluginInfo {
                    manifest: p.manifest.clone(),
                })
                .collect(),
        })),
        cmd::INSTALL => {
            let req: InstallRequest = parse(payload)?;
            let registry = Arc::clone(&host.registry);
            let source = req.path;
            let installed =
                tokio::task::spawn_blocking(move || registry.install(Path::new(&source)))
                    .await
                    .map_err(|err| error(code::INTERNAL, err.to_string()))?;
            match installed {
                Ok(package) => Ok(ok_json(&PluginReply {
                    v: PROTOCOL_VERSION,
                    ok: true,
                    plugin: PluginInfo {
                        manifest: package.manifest.clone(),
                    },
                })),
                Err(RegistryError::Invalid(msg)) => Err(error(code::INVALID, msg)),
                Err(RegistryError::Exists(msg)) => {
                    Err(error(code::CONFLICT, format!("{msg} is already installed")))
                }
                Err(RegistryError::NotFound(msg)) => Err(error(code::NOT_FOUND, msg)),
                Err(RegistryError::Io(err)) => Err(error(code::INTERNAL, err.to_string())),
            }
        }
        cmd::REMOVE => {
            let req: RemoveRequest = parse(payload)?;
            if let Some(running) = host
                .supervisor
                .list()
                .iter()
                .find(|i| i.plugin_id == req.plugin_id)
            {
                return Err(error(
                    code::CONFLICT,
                    format!("instance {} is still running; stop it first", running.id),
                ));
            }
            let registry = Arc::clone(&host.registry);
            let id = req.plugin_id.clone();
            let removed = tokio::task::spawn_blocking(move || registry.remove(&id))
                .await
                .map_err(|err| error(code::INTERNAL, err.to_string()))?;
            match removed {
                Ok(versions) => Ok(ok_json(&RemovedReply {
                    v: PROTOCOL_VERSION,
                    ok: true,
                    plugin_id: req.plugin_id,
                    versions,
                })),
                Err(RegistryError::NotFound(msg)) => Err(error(code::NOT_FOUND, msg)),
                Err(RegistryError::Invalid(msg)) => Err(error(code::INVALID, msg)),
                Err(RegistryError::Exists(msg)) => Err(error(code::CONFLICT, msg)),
                Err(RegistryError::Io(err)) => Err(error(code::INTERNAL, err.to_string())),
            }
        }
        cmd::INSTANCES => Ok(ok_json(&InstancesReply {
            v: PROTOCOL_VERSION,
            ok: true,
            instances: host.supervisor.list(),
        })),
        cmd::GET => {
            let req: InstanceRef = parse(payload)?;
            host.supervisor
                .get(&req.id)
                .map(instance)
                .ok_or_else(|| start_error(&StartError::NotFound))
        }
        cmd::START => {
            let req: StartRequest = parse(payload)?;
            let package = host.registry.get(&req.plugin_id).ok_or_else(|| {
                error(
                    code::NOT_FOUND,
                    format!("plugin {} is not installed", req.plugin_id),
                )
            })?;
            let spec = StartSpec {
                device: req.device,
                vfo: req.vfo,
                freq_hz: req.freq_hz,
                config: req.config,
            };
            let info = host
                .supervisor
                .start(package, spec.clone())
                .map_err(|err| start_error(&err))?;
            host.store.save(&info.id, &info.plugin_id, &spec).await;
            Ok(instance(info))
        }
        cmd::STOP => {
            let req: InstanceRef = parse(payload)?;
            let info = host
                .supervisor
                .stop(&req.id)
                .await
                .ok_or_else(|| start_error(&StartError::NotFound))?;
            host.store.forget(&req.id).await;
            Ok(instance(info))
        }
        cmd::RESTART => {
            let req: InstanceRef = parse(payload)?;
            host.supervisor
                .restart(&req.id)
                .map(instance)
                .map_err(|err| start_error(&err))
        }
        cmd::CONTROL => {
            let req: ControlRequest = parse(payload)?;
            host.supervisor
                .control(&req.id, &req.message)
                .map_err(|err| start_error(&err))?;
            Ok(ok_json(&OkReply {
                v: PROTOCOL_VERSION,
                ok: true,
            }))
        }
        cmd::STATUS => {
            let req: InstanceRef = parse(payload)?;
            let snapshot = host
                .supervisor
                .status(&req.id)
                .ok_or_else(|| start_error(&StartError::NotFound))?;
            Ok(ok_json(&StatusReply {
                v: PROTOCOL_VERSION,
                ok: true,
                status: snapshot.status,
                updated_unix_ms: snapshot.updated_unix_ms,
                media: snapshot.media,
            }))
        }
        other => Err(error(code::NOT_FOUND, format!("unknown command {other:?}"))),
    }
}

/// Serve the control subjects until the subscription ends.
///
/// # Errors
/// Fails when the subscription cannot be created.
pub async fn serve(
    client: async_nats::Client,
    host: Host,
) -> Result<(), async_nats::SubscribeError> {
    let mut sub = client.subscribe(subjects::ALL_PLUGIN_CTL).await?;
    while let Some(msg) = sub.next().await {
        let Some(reply_to) = msg.reply.clone() else {
            continue;
        };
        let command = msg
            .subject
            .as_str()
            .strip_prefix("sh.v1.plugin.ctl.")
            .unwrap_or_default()
            .to_owned();
        let host = host.clone();
        let client = client.clone();
        // One task per request: `stop` waits for the process and must not block other requests.
        tokio::spawn(async move {
            let body = handle_bytes(&host, &command, &msg.payload).await;
            if let Err(err) = client.publish(reply_to, body.into()).await {
                tracing::warn!(%err, "could not send control reply");
            }
        });
    }
    Ok(())
}

/// Store the new frequency of an instance whose plugin moved its window, so a restart of the
/// host brings it back on the same block.
pub async fn persist_retunes(supervisor: Supervisor, store: InstanceStore) {
    if !store.is_persistent() {
        return;
    }
    let mut rx = supervisor.subscribe();
    let mut known: std::collections::HashMap<String, Option<u64>> =
        std::collections::HashMap::new();
    loop {
        match rx.recv().await {
            Ok(info) => {
                let last = known.insert(info.id.clone(), info.freq_hz);
                if let (Some(previous), Some(freq)) = (last, info.freq_hz)
                    && previous != Some(freq)
                {
                    store.set_freq(&info.id, freq).await;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Publish every instance state change on `sh.v1.plugin.state`.
pub async fn publish_state(client: async_nats::Client, supervisor: Supervisor) {
    let mut rx = supervisor.subscribe();
    loop {
        match rx.recv().await {
            Ok(info) => {
                if let Ok(body) = serde_json::to_vec(&info)
                    && let Err(err) = client.publish(subjects::PLUGIN_STATE, body.into()).await
                {
                    tracing::warn!(%err, "could not publish instance state");
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "state feed lagged");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// [`EventSink`] that queues events for [`publish_events`]; a full queue drops the event.
pub struct QueueSink {
    tx: mpsc::Sender<HostEvent>,
}

impl QueueSink {
    /// Create the sink and the receiver for [`publish_events`].
    #[must_use]
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<HostEvent>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self { tx }, rx)
    }
}

impl EventSink for QueueSink {
    fn emit(&self, event: HostEvent) {
        if self.tx.try_send(event).is_err() {
            tracing::warn!("event queue full, dropping event");
        }
    }
}

/// How long publishing one event to the stream may take before it falls back to core NATS.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(3);
/// Retry delay while the durable stream cannot be created.
const STREAM_RETRY: Duration = Duration::from_secs(5);

/// Create the durable `EVENTS` stream (idempotent), retrying until the server accepts it.
pub async fn ensure_events_stream(client: async_nats::Client) {
    let js = jetstream::new(client);
    loop {
        match tokio::time::timeout(
            PUBLISH_TIMEOUT,
            js.get_or_create_stream(superhet_bus::events_stream_config()),
        )
        .await
        {
            Ok(Ok(_)) => return,
            Ok(Err(err)) => tracing::warn!(%err, "cannot create the EVENTS stream; retrying"),
            Err(_) => tracing::warn!("creating the EVENTS stream timed out; retrying"),
        }
        tokio::time::sleep(STREAM_RETRY).await;
    }
}

/// Publish queued events on `sh.v1.events.<plugin>.<type>` into the durable `EVENTS` stream
/// (acknowledged by the server; the worker stores them in the decoder history). If the stream
/// does not take an event the event is still published on core NATS for live subscribers.
pub async fn publish_events(client: async_nats::Client, mut rx: mpsc::Receiver<HostEvent>) {
    let js = jetstream::new(client.clone());
    let mut failures = 0u64;
    while let Some(event) = rx.recv().await {
        let Ok(subject) = subjects::plugin_event(&event.plugin_id, &event.event_type) else {
            continue;
        };
        let Ok(body) = serde_json::to_vec(&event) else {
            continue;
        };
        let durable = async {
            js.publish(subject.clone(), body.clone().into())
                .await
                .map_err(|err| err.to_string())?
                .await
                .map_err(|err| err.to_string())
        };
        let outcome = match tokio::time::timeout(PUBLISH_TIMEOUT, durable).await {
            Ok(result) => result.map(|_| ()),
            Err(_) => Err("timed out".to_owned()),
        };
        if let Err(err) = outcome {
            failures += 1;
            if failures == 1 || failures.is_multiple_of(100) {
                tracing::warn!(%err, failures, "event not stored durably; publishing on core NATS");
            }
            if let Err(err) = client.publish(subject, body.into()).await {
                tracing::warn!(%err, "could not publish plugin event");
            }
        }
    }
}
