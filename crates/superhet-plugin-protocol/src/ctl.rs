//! Control API of the plugin host on the bus (`sh.v1.plugin.ctl.<cmd>`, request/reply, JSON).
//!
//! Requests carry `"v":1`; replies are `{"v":1,"ok":true,...}` or
//! `{"v":1,"ok":false,"code":"...","error":"..."}` like the device daemon. State changes of an
//! instance are published as [`InstanceInfo`] on `sh.v1.plugin.state`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::manifest::Manifest;

/// Command tokens (last token of `sh.v1.plugin.ctl.<cmd>`).
pub mod cmd {
    /// List installed plugins.
    pub const LIST: &str = "list";
    /// Install a plugin from a local path.
    pub const INSTALL: &str = "install";
    /// List instances.
    pub const INSTANCES: &str = "instances";
    /// Fetch one instance.
    pub const GET: &str = "get";
    /// Create and start an instance.
    pub const START: &str = "start";
    /// Stop and remove an instance.
    pub const STOP: &str = "stop";
    /// Start an instance in state `error` or `stopped` again.
    pub const RESTART: &str = "restart";
    /// Remove an installed plugin (all versions); refused while instances of it exist.
    pub const REMOVE: &str = "remove";
    /// Send a control message to a framed instance.
    pub const CONTROL: &str = "control";
    /// Read the latest `status` document of a framed instance.
    pub const STATUS: &str = "status";
    /// Read the latest `media` object of a framed instance.
    pub const MEDIA: &str = "media";
}

/// First id of the pseudo VFOs under which plugin audio is published
/// (`sh.v1.dev.<dev>.audio.<id>`, `docs/protocol-audio.md`); real VFO ids are far below.
pub const AUDIO_VFO_BASE: u32 = 1 << 30;

/// Whether `vfo_id` names the audio of a plugin instance rather than a VFO.
#[must_use]
pub const fn is_plugin_audio_vfo(vfo_id: u32) -> bool {
    vfo_id >= AUDIO_VFO_BASE
}

/// The pseudo VFO id under which instance `instance_id` publishes its audio (stable for the id).
#[must_use]
pub fn audio_vfo_of(instance_id: &str) -> u32 {
    // FNV-1a, folded into the reserved range.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in instance_id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    AUDIO_VFO_BASE | (hash & (AUDIO_VFO_BASE - 1))
}

/// Lifecycle state of an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    /// Process is being spawned.
    Starting,
    /// Process is up.
    Running,
    /// Crashed; waiting before the next start.
    Backoff,
    /// Too many crashes (5 in 5 minutes) or not startable; needs `restart`.
    Error,
    /// Stopped on request.
    Stopped,
}

/// Snapshot of an instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InstanceInfo {
    /// Instance id.
    pub id: String,
    /// Plugin id.
    pub plugin_id: String,
    /// Device the instance is bound to.
    pub device: Option<String>,
    /// VFO of the device whose audio or IQ feeds the instance.
    #[serde(default)]
    pub vfo: Option<u32>,
    /// Frequency (Hz) the instance is bound to.
    pub freq_hz: Option<u64>,
    /// Operator configuration passed to the plugin.
    pub config: Value,
    /// Whether the plugin claims the device exclusively.
    pub exclusive: bool,
    /// Lifecycle state.
    pub state: InstanceState,
    /// Process id while running.
    pub pid: Option<u32>,
    /// How often the process was started again after a crash.
    pub restarts: u32,
    /// Crashes inside the current 5 minute window.
    pub crashes_in_window: u32,
    /// Milliseconds until the next start while in `backoff`.
    pub retry_in_ms: Option<u64>,
    /// Description of the last exit or failure.
    pub last_error: Option<String>,
    /// Valid events received.
    pub events_ok: u64,
    /// Rejected event lines (counted and dropped).
    pub events_invalid: u64,
    /// Input chunks dropped because the plugin did not read its `stdin` fast enough.
    #[serde(default)]
    pub input_dropped: u64,
    /// Why the input is not flowing (VFO missing, device claimed, daemon unreachable).
    #[serde(default)]
    pub input_error: Option<String>,
    /// Pseudo VFO id the instance's audio is published under (plugins with `[output.audio]`),
    /// subscribable on the audio WebSocket like a VFO.
    #[serde(default)]
    pub audio_vfo: Option<u32>,
    /// Unix milliseconds of the last state change.
    pub updated_unix_ms: u64,
}

