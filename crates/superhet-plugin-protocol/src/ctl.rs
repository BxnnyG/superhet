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
    /// Replace the configuration of an instance and restart its process with it (same id,
    /// counters and binding are kept).
    pub const RECONFIGURE: &str = "reconfigure";
    /// Remove an installed plugin (all versions); refused while instances of it exist.
    pub const REMOVE: &str = "remove";
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
    /// Unix milliseconds of the last state change.
    pub updated_unix_ms: u64,
    /// Sample bytes delivered to the plugin's `stdin` since the instance was created (survives
    /// process restarts); `0` while nothing flowed.
    #[serde(default)]
    pub input_bytes: u64,
    /// Smoothed RMS level of the input in dBFS (`None` until the first chunk arrived).
    #[serde(default)]
    pub input_level_db: Option<f32>,
    /// Unix milliseconds of the last input chunk.
    #[serde(default)]
    pub last_input_unix_ms: Option<u64>,
    /// Unix milliseconds of the last valid event.
    #[serde(default)]
    pub last_event_unix_ms: Option<u64>,
    /// Unix milliseconds the instance was created (not reset by restarts or reconfigures).
    #[serde(default)]
    pub created_unix_ms: u64,
    /// How often the configuration was replaced with `reconfigure`.
    #[serde(default)]
    pub reconfigures: u32,
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

/// `reconfigure`: new configuration for an instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconfigureRequest {
    /// Contract version.
    pub v: u32,
    /// Instance id.
    pub id: String,
    /// The complete new configuration (object or `null` for the plugin's defaults).
    #[serde(default)]
    pub config: Value,
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
