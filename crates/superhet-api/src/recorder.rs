//! The recording engine (ADR-0028): the replay ring buffers, the running recordings, the quota
//! janitor and the scanner hit hook. REST lives in `recordings.rs`.
//!
//! One task subscribes to every audio stream on the bus. Each frame goes into the ring buffer of
//! its VFO (instant replay) and to the recorders that follow that VFO. An audio recording is a
//! task of its own that wraps the Opus packets it receives into Ogg pages as they are (no
//! re-encoding). An IQ recording opens a tap at the daemon (`docs/plugin-contract.md`, taps) and
//! appends the bytes it receives to a SigMF data file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::Client;
use futures_util::StreamExt;
use serde_json::{Value, json};
use superhet_bus::audio::{self, AudioFrame};
use superhet_bus::audit::Outcome;
use superhet_bus::rds::RdsMessage;
use superhet_bus::subjects;
use superhet_bus::tap::{self, TapFormat, TapKind, TapOpen};
use superhet_db::{PgPool, recordings as db};
use superhet_recording::files::{self, Kind, Part};
use superhet_recording::gate::{Gate, Kept};
use superhet_recording::ogg::{OggOpusWriter, OpusTags, PACKET_SAMPLES};
use superhet_recording::quota::{self, Entry};
use superhet_recording::ring::{self, ReplayFrame, ReplayHub};
use superhet_recording::sigmf::{DataHash, Datatype, Meta};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, watch};

use crate::audit::{Audit, Who};

/// Default directory (a volume in the compose file).
pub const DEFAULT_DIR: &str = "/data/recordings";
/// Default quota in GB.
pub const DEFAULT_MAX_GB: u64 = 20;
/// Most recordings written at the same time.
pub const MAX_ACTIVE: usize = 16;
/// Default and hard limits of one audio recording.
pub const AUDIO_DEFAULT_SECONDS: u64 = 4 * 3600;
/// See [`AUDIO_DEFAULT_SECONDS`].
pub const AUDIO_MAX_SECONDS: u64 = 24 * 3600;
/// Default and hard limits of one IQ recording.
pub const IQ_DEFAULT_SECONDS: u64 = 300;
/// See [`IQ_DEFAULT_SECONDS`].
pub const IQ_MAX_SECONDS: u64 = 3600;
/// Default IQ size limit in MiB.
pub const IQ_DEFAULT_MB: u64 = 2048;
/// Hard IQ size limit in MiB.
pub const IQ_MAX_MB: u64 = 16_384;
/// A recording without data for this long ends (the VFO is gone, the device retuned).
const IDLE: Duration = Duration::from_secs(15);
/// Frames of squelch-closed audio after which a scanner hit recording ends (4 s).
const HIT_SILENCE_FRAMES: usize = 200;
/// Hit recordings shorter than this are discarded (squelch tails, noise).
const HIT_MIN_MS: i64 = 300;
/// Longest scanner hit recording.
const HIT_MAX_SECONDS: u64 = 900;
/// The janitor runs this often.
const JANITOR_EVERY: Duration = Duration::from_secs(30);
/// Space a new recording is assumed to need on top of the current total.
const HEADROOM: u64 = 32 * 1024 * 1024;

/// Settings from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// Recordings directory.
    pub dir: PathBuf,
    /// Quota in bytes, 0 = unlimited.
    pub max_bytes: u64,
    /// Replay window in seconds.
    pub replay_seconds: u32,
}

impl Config {
    /// Read `SUPERHET_RECORDINGS_DIR`, `SUPERHET_RECORDINGS_MAX_GB`, `SUPERHET_REPLAY_SECONDS`.
    #[must_use]
    pub fn from_env() -> Self {
        let dir = std::env::var("SUPERHET_RECORDINGS_DIR")
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| DEFAULT_DIR.to_owned());
        let gb = std::env::var("SUPERHET_RECORDINGS_MAX_GB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_MAX_GB);
        let seconds = std::env::var("SUPERHET_REPLAY_SECONDS")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(ring::DEFAULT_SECONDS);
        Self {
            dir: PathBuf::from(dir),
            max_bytes: gb.saturating_mul(1024 * 1024 * 1024),
            replay_seconds: ring::clamp_seconds(seconds),
        }
    }
}

/// Who a recording belongs to.
#[derive(Debug, Clone)]
pub struct Owner {
    /// Display name.
    pub name: String,
    /// `u:<id>`.
    pub id: String,
}

/// Why a recording could not be started.
#[derive(Debug)]
pub enum StartError {
    /// No database.
    NoDatabase,
    /// No bus connection.
    NoBus,
    /// The quota is used up by recordings marked keep (or nothing can be freed).
    QuotaFull,
    /// Too many recordings running.
    TooMany,
    /// The request is not acceptable.
    Invalid(String),
    /// The daemon refused the tap.
    Daemon(String),
    /// Nothing buffered to save.
    Empty,
    /// Disk or database failure.
    Internal(String),
}

fn internal(err: impl std::fmt::Display) -> StartError {
    tracing::error!(%err, "recording failed");
    StartError::Internal("storage error".into())
}

