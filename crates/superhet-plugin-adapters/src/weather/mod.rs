//! Weather satellite reception: Meteor-M LRPT (and NOAA APT for recordings) through SatDump
//! (ADR-0034, `docs/plugins-weather.md`).
//!
//! The plugin takes the complex baseband of a VFO (`cs16`, 192 kS/s, 144 kHz wide) and watches its
//! level. When a satellite's carrier rises above the noise ([`detect`]) the adapter writes the IQ
//! of the pass to a file, and when the signal is gone it hands the file to SatDump (GPL-3.0, a
//! separate process), which demodulates and decodes it and renders the images. The images, their
//! thumbnails and `pass.json` land in a directory of the weather store ([`products`]), which the
//! API serves as the gallery. Leaving the decoder running on a VFO at 137.900 or 137.100 MHz is
//! therefore all it takes to record every pass; nothing retunes the receiver.
//!
//! A pass that was still being decoded (or recorded) when the instance stopped is finished the
//! next time an instance starts: the IQ file and a sidecar stay in `<store>/.work/`.

pub mod detect;
pub mod products;

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::event::event;
use detect::{BLOCK_BYTES, BLOCK_SECONDS, Detector, Params, SAMPLE_RATE, Verdict, power_db};
use products::PassMeta;

/// Environment variable naming the weather store (shared with the API).
pub const DIR_ENV: &str = "SUPERHET_WEATHER_DIR";
/// Environment variable naming an alternative `satdump` (a file, or a directory containing it).
pub const PATH_ENV: &str = "SUPERHET_SATDUMP_PATH";
/// Where the store is without [`DIR_ENV`] (a directory of the recordings volume).
pub const DEFAULT_DIR: &str = "/data/recordings/weather";
/// Name of the SatDump program.
const TOOL: &str = "satdump";
/// Seconds of signal kept from before the detector was sure.
const PREROLL_BLOCKS: usize = 4;
/// SatDump gets this long for one pass.
const DECODE_TIMEOUT: Duration = Duration::from_secs(40 * 60);
/// How long a stopping instance waits for a decode in progress (the host allows 5 s).
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);
/// Leftovers of dead instances are removed after this long.
const STALE_MS: i64 = 24 * 3600 * 1000;

/// Which decoder pipeline to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pipeline {
    /// Meteor-M N2-x LRPT, 72 ksym/s QPSK.
    Lrpt72,
    /// Meteor-M N2-x LRPT, 80 ksym/s.
    Lrpt80,
    /// NOAA APT (no satellite transmits it any more; for recordings).
    Apt,
}

impl Pipeline {
    /// SatDump's name of the pipeline.
    #[must_use]
    pub fn satdump_name(self) -> &'static str {
        match self {
            Self::Lrpt72 => "meteor_m2-x_lrpt",
            Self::Lrpt80 => "meteor_m2-x_lrpt_80k",
            Self::Apt => "noaa_apt",
        }
    }

    /// `lrpt` or `apt`, as stored in `pass.json`.
    #[must_use]
    pub fn kind(self) -> &'static str {
        match self {
            Self::Lrpt72 | Self::Lrpt80 => "lrpt",
            Self::Apt => "apt",
        }
    }
}

/// Instance configuration (`plugins/weather/schema/config.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub pipeline: Pipeline,
    /// The satellite the user expects, used when SatDump does not name one.
    pub satellite: Option<String>,
    /// dB over the noise floor that count as a signal.
    pub threshold_db: f64,
    /// Shortest recording that is decoded (seconds).
    pub min_seconds: u32,
    /// Longest recording; the pass is cut there (minutes).
    pub max_minutes: u32,
    /// How long the signal must stay up before recording is certain (seconds).
    pub arm_seconds: u32,
    /// How long it must stay down before the pass is over (seconds).
    pub hang_seconds: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pipeline: Pipeline::Lrpt72,
            satellite: None,
            threshold_db: 3.0,
            min_seconds: 60,
            max_minutes: 20,
            arm_seconds: 5,
            hang_seconds: 30,
        }
    }
}

