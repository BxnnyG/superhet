//! The ADS-B plugin (`plugins/adsb`, ADR-0017): a framed-transport adapter around `readsb`
//! (wiedehopf's fork of dump1090, GPL-3.0-or-later, a separate process).
//!
//! ```text
//!  host (cu8 IQ, 2.4 MS/s at 1090 MHz, `samples` frames) --> readsb --ifile - --iformat UC8
//!  readsb --write-json <dir> --> aircraft.json (about once a second)
//!  host <-- status (the live picture) / events (rationed) / heartbeat <-- this adapter
//! ```
//!
//! No sockets are involved on the decoder side: IQ goes in on stdin and the picture comes out as
//! a file that readsb replaces atomically. The parser ([`aircraft`]) and the event / status logic
//! ([`tracker`]) are plain functions, unit-tested on recorded `aircraft.json` files;
//! `tests/adsb_plugin.rs` runs the whole adapter against a fake host and a stand-in `readsb`.

pub mod aircraft;
pub mod tracker;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use superhet_plugin_protocol::framed::{Frame, FrameKind};

use crate::event::round5;
use crate::run::exit_code_of;
use tracker::Tracker;

/// Interval of the plugin's heartbeat frames (the host's watchdog allows 15 s).
const HEARTBEAT_EVERY: Duration = Duration::from_secs(3);
/// IQ chunks waiting for readsb (about 2.5 s at the host's chunk size); more are dropped.
const IQ_QUEUE: usize = 128;
/// Default time between two history events of one aircraft.
const DEFAULT_EVENT_INTERVAL_S: u64 = 60;

/// Settings from the instance configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Receiver position: lets readsb decode positions from a single message and puts a marker
    /// on the map.
    pub receiver: Option<(f64, f64)>,
    /// Shortest time between two history events of one aircraft.
    pub event_interval: Duration,
    /// How often `aircraft.json` is read (tests use less; readsb writes once a second).
    pub poll: Duration,
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
        if let Some(key) = map
            .keys()
            .find(|k| !["lat", "lon", "event_interval_s", "poll_ms"].contains(&k.as_str()))
        {
            return Err(format!(
                "unknown config key {key:?} (lat, lon, event_interval_s)"
            ));
        }
        let number = |key: &str, lo: f64, hi: f64| -> Result<Option<f64>, String> {
            match map.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => v
                    .as_f64()
                    .filter(|n| (lo..=hi).contains(n))
                    .map(Some)
                    .ok_or_else(|| format!("{key} must be a number from {lo} to {hi}")),
            }
        };
        let receiver = match (number("lat", -90.0, 90.0)?, number("lon", -180.0, 180.0)?) {
            (Some(lat), Some(lon)) => Some((round5(lat), round5(lon))),
            (None, None) => None,
            _ => return Err("lat and lon go together".into()),
        };
        let mut out = Self {
            receiver,
            ..Self::default()
        };
        if let Some(secs) = number("event_interval_s", 5.0, 86_400.0)? {
            out.event_interval = Duration::from_secs_f64(secs);
        }
        if let Some(ms) = number("poll_ms", 50.0, 10_000.0)? {
            out.poll = Duration::from_secs_f64(ms / 1000.0);
        }
        Ok(out)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            receiver: None,
            event_interval: Duration::from_secs(DEFAULT_EVENT_INTERVAL_S),
            poll: Duration::from_secs(1),
        }
    }
}