/// Parameters of a saved replay.
#[derive(Debug, Clone)]
pub struct SaveReplay<'a> {
    /// Device id.
    pub device: &'a str,
    /// VFO id.
    pub vfo: u32,
    /// Seconds to save.
    pub seconds: u32,
    /// Cut squelch-closed audio.
    pub gated: bool,
    /// Display name.
    pub title: &'a str,
    /// Owner.
    pub owner: &'a Owner,
    /// Frequency.
    pub freq_hz: Option<i64>,
    /// Mode.
    pub mode: Option<String>,
}

/// Parameters of an audio recording.
#[derive(Debug, Clone)]
pub struct AudioStart {
    /// Device id.
    pub device: String,
    /// VFO id.
    pub vfo: u32,
    /// Skip squelch-closed audio (segments keep their timestamps).
    pub gated: bool,
    /// Display name.
    pub title: String,
    /// `manual` or `scanner`.
    pub source: &'static str,
    /// Owner.
    pub owner: Owner,
    /// Tuned frequency.
    pub freq_hz: Option<i64>,
    /// Mode.
    pub mode: Option<String>,
    /// Longest recording.
    pub max_seconds: u64,
    /// Largest file in bytes.
    pub max_bytes: u64,
    /// End after this many consecutive squelch-closed frames (scanner hits).
    pub idle_stop_frames: Option<usize>,
}

/// What an IQ recording captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IqSource {
    /// The whole sample window of the device at its sample rate.
    Window,
    /// The baseband of one VFO, decimated to the requested rate.
    Vfo(u32),
}

/// Parameters of an IQ recording.
#[derive(Debug, Clone)]
pub struct IqStart {
    /// Device id.
    pub device: String,
    /// Window or VFO slice.
    pub source: IqSource,
    /// Sample encoding.
    pub datatype: Datatype,
    /// Sample rate.
    pub rate: u32,
    /// Centre frequency of the samples.
    pub center_hz: u64,
    /// Display name.
    pub title: String,
    /// Owner.
    pub owner: Owner,
    /// Mode of the VFO (slices).
    pub mode: Option<String>,
    /// Bandwidth of the VFO (slices).
    pub bandwidth_hz: Option<f64>,
    /// Device label for `core:hw`.
    pub hw: Option<String>,
    /// Longest recording.
    pub max_seconds: u64,
    /// Largest data file in bytes.
    pub max_bytes: u64,
}

/// Why a running recording is asked to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// A user stopped it.
    User,
    /// The service shuts down.
    Shutdown,
}

struct Shared {
    bytes: AtomicU64,
    duration_ms: AtomicU64,
}

struct Active {
    stop: watch::Sender<Option<Stop>>,
    done: Option<oneshot::Receiver<()>>,
    shared: Arc<Shared>,
}

#[derive(Default, Clone)]
struct Tags {
    ps: Option<String>,
    rt: Option<String>,
}

type Feeds = HashMap<(String, u32), Vec<(i64, mpsc::Sender<ReplayFrame>)>>;

struct Hook {
    owner: Owner,
    who: Who,
    current: Option<i64>,
}

/// The engine.
pub struct Manager {
    cfg: Config,
    nats: Option<Client>,
    db: Option<PgPool>,
    audit: Audit,
    replay: Mutex<ReplayHub>,
    tags: Mutex<HashMap<(String, u32), Tags>>,
    feeds: Mutex<Feeds>,
    active: Mutex<HashMap<i64, Active>>,
    hooks: Mutex<HashMap<String, Hook>>,
    over_quota: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn device_of(subject: &str, kind: &str) -> Option<(String, String)> {
    // sh.v1.dev.<dev>.<kind>.<tail>
    let mut it = subject.split('.');
    let (_, _, d, dev, k, tail) = (
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
    );
    (d == "dev" && k == kind && it.next().is_none()).then(|| (dev.to_owned(), tail.to_owned()))
}

fn mhz(freq_hz: i64) -> String {
    let text = format!("{:.6}", freq_hz as f64 / 1e6);
    format!("{} MHz", text.trim_end_matches('0').trim_end_matches('.'))
}

impl Manager {
    /// Create the engine.
    #[must_use]
    pub fn new(cfg: Config, nats: Option<Client>, db: Option<PgPool>, audit: Audit) -> Arc<Self> {
        Arc::new(Self {
            replay: Mutex::new(ReplayHub::new(cfg.replay_seconds)),
            cfg,
            nats,
            db,
            audit,
            tags: Mutex::default(),
            feeds: Mutex::default(),
            active: Mutex::default(),
            hooks: Mutex::default(),
            over_quota: AtomicBool::new(false),
        })
    }

    /// An engine without bus and database for unit tests of routes.
    #[cfg(test)]
    #[must_use]
    pub fn for_tests(dir: PathBuf) -> Arc<Self> {
        Self::new(
            Config {
                dir,
                max_bytes: 0,
                replay_seconds: 60,
            },
            None,
            None,
            crate::auth::AuthState::for_tests().audit().clone(),
        )
    }

    /// The settings.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Whether the database is configured.
    #[must_use]
    pub fn has_db(&self) -> bool {
        self.db.is_some()
    }

    /// The pool.
    #[must_use]
    pub fn pool(&self) -> Option<&PgPool> {
        self.db.as_ref()
    }

    /// Whether the quota janitor found the disk budget exhausted.
    #[must_use]
    pub fn quota_exhausted(&self) -> bool {
        self.over_quota.load(Ordering::Relaxed)
    }