impl Config {
    /// Read the instance configuration (`null` or an object). Unknown keys are refused.
    ///
    /// # Errors
    /// A message naming the offending key.
    pub fn from_json(config: &Value) -> Result<Self, String> {
        let map = match config {
            Value::Null => return Ok(Self::default()),
            Value::Object(map) => map,
            _ => return Err("config must be an object".into()),
        };
        let mut out = Self::default();
        let mut rate = "72k";
        let mut apt = false;
        let int = |key: &str, value: &Value, lo: u32, hi: u32| {
            value
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| (lo..=hi).contains(v))
                .ok_or(format!("{key} must be a whole number from {lo} to {hi}"))
        };
        for (key, value) in map {
            match key.as_str() {
                "pipeline" => match value.as_str() {
                    Some("lrpt") => apt = false,
                    Some("apt") => apt = true,
                    _ => return Err("pipeline must be \"lrpt\" or \"apt\"".into()),
                },
                "symbol_rate" => match value.as_str() {
                    Some("72k") => rate = "72k",
                    Some("80k") => rate = "80k",
                    _ => return Err("symbol_rate must be \"72k\" or \"80k\"".into()),
                },
                "satellite" => {
                    let name = value
                        .as_str()
                        .map(str::trim)
                        .filter(|s| (1..=64).contains(&s.chars().count()))
                        .ok_or("satellite must be a name of 1 to 64 characters")?;
                    out.satellite = Some(name.to_owned());
                }
                "threshold_db" => {
                    out.threshold_db = value
                        .as_f64()
                        .filter(|v| (0.5..=30.0).contains(v))
                        .ok_or("threshold_db must be a number from 0.5 to 30")?;
                }
                "min_seconds" => out.min_seconds = int(key, value, 3, 900)?,
                "max_minutes" => out.max_minutes = int(key, value, 2, 40)?,
                "arm_seconds" => out.arm_seconds = int(key, value, 1, 60)?,
                "hang_seconds" => out.hang_seconds = int(key, value, 1, 300)?,
                other => return Err(format!("unknown config key {other:?}")),
            }
        }
        out.pipeline = match (apt, rate) {
            (true, _) => Pipeline::Apt,
            (false, "80k") => Pipeline::Lrpt80,
            (false, _) => Pipeline::Lrpt72,
        };
        Ok(out)
    }

    fn params(&self) -> Params {
        let blocks = |seconds: u32| (f64::from(seconds) / BLOCK_SECONDS).ceil() as usize;
        Params {
            threshold_db: self.threshold_db,
            arm_blocks: blocks(self.arm_seconds).max(1),
            hang_blocks: blocks(self.hang_seconds).max(1),
        }
    }
}

/// Where things are, from the plugin's environment.
#[derive(Debug, Clone)]
pub struct Env {
    /// The weather store.
    pub store: PathBuf,
    /// Location of `satdump`: a path, or the bare name looked up on `PATH`.
    pub satdump: PathBuf,
    pub instance: String,
    pub device: Option<String>,
    pub freq_hz: Option<u64>,
}

impl Env {
    /// Read the environment the plugin host sets.
    #[must_use]
    pub fn from_process() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Self {
            store: var(DIR_ENV).map_or_else(|| PathBuf::from(DEFAULT_DIR), PathBuf::from),
            satdump: locate(var(PATH_ENV).as_deref()),
            instance: var("SUPERHET_INSTANCE_ID").unwrap_or_else(|| "manual".into()),
            device: var("SUPERHET_DEVICE"),
            freq_hz: var("SUPERHET_FREQ_HZ").and_then(|v| v.parse().ok()),
        }
    }

    fn work(&self) -> PathBuf {
        self.store.join(".work")
    }
}

