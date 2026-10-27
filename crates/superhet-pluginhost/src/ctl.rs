//! The plugin host's control API on the bus (`sh.v1.plugin.ctl.<cmd>`), the instance state feed
//! (`sh.v1.plugin.state`) and the event publisher (`sh.v1.events.<plugin>.<type>`).
//!
//! [`handle`] is the transport independent request handler (JSON in, JSON out); [`serve`] wires
//! it to NATS. Who may call it is decided by the API gateway (admin for install, operator or
//! admin for start/stop/restart, ARCHITECTURE section 6.7), the host trusts its bus peers.

use std::path::Path;
use std::sync::Arc;

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::Value;
use superhet_bus::subjects;
use superhet_plugin_protocol::PROTOCOL_VERSION;
use superhet_plugin_protocol::ctl::{
    ErrorReply, InstallRequest, InstanceRef, InstanceReply, InstancesReply, PluginInfo,
    PluginReply, PluginsReply, StartRequest, cmd, code,
};
use superhet_plugin_protocol::event::HostEvent;
use tokio::sync::mpsc;

use crate::registry::{Registry, RegistryError};
use crate::supervisor::{EventSink, StartError, StartSpec, Supervisor};

/// Everything a request needs.
#[derive(Clone)]
pub struct Host {
    /// Installed plugins.
    pub registry: Arc<Registry>,
    /// Running instances.
    pub supervisor: Supervisor,
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
        StartError::NotFound => code::NOT_FOUND,
    };
    error(class, err.to_string())
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
            host.supervisor
                .start(
                    package,
                    StartSpec {
                        device: req.device,
                        freq_hz: req.freq_hz,
                        config: req.config,
                    },
                )
                .map(instance)
                .map_err(|err| start_error(&err))
        }
        cmd::STOP => {
            let req: InstanceRef = parse(payload)?;
            host.supervisor
                .stop(&req.id)
                .await
                .map(instance)
                .ok_or_else(|| start_error(&StartError::NotFound))
        }
        cmd::RESTART => {
            let req: InstanceRef = parse(payload)?;
            host.supervisor
                .restart(&req.id)
                .map(instance)
                .map_err(|err| start_error(&err))
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
            let reply = handle(&host, &command, &msg.payload).await;
            let body = serde_json::to_vec(&reply).unwrap_or_default();
            if let Err(err) = client.publish(reply_to, body.into()).await {
                tracing::warn!(%err, "could not send control reply");
            }
        });
    }
    Ok(())
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

/// Publish queued events on `sh.v1.events.<plugin>.<type>` (core NATS; the durable stream and
/// persistence follow with the worker).
pub async fn publish_events(client: async_nats::Client, mut rx: mpsc::Receiver<HostEvent>) {
    while let Some(event) = rx.recv().await {
        let Ok(subject) = subjects::plugin_event(&event.plugin_id, &event.event_type) else {
            continue;
        };
        if let Ok(body) = serde_json::to_vec(&event)
            && let Err(err) = client.publish(subject, body.into()).await
        {
            tracing::warn!(%err, "could not publish plugin event");
        }
    }
}