    /// Live size and duration of a running recording.
    #[must_use]
    pub fn live(&self, id: i64) -> Option<(u64, u64)> {
        lock(&self.active).get(&id).map(|a| {
            (
                a.shared.bytes.load(Ordering::Relaxed),
                a.shared.duration_ms.load(Ordering::Relaxed),
            )
        })
    }

    /// Replay buffer status: number of buffered VFOs and bytes.
    #[must_use]
    pub fn replay_usage(&self) -> (usize, usize) {
        lock(&self.replay).usage()
    }

    // ---- replay -------------------------------------------------------------------------

    /// The last `seconds` of a VFO as an Ogg file, with the number of 20 ms frames in it.
    #[must_use]
    pub fn replay_ogg(
        &self,
        device: &str,
        vfo: u32,
        seconds: u32,
        gated: bool,
    ) -> Option<(Vec<u8>, usize)> {
        let frames = lock(&self.replay).last(device, vfo, seconds, gated)?;
        let mut tags = OpusTags::default();
        tags.add("TITLE", "Superhet instant replay");
        tags.add("DEVICE", device);
        let channels = frames.first().map_or(1, |f| f.channels);
        let (mut writer, mut bytes) = OggOpusWriter::new(serial(), channels, &tags);
        for frame in &frames {
            if let Some(page) = writer.push(&frame.payload, PACKET_SAMPLES) {
                bytes.extend(page);
            }
        }
        bytes.extend(writer.finish());
        Some((bytes, frames.len()))
    }

    /// Turn the replay buffer into a recording.
    ///
    /// # Errors
    /// [`StartError`] when nothing is buffered, the quota is full or storage fails.
    pub async fn save_replay(&self, req: SaveReplay<'_>) -> Result<db::Record, StartError> {
        let SaveReplay {
            device,
            vfo,
            seconds,
            gated,
            title,
            owner,
            freq_hz,
            mode,
        } = req;
        let pool = self.db.as_ref().ok_or(StartError::NoDatabase)?;
        let (bytes, frames) = self
            .replay_ogg(device, vfo, seconds, gated)
            .ok_or(StartError::Empty)?;
        if !self.ensure_space(bytes.len() as u64).await? {
            return Err(StartError::QuotaFull);
        }
        let id = db::reserve_id(pool).await.map_err(internal)?;
        let stem = files::make_stem(id, freq_hz);
        let path = files::resolve(&self.cfg.dir, Kind::Audio, &stem, Part::Main)
            .ok_or_else(|| StartError::Internal("bad stem".into()))?;
        tokio::fs::create_dir_all(self.cfg.dir.join("audio"))
            .await
            .map_err(internal)?;
        tokio::fs::write(&path, &bytes).await.map_err(internal)?;
        let duration_ms = i64::try_from(frames).unwrap_or(0) * 20;
        let started_ms = now_ms() - duration_ms;
        let new = db::New {
            id,
            kind: "audio".into(),
            source: "replay".into(),
            stem,
            title: title.to_owned(),
            device: device.to_owned(),
            vfo: i32::try_from(vfo).ok(),
            freq_hz,
            mode,
            owner_name: Some(owner.name.clone()),
            owner_id: Some(owner.id.clone()),
            started_ms,
            state: "done".into(),
            duration_ms,
            size_bytes: i64::try_from(bytes.len()).unwrap_or(0),
            meta: json!({"gated": gated, "replay_seconds": seconds, "segments": [{"at_ms": 0, "unix_ms": started_ms}]}),
        };
        if let Err(err) = db::insert(pool, &new).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(internal(err));
        }
        db::get(pool, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| StartError::Internal("row vanished".into()))
    }

    // ---- quota --------------------------------------------------------------------------

    /// Free space for `extra` more bytes by deleting the oldest recordings that are not marked
    /// keep. Returns whether it fits afterwards.
    async fn ensure_space(&self, extra: u64) -> Result<bool, StartError> {
        let Some(pool) = &self.db else {
            return Err(StartError::NoDatabase);
        };
        if self.cfg.max_bytes == 0 {
            return Ok(true);
        }
        let rows = db::finished_sizes(pool).await.map_err(internal)?;
        let entries: Vec<Entry> = rows
            .iter()
            .map(|(id, size, started, keep)| Entry {
                id: *id,
                bytes: u64::try_from(*size).unwrap_or(0),
                started_ms: *started,
                keep: *keep,
            })
            .collect();
        let running: u64 = lock(&self.active)
            .values()
            .map(|a| a.shared.bytes.load(Ordering::Relaxed))
            .sum();
        let plan = quota::plan_cleanup(&entries, running.saturating_add(extra), self.cfg.max_bytes);
        for id in &plan.delete {
            if let Ok(Some(row)) = db::delete(pool, *id).await {
                self.remove_files(&row).await;
                tracing::info!(id, "recording removed by quota cleanup");
            }
        }
        self.over_quota.store(!plan.fits, Ordering::Relaxed);
        Ok(plan.fits)
    }

    /// Delete the files of a recording (missing files are fine).
    pub async fn remove_files(&self, rec: &db::Record) {
        let Some(kind) = Kind::parse(&rec.kind) else {
            return;
        };
        for part in [Part::Main, Part::Meta] {
            if let Some(path) = files::resolve(&self.cfg.dir, kind, &rec.stem, part) {
                match tokio::fs::remove_file(&path).await {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => tracing::warn!(%err, "could not remove recording file"),
                }
            }
        }
    }