/// readsb's command line: IQ from stdin (`cu8`, 2.4 MS/s is the only rate it decodes), the
/// aircraft list written to `dir`, no network ports, no console output.
#[must_use]
pub fn readsb_args(dir: &Path, receiver: Option<(f64, f64)>) -> Vec<String> {
    let mut args: Vec<String> = [
        "--device-type",
        "ifile",
        "--ifile",
        "-",
        "--iformat",
        "UC8",
        // The input is a live stream: stamp messages with the wall clock rather than a clock
        // derived from the sample count (which only makes sense for files fed faster than real time).
        "--devel=ifile_no_synthetic",
        "--quiet",
        "--write-json",
    ]
    .map(String::from)
    .to_vec();
    args.push(dir.display().to_string());
    args.extend(["--write-json-every", "1"].map(String::from));
    if let Some((lat, lon)) = receiver {
        args.extend([
            "--lat".to_owned(),
            lat.to_string(),
            "--lon".to_owned(),
            lon.to_string(),
        ]);
    }
    args
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Run the plugin: connect to the host, start `program` (readsb) and serve until the host goes
/// away or the decoder dies.
#[must_use]
pub fn run(program: &str, config: &Value, socket: &str) -> ExitCode {
    let config = match Config::from_json(config) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let (link, mut incoming, hello) = match crate::dab::link::connect(socket) {
        Ok(parts) => parts,
        Err(err) => {
            eprintln!("cannot reach the host at {socket}: {err}");
            return ExitCode::from(2);
        }
    };
    let link = Arc::new(link);

    let dir = std::env::temp_dir().join(format!("superhet-adsb-{}", hello.instance_id));
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        let _ = link.log(&format!("cannot create {}: {err}", dir.display()));
        return ExitCode::FAILURE;
    }
    let mut child = match Command::new(program)
        .args(readsb_args(&dir, config.receiver))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            let _ = link.log(&format!(
                "cannot start {program}: {err} (is readsb installed?)"
            ));
            let _ = std::fs::remove_dir_all(&dir);
            return ExitCode::from(127);
        }
    };
    let Some(mut readsb_in) = child.stdin.take() else {
        let _ = child.kill();
        return ExitCode::FAILURE;
    };
    let child = Arc::new(Mutex::new(child));
    let stopping = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(false));

    // Heartbeats: the host kills a framed plugin that stays silent.
    {
        let (link, stopping) = (Arc::clone(&link), Arc::clone(&stopping));
        std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) && link.heartbeat().is_ok() {
                std::thread::sleep(HEARTBEAT_EVERY);
            }
        });
    }
    // The host closes stdin to ask the plugin to end.
    {
        let (link, stopping) = (Arc::clone(&link), Arc::clone(&stopping));
        std::thread::spawn(move || {
            let mut sink = [0u8; 64];
            while std::io::stdin().read(&mut sink).is_ok_and(|n| n > 0) {}
            stopping.store(true, Ordering::SeqCst);
            link.close();
        });
    }
    // IQ goes through its own thread so a slow readsb never blocks the control path.
    let (iq, iq_rx) = mpsc::sync_channel::<Vec<u8>>(IQ_QUEUE);
    std::thread::spawn(move || {
        for chunk in iq_rx {
            if readsb_in.write_all(&chunk).is_err() {
                return;
            }
        }
        // The channel closed: dropping `readsb_in` closes readsb's stdin, which ends it.
    });
    // The live picture.
    let poller = {
        let (link, child, stopping, failed) = (
            Arc::clone(&link),
            Arc::clone(&child),
            Arc::clone(&stopping),
            Arc::clone(&failed),
        );
        let file = dir.join("aircraft.json");
        std::thread::spawn(move || poll(&config, &file, &link, &child, &stopping, &failed))
    };

    loop {
        match incoming.read_frame() {
            Ok(Some(frame)) => handle(frame, &iq),
            Ok(None) => break,
            Err(err) => {
                eprintln!("host connection: {err}");
                break;
            }
        }
    }
    stopping.store(true, Ordering::SeqCst);
    drop(iq);
    let _ = poller.join();
    let status = {
        let mut child = child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = child.kill();
        child.wait().ok()
    };
    let _ = std::fs::remove_dir_all(&dir);
    if failed.load(Ordering::SeqCst) {
        return status.map_or(ExitCode::FAILURE, |s| exit_code_of(s, program));
    }
    ExitCode::SUCCESS
}

fn handle(frame: Frame, iq: &SyncSender<Vec<u8>>) {
    if frame.kind == FrameKind::Samples
        && let Ok((_, samples)) =
            superhet_plugin_protocol::framed::SamplesHeader::decode(&frame.payload)
    {
        // Dropping on a full queue is the contract: a slow plugin loses samples.
        let _ = iq.try_send(samples.to_vec());
    }
}

/// Read `aircraft.json` every `config.poll`, send the status and the due events, and watch
/// readsb. Ends when `stopping` is set or readsb exited (then `failed` is set and the socket
/// closed, which ends the adapter's main loop).
fn poll(
    config: &Config,
    file: &PathBuf,
    link: &crate::dab::link::Link,
    child: &Mutex<Child>,
    stopping: &AtomicBool,
    failed: &AtomicBool,
) {
    let mut tracker = Tracker::new(config.event_interval);
    let mut running = false;
    loop {
        if stopping.load(Ordering::Relaxed) {
            return;
        }
        if let Some(code) = exited(child) {
            let message = format!("readsb ended ({code})");
            let _ = link.log(&message);
            let _ = link.event(
                "adsb.error",
                &serde_json::json!({"summary": message, "message": message}),
            );
            let _ =
                link.status(&tracker.status("error", config.receiver, now_ms(), Some(&message)));
            failed.store(true, Ordering::SeqCst);
            link.close();
            return;
        }
        if let Some(snapshot) = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| aircraft::parse_snapshot(&text).ok())
        {
            running = true;
            for data in tracker.update(&snapshot, Instant::now()) {
                if link.event("adsb.aircraft", &Value::Object(data)).is_err() {
                    return;
                }
            }
        }
        let mode = if running { "running" } else { "starting" };
        if link
            .status(&tracker.status(mode, config.receiver, now_ms(), None))
            .is_err()
        {
            return;
        }
        std::thread::sleep(config.poll);
    }
}

/// A description of how readsb ended, if it did.
fn exited(child: &Mutex<Child>) -> Option<String> {
    let mut child = child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match child.try_wait() {
        Ok(Some(status)) => Some(status.to_string()),
        Ok(None) => None,
        Err(err) => Some(err.to_string()),
    }
}
