//! Instance supervision: spawn, watch, restart with backoff, give up after repeated crashes.
//!
//! One tokio task per instance runs the loop *start -> run -> (crash -> backoff)*. A process that
//! exits on its own, is killed from outside or stays silent past the manifest's heartbeat
//! timeout counts as a crash. Crashes are remembered for [`Policy::window`]; reaching
//! [`Policy::max_crashes`] inside the window puts the instance into
//! [`InstanceState::Error`], where it stays until `restart`. Both transports run (the framed one in
//! `framed_session`); container and UI plugins are refused with [`StartError::Unsupported`].
//!
//! Input: an instance bound to a device and a VFO (or an exclusive plugin bound to a device) gets
//! its samples through an [`Inputs`] provider, which the supervisor attaches for every process
//! start and drops when the process ends; the bytes are written to the plugin's `stdin`. When
//! the process ends, its whole process group is killed, so helpers the plugin started cannot
//! outlive it.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;
use superhet_bus::tap::{ClaimAcquire, TapFormat, TapKind, TapOpen};
use superhet_plugin_protocol::ctl::{InstanceInfo, InstanceState, MediaInfo, audio_vfo_of};
use superhet_plugin_protocol::event::{HostEvent, Line, MAX_EVENT_LINE};
use superhet_plugin_protocol::manifest::{
    InputKind, PluginType, RuntimeKind, SampleFormat, Transport,
};
use superhet_plugin_protocol::{InstanceEnv, PROTOCOL_VERSION, Package};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, mpsc, watch};

use crate::framed_session;
use crate::outputs::{NoOutputs, Outputs};

/// Environment variables passed through to plugins; everything else (NATS and database
/// credentials in particular) is withheld.
const PASSTHROUGH_ENV: &[&str] = &["PATH", "LANG", "LC_ALL", "TZ", "TMPDIR"];

/// Restart and shutdown behaviour.
#[derive(Debug, Clone)]
pub struct Policy {
    /// This many crashes inside [`Self::window`] put the instance into `error`.
    pub max_crashes: u32,
    /// Sliding window the crashes are counted in.
    pub window: Duration,
    /// Delay after the first crash; doubles with every further crash in the window.
    pub backoff_base: Duration,
    /// Upper bound of the delay.
    pub backoff_max: Duration,
    /// Time a plugin gets to exit after its stdin was closed before it is killed.
    pub stop_grace: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_crashes: 5,
            window: Duration::from_secs(5 * 60),
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            stop_grace: Duration::from_secs(5),
        }
    }
}

impl Policy {
    /// Delay before the restart that follows the `crashes`-th crash inside the window.
    #[must_use]
    pub fn backoff(&self, crashes: u32) -> Duration {
        let shift = crashes.saturating_sub(1).min(16);
        self.backoff_base
            .saturating_mul(1u32 << shift)
            .min(self.backoff_max)
    }
}

/// Receiver of validated plugin events. Must not block.
pub trait EventSink: Send + Sync + 'static {
    /// Called for every valid event of every instance.
    fn emit(&self, event: HostEvent);
}

/// Discards events.
#[derive(Debug, Default)]
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: HostEvent) {}
}

/// What an instance needs from the device daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRequest {
    /// Device the daemon serves.
    pub device: String,
    /// The tap to open (its id is the instance id).
    pub tap: TapOpen,
    /// The exclusive claim to hold while the tap is open.
    pub claim: Option<ClaimAcquire>,
}

/// Counters and status an [`Inputs`] provider reports about one attachment.
#[derive(Debug, Clone)]
pub struct InputHealth {
    /// Chunks dropped because the plugin did not read fast enough.
    pub dropped: Arc<AtomicU64>,
    /// Why the input is not flowing; `None` while it is.
    pub status: watch::Sender<Option<String>>,
}

/// A live input: chunks of sample bytes for `stdin`. Dropping the guard releases the tap and the
/// claim.
pub struct Attachment {
    /// Sample bytes in order.
    pub chunks: mpsc::Receiver<Vec<u8>>,
    /// Keeps the tap (and claim) alive.
    pub guard: Box<dyn Send>,
    /// Moves the centre frequency of an exclusive claim (the new value is applied at once and
    /// kept in every lease refresh); `None` when the provider cannot retune.
    pub retune: Option<watch::Sender<u64>>,
}

/// Source of plugin input (the bus in production). `attach` must not block.
pub trait Inputs: Send + Sync + 'static {
    /// Start delivering the input of `request`; `None` when this provider cannot.
    fn attach(&self, request: &InputRequest, health: InputHealth) -> Option<Attachment>;
}

/// No input source: instances keep their `stdin` open and idle.
#[derive(Debug, Default)]
pub struct NoInputs;