/// An installed plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginInfo {
    /// The validated manifest.
    pub manifest: Manifest,
}

/// `install`: copy a plugin directory from a path on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    /// Contract version.
    pub v: u32,
    /// Directory containing `plugin.toml`, on the host running the plugin host.
    pub path: String,
}

/// `start`: create an instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    /// Contract version.
    pub v: u32,
    /// Plugin to run.
    pub plugin_id: String,
    /// Device to bind to.
    #[serde(default)]
    pub device: Option<String>,
    /// VFO of the device to take audio or IQ from (with `device`; absent for exclusive plugins,
    /// which get the whole window).
    #[serde(default)]
    pub vfo: Option<u32>,
    /// Frequency (Hz) to bind to; for exclusive plugins the centre frequency of the claim.
    #[serde(default)]
    pub freq_hz: Option<u64>,
    /// Plugin specific configuration.
    #[serde(default)]
    pub config: Value,
}

/// `remove`: delete an installed plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveRequest {
    /// Contract version.
    pub v: u32,
    /// Plugin id.
    pub plugin_id: String,
}

/// Reply to `remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemovedReply {
    /// Contract version.
    pub v: u32,
    /// Always true.
    pub ok: bool,
    /// Plugin id.
    pub plugin_id: String,
    /// Versions that were deleted.
    pub versions: Vec<String>,
}

/// `get`, `stop`, `restart`: address one instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstanceRef {
    /// Contract version.
    pub v: u32,
    /// Instance id.
    pub id: String,
}

/// `control`: send a JSON message to a framed instance (plugin defined, for example a service
/// selection). The plugin receives it as a `control` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    /// Contract version.
    pub v: u32,
    /// Instance id.
    pub id: String,
    /// The message (a JSON object, at most 64 KiB serialised).
    pub message: Value,
}

/// `media`: address one stored object of an instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaRequest {
    /// Contract version.
    pub v: u32,
    /// Instance id.
    pub id: String,
    /// Object key (`MediaHeader::valid_key`).
    pub key: String,
}

/// Reply to `control`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OkReply {
    /// Contract version.
    pub v: u32,
    /// Always true.
    pub ok: bool,
}

/// Reply to `status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatusReply {
    /// Contract version.
    pub v: u32,
    /// Always true.
    pub ok: bool,
    /// Plugin the instance runs.
    pub plugin_id: String,
    /// Device the instance is bound to.
    #[serde(default)]
    pub device: Option<String>,
    /// Lifecycle state.
    pub state: InstanceState,
    /// Centre frequency (Hz) the instance is on now.
    #[serde(default)]
    pub freq_hz: Option<u64>,
    /// Pseudo VFO id of the instance's audio (see [`InstanceInfo::audio_vfo`]).
    #[serde(default)]
    pub audio_vfo: Option<u32>,
    /// The latest document the plugin sent, `null` before the first one.
    pub status: Value,
    /// Unix milliseconds at which the host received it (0 before the first one).
    pub updated_unix_ms: u64,
    /// Receiver time of the object per key, so a client can tell whether a slide changed.
    #[serde(default)]
    pub media: Vec<MediaInfo>,
}

/// One stored media object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaInfo {
    /// Object key.
    pub key: String,
    /// MIME type.
    pub mime: String,
    /// Size in bytes.
    pub size: usize,
    /// Counts the replacements of this key, starting at 1.
    pub revision: u64,
}

