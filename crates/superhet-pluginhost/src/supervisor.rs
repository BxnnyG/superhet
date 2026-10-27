//! Instance supervision: spawn, watch, restart with backoff, give up after repeated crashes.
//!
//! One tokio task per instance runs the loop *start -> run -> (crash -> backoff)*. A process that
//! exits on its own, is killed from outside or stays silent past the manifest's heartbeat
//! timeout counts as a crash. Crashes are remembered for [`Policy::window`]; reaching
//! [`Policy::max_crashes`] inside the window puts the instance into
//! [`InstanceState::Error`], where it stays until `restart`. Simple transport only; the framed
//! transport, container and UI plugins are refused with [`StartError::Unsupported`].

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;
use superhet_plugin_protocol::ctl::{InstanceInfo, InstanceState};
use superhet_plugin_protocol::event::{HostEvent, Line, MAX_EVENT_LINE};
use superhet_plugin_protocol::manifest::{PluginType, RuntimeKind, Transport};
use superhet_plugin_protocol::{InstanceEnv, PROTOCOL_VERSION, Package};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, watch};

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

/// Parameters of a new instance.
#[derive(Debug, Clone, Default)]
pub struct StartSpec {
    /// Device to bind to.
    pub device: Option<String>,
    /// Frequency (Hz) to bind to.
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
    /// Unknown instance.
    #[error("no such instance")]
    NotFound,
}

struct Shared {
    id: String,
    package: Arc<Package>,
    spec: StartSpec,
    info: Mutex<Mutable>,
    events_ok: AtomicU64,
    events_invalid: AtomicU64,
    restarts: AtomicU32,
    tx: broadcast::Sender<InstanceInfo>,
}

struct Mutable {
    state: InstanceState,
    pid: Option<u32>,
    crashes_in_window: u32,
    retry_in: Option<Duration>,
    last_error: Option<String>,
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
            freq_hz: self.spec.freq_hz,
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
            updated_unix_ms: m.updated_unix_ms,
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
        let (tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                policy,
                sink,
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
        check_runnable(&package)?;
        let id = format!("i-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let mut instances = self.instances();
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
            info: Mutex::new(Mutable {
                state: InstanceState::Starting,
                pid: None,
                crashes_in_window: 0,
                retry_in: None,
                last_error: None,
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

async fn run_instance(
    policy: Policy,
    sink: Arc<dyn EventSink>,
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
            Ok(child) => run_process(child, &policy, &sink, &shared, &mut stop).await,
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

fn spawn(shared: &Shared) -> std::io::Result<Child> {
    let package = &shared.package;
    let (program, args) = package
        .command()
        .ok_or_else(|| std::io::Error::other("manifest has no runtime.command"))?;
    let env = InstanceEnv {
        plugin_id: &package.manifest.plugin.id,
        instance_id: &shared.id,
        plugin_dir: &package.dir,
        device: shared.spec.device.as_deref(),
        freq_hz: shared.spec.freq_hz,
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Own process group: a Ctrl-C at the host's terminal must not reach the plugins; the
        // host stops them in an orderly way.
        .process_group(0);
    command.spawn()
}

async fn run_process(
    mut child: Child,
    policy: &Policy,
    sink: &Arc<dyn EventSink>,
    shared: &Arc<Shared>,
    stop: &mut watch::Receiver<bool>,
) -> Outcome {
    let pid = child.id();
    shared.update(|m| {
        m.state = InstanceState::Running;
        m.pid = pid;
    });
    let started = Instant::now();
    let activity = Arc::new(AtomicU64::new(0));
    // Held open so the plugin sees EOF only when the host wants it to end.
    let stdin = child.stdin.take();
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
                drop(stdin);
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
                sink.emit(HostEvent {
                    v: PROTOCOL_VERSION,
                    plugin_id: shared.package.manifest.plugin.id.clone(),
                    instance_id: shared.id.clone(),
                    device: shared.spec.device.clone(),
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
}