impl Inputs for NoInputs {
    fn attach(&self, _request: &InputRequest, _health: InputHealth) -> Option<Attachment> {
        None
    }
}

/// Largest `control` message (serialised bytes).
const MAX_CONTROL: usize = 64 * 1024;

/// What `status` returns.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusSnapshot {
    /// The plugin's latest status document, `null` before the first.
    pub status: Value,
    /// Unix milliseconds at which it arrived (0 before the first).
    pub updated_unix_ms: u64,
    /// Stored media objects.
    pub media: Vec<MediaInfo>,
}

/// Parameters of a new instance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StartSpec {
    /// Device to bind to.
    pub device: Option<String>,
    /// VFO of the device that feeds the instance.
    pub vfo: Option<u32>,
    /// Frequency (Hz) to bind to; the centre of the claim for exclusive plugins.
    pub freq_hz: Option<u64>,
    /// Plugin specific configuration.
    pub config: Value,
}

/// An instance cannot be created or restarted.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// Valid, but not something this host can run yet.
    #[error("{0}")]
    Unsupported(String),
    /// Refused because of the current state.
    #[error("{0}")]
    Conflict(String),
    /// The binding does not fit the plugin.
    #[error("{0}")]
    Invalid(String),
    /// Unknown instance.
    #[error("no such instance")]
    NotFound,
}

pub(crate) struct Shared {
    pub(crate) id: String,
    pub(crate) package: Arc<Package>,
    pub(crate) spec: StartSpec,
    pub(crate) input: Option<InputRequest>,
    pub(crate) input_dropped: Arc<AtomicU64>,
    info: Mutex<Mutable>,
    live: Mutex<Live>,
    pub(crate) events_ok: AtomicU64,
    pub(crate) events_invalid: AtomicU64,
    restarts: AtomicU32,
    tx: broadcast::Sender<InstanceInfo>,
}

/// What a framed plugin has told the host (cleared when its session ends).
#[derive(Default)]
pub(crate) struct Live {
    /// Encoded `control` payloads for the running session.
    pub(crate) control: Option<mpsc::Sender<Vec<u8>>>,
    /// Latest `status` document and the Unix ms it arrived.
    pub(crate) status: Option<(serde_json::Value, u64)>,
    /// Latest `media` objects by key.
    pub(crate) media: HashMap<String, MediaObject>,
    /// Centre frequency after a `tune`, if the plugin moved its window.
    pub(crate) freq_hz: Option<u64>,
}

/// A stored `media` object.
#[derive(Clone)]
pub(crate) struct MediaObject {
    pub(crate) mime: String,
    pub(crate) bytes: Arc<Vec<u8>>,
    pub(crate) revision: u64,
}

pub(crate) struct Mutable {
    state: InstanceState,
    pid: Option<u32>,
    crashes_in_window: u32,
    retry_in: Option<Duration>,
    last_error: Option<String>,
    pub(crate) input_error: Option<String>,
    updated_unix_ms: u64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Mutable> {
        self.info
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn live(&self) -> MutexGuard<'_, Live> {
        self.live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The frequency the instance is on now (after a `tune`, else the start binding).
    pub(crate) fn freq_hz(&self) -> Option<u64> {
        self.live().freq_hz.or(self.spec.freq_hz)
    }

    /// Record a new centre frequency and announce it.
    pub(crate) fn set_freq(&self, freq_hz: u64) {
        self.live().freq_hz = Some(freq_hz);
        self.update(|_| {});
    }

    /// Record an input problem (or its end) for the instance state.
    pub(crate) fn set_input_error(&self, error: Option<String>) {
        self.update(|m| m.input_error = error);
    }

    fn snapshot(&self) -> InstanceInfo {
        let m = self.lock();
        InstanceInfo {
            id: self.id.clone(),
            plugin_id: self.package.manifest.plugin.id.clone(),
            device: self.spec.device.clone(),
            vfo: self.spec.vfo,
            freq_hz: self.freq_hz(),
            config: self.spec.config.clone(),
            exclusive: self.package.manifest.is_exclusive(),
            state: m.state,
            pid: m.pid,
            restarts: self.restarts.load(Ordering::Relaxed),
            crashes_in_window: m.crashes_in_window,
            retry_in_ms: m
                .retry_in
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            last_error: m.last_error.clone(),
            events_ok: self.events_ok.load(Ordering::Relaxed),
            events_invalid: self.events_invalid.load(Ordering::Relaxed),
            input_dropped: self.input_dropped.load(Ordering::Relaxed),
            input_error: m.input_error.clone(),
            audio_vfo: (self.package.manifest.output.audio.is_some()
                && self.package.manifest.runtime.transport == Transport::Framed)
                .then(|| audio_vfo_of(&self.id)),
            updated_unix_ms: m.updated_unix_ms,
        }
    }

    /// Change the mutable part and broadcast the new snapshot.
    pub(crate) fn update(&self, change: impl FnOnce(&mut Mutable)) {
        {
            let mut m = self.lock();
            change(&mut m);
            m.updated_unix_ms = unix_ms();
        }
        // No subscriber is fine.
        let _ = self.tx.send(self.snapshot());
    }
}

struct Entry {
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

struct Inner {
    policy: Policy,
    sink: Arc<dyn EventSink>,
    inputs: Arc<dyn Inputs>,
    outputs: Arc<dyn Outputs>,
    tx: broadcast::Sender<InstanceInfo>,
    instances: Mutex<HashMap<String, Entry>>,
}

/// Supervises plugin instances.
#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Inner>,
}

impl Supervisor {
    /// Create a supervisor.
    #[must_use]
    pub fn new(policy: Policy, sink: Arc<dyn EventSink>) -> Self {
        Self::with_inputs(policy, sink, Arc::new(NoInputs))
    }