/// `SUPERHET_SATDUMP_PATH` (a file, or a directory containing the tool), else `satdump` on `PATH`.
#[must_use]
pub fn locate(given: Option<&str>) -> PathBuf {
    match given.map(str::trim).filter(|g| !g.is_empty()) {
        Some(g) if Path::new(g).is_dir() => Path::new(g).join(TOOL),
        Some(g) => PathBuf::from(g),
        None => PathBuf::from(TOOL),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn emit(kind: &str, data: Value) {
    let Value::Object(map) = data else {
        return;
    };
    let line = event(kind, map);
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Arguments of one SatDump run on a baseband file.
#[must_use]
pub fn satdump_args(
    pipeline: Pipeline,
    input: &Path,
    out_dir: &Path,
    sample_rate: u32,
    format: &str,
) -> Vec<String> {
    vec![
        pipeline.satdump_name().to_owned(),
        "baseband".to_owned(),
        input.display().to_string(),
        out_dir.display().to_string(),
        "--samplerate".to_owned(),
        sample_rate.to_string(),
        "--baseband_format".to_owned(),
        format.to_owned(),
    ]
}

/// How a SatDump run ended.
#[derive(Debug, PartialEq, Eq)]
enum ToolResult {
    Ok,
    Missing,
    Failed(String),
}

fn tail_of(path: &Path, lines: usize) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    // SatDump colours its log with escape sequences.
    let clean: String = {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    };
    let all: Vec<&str> = clean.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn run_satdump(env: &Env, args: &[String], work: &Path, log: &Path) -> ToolResult {
    let Ok(log_file) = File::create(log) else {
        return ToolResult::Failed(format!("cannot write {}", log.display()));
    };
    let Ok(log_err) = log_file.try_clone() else {
        return ToolResult::Failed("cannot duplicate the log handle".into());
    };
    let spawned = Command::new(&env.satdump)
        .args(args)
        // SatDump keeps a user configuration below $HOME; give it a place that is ours.
        .env("HOME", work)
        .current_dir(work)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return ToolResult::Missing,
        Err(err) => return ToolResult::Failed(format!("cannot start satdump: {err}")),
    };
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return ToolResult::Ok,
            Ok(Some(status)) => {
                return ToolResult::Failed(format!(
                    "satdump ended with {status}: {}",
                    tail_of(log, 6)
                ));
            }
            Ok(None) if started.elapsed() > DECODE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return ToolResult::Failed("satdump took too long".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(err) => return ToolResult::Failed(format!("waiting for satdump: {err}")),
        }
    }
}

/// What the IQ file of one pass is about.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentMeta {
    pub instance: String,
    pub device: Option<String>,
    pub started_ms: i64,
    /// `None` while recording (a crash leaves it so).
    pub ended_ms: Option<i64>,
    pub peak_db: Option<f64>,
    pub frequency_hz: Option<u64>,
    pub pipeline: Pipeline,
    pub satellite: Option<String>,
    pub sample_rate: u32,
    pub format: String,
}

