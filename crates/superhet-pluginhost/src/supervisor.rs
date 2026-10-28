//! Instance supervision: spawn, watch, restart with backoff, give up after repeated crashes.
//!
//! One tokio task per instance runs the loop *start -> run -> (crash -> backoff)*. A process that
//! exits on its own, is killed from outside or stays silent past the manifest's heartbeat
//! timeout counts as a crash. Crashes are remembered for [`Policy::window`]; reaching
//! [`Policy::max_crashes`] inside the window puts the instance into
//! [`InstanceState::Error`], where it stays until `restart`. Simple transport only; the framed
//! transport, container and UI plugins are refused with [`StartError::Unsupported`].
//!
//! Input: an instance bound to a device and a VFO (or an exclusive plugin bound to a device) gets
//! its samples through an [`Inputs`] provider, which the supervisor attaches for every process
//! start and drops when the process ends; the bytes are written to the plugin's `stdin`. When
//! the process ends, its whole process group is killed, so helpers the plugin started cannot
//! outlive it.

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;
use superhet_bus::tap::{ClaimAcquire, TapFormat, TapKind, TapOpen};
use superhet_plugin_protocol::ctl::{InstanceInfo, InstanceState};
use superhet_plugin_protocol::event::{HostEvent, Line, MAX_EVENT_LINE};
use superhet_plugin_protocol::manifest::{
    InputKind, PluginType, RuntimeKind, SampleFormat, Transport,
};
use superhet_plugin_protocol::{InstanceEnv, PROTOCOL_VERSION, Package};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Notify, broadcast, mpsc, watch};

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

struct Shared {
    id: String,
    package: Arc<Package>,
    spec: StartSpec,
    input: Option<InputRequest>,
    input_dropped: Arc<AtomicU64>,
    info: Mutex<Mutable>,
    events_ok: AtomicU64,
    events_invalid: AtomicU64,
    restarts: AtomicU32,
    reconfigures: AtomicU32,
    /// Liveness of the input, written by the feeder task.
    live: Arc<InputLive>,
    last_event_ms: AtomicU64,
    created_ms: u64,
    /// Wakes the supervising task when the configuration was replaced.
    reload: Notify,
    tx: broadcast::Sender<InstanceInfo>,
}

/// What the feeder reports about the input of one instance.
#[derive(Debug)]
struct InputLive {
    bytes: AtomicU64,
    last_ms: AtomicU64,
    /// Bits of the smoothed mean square (f64); [`NO_LEVEL`] before the first chunk.
    mean_square: AtomicU64,
}

const NO_LEVEL: u64 = u64::MAX;

impl InputLive {
    fn new() -> Self {
        Self {
            bytes: AtomicU64::new(0),
            last_ms: AtomicU64::new(0),
            mean_square: AtomicU64::new(NO_LEVEL),
        }
    }

    fn record(&self, format: TapFormat, chunk: &[u8]) {
        self.bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        self.last_ms.store(unix_ms(), Ordering::Relaxed);
        let Some(ms) = mean_square(format, chunk) else {
            return;
        };
        let previous = self.mean_square.load(Ordering::Relaxed);
        let smoothed = if previous == NO_LEVEL {
            ms
        } else {
            0.8f64.mul_add(f64::from_bits(previous), 0.2 * ms)
        };
        self.mean_square
            .store(smoothed.to_bits(), Ordering::Relaxed);
    }

    fn level_db(&self) -> Option<f32> {
        let bits = self.mean_square.load(Ordering::Relaxed);
        if bits == NO_LEVEL {
            return None;
        }
        let ms = f64::from_bits(bits);
        // Floor at -120 dBFS so silence is a number, not minus infinity.
        #[allow(clippy::cast_possible_truncation)]
        Some((10.0 * ms.max(1e-12).log10()) as f32)
    }
}