    /// Create a supervisor that feeds instances from `inputs`.
    #[must_use]
    pub fn with_inputs(policy: Policy, sink: Arc<dyn EventSink>, inputs: Arc<dyn Inputs>) -> Self {
        Self::with_io(policy, sink, inputs, Arc::new(NoOutputs))
    }

    /// Create a supervisor that feeds instances from `inputs` and sends their audio to
    /// `outputs`.
    #[must_use]
    pub fn with_io(
        policy: Policy,
        sink: Arc<dyn EventSink>,
        inputs: Arc<dyn Inputs>,
        outputs: Arc<dyn Outputs>,
    ) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                policy,
                sink,
                inputs,
                outputs,
                tx,
                instances: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// State changes of all instances (lagging receivers lose old entries).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<InstanceInfo> {
        self.inner.tx.subscribe()
    }

    fn instances(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.inner
            .instances
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Create an instance and start it.
    ///
    /// # Errors
    /// [`StartError::Unsupported`] for plugins this host cannot run, [`StartError::Conflict`]
    /// when an exclusive claim on the same device is already held.
    pub fn start(
        &self,
        package: Arc<Package>,
        spec: StartSpec,
    ) -> Result<InstanceInfo, StartError> {
        self.start_as(None, package, spec)
    }

    /// Like [`Self::start`], with a given instance id (restoring a stored instance).
    ///
    /// # Errors
    /// As [`Self::start`]; [`StartError::Conflict`] also when `id` is taken, and
    /// [`StartError::Invalid`] when the binding does not fit the plugin.
    pub fn start_as(
        &self,
        id: Option<String>,
        package: Arc<Package>,
        spec: StartSpec,
    ) -> Result<InstanceInfo, StartError> {
        check_runnable(&package)?;
        let id =
            id.unwrap_or_else(|| format!("i-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        let input = input_request(&package, &id, &spec)?;
        let mut instances = self.instances();
        if instances.contains_key(&id) {
            return Err(StartError::Conflict(format!(
                "instance {id} already exists"
            )));
        }
        if package.manifest.is_exclusive()
            && let Some(other) = instances.values().find(|e| {
                e.shared.package.manifest.is_exclusive()
                    && e.shared.spec.device == spec.device
                    && e.shared.snapshot().state != InstanceState::Error
            })
        {
            return Err(StartError::Conflict(format!(
                "device is already claimed by instance {}",
                other.shared.id
            )));
        }
        let shared = Arc::new(Shared {
            id: id.clone(),
            package,
            spec,
            input,
            input_dropped: Arc::new(AtomicU64::new(0)),
            live: Mutex::new(Live::default()),
            info: Mutex::new(Mutable {
                state: InstanceState::Starting,
                pid: None,
                crashes_in_window: 0,
                retry_in: None,
                last_error: None,
                input_error: None,
                updated_unix_ms: unix_ms(),
            }),
            events_ok: AtomicU64::new(0),
            events_invalid: AtomicU64::new(0),
            restarts: AtomicU32::new(0),
            tx: self.inner.tx.clone(),
        });
        let info = shared.snapshot();
        let _ = shared.tx.send(info.clone());
        let entry = self.spawn_task(shared);
        instances.insert(id, entry);
        Ok(info)
    }

    fn spawn_task(&self, shared: Arc<Shared>) -> Entry {
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(run_instance(
            self.inner.policy.clone(),
            Arc::clone(&self.inner.sink),
            Arc::clone(&self.inner.inputs),
            Arc::clone(&self.inner.outputs),
            Arc::clone(&shared),
            stop_rx,
        ));
        Entry {
            shared,
            stop,
            task: Some(task),
        }
    }

    /// All instances.
    #[must_use]
    pub fn list(&self) -> Vec<InstanceInfo> {
        let mut all: Vec<_> = self
            .instances()
            .values()
            .map(|e| e.shared.snapshot())
            .collect();
        all.sort_by(|a, b| a.id.cmp(&b.id));
        all
    }

    /// One instance.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<InstanceInfo> {
        self.instances().get(id).map(|e| e.shared.snapshot())
    }

    /// Stop an instance (stdin closed, killed after the grace period) and forget it. Returns
    /// the final snapshot (`stopped`).
    pub async fn stop(&self, id: &str) -> Option<InstanceInfo> {
        let mut entry = self.instances().remove(id)?;
        let _ = entry.stop.send(true);
        if let Some(task) = entry.task.take() {
            let _ = task.await;
        }
        let mut info = entry.shared.snapshot();
        // An instance that was in `error` has no task to publish the final state.
        if info.state != InstanceState::Stopped {
            entry.shared.update(|m| {
                m.state = InstanceState::Stopped;
                m.pid = None;
                m.retry_in = None;
            });
            info = entry.shared.snapshot();
        }
        Some(info)
    }

    /// Start an instance in state `error` again with a clean crash history.
    ///
    /// # Errors
    /// [`StartError::NotFound`] for unknown ids, [`StartError::Conflict`] for instances that
    /// are not in `error`.
    pub fn restart(&self, id: &str) -> Result<InstanceInfo, StartError> {
        let mut instances = self.instances();
        let entry = instances.get_mut(id).ok_or(StartError::NotFound)?;
        if entry.shared.snapshot().state != InstanceState::Error {
            return Err(StartError::Conflict(
                "only instances in state error can be restarted".into(),
            ));
        }
        let shared = Arc::clone(&entry.shared);
        shared.update(|m| {
            m.state = InstanceState::Starting;
            m.crashes_in_window = 0;
            m.retry_in = None;
        });
        let fresh = self.spawn_task(shared);
        let info = fresh.shared.snapshot();
        *entry = fresh;
        Ok(info)
    }

    /// Send a JSON message to the framed session of an instance (a `control` frame).
    ///
    /// # Errors
    /// [`StartError::NotFound`] for unknown ids, [`StartError::Invalid`] when the message is not
    /// an object or too large, [`StartError::Conflict`] while the plugin has no session (not
    /// started yet, crashed) or does not read its control queue.
    pub fn control(&self, id: &str, message: &Value) -> Result<(), StartError> {
        if !message.is_object() {
            return Err(StartError::Invalid("message must be a JSON object".into()));
        }
        let payload =
            serde_json::to_vec(message).map_err(|err| StartError::Invalid(err.to_string()))?;
        if payload.len() > MAX_CONTROL {
            return Err(StartError::Invalid("message is too large".into()));
        }
        let shared = self
            .instances()
            .get(id)
            .map(|e| Arc::clone(&e.shared))
            .ok_or(StartError::NotFound)?;
        let sender = shared.live().control.clone();
        let sender = sender.ok_or_else(|| {
            StartError::Conflict("the plugin has no running framed session".into())
        })?;
        sender
            .try_send(payload)
            .map_err(|_| StartError::Conflict("the plugin does not take messages now".into()))
    }

    /// The latest `status` document of an instance and the stored media objects.
    #[must_use]
    pub fn status(&self, id: &str) -> Option<StatusSnapshot> {
        let shared = self.instances().get(id).map(|e| Arc::clone(&e.shared))?;
        let live = shared.live();
        let (status, updated_unix_ms) = live.status.clone().unwrap_or((Value::Null, 0));
        let mut media: Vec<MediaInfo> = live
            .media
            .iter()
            .map(|(key, m)| MediaInfo {
                key: key.clone(),
                mime: m.mime.clone(),
                size: m.bytes.len(),
                revision: m.revision,
            })
            .collect();
        media.sort_by(|a, b| a.key.cmp(&b.key));
        Some(StatusSnapshot {
            status,
            updated_unix_ms,
            media,
        })
    }

    /// One stored media object: MIME type, revision and bytes.
    #[must_use]
    pub fn media(&self, id: &str, key: &str) -> Option<(String, u64, Arc<Vec<u8>>)> {
        let shared = self.instances().get(id).map(|e| Arc::clone(&e.shared))?;
        let live = shared.live();
        let object = live.media.get(key)?;
        Some((
            object.mime.clone(),
            object.revision,
            Arc::clone(&object.bytes),
        ))
    }

    /// Stop every instance (host shutdown).
    pub async fn shutdown(&self) {
        let ids: Vec<String> = self.instances().keys().cloned().collect();
        for id in ids {
            let _ = self.stop(&id).await;
        }
    }
}

fn check_runnable(package: &Package) -> Result<(), StartError> {
    let manifest = &package.manifest;
    if manifest.plugin.kind == PluginType::Ui {
        return Err(StartError::Unsupported(
            "ui plugins have no process; the frontend loads them".into(),
        ));
    }
    if manifest.runtime.kind == RuntimeKind::Container {
        return Err(StartError::Unsupported(
            "container plugins are started through compose.plugins.yaml, not by the host".into(),
        ));
    }
    Ok(())
}

/// What the daemon has to deliver for this instance, if anything.
fn input_request(
    package: &Package,
    id: &str,
    spec: &StartSpec,
) -> Result<Option<InputRequest>, StartError> {
    let manifest = &package.manifest;
    let Some(input) = &manifest.input else {
        return Ok(None);
    };
    let format = match input.format {
        SampleFormat::Cf32 => TapFormat::Cf32,
        SampleFormat::Cs16 => TapFormat::Cs16,
        SampleFormat::Cu8 => TapFormat::Cu8,
        SampleFormat::S16le => TapFormat::S16le,
        SampleFormat::F32le => TapFormat::F32le,
    };
    let invalid = StartError::Invalid;
    let request = if manifest.is_exclusive() {
        let device = spec
            .device
            .clone()
            .ok_or_else(|| invalid("exclusive plugins need a device".into()))?;
        let dev = &manifest.device;
        let center = match (dev.center_hz, spec.freq_hz) {
            (Some(fixed), _) => fixed,
            (None, Some(chosen)) => chosen,
            (None, None) => {
                return Err(invalid("freq_hz (the centre frequency) is required".into()));
            }
        };
        if let (Some(min), Some(max)) = (dev.center_hz_min, dev.center_hz_max)
            && !(min..=max).contains(&center)
        {
            return Err(invalid(format!("freq_hz must be within {min}..={max}")));
        }
        let rate = dev.sample_rate.unwrap_or(input.sample_rate);
        if rate != input.sample_rate {
            return Err(invalid(
                "exclusive plugins receive the window at device.sample_rate; input.sample_rate must match"
                    .into(),
            ));
        }
        InputRequest {
            device,
            tap: TapOpen {
                v: superhet_bus::tap::VERSION,
                id: id.to_owned(),
                kind: TapKind::Window,
                vfo: None,
                rate,
                format,
                center_hz: Some(center),
            },
            claim: Some(ClaimAcquire {
                v: superhet_bus::tap::VERSION,
                id: id.to_owned(),
                center_hz: center,
                sample_rate: Some(rate),
            }),
        }
    } else {
        let (device, vfo) = match (&spec.device, spec.vfo) {
            // Bound for information only (environment, event metadata); nothing to feed.
            (_, None) => return Ok(None),
            (Some(device), Some(vfo)) => (device.clone(), vfo),
            (None, Some(_)) => return Err(invalid("a vfo needs a device".into())),
        };
        let kind = match input.kind {
            InputKind::Iq => TapKind::Iq,
            // The VFO's demodulated audio; a discriminator plugin uses an NFM VFO.
            InputKind::Discriminator | InputKind::Audio => TapKind::Audio,
            InputKind::VoiceFrames => {
                return Err(StartError::Unsupported(
                    "voice-frame input is not available yet".into(),
                ));
            }
        };
        InputRequest {
            device,
            tap: TapOpen {
                v: superhet_bus::tap::VERSION,
                id: id.to_owned(),
                kind,
                vfo: Some(vfo),
                rate: input.sample_rate,
                format,
                center_hz: None,
            },
            claim: None,
        }
    };
    request
        .tap
        .validate()
        .map_err(|err| StartError::Invalid(err.to_string()))?;
    superhet_bus::subjects::tap(&request.device, &request.tap.id)
        .map_err(|err| StartError::Invalid(err.to_string()))?;
    Ok(Some(request))
}

async fn run_instance(
    policy: Policy,
    sink: Arc<dyn EventSink>,
    inputs: Arc<dyn Inputs>,
    outputs: Arc<dyn Outputs>,
    shared: Arc<Shared>,
    mut stop: watch::Receiver<bool>,
) {
    let mut crashes: VecDeque<Instant> = VecDeque::new();
    loop {
        shared.update(|m| {
            m.state = InstanceState::Starting;
            m.pid = None;
            m.retry_in = None;
        });
        let ctx = Context {
            policy: &policy,
            sink: &sink,
            inputs: &inputs,
            outputs: &outputs,
            shared: &shared,
        };
        let outcome = match start_process(&shared) {
            Ok((child, socket)) => run_process(child, socket, &ctx, &mut stop).await,
            Err(err) => Outcome::Crashed(format!("spawn failed: {err}")),
        };
        let reason = match outcome {
            Outcome::Stopped => {
                shared.update(|m| {
                    m.state = InstanceState::Stopped;
                    m.pid = None;
                });
                return;
            }
            Outcome::Crashed(reason) => reason,
        };
        let now = Instant::now();
        crashes.push_back(now);
        while crashes
            .front()
            .is_some_and(|t| now.duration_since(*t) > policy.window)
        {
            crashes.pop_front();
        }
        let count = u32::try_from(crashes.len()).unwrap_or(u32::MAX);
        tracing::warn!(
            instance = %shared.id,
            plugin = %shared.package.manifest.plugin.id,
            crashes = count,
            %reason,
            "plugin instance crashed"
        );
        if count >= policy.max_crashes {
            shared.update(|m| {
                m.state = InstanceState::Error;
                m.pid = None;
                m.crashes_in_window = count;
                m.last_error = Some(format!(
                    "{reason}; {count} crashes within {} s, giving up",
                    policy.window.as_secs()
                ));
            });
            return;
        }
        let delay = policy.backoff(count);
        shared.update(|m| {
            m.state = InstanceState::Backoff;
            m.pid = None;
            m.crashes_in_window = count;
            m.retry_in = Some(delay);
            m.last_error = Some(reason);
        });
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            _ = stop.changed() => {
                shared.update(|m| {
                    m.state = InstanceState::Stopped;
                    m.retry_in = None;
                });
                return;
            }
        }
        shared.restarts.fetch_add(1, Ordering::Relaxed);
    }
}

enum Outcome {
    Stopped,
    Crashed(String),
}

/// What one process run needs besides the process.
struct Context<'a> {
    policy: &'a Policy,
    sink: &'a Arc<dyn EventSink>,
    inputs: &'a Arc<dyn Inputs>,
    outputs: &'a Arc<dyn Outputs>,
    shared: &'a Arc<Shared>,
}