/// Header of a successful `media` reply. The reply payload is this JSON on one line, a newline
/// and the object bytes; a failure is a plain [`ErrorReply`] without newline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaReplyHeader {
    /// Contract version.
    pub v: u32,
    /// Always true.
    pub ok: bool,
    /// Object key.
    pub key: String,
    /// MIME type.
    pub mime: String,
    /// Revision of the key.
    pub revision: u64,
}

impl MediaReplyHeader {
    /// The reply payload for `bytes`.
    #[must_use]
    pub fn encode(&self, bytes: &[u8]) -> Vec<u8> {
        let mut out = serde_json::to_vec(self).unwrap_or_default();
        out.push(b'\n');
        out.extend_from_slice(bytes);
        out
    }

    /// Split a successful reply payload; `None` for a failure reply or garbage.
    #[must_use]
    pub fn decode(payload: &[u8]) -> Option<(Self, &[u8])> {
        let split = payload.iter().position(|&b| b == b'\n')?;
        let header: Self = serde_json::from_slice(&payload[..split]).ok()?;
        header.ok.then_some((header, &payload[split + 1..]))
    }
}

/// Machine-readable error classes.
pub mod code {
    /// Malformed request or manifest.
    pub const INVALID: &str = "invalid";
    /// Unknown plugin or instance.
    pub const NOT_FOUND: &str = "not_found";
    /// Refused because of current state (for example an exclusive plugin is already running).
    pub const CONFLICT: &str = "conflict";
    /// Valid but not supported by this host (container plugins, UI plugins).
    pub const UNSUPPORTED: &str = "unsupported";
    /// Host failure.
    pub const INTERNAL: &str = "internal";
}

/// Failure reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorReply {
    /// Contract version.
    pub v: u32,
    /// Always false.
    pub ok: bool,
    /// One of [`code`].
    pub code: String,
    /// Human readable explanation.
    pub error: String,
}

impl ErrorReply {
    /// Build a failure reply.
    #[must_use]
    pub fn new(code: &str, error: impl Into<String>) -> Self {
        Self {
            v: crate::PROTOCOL_VERSION,
            ok: false,
            code: code.to_owned(),
            error: error.into(),
        }
    }
}

/// Reply to `list` and `install`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginsReply {
    /// Contract version.
    pub v: u32,
    /// Always true.
    pub ok: bool,
    /// Installed plugins.
    pub plugins: Vec<PluginInfo>,
}

/// Reply to `install`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginReply {
    /// Contract version.
    pub v: u32,
    /// Always true.
    pub ok: bool,
    /// The installed plugin.
    pub plugin: PluginInfo,
}

/// Reply to `instances`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InstancesReply {
    /// Contract version.
    pub v: u32,
    /// Always true.
    pub ok: bool,
    /// All instances.
    pub instances: Vec<InstanceInfo>,
}

/// Reply to `get`, `start`, `stop` (final snapshot) and `restart`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InstanceReply {
    /// Contract version.
    pub v: u32,
    /// Always true.
    pub ok: bool,
    /// The instance.
    pub instance: InstanceInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_audio_ids_are_reserved_and_stable() {
        let a = audio_vfo_of("i-ab12cd34");
        assert!(is_plugin_audio_vfo(a));
        assert_eq!(a, audio_vfo_of("i-ab12cd34"));
        assert_ne!(a, audio_vfo_of("i-ab12cd35"));
        assert!(!is_plugin_audio_vfo(64));
    }

    #[test]
    fn media_reply_round_trips_and_rejects_errors() {
        let head = MediaReplyHeader {
            v: 1,
            ok: true,
            key: "slide.x".into(),
            mime: "image/png".into(),
            revision: 3,
        };
        let payload = head.encode(&[0, 10, 255]);
        let (got, bytes) = MediaReplyHeader::decode(&payload).expect("decode");
        assert_eq!((got, bytes), (head, &[0u8, 10, 255][..]));
        let err = serde_json::to_vec(&ErrorReply::new(code::NOT_FOUND, "no")).expect("json");
        assert!(MediaReplyHeader::decode(&err).is_none());
    }
}