    // ---- starting -----------------------------------------------------------------------

    fn register(&self, id: i64, active: Active) -> Result<(), StartError> {
        let mut map = lock(&self.active);
        if map.len() >= MAX_ACTIVE {
            return Err(StartError::TooMany);
        }
        map.insert(id, active);
        Ok(())
    }

    /// Start an audio recording of a VFO.
    ///
    /// # Errors
    /// [`StartError`].
    pub async fn start_audio(self: &Arc<Self>, req: AudioStart) -> Result<db::Record, StartError> {
        let pool = self.db.clone().ok_or(StartError::NoDatabase)?;
        if lock(&self.active).len() >= MAX_ACTIVE {
            return Err(StartError::TooMany);
        }
        if !self.ensure_space(HEADROOM).await? {
            return Err(StartError::QuotaFull);
        }
        let id = db::reserve_id(&pool).await.map_err(internal)?;
        let stem = files::make_stem(id, req.freq_hz);
        let path = files::resolve(&self.cfg.dir, Kind::Audio, &stem, Part::Main)
            .ok_or_else(|| StartError::Internal("bad stem".into()))?;
        tokio::fs::create_dir_all(self.cfg.dir.join("audio"))
            .await
            .map_err(internal)?;
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(internal)?;
        let started_ms = now_ms();
        let new = db::New {
            id,
            kind: "audio".into(),
            source: req.source.into(),
            stem,
            title: req.title.clone(),
            device: req.device.clone(),
            vfo: i32::try_from(req.vfo).ok(),
            freq_hz: req.freq_hz,
            mode: req.mode.clone(),
            owner_name: Some(req.owner.name.clone()),
            owner_id: Some(req.owner.id.clone()),
            started_ms,
            state: "recording".into(),
            duration_ms: 0,
            size_bytes: 0,
            meta: json!({"gated": req.gated}),
        };
        if let Err(err) = db::insert(&pool, &new).await {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err(internal(err));
        }
        let (tx, rx) = mpsc::channel(512);
        let (stop_tx, stop_rx) = watch::channel(None);
        let (done_tx, done_rx) = oneshot::channel();
        let shared = Arc::new(Shared {
            bytes: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
        });
        let registered = self.register(
            id,
            Active {
                stop: stop_tx,
                done: Some(done_rx),
                shared: Arc::clone(&shared),
            },
        );
        if let Err(err) = registered {
            let _ = db::delete(&pool, id).await;
            let _ = tokio::fs::remove_file(&path).await;
            return Err(err);
        }
        lock(&self.feeds)
            .entry((req.device.clone(), req.vfo))
            .or_default()
            .push((id, tx));
        let manager = Arc::clone(self);
        let tags = lock(&self.tags)
            .get(&(req.device.clone(), req.vfo))
            .cloned()
            .unwrap_or_default();
        tokio::spawn(async move {
            manager
                .run_audio(id, req, file, rx, stop_rx, shared, tags)
                .await;
            let _ = done_tx.send(());
        });
        db::get(&pool, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| StartError::Internal("row vanished".into()))
    }