impl SegmentMeta {
    fn to_json(&self) -> Value {
        json!({
            "instance": self.instance, "device": self.device, "started_ms": self.started_ms,
            "ended_ms": self.ended_ms, "peak_db": self.peak_db, "frequency_hz": self.frequency_hz,
            "pipeline": self.pipeline.satdump_name(), "satellite": self.satellite,
            "sample_rate": self.sample_rate, "format": self.format,
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        let pipeline = match v["pipeline"].as_str()? {
            "meteor_m2-x_lrpt" => Pipeline::Lrpt72,
            "meteor_m2-x_lrpt_80k" => Pipeline::Lrpt80,
            "noaa_apt" => Pipeline::Apt,
            _ => return None,
        };
        Some(Self {
            instance: v["instance"].as_str()?.to_owned(),
            device: v["device"].as_str().map(str::to_owned),
            started_ms: v["started_ms"].as_i64()?,
            ended_ms: v["ended_ms"].as_i64(),
            peak_db: v["peak_db"].as_f64(),
            frequency_hz: v["frequency_hz"].as_u64(),
            pipeline,
            satellite: v["satellite"].as_str().map(str::to_owned),
            sample_rate: u32::try_from(v["sample_rate"].as_u64()?).ok()?,
            format: v["format"].as_str()?.to_owned(),
        })
    }
}

/// `<unix seconds>-<4 hex digits>`: the name of a pass directory.
fn pass_id(started_ms: i64) -> String {
    let mix = u64::try_from(started_ms)
        .unwrap_or(0)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ u64::from(std::process::id());
    format!("{}-{:04x}", started_ms / 1000, (mix >> 24) & 0xffff)
}

/// The satellite SatDump reports in `dataset.json`, if it knows one.
fn satellite_of(out_dir: &Path) -> Option<String> {
    let doc: Value = serde_json::from_slice(&fs::read(out_dir.join("dataset.json")).ok()?).ok()?;
    let name = doc["satellite"].as_str()?.trim();
    (!name.is_empty() && !name.to_lowercase().starts_with("unknown")).then(|| name.to_owned())
}

/// Result of decoding one IQ file.
#[derive(Debug, PartialEq)]
pub enum Decoded {
    /// A pass directory with this many images was stored.
    Stored { id: String, images: usize },
    /// SatDump ran but there was nothing to show (a weak or no pass).
    NoImage,
    /// `satdump` is not installed.
    ToolMissing,
    /// SatDump failed.
    Failed(String),
}

/// Decode one IQ file with SatDump and store the images in the weather store. The IQ file and
/// the work directory are removed unless the tool is missing.
pub fn decode_file(env: &Env, iq: &Path, meta: &SegmentMeta) -> Decoded {
    let work = env.work();
    let stem = iq
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out_dir = work.join(format!("{stem}-out"));
    let log = work.join(format!("{stem}.log"));
    let _ = fs::remove_dir_all(&out_dir);
    let args = satdump_args(meta.pipeline, iq, &out_dir, meta.sample_rate, &meta.format);
    let result = run_satdump(env, &args, &work, &log);
    let outcome = match result {
        ToolResult::Missing => return Decoded::ToolMissing,
        ToolResult::Failed(reason) => Decoded::Failed(reason),
        ToolResult::Ok => store_products(env, &out_dir, meta),
    };
    let _ = fs::remove_dir_all(&out_dir);
    let _ = fs::remove_file(&log);
    let _ = fs::remove_file(iq);
    outcome
}

fn store_products(env: &Env, out_dir: &Path, meta: &SegmentMeta) -> Decoded {
    let id = pass_id(meta.started_ms);
    // Built under .work and renamed, so the gallery never sees a half-written pass.
    let staging = env.work().join(format!("{id}.stage"));
    let _ = fs::remove_dir_all(&staging);
    let images = match products::collect_images(out_dir, &staging) {
        Ok(images) => images,
        Err(err) => return Decoded::Failed(err),
    };
    if images.is_empty() {
        let _ = fs::remove_dir_all(&staging);
        return Decoded::NoImage;
    }
    let count = images.len();
    let pass = PassMeta {
        id: id.clone(),
        satellite: satellite_of(out_dir).or_else(|| meta.satellite.clone()),
        pipeline: meta.pipeline.kind().to_owned(),
        started_ms: meta.started_ms,
        ended_ms: meta.ended_ms.unwrap_or(meta.started_ms),
        frequency_hz: meta.frequency_hz,
        device: meta.device.clone(),
        instance: Some(meta.instance.clone()),
        peak_db: meta.peak_db,
        images,
    };
    if let Err(err) = pass.write(&staging) {
        return Decoded::Failed(err);
    }
    if let Err(err) = fs::rename(&staging, env.store.join(&id)) {
        return Decoded::Failed(format!("storing the pass: {err}"));
    }
    Decoded::Stored { id, images: count }
}

/// Decode a finished segment and report the outcome as an event.
fn finish_segment(env: &Env, iq: &Path, sidecar: &Path, meta: &SegmentMeta) {
    let seconds = meta
        .ended_ms
        .map_or(0, |e| (e - meta.started_ms).max(0) / 1000);
    let minutes = seconds as f64 / 60.0;
    match decode_file(env, iq, meta) {
        Decoded::Stored { id, images } => {
            let satellite = fs::read(env.store.join(&id).join("pass.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .and_then(|d| d["satellite"].as_str().map(str::to_owned));
            let name = satellite
                .clone()
                .unwrap_or_else(|| "unknown satellite".into());
            emit(
                "weather.pass",
                json!({
                    "summary": format!("{name}: {images} images in a {minutes:.0} min pass"),
                    "id": id, "satellite": satellite, "pipeline": meta.pipeline.kind(),
                    "started_ms": meta.started_ms, "duration_s": seconds, "images": images,
                    "peak_db": meta.peak_db.map(|d| (d * 10.0).round() / 10.0),
                    "frequency_hz": meta.frequency_hz,
                }),
            );
            let _ = fs::remove_file(sidecar);
        }
        Decoded::NoImage => {
            emit(
                "weather.failed",
                json!({
                    "summary": format!("no image could be decoded from a {minutes:.0} min recording (weak or interfered signal)"),
                    "reason": "no_image", "started_ms": meta.started_ms, "duration_s": seconds,
                }),
            );
            let _ = fs::remove_file(sidecar);
        }
        Decoded::ToolMissing => {
            emit(
                "weather.failed",
                json!({
                    "summary": "satdump is not installed; the recording is kept and decoded when it is",
                    "reason": "tool_missing", "started_ms": meta.started_ms, "duration_s": seconds,
                }),
            );
        }
        Decoded::Failed(detail) => {
            emit(
                "weather.failed",
                json!({
                    "summary": "satdump failed on the recording",
                    "reason": "tool_failed", "detail": detail.chars().take(500).collect::<String>(),
                    "started_ms": meta.started_ms, "duration_s": seconds,
                }),
            );
            let _ = fs::remove_file(sidecar);
        }
    }
}

/// Segments left by an instance that stopped or crashed: decode ours, delete stale strangers'.
fn recover(env: &Env, cfg: &Config) {
    let Ok(entries) = fs::read_dir(env.work()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let iq = path.with_extension("cs16");
        let Some(mut meta) = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| SegmentMeta::from_json(&v))
        else {
            continue;
        };
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(now_ms(), |d| i64::try_from(d.as_millis()).unwrap_or(0));
        if now_ms() - modified > STALE_MS {
            let _ = fs::remove_file(&iq);
            let _ = fs::remove_file(&path);
            continue;
        }
        if meta.instance != env.instance {
            continue;
        }
        let bytes_per_s = u64::from(meta.sample_rate) * 4;
        let length = fs::metadata(&iq).map_or(0, |m| m.len());
        let ended = meta
            .ended_ms
            .unwrap_or(meta.started_ms + i64::try_from(length / bytes_per_s * 1000).unwrap_or(0));
        meta.ended_ms = Some(ended);
        if (ended - meta.started_ms) / 1000 < i64::from(cfg.min_seconds) || !iq.is_file() {
            let _ = fs::remove_file(&iq);
            let _ = fs::remove_file(&path);
            continue;
        }
        finish_segment(env, &iq, &path, &meta);
    }
}

/// A recording in progress.
struct Segment {
    iq: PathBuf,
    sidecar: PathBuf,
    writer: BufWriter<File>,
    meta: SegmentMeta,
    blocks: usize,
}

fn write_sidecar(path: &Path, meta: &SegmentMeta) {
    let _ = fs::write(path, meta.to_json().to_string());
}

/// Read up to one block; returns the bytes read (0 at the end of the input).
fn read_block(input: &mut impl Read, buf: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buf.len() {
        match input.read(&mut buf[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => filled += n,
        }
    }
    filled
}

/// Run on a stream of `cs16` IQ until it ends.
pub fn run_stream(cfg: &Config, env: &Env, mut input: impl Read) -> ExitCode {
    let work = env.work();
    if let Err(err) = fs::create_dir_all(&work) {
        eprintln!(
            "cannot use the weather store {}: {err} (mount the recordings volume into the plugin host)",
            env.store.display()
        );
        return ExitCode::from(2);
    }
    let max_blocks = (f64::from(cfg.max_minutes) * 60.0 / BLOCK_SECONDS) as usize;
    let params = cfg.params();
    let mut detector = Detector::new(params);
    let mut ring: VecDeque<Vec<u8>> = VecDeque::new();
    let ring_len = params.arm_blocks + PREROLL_BLOCKS;
    let mut segment: Option<Segment> = None;
    let mut buf = vec![0u8; BLOCK_BYTES];

    // Passes interrupted earlier are finished in the background while we listen.
    let mut decoder: Option<std::thread::JoinHandle<()>> = {
        let (env, cfg) = (env.clone(), cfg.clone());
        Some(std::thread::spawn(move || recover(&env, &cfg)))
    };
    eprintln!(
        "weather: waiting for a satellite (threshold {:.1} dB over the noise, store {})",
        cfg.threshold_db,
        env.store.display()
    );

    loop {
        let n = read_block(&mut input, &mut buf) / 4 * 4;
        if n == 0 {
            break;
        }
        let block = &buf[..n];
        let verdict = detector.push(power_db(block));
        if segment.is_none() {
            ring.push_back(block.to_vec());
            while ring.len() > ring_len {
                ring.pop_front();
            }
        }
        let mut forced_end = false;
        match verdict {
            Verdict::Begin if segment.is_none() => {
                let started_ms = now_ms() - (ring.len() as f64 * BLOCK_SECONDS * 1000.0) as i64;
                let stem = format!("{}-{started_ms}", env.instance);
                let iq = work.join(format!("{stem}.cs16"));
                let sidecar = work.join(format!("{stem}.json"));
                let meta = SegmentMeta {
                    instance: env.instance.clone(),
                    device: env.device.clone(),
                    started_ms,
                    ended_ms: None,
                    peak_db: None,
                    frequency_hz: env.freq_hz,
                    pipeline: cfg.pipeline,
                    satellite: cfg.satellite.clone(),
                    sample_rate: SAMPLE_RATE,
                    format: "cs16".into(),
                };
                match File::create(&iq) {
                    Ok(file) => {
                        let mut writer = BufWriter::with_capacity(1 << 20, file);
                        let mut failed = false;
                        for chunk in &ring {
                            failed |= writer.write_all(chunk).is_err();
                        }
                        write_sidecar(&sidecar, &meta);
                        if failed {
                            eprintln!("weather: cannot write {}", iq.display());
                        }
                        let blocks = ring.len();
                        ring.clear();
                        segment = Some(Segment {
                            iq,
                            sidecar,
                            writer,
                            meta,
                            blocks,
                        });
                        emit(
                            "weather.recording",
                            json!({"summary": "signal found, recording the pass", "started_ms": started_ms}),
                        );
                    }
                    Err(err) => eprintln!("weather: cannot create {}: {err}", iq.display()),
                }
            }
            _ => {
                if let Some(seg) = segment.as_mut() {
                    // The block that ended the pass is part of it.
                    if seg.writer.write_all(block).is_err() {
                        eprintln!("weather: write failed (disk full?), ending the recording");
                        forced_end = true;
                    }
                    seg.blocks += 1;
                    if seg.blocks >= max_blocks {
                        forced_end = true;
                    }
                }
            }
        }
        let end = match verdict {
            Verdict::End { peak_db } => Some(Some(peak_db)),
            _ if forced_end => Some(None),
            _ => None,
        };
        if let Some(peak) = end
            && let Some(mut seg) = segment.take()
        {
            let _ = seg.writer.flush();
            drop(seg.writer);
            seg.meta.ended_ms = Some(now_ms());
            seg.meta.peak_db = peak;
            if forced_end {
                detector.reset_idle();
            }
            let seconds = (seg.blocks as f64 * BLOCK_SECONDS) as u32;
            if seconds < cfg.min_seconds {
                eprintln!("weather: signal of {seconds} s is too short, dropped");
                let _ = fs::remove_file(&seg.iq);
                let _ = fs::remove_file(&seg.sidecar);
            } else {
                write_sidecar(&seg.sidecar, &seg.meta);
                // One decode at a time; the next pass waits for the previous one.
                if let Some(previous) = decoder.take() {
                    let _ = previous.join();
                }
                let env = env.clone();
                decoder = Some(std::thread::spawn(move || {
                    finish_segment(&env, &seg.iq, &seg.sidecar, &seg.meta);
                }));
            }
        }
    }

    // The input ended (the host asked us to stop). A pass under way stays on disk as it is and
    // is decoded by the next instance; there is no time to decode now.
    if let Some(mut seg) = segment.take() {
        let _ = seg.writer.flush();
        drop(seg.writer);
        seg.meta.ended_ms = Some(now_ms());
        write_sidecar(&seg.sidecar, &seg.meta);
        eprintln!("weather: stopped during a pass; the recording is decoded at the next start");
    }
    // A decode that is nearly done may finish; the host kills us 5 s after it closed the input, so
    // wait only briefly. What is cut off stays in the work directory and is redone later.
    if let Some(d) = decoder.take() {
        let until = std::time::Instant::now() + SHUTDOWN_WAIT;
        while !d.is_finished() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        if d.is_finished() {
            let _ = d.join();
        } else {
            eprintln!("weather: still decoding; it is finished at the next start");
        }
    }
    ExitCode::SUCCESS
}

/// Entry of the plugin: parse the config and run on stdin.
///
/// # Errors
/// A bad configuration.
pub fn start(config: &Value) -> Result<ExitCode, String> {
    let cfg = Config::from_json(config)?;
    let env = Env::from_process();
    Ok(run_stream(&cfg, &env, std::io::stdin().lock()))
}

/// `superhet-plugin-adapter weather-decode [--pipeline lrpt|lrpt80|apt] [--format cs16|cu8|cf32]
/// --rate N [--satellite NAME] [--freq HZ] FILE`: decode a recording that was not made by the
/// plugin (an IQ file from the recordings library, or a capture of an APT source) into the
/// gallery.
///
/// # Errors
/// A usage problem.
pub fn decode_cli(args: &[String]) -> Result<ExitCode, String> {
    let mut pipeline = Pipeline::Lrpt72;
    let mut format = "cs16".to_owned();
    let mut rate: Option<u32> = None;
    let mut satellite = None;
    let mut freq = None;
    let mut file = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |what: &str| it.next().cloned().ok_or(format!("{what} needs a value"));
        match arg.as_str() {
            "--pipeline" => {
                pipeline = match value("--pipeline")?.as_str() {
                    "lrpt" => Pipeline::Lrpt72,
                    "lrpt80" => Pipeline::Lrpt80,
                    "apt" => Pipeline::Apt,
                    other => return Err(format!("unknown pipeline {other:?} (lrpt, lrpt80, apt)")),
                };
            }
            "--format" => {
                format = value("--format")?;
                if !["cs16", "cu8", "cf32", "cs8"].contains(&format.as_str()) {
                    return Err("format must be cs16, cu8, cs8 or cf32".into());
                }
            }
            "--rate" => {
                rate = Some(
                    value("--rate")?
                        .parse()
                        .map_err(|_| "--rate: not a number")?,
                );
            }
            "--satellite" => satellite = Some(value("--satellite")?),
            "--freq" => {
                freq = Some(
                    value("--freq")?
                        .parse()
                        .map_err(|_| "--freq: not a number")?,
                );
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => file = Some(PathBuf::from(other)),
        }
    }
    let file = file.ok_or("usage: weather-decode [--pipeline P] [--format F] --rate N FILE")?;
    let rate = rate.ok_or("--rate (the sample rate of the file in Hz) is required")?;
    let mut env = Env::from_process();
    env.instance = "weather-decode".into();
    fs::create_dir_all(env.work()).map_err(|e| format!("{}: {e}", env.work().display()))?;
    let started_ms = fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(now_ms(), |d| i64::try_from(d.as_millis()).unwrap_or(0));
    let meta = SegmentMeta {
        instance: env.instance.clone(),
        device: None,
        started_ms,
        ended_ms: Some(started_ms),
        peak_db: None,
        frequency_hz: freq,
        pipeline,
        satellite,
        sample_rate: rate,
        format,
    };
    // The tool must not delete the user's file: decode a hard link or copy under .work.
    let link = env
        .work()
        .join(format!("import-{started_ms}.{}", meta.format));
    let _ = fs::remove_file(&link);
    if fs::hard_link(&file, &link).is_err() {
        fs::copy(&file, &link).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    match decode_file(&env, &link, &meta) {
        Decoded::Stored { id, images } => {
            println!("stored pass {id} with {images} images");
            Ok(ExitCode::SUCCESS)
        }
        Decoded::NoImage => {
            eprintln!("satdump ran, but no image came out of this recording");
            Ok(ExitCode::from(1))
        }
        Decoded::ToolMissing => {
            let _ = fs::remove_file(&link);
            Err(format!("satdump not found ({})", env.satdump.display()))
        }
        Decoded::Failed(reason) => Err(reason),
    }
}

#[cfg(test)]
mod tests;