/// The listening socket of a framed plugin; its directory is removed on drop.
struct SocketDir {
    dir: PathBuf,
    listener: Option<UnixListener>,
}

impl SocketDir {
    fn path(&self) -> PathBuf {
        self.dir.join("plugin.sock")
    }
}

impl Drop for SocketDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Create the socket (framed plugins) and spawn the process.
fn start_process(shared: &Shared) -> std::io::Result<(Child, Option<SocketDir>)> {
    if shared.package.manifest.runtime.transport != Transport::Framed {
        return spawn(shared, None).map(|child| (child, None));
    }
    use std::os::unix::fs::DirBuilderExt;
    let dir = std::env::temp_dir().join(format!(
        "superhet-plugin-{}-{}",
        shared.id,
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let mut socket = SocketDir {
        dir,
        listener: None,
    };
    socket.listener = Some(UnixListener::bind(socket.path())?);
    let child = spawn(shared, Some(&socket.path()))?;
    Ok((child, Some(socket)))
}

fn spawn(shared: &Shared, socket: Option<&std::path::Path>) -> std::io::Result<Child> {
    let package = &shared.package;
    let (program, args) = package
        .command()
        .ok_or_else(|| std::io::Error::other("manifest has no runtime.command"))?;
    let env = InstanceEnv {
        plugin_id: &package.manifest.plugin.id,
        instance_id: &shared.id,
        plugin_dir: &package.dir,
        device: shared.spec.device.as_deref(),
        vfo: shared.spec.vfo,
        freq_hz: shared.freq_hz(),
        config: &shared.spec.config,
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&package.dir)
        .env_clear()
        .envs(
            PASSTHROUGH_ENV
                .iter()
                .filter_map(|name| std::env::var(name).ok().map(|value| (*name, value))),
        )
        .envs(env.vars())
        .envs(socket.map(|path| (superhet_plugin_protocol::env::SOCKET, path.as_os_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Own process group: a Ctrl-C at the host's terminal must not reach the plugins; the
        // host stops them in an orderly way.
        .process_group(0);
    command.spawn()
}

/// Kill every process in the group `pid` leads (the plugin and whatever it started).
#[allow(unsafe_code)]
fn kill_group(pid: u32) {
    // Never signal group 0/1 (our own group, init) because of a bad pid.
    let Ok(pid) = i32::try_from(pid) else { return };
    if pid <= 1 {
        return;
    }
    // SAFETY: kill(2) with a negative pid only sends a signal to a process group; it touches
    // no memory. The plugin was spawned as the leader of its own group (`process_group(0)`).
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

/// What holds the plugin's `stdin`: the input feeder, or just the open pipe.
enum Stdin {
    /// Held open and idle until the host wants the plugin to end.
    Idle(Option<ChildStdin>),
    /// The feeder owns the pipe and the attachment; aborting it closes both.
    Feeding(tokio::task::JoinHandle<()>),
}

impl Stdin {
    /// Close the pipe (and release the input) and wait until that has happened.
    async fn close(self) {
        match self {
            Self::Idle(stdin) => drop(stdin),
            Self::Feeding(task) => {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

/// Write the attachment's chunks to the plugin until either side ends.
async fn feed_stdin(mut stdin: ChildStdin, mut attachment: Attachment) {
    while let Some(chunk) = attachment.chunks.recv().await {
        if stdin.write_all(&chunk).await.is_err() {
            break;
        }
    }
    // The plugin closed its stdin or the input ended; keep the pipe (and the attachment) until
    // the process is stopped, so the plugin is not told to end by accident.
    std::future::pending::<()>().await;
}

/// Attach the instance's input to `stdin`, if it has one and a provider takes it.
fn open_stdin(stdin: Option<ChildStdin>, inputs: &Arc<dyn Inputs>, shared: &Arc<Shared>) -> Stdin {
    let Some(request) = shared.input.as_ref() else {
        return Stdin::Idle(stdin);
    };
    let Some(pipe) = stdin else {
        return Stdin::Idle(None);
    };
    let (status, mut state) = watch::channel(None);
    let health = InputHealth {
        dropped: Arc::clone(&shared.input_dropped),
        status,
    };
    let Some(attachment) = inputs.attach(request, health) else {
        return Stdin::Idle(Some(pipe));
    };
    let shared = Arc::clone(shared);
    Stdin::Feeding(tokio::spawn(async move {
        let watcher = {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                while state.changed().await.is_ok() {
                    let error = state.borrow_and_update().clone();
                    shared.update(|m| m.input_error = error);
                }
            })
        };
        feed_stdin(pipe, attachment).await;
        watcher.abort();
    }))
}

/// Wait for the framed session to end; never completes without one.
async fn session_ended(session: &mut Option<tokio::task::JoinHandle<String>>) -> String {
    match session.as_mut() {
        Some(handle) => {
            let reason = handle
                .await
                .unwrap_or_else(|err| format!("framed session task failed: {err}"));
            *session = None;
            reason
        }
        None => std::future::pending().await,
    }
}

async fn run_process(
    mut child: Child,
    mut socket: Option<SocketDir>,
    ctx: &Context<'_>,
    stop: &mut watch::Receiver<bool>,
) -> Outcome {
    let Context {
        policy,
        sink,
        inputs,
        outputs,
        shared,
    } = ctx;
    let pid = child.id();
    shared.update(|m| {
        m.state = InstanceState::Running;
        m.pid = pid;
        m.input_error = None;
    });
    let started = Instant::now();
    let activity = Arc::new(AtomicU64::new(0));
    // Held open so the plugin sees EOF only when the host wants it to end. A framed plugin gets
    // its samples through the socket instead.
    let framed = socket.is_some();
    let mut stdin = Some(if framed {
        Stdin::Idle(child.stdin.take())
    } else {
        open_stdin(child.stdin.take(), inputs, shared)
    });
    let mut session = socket
        .as_mut()
        .and_then(|socket| socket.listener.take())
        .map(|listener| {
            tokio::spawn(framed_session::run(
                listener,
                framed_session::Session {
                    shared: Arc::clone(shared),
                    sink: Arc::clone(sink),
                    inputs: Arc::clone(inputs),
                    outputs: Arc::clone(outputs),
                    activity: Arc::clone(&activity),
                    started,
                },
            ))
        });
    let readers = [
        child.stdout.take().map(|out| {
            tokio::spawn(read_events(
                out,
                Arc::clone(shared),
                Arc::clone(sink),
                Arc::clone(&activity),
                started,
            ))
        }),
        child
            .stderr
            .take()
            .map(|err| tokio::spawn(read_log(err, Arc::clone(shared)))),
    ];
    let timeout = shared
        .package
        .manifest
        .heartbeat_timeout_s()
        .map(|secs| Duration::from_secs(u64::from(secs)));
    let mut watchdog = tokio::time::interval(Duration::from_millis(250));

    let outcome = loop {
        tokio::select! {
            status = child.wait() => {
                break Outcome::Crashed(match status {
                    Ok(status) => format!("exited with {status}"),
                    Err(err) => format!("wait failed: {err}"),
                });
            }
            reason = session_ended(&mut session) => {
                let _ = child.kill().await;
                break Outcome::Crashed(reason);
            }
            _ = stop.changed() => {
                if let Some(stdin) = stdin.take() {
                    stdin.close().await;
                }
                if tokio::time::timeout(policy.stop_grace, child.wait()).await.is_err() {
                    let _ = child.kill().await;
                }
                break Outcome::Stopped;
            }
            _ = watchdog.tick(), if timeout.is_some() => {
                let quiet = started.elapsed().saturating_sub(Duration::from_millis(activity.load(Ordering::Relaxed)));
                if let Some(limit) = timeout && quiet > limit {
                    let _ = child.kill().await;
                    break Outcome::Crashed(format!("silent for {} s, killed", quiet.as_secs()));
                }
            }
        }
    };
    // Release the input and take down anything the plugin left running in its group.
    if let Some(stdin) = stdin.take() {
        stdin.close().await;
    }
    if let Some(session) = session.take() {
        session.abort();
        let _ = session.await;
    }
    drop(socket);
    if let Some(pid) = pid {
        kill_group(pid);
    }
    // Drain what the process wrote right before it ended.
    for reader in readers.into_iter().flatten() {
        let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
    }
    outcome
}

async fn read_events(
    out: impl AsyncRead + Unpin,
    shared: Arc<Shared>,
    sink: Arc<dyn EventSink>,
    activity: Arc<AtomicU64>,
    started: Instant,
) {
    let mut lines = LineReader::new(out);
    while let Some(line) = lines.next_line().await {
        activity.store(
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        let verdict = match line {
            Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => continue,
            Ok(bytes) => shared
                .package
                .validator
                .check_line(&bytes)
                .map_err(|err| err.to_string()),
            Err(()) => Err("line too long".to_owned()),
        };
        deliver(&shared, sink.as_ref(), verdict);
    }
}

/// Count and forward the verdict on one event line (from `stdout` or an `event` frame).
pub(crate) fn deliver(shared: &Shared, sink: &dyn EventSink, verdict: Result<Line, String>) {
    match verdict {
        Ok(Line::Heartbeat) => {}
        Ok(Line::Event(event)) => {
            shared.events_ok.fetch_add(1, Ordering::Relaxed);
            sink.emit(HostEvent {
                v: PROTOCOL_VERSION,
                plugin_id: shared.package.manifest.plugin.id.clone(),
                instance_id: shared.id.clone(),
                device: shared.spec.device.clone(),
                vfo: shared.spec.vfo,
                freq_hz: shared.freq_hz(),
                event_type: event.event_type,
                ts: event.ts,
                host_ts_ms: unix_ms(),
                data: event.data,
            });
        }
        Err(reason) => {
            let n = shared.events_invalid.fetch_add(1, Ordering::Relaxed) + 1;
            // Log the first few and then every thousandth so a noisy plugin cannot flood.
            if n <= 5 || n.is_multiple_of(1000) {
                tracing::warn!(
                    instance = %shared.id,
                    plugin = %shared.package.manifest.plugin.id,
                    invalid_total = n,
                    %reason,
                    "dropped invalid event line"
                );
            }
        }
    }
}

async fn read_log(err: impl AsyncRead + Unpin, shared: Arc<Shared>) {
    let mut lines = LineReader::new(err);
    while let Some(line) = lines.next_line().await {
        let text = match line {
            Ok(bytes) => String::from_utf8_lossy(&bytes).trim_end().to_owned(),
            Err(()) => "<log line too long>".to_owned(),
        };
        if !text.is_empty() {
            tracing::info!(
                instance = %shared.id,
                plugin = %shared.package.manifest.plugin.id,
                "{text}"
            );
        }
    }
}

/// Newline-separated records with a size cap; an oversized record yields `Err(())` once and is
/// skipped without buffering it.
struct LineReader<R> {
    inner: BufReader<R>,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner: BufReader::new(inner),
        }
    }

    async fn next_line(&mut self) -> Option<Result<Vec<u8>, ()>> {
        let mut line = Vec::new();
        let mut oversized = false;
        loop {
            let chunk = match self.inner.fill_buf().await {
                Ok([]) | Err(_) => break,
                Ok(chunk) => chunk,
            };
            let (take, done) = match chunk.iter().position(|&b| b == b'\n') {
                Some(pos) => (pos + 1, true),
                None => (chunk.len(), false),
            };
            if !oversized {
                line.extend_from_slice(&chunk[..take]);
                if line.len() > MAX_EVENT_LINE + 1 {
                    oversized = true;
                    line.clear();
                }
            }
            self.inner.consume(take);
            if done {
                return Some(if oversized {
                    Err(())
                } else {
                    line.pop();
                    Ok(line)
                });
            }
        }
        if oversized {
            Some(Err(()))
        } else if line.is_empty() {
            None
        } else {
            Some(Ok(line))
        }
    }
}

pub(crate) fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let policy = Policy {
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            ..Policy::default()
        };
        let secs: Vec<u64> = (1..=7).map(|n| policy.backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(policy.backoff(0).as_secs(), 1);
        assert_eq!(policy.backoff(u32::MAX).as_secs(), 30);
    }

    #[test]
    fn default_policy_is_five_crashes_in_five_minutes() {
        let policy = Policy::default();
        assert_eq!((policy.max_crashes, policy.window.as_secs()), (5, 300));
    }
}