    /// Start an IQ recording.
    ///
    /// # Errors
    /// [`StartError`].
    pub async fn start_iq(self: &Arc<Self>, req: IqStart) -> Result<db::Record, StartError> {
        let pool = self.db.clone().ok_or(StartError::NoDatabase)?;
        let client = self.nats.clone().ok_or(StartError::NoBus)?;
        if lock(&self.active).len() >= MAX_ACTIVE {
            return Err(StartError::TooMany);
        }
        if !self.ensure_space(req.max_bytes.min(HEADROOM)).await? {
            return Err(StartError::QuotaFull);
        }
        let id = db::reserve_id(&pool).await.map_err(internal)?;
        let freq = i64::try_from(req.center_hz).ok();
        let stem = files::make_stem(id, freq);
        let data = files::resolve(&self.cfg.dir, Kind::Iq, &stem, Part::Main)
            .ok_or_else(|| StartError::Internal("bad stem".into()))?;
        let meta_path = files::resolve(&self.cfg.dir, Kind::Iq, &stem, Part::Meta)
            .ok_or_else(|| StartError::Internal("bad stem".into()))?;
        tokio::fs::create_dir_all(self.cfg.dir.join("iq"))
            .await
            .map_err(internal)?;
        let tap_id = format!("rec{id}");
        let open = TapOpen {
            v: tap::VERSION,
            id: tap_id.clone(),
            kind: if matches!(req.source, IqSource::Window) {
                TapKind::Window
            } else {
                TapKind::Iq
            },
            vfo: match req.source {
                IqSource::Vfo(v) => Some(v),
                IqSource::Window => None,
            },
            rate: req.rate,
            format: match req.datatype {
                Datatype::Cu8 => TapFormat::Cu8,
                Datatype::Ci16Le => TapFormat::Cs16,
                Datatype::Cf32Le => TapFormat::Cf32,
            },
            center_hz: matches!(req.source, IqSource::Window).then_some(req.center_hz),
        };
        open.validate()
            .map_err(|e| StartError::Invalid(e.to_string()))?;
        let data_subject =
            subjects::tap(&req.device, &tap_id).map_err(|e| StartError::Invalid(e.to_string()))?;
        let open_subject = subjects::ctl_tap(&req.device, "open")
            .map_err(|e| StartError::Invalid(e.to_string()))?;
        let close_subject = subjects::ctl_tap(&req.device, "close")
            .map_err(|e| StartError::Invalid(e.to_string()))?;
        // Subscribe before the tap opens so the first chunk is not lost.
        let sub = client.subscribe(data_subject).await.map_err(internal)?;
        let payload = serde_json::to_vec(&open).map_err(internal)?;
        let reply = tokio::time::timeout(
            Duration::from_secs(3),
            client.request(open_subject.clone(), payload.clone().into()),
        )
        .await
        .map_err(|_| StartError::Daemon("the device daemon did not answer".into()))?
        .map_err(|_| StartError::Daemon("the device daemon is not reachable".into()))?;
        let reply: Value = serde_json::from_slice(&reply.payload).unwrap_or(Value::Null);
        if reply["ok"] != true {
            let why = reply["error"].as_str().unwrap_or("tap refused").to_owned();
            return Err(StartError::Daemon(why));
        }
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&data)
            .await
            .map_err(internal)?;
        let started_ms = now_ms();
        let meta = Meta {
            datatype: req.datatype,
            sample_rate: req.rate,
            frequency_hz: req.center_hz as f64,
            start_unix_ms: started_ms,
            description: req.title.clone(),
            recorder: format!("Superhet {}", env!("CARGO_PKG_VERSION")),
            hw: req.hw.clone(),
            bandwidth_hz: req.bandwidth_hz,
            label: req.mode.clone(),
        };
        let _ = tokio::fs::write(&meta_path, meta.to_json(None, None).to_string()).await;
        let new = db::New {
            id,
            kind: "iq".into(),
            source: "manual".into(),
            stem,
            title: req.title.clone(),
            device: req.device.clone(),
            vfo: match req.source {
                IqSource::Vfo(v) => i32::try_from(v).ok(),
                IqSource::Window => None,
            },
            freq_hz: freq,
            mode: req.mode.clone(),
            owner_name: Some(req.owner.name.clone()),
            owner_id: Some(req.owner.id.clone()),
            started_ms,
            state: "recording".into(),
            duration_ms: 0,
            size_bytes: 0,
            meta: json!({
                "datatype": req.datatype.as_str(), "sample_rate": req.rate,
                "source": if matches!(req.source, IqSource::Window) { "window" } else { "vfo" },
            }),
        };
        let cleanup = |data: PathBuf, meta: PathBuf| async move {
            let _ = tokio::fs::remove_file(data).await;
            let _ = tokio::fs::remove_file(meta).await;
        };
        if let Err(err) = db::insert(&pool, &new).await {
            let _ = client
                .publish(close_subject.clone(), close_payload(&tap_id).into())
                .await;
            cleanup(data, meta_path).await;
            return Err(internal(err));
        }
        let (stop_tx, stop_rx) = watch::channel(None);
        let (done_tx, done_rx) = oneshot::channel();
        let shared = Arc::new(Shared {
            bytes: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
        });
        let registered = self.register(
            id,
            Active {
                stop: stop_tx,
                done: Some(done_rx),
                shared: Arc::clone(&shared),
            },
        );
        if let Err(err) = registered {
            let _ = db::delete(&pool, id).await;
            let _ = client
                .publish(close_subject, close_payload(&tap_id).into())
                .await;
            cleanup(data, meta_path).await;
            return Err(err);
        }
        let manager = Arc::clone(self);
        let job = IqJob {
            id,
            req,
            meta,
            meta_path,
            open_subject,
            close_subject,
            open_payload: payload,
            tap_id,
            client,
        };
        tokio::spawn(async move {
            manager.run_iq(job, file, sub, stop_rx, shared).await;
            let _ = done_tx.send(());
        });
        db::get(&pool, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| StartError::Internal("row vanished".into()))
    }

    /// Ask a running recording to end and wait until its file is closed. Returns whether it was
    /// running.
    pub async fn stop(&self, id: i64, why: Stop) -> bool {
        let done = {
            let mut map = lock(&self.active);
            let Some(active) = map.get_mut(&id) else {
                return false;
            };
            let _ = active.stop.send(Some(why));
            active.done.take()
        };
        if let Some(done) = done {
            let _ = tokio::time::timeout(Duration::from_secs(10), done).await;
        }
        true
    }

    /// Stop everything (shutdown).
    pub async fn stop_all(&self) {
        let ids: Vec<i64> = lock(&self.active).keys().copied().collect();
        for id in ids {
            self.stop(id, Stop::Shutdown).await;
        }
    }

    // ---- tasks --------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn run_audio(
        self: Arc<Self>,
        id: i64,
        req: AudioStart,
        mut file: tokio::fs::File,
        mut rx: mpsc::Receiver<ReplayFrame>,
        mut stop: watch::Receiver<Option<Stop>>,
        shared: Arc<Shared>,
        start_tags: Tags,
    ) {
        let mut gate: Gate<ReplayFrame> = Gate::new();
        let mut writer: Option<OggOpusWriter> = None;
        let mut segments: Vec<Value> = Vec::new();
        let mut first = true;
        let mut reason = "stopped";
        let mut failed = false;
        let mut check = tokio::time::interval(Duration::from_secs(5));
        let started = now_ms();
        loop {
            let frame = tokio::select! {
                _ = stop.changed() => break,
                _ = check.tick() => {
                    if self.quota_exhausted() {
                        reason = "quota";
                        break;
                    }
                    continue;
                }
                frame = tokio::time::timeout(IDLE, rx.recv()) => match frame {
                    Ok(Some(frame)) => frame,
                    _ => {
                        reason = "stream_ended";
                        break;
                    }
                },
            };
            let kept: Vec<Kept<ReplayFrame>> = if req.gated {
                let open = frame.open;
                gate.push(frame, open)
            } else {
                vec![Kept {
                    frame,
                    starts_segment: std::mem::take(&mut first),
                }]
            };
            for item in kept {
                let w = match &mut writer {
                    Some(w) => w,
                    None => {
                        let mut tags = OpusTags::default();
                        tags.add("TITLE", &req.title);
                        tags.add("DEVICE", &req.device);
                        tags.add("VFO", &req.vfo.to_string());
                        if let Some(f) = req.freq_hz {
                            tags.add("FREQUENCY", &f.to_string());
                        }
                        if let Some(mode) = &req.mode {
                            tags.add("MODE", mode);
                        }
                        tags.add("ARTIST", &req.owner.name);
                        tags.add("DATE", &superhet_recording::sigmf::iso8601_utc(started));
                        tags.add("RDS_PS", start_tags.ps.as_deref().unwrap_or(""));
                        let (w, head) = OggOpusWriter::new(serial(), item.frame.channels, &tags);
                        if file.write_all(&head).await.is_err() {
                            failed = true;
                            break;
                        }
                        shared.bytes.fetch_add(head.len() as u64, Ordering::Relaxed);
                        writer.insert(w)
                    }
                };
                if item.starts_segment {
                    segments.push(json!({
                        "at_ms": w.samples() * 1000 / 48_000,
                        "unix_ms": item.frame.capture_unix_ns / 1_000_000,
                    }));
                }
                if let Some(page) = w.push(&item.frame.payload, PACKET_SAMPLES) {
                    if file.write_all(&page).await.is_err() {
                        failed = true;
                        break;
                    }
                    shared.bytes.fetch_add(page.len() as u64, Ordering::Relaxed);
                }
                shared
                    .duration_ms
                    .store(w.samples() * 1000 / 48_000, Ordering::Relaxed);
            }
            if failed {
                reason = "error";
                break;
            }
            let seconds = shared.duration_ms.load(Ordering::Relaxed) / 1000;
            if seconds >= req.max_seconds || shared.bytes.load(Ordering::Relaxed) >= req.max_bytes {
                reason = "limit";
                break;
            }
            if let Some(n) = req.idle_stop_frames
                && gate.closed_run() >= n
            {
                reason = "silence";
                break;
            }
        }
        let tags = lock(&self.tags)
            .get(&(req.device.clone(), req.vfo))
            .cloned()
            .unwrap_or_default();
        let channels = writer.is_some();
        let duration_ms = writer.as_ref().map_or(0, |w| w.samples() * 1000 / 48_000);
        if let Some(w) = writer
            && !failed
        {
            let tail = w.finish();
            if file.write_all(&tail).await.is_err() {
                failed = true;
            } else {
                shared.bytes.fetch_add(tail.len() as u64, Ordering::Relaxed);
            }
        }
        let _ = file.flush().await;
        drop(file);
        lock(&self.feeds)
            .entry((req.device.clone(), req.vfo))
            .and_modify(|v| v.retain(|(i, _)| *i != id));
        let mut meta = json!({
            "gated": req.gated, "segments": segments, "end_reason": reason,
            "ps": tags.ps, "rt": tags.rt,
        });
        if let Some(n) = req.idle_stop_frames {
            meta["idle_stop_frames"] = json!(n);
        }
        let short_hit =
            req.source == "scanner" && i64::try_from(duration_ms).unwrap_or(0) < HIT_MIN_MS;
        let discard = !channels || short_hit;
        self.finish(
            id,
            Kind::Audio,
            failed || (!channels && reason == "error"),
            discard,
            duration_ms,
            meta,
        )
        .await;
    }

    async fn finish(
        &self,
        id: i64,
        kind: Kind,
        failed: bool,
        discard: bool,
        duration_ms: u64,
        meta: Value,
    ) {
        lock(&self.active).remove(&id);
        let Some(pool) = &self.db else {
            return;
        };
        let Ok(Some(row)) = db::get(pool, id).await else {
            return;
        };
        if discard {
            if let Ok(Some(row)) = db::delete(pool, id).await {
                self.remove_files(&row).await;
            }
            return;
        }
        let size = files::size_on_disk(&self.cfg.dir, kind, &row.stem);
        let done = db::Finish {
            state: if failed { "failed" } else { "done" }.into(),
            stopped_ms: now_ms(),
            duration_ms: i64::try_from(duration_ms).unwrap_or(0),
            size_bytes: i64::try_from(size).unwrap_or(0),
            meta,
        };
        if let Err(err) = db::finish(pool, id, &done).await {
            tracing::error!(%err, id, "could not finish recording row");
        }
    }

    async fn run_iq(
        self: Arc<Self>,
        job: IqJob,
        mut file: tokio::fs::File,
        mut sub: async_nats::Subscriber,
        mut stop: watch::Receiver<Option<Stop>>,
        shared: Arc<Shared>,
    ) {
        let IqJob {
            id,
            req,
            meta,
            meta_path,
            open_subject,
            close_subject,
            open_payload,
            tap_id,
            client,
        } = job;
        let sample_bytes = req.datatype.sample_bytes();
        let mut hash = DataHash::new();
        let mut written: u64 = 0;
        let mut reason = "stopped";
        let mut failed = false;
        let started = std::time::Instant::now();
        let mut refresh = tokio::time::interval(tap::LEASE_REFRESH);
        refresh.tick().await;
        let mut check = tokio::time::interval(Duration::from_secs(5));
        let mut last_data = std::time::Instant::now();
        loop {
            tokio::select! {
                _ = stop.changed() => break,
                _ = refresh.tick() => {
                    let _ = client.publish(open_subject.clone(), open_payload.clone().into()).await;
                }
                _ = check.tick() => {
                    if self.quota_exhausted() { reason = "quota"; break; }
                    if last_data.elapsed() > IDLE { reason = "stream_ended"; break; }
                }
                msg = sub.next() => {
                    let Some(msg) = msg else { reason = "stream_ended"; break; };
                    last_data = std::time::Instant::now();
                    let remaining = req.max_bytes.saturating_sub(written);
                    let take = usize::try_from(remaining).unwrap_or(usize::MAX).min(msg.payload.len());
                    // Keep whole samples.
                    let take = take - take % usize::try_from(sample_bytes).unwrap_or(1);
                    if take > 0 {
                        let chunk = &msg.payload[..take];
                        if file.write_all(chunk).await.is_err() { failed = true; reason = "error"; break; }
                        hash.update(chunk);
                        written += take as u64;
                        shared.bytes.store(written, Ordering::Relaxed);
                        shared.duration_ms.store(written / sample_bytes * 1000 / u64::from(req.rate.max(1)), Ordering::Relaxed);
                    }
                    if written >= req.max_bytes || shared.duration_ms.load(Ordering::Relaxed) >= req.max_seconds * 1000 {
                        reason = "limit";
                        break;
                    }
                }
            }
        }
        let _ = client
            .publish(close_subject, close_payload(&tap_id).into())
            .await;
        drop(sub);
        let _ = file.flush().await;
        drop(file);
        let samples = written / sample_bytes;
        let duration_ms = samples * 1000 / u64::from(req.rate.max(1));
        let elapsed_ms = u64::try_from(started.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        let completeness = ((duration_ms as f64 / elapsed_ms as f64) * 100.0)
            .clamp(0.0, 100.0)
            .round();
        let digest = hash.finish_hex();
        let doc = meta.to_json(Some(samples), Some(&digest));
        let _ = tokio::fs::write(&meta_path, doc.to_string()).await;
        let info = json!({
            "datatype": req.datatype.as_str(), "sample_rate": req.rate, "samples": samples,
            "sha512": digest, "end_reason": reason, "completeness_percent": completeness,
            "source": if matches!(req.source, IqSource::Window) { "window" } else { "vfo" },
            "center_hz": req.center_hz,
        });
        self.finish(id, Kind::Iq, failed, samples == 0, duration_ms, info)
            .await;
    }

    // ---- ingest and janitor -------------------------------------------------------------

    /// Run until the process ends: feed the rings and recorders from the bus, clean up the quota.
    pub async fn run(self: Arc<Self>) {
        let Some(client) = self.nats.clone() else {
            return;
        };
        let Some(pool) = self.db.clone() else {
            return;
        };
        if let Ok(rows) = db::mark_interrupted(&pool).await {
            for row in rows {
                let size = files::size_on_disk(
                    &self.cfg.dir,
                    Kind::parse(&row.kind).unwrap_or(Kind::Audio),
                    &row.stem,
                );
                let _ = db::set_size(&pool, row.id, i64::try_from(size).unwrap_or(0)).await;
                tracing::warn!(id = row.id, "recording interrupted by a restart");
            }
        }
        let mut janitor = tokio::time::interval(JANITOR_EVERY);
        let mut audio_sub = None;
        let mut rds_sub = None;
        let mut hit_sub = None;
        loop {
            // (Re)subscribe lazily: the client connects in the background.
            if audio_sub.is_none() {
                audio_sub = client.subscribe(subjects::ALL_AUDIO).await.ok();
                rds_sub = client.subscribe(subjects::ALL_RDS).await.ok();
                hit_sub = client.subscribe(subjects::ALL_SCANNER_HITS).await.ok();
                if audio_sub.is_none() {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            }
            let (Some(audio), Some(rds), Some(hits)) =
                (audio_sub.as_mut(), rds_sub.as_mut(), hit_sub.as_mut())
            else {
                audio_sub = None;
                continue;
            };
            tokio::select! {
                msg = audio.next() => match msg {
                    Some(msg) => self.on_audio(&msg.subject, &msg.payload),
                    None => audio_sub = None,
                },
                msg = rds.next() => if let Some(msg) = msg { self.on_rds(&msg.subject, &msg.payload); },
                msg = hits.next() => if let Some(msg) = msg { self.on_hit(&msg.subject, &msg.payload).await; },
                _ = janitor.tick() => {
                    lock(&self.replay).expire(now_ns());
                    let _ = self.ensure_space(0).await;
                }
            }
        }
    }

    fn on_audio(&self, subject: &str, payload: &[u8]) {
        let Some((device, vfo)) = device_of(subject, "audio") else {
            return;
        };
        let Ok(vfo) = vfo.parse::<u32>() else {
            return;
        };
        if audio::is_plugin_vfo(vfo) {
            return;
        }
        let Ok(frame) = AudioFrame::decode(payload) else {
            return;
        };
        let item = ReplayFrame {
            capture_unix_ns: frame.capture_unix_ns,
            channels: frame.channels,
            open: frame.squelch_open(),
            payload: Arc::from(frame.payload),
        };
        if let Some(feeds) = lock(&self.feeds).get(&(device.clone(), vfo)) {
            for (_, tx) in feeds {
                let _ = tx.try_send(item.clone());
            }
        }
        lock(&self.replay).push(&device, vfo, item);
    }

    fn on_rds(&self, subject: &str, payload: &[u8]) {
        let Some((device, vfo)) = device_of(subject, "rds") else {
            return;
        };
        let (Ok(vfo), Ok(msg)) = (
            vfo.parse::<u32>(),
            serde_json::from_slice::<RdsMessage>(payload),
        ) else {
            return;
        };
        let mut tags = lock(&self.tags);
        if tags.len() > 256 {
            tags.clear();
        }
        let entry = tags.entry((device, vfo)).or_default();
        if msg.ps.is_some() {
            entry.ps = msg.ps;
        }
        if msg.rt.is_some() {
            entry.rt = msg.rt;
        }
    }

    // ---- scanner hook -------------------------------------------------------------------

    /// Record the hits of the scanner of `device` until [`Self::disable_hit_recording`].
    pub fn enable_hit_recording(&self, device: &str, owner: Owner, who: Who) {
        lock(&self.hooks).insert(
            device.to_owned(),
            Hook {
                owner,
                who,
                current: None,
            },
        );
    }

    /// Stop recording scanner hits of `device`; a hit recording in progress is finished.
    pub async fn disable_hit_recording(&self, device: &str) {
        let current = lock(&self.hooks).remove(device).and_then(|h| h.current);
        if let Some(id) = current {
            self.stop(id, Stop::User).await;
        }
    }

    /// Devices whose scanner hits are being recorded.
    #[must_use]
    pub fn hit_recording_devices(&self) -> Vec<String> {
        lock(&self.hooks).keys().cloned().collect()
    }

    async fn on_hit(self: &Arc<Self>, subject: &str, payload: &[u8]) {
        let Some(device) = subject.split('.').nth(3).map(str::to_owned) else {
            return;
        };
        let (owner, who, previous) = {
            let hooks = lock(&self.hooks);
            let Some(hook) = hooks.get(&device) else {
                return;
            };
            (hook.owner.clone(), hook.who.clone(), hook.current)
        };
        let Ok(doc) = serde_json::from_slice::<Value>(payload) else {
            return;
        };
        let (Some(vfo), Some(freq)) = (
            doc["vfo"].as_u64().and_then(|v| u32::try_from(v).ok()),
            doc["freq_hz"].as_u64().and_then(|v| i64::try_from(v).ok()),
        ) else {
            return;
        };
        if let Some(previous) = previous {
            self.stop(previous, Stop::User).await;
        }
        let label = doc["label"].as_str().filter(|l| !l.is_empty());
        let title = match label {
            Some(l) => format!("Scanner: {l} ({})", mhz(freq)),
            None => format!("Scanner: {}", mhz(freq)),
        };
        let started = self
            .start_audio(AudioStart {
                device: device.clone(),
                vfo,
                gated: true,
                title,
                source: "scanner",
                owner,
                freq_hz: Some(freq),
                mode: doc["mode"].as_str().map(str::to_owned),
                max_seconds: HIT_MAX_SECONDS,
                max_bytes: 256 * 1024 * 1024,
                idle_stop_frames: Some(HIT_SILENCE_FRAMES),
            })
            .await;
        match started {
            Ok(rec) => {
                if let Some(hook) = lock(&self.hooks).get_mut(&device) {
                    hook.current = Some(rec.id);
                }
                self.audit
                    .log(&who, "recording.start", Some(&device), Outcome::Ok, json!({"id": rec.id, "kind": "audio", "source": "scanner", "freq_hz": freq}))
                    .await;
            }
            Err(err) => tracing::warn!(?err, "scanner hit recording not started"),
        }
    }
}

struct IqJob {
    id: i64,
    req: IqStart,
    meta: Meta,
    meta_path: PathBuf,
    open_subject: String,
    close_subject: String,
    open_payload: Vec<u8>,
    tap_id: String,
    client: Client,
}

fn close_payload(tap_id: &str) -> Vec<u8> {
    json!({"v": tap::VERSION, "id": tap_id})
        .to_string()
        .into_bytes()
}

/// A serial number for an Ogg stream; uniqueness only matters inside one file.
fn serial() -> u32 {
    (now_ns() & 0xFFFF_FFFF) as u32
}

#[cfg(test)]
#[path = "recorder_tests.rs"]
mod tests;