/// Mean square of the samples in `chunk` relative to full scale (1.0 = full scale), looking at
/// no more than about 2048 samples.
fn mean_square(format: TapFormat, chunk: &[u8]) -> Option<f64> {
    let (width, read): (usize, fn(&[u8]) -> f64) = match format {
        TapFormat::S16le | TapFormat::Cs16 => (2, |b| {
            let v = f64::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0;
            v * v
        }),
        TapFormat::Cu8 => (1, |b| {
            let v = (f64::from(b[0]) - 127.5) / 127.5;
            v * v
        }),
        TapFormat::Cf32 | TapFormat::F32le => (4, |b| {
            let v = f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            if v.is_finite() { v * v } else { 0.0 }
        }),
    };
    let count = chunk.len() / width;
    if count == 0 {
        return None;
    }
    let step = (count / 2048).max(1);
    let (mut sum, mut n) = (0.0, 0u32);
    let mut i = 0;
    while i < count {
        sum += read(&chunk[i * width..]);
        n += 1;
        i += step;
    }
    Some(sum / f64::from(n))
}

struct Mutable {
    config: Value,
    state: InstanceState,
    pid: Option<u32>,
    crashes_in_window: u32,
    retry_in: Option<Duration>,
    last_error: Option<String>,
    input_error: Option<String>,
    updated_unix_ms: u64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Mutable> {
        self.info
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn snapshot(&self) -> InstanceInfo {
        let m = self.lock();
        InstanceInfo {
            id: self.id.clone(),
            plugin_id: self.package.manifest.plugin.id.clone(),
            device: self.spec.device.clone(),
            vfo: self.spec.vfo,
            freq_hz: self.spec.freq_hz,
            config: m.config.clone(),
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
            updated_unix_ms: m.updated_unix_ms,
            input_bytes: self.live.bytes.load(Ordering::Relaxed),
            input_level_db: self.live.level_db(),
            last_input_unix_ms: Some(self.live.last_ms.load(Ordering::Relaxed)).filter(|t| *t > 0),
            last_event_unix_ms: Some(self.last_event_ms.load(Ordering::Relaxed)).filter(|t| *t > 0),
            created_unix_ms: self.created_ms,
            reconfigures: self.reconfigures.load(Ordering::Relaxed),
        }
    }

    /// Change the mutable part and broadcast the new snapshot.
    fn update(&self, change: impl FnOnce(&mut Mutable)) {
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
        let (tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                policy,
                sink,
                inputs,
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
        let config = spec.config.clone();
        let shared = Arc::new(Shared {
            id: id.clone(),
            package,
            spec,
            input,
            input_dropped: Arc::new(AtomicU64::new(0)),
            info: Mutex::new(Mutable {
                config,
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
            reconfigures: AtomicU32::new(0),
            live: Arc::new(InputLive::new()),
            last_event_ms: AtomicU64::new(0),
            created_ms: unix_ms(),
            reload: Notify::new(),
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

    /// Replace the configuration of an instance and restart its process with it. The instance
    /// keeps its id, binding and counters; a running process is stopped like on `stop` (stdin
    /// closed, killed after the grace period) and started again, an instance in backoff or
    /// `error` starts at once with a clean crash history.
    ///
    /// # Errors
    /// [`StartError::NotFound`] for unknown ids, [`StartError::Invalid`] when `config` is
    /// neither `null` nor an object.
    pub fn reconfigure(&self, id: &str, config: Value) -> Result<InstanceInfo, StartError> {
        if !(config.is_null() || config.is_object()) {
            return Err(StartError::Invalid("config must be an object".into()));
        }
        let mut instances = self.instances();
        let entry = instances.get_mut(id).ok_or(StartError::NotFound)?;
        let shared = Arc::clone(&entry.shared);
        let was_error = shared.snapshot().state == InstanceState::Error;
        shared.reconfigures.fetch_add(1, Ordering::Relaxed);
        shared.update(|m| {
            m.config = config;
            if was_error {
                m.state = InstanceState::Starting;
                m.crashes_in_window = 0;
                m.retry_in = None;
            }
        });
        if was_error {
            let fresh = self.spawn_task(shared);
            let info = fresh.shared.snapshot();
            *entry = fresh;
            return Ok(info);
        }
        shared.reload.notify_one();
        Ok(shared.snapshot())
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
    if manifest.runtime.transport == Transport::Framed {
        return Err(StartError::Unsupported(
            "the framed transport is not implemented by this host yet".into(),
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
        let outcome = match spawn(&shared) {
            Ok(child) => run_process(child, &policy, &sink, &inputs, &shared, &mut stop).await,
            Err(err) => Outcome::Crashed(format!("spawn failed: {err}")),
        };
        let reason = match outcome {
            Outcome::Reload => {
                // A new configuration is not a crash: start again at once, history cleared.
                crashes.clear();
                shared.update(|m| {
                    m.crashes_in_window = 0;
                    m.last_error = None;
                });
                continue;
            }
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
            () = shared.reload.notified() => {
                crashes.clear();
                shared.update(|m| {
                    m.crashes_in_window = 0;
                    m.last_error = None;
                });
            }
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
    /// Stopped to start again with a new configuration.
    Reload,
    Crashed(String),
}

fn spawn(shared: &Shared) -> std::io::Result<Child> {
    let package = &shared.package;
    let (program, args) = package
        .command()
        .ok_or_else(|| std::io::Error::other("manifest has no runtime.command"))?;
    let config = shared.lock().config.clone();
    let env = InstanceEnv {
        plugin_id: &package.manifest.plugin.id,
        instance_id: &shared.id,
        plugin_dir: &package.dir,
        device: shared.spec.device.as_deref(),
        vfo: shared.spec.vfo,
        freq_hz: shared.spec.freq_hz,
        config: &config,
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
async fn feed_stdin(
    mut stdin: ChildStdin,
    mut attachment: Attachment,
    live: Arc<InputLive>,
    format: TapFormat,
) {
    while let Some(chunk) = attachment.chunks.recv().await {
        live.record(format, &chunk);
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
    let format = request.tap.format;
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
        feed_stdin(pipe, attachment, Arc::clone(&shared.live), format).await;
        watcher.abort();
    }))
}

async fn run_process(
    mut child: Child,
    policy: &Policy,
    sink: &Arc<dyn EventSink>,
    inputs: &Arc<dyn Inputs>,
    shared: &Arc<Shared>,
    stop: &mut watch::Receiver<bool>,
) -> Outcome {
    let pid = child.id();
    shared.update(|m| {
        m.state = InstanceState::Running;
        m.pid = pid;
        m.input_error = None;
    });
    let started = Instant::now();
    let activity = Arc::new(AtomicU64::new(0));
    // Held open so the plugin sees EOF only when the host wants it to end.
    let mut stdin = Some(open_stdin(child.stdin.take(), inputs, shared));
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
            _ = stop.changed() => {
                if let Some(stdin) = stdin.take() {
                    stdin.close().await;
                }
                if tokio::time::timeout(policy.stop_grace, child.wait()).await.is_err() {
                    let _ = child.kill().await;
                }
                break Outcome::Stopped;
            }
            () = shared.reload.notified() => {
                if let Some(stdin) = stdin.take() {
                    stdin.close().await;
                }
                if tokio::time::timeout(policy.stop_grace, child.wait()).await.is_err() {
                    let _ = child.kill().await;
                }
                break Outcome::Reload;
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
        match verdict {
            Ok(Line::Heartbeat) => {}
            Ok(Line::Event(event)) => {
                shared.events_ok.fetch_add(1, Ordering::Relaxed);
                shared.last_event_ms.store(unix_ms(), Ordering::Relaxed);
                sink.emit(HostEvent {
                    v: PROTOCOL_VERSION,
                    plugin_id: shared.package.manifest.plugin.id.clone(),
                    instance_id: shared.id.clone(),
                    device: shared.spec.device.clone(),
                    vfo: shared.spec.vfo,
                    freq_hz: shared.spec.freq_hz,
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

fn unix_ms() -> u64 {
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

    #[test]
    fn input_level_follows_the_signal() {
        let live = InputLive::new();
        assert_eq!(live.level_db(), None);
        // Full-scale square wave: 0 dBFS.
        let loud: Vec<u8> = (0..1000)
            .flat_map(|i| {
                if i % 2 == 0 { i16::MAX } else { i16::MIN }
                    .to_le_bytes()
                    .to_vec()
            })
            .collect();
        live.record(TapFormat::S16le, &loud);
        assert!(live.level_db().is_some_and(|db| db > -0.1));
        assert_eq!(live.bytes.load(Ordering::Relaxed), 2000);
        // Silence in cu8 is a mid-scale value (about -48 dBFS); the level decays towards it.
        let quiet = vec![128u8; 4000];
        for _ in 0..80 {
            live.record(TapFormat::Cu8, &quiet);
        }
        assert!(live.level_db().is_some_and(|db| db < -40.0));
    }
}
