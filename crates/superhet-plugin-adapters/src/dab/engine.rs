//! The DAB engine: owns the `welle-cli` process of the tuned block, polls its web interface,
//! turns what it learns into events / status / media for the host, plays the selected service
//! and runs the ensemble scan.
//!
//! One thread runs the engine; commands arrive over a channel (from the host's `control`
//! frames), the audio of the selected service is decoded in its own thread.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::audio;
use super::channels;
use super::dwell::{Dwell, Timing, Verdict};
use super::http;
use super::link::Link;
use super::mux::{self, Mux, Service};
use super::rtl_tcp::FakeRtl;

/// Largest slide the host stores.
const MAX_SLIDE: usize = 256 * 1024;
/// How often the engine polls welle-cli.
const POLL: Duration = Duration::from_millis(700);
/// IQ is dropped for this long after a retune (samples of the old block are still in flight).
const SETTLE: Duration = Duration::from_millis(800);
/// Name of the decoder binary.
const WELLE: &str = "welle-cli";
/// welle-cli lines forwarded to the host log per run.
const LOG_LINES: usize = 60;

/// What the host can ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// Receive block `channel`.
    Tune(&'static str),
    /// Play service `sid` (or stop with `None`).
    Select(Option<u32>),
    /// Scan these blocks one after another.
    Scan(Vec<&'static str>),
    /// End a running scan.
    ScanStop,
    /// The host is done with the plugin.
    Shutdown,
}

/// Read a `control` message of the host.
///
/// # Errors
/// What is wrong with the message.
pub fn parse_control(message: &Value) -> Result<Cmd, String> {
    let name = message["cmd"].as_str().ok_or("\"cmd\" is required")?;
    match name {
        "tune" => {
            let raw = message["channel"]
                .as_str()
                .ok_or("\"channel\" is required")?;
            channels::canonical(raw)
                .map(Cmd::Tune)
                .ok_or_else(|| format!("unknown DAB block {raw:?}"))
        }
        "select" => match &message["service"] {
            Value::Null => Ok(Cmd::Select(None)),
            Value::String(raw) => u32::from_str_radix(raw.trim_start_matches("0x"), 16)
                .map(|sid| Cmd::Select(Some(sid)))
                .map_err(|_| format!("service {raw:?} is not a hexadecimal service id")),
            _ => Err("\"service\" must be a hexadecimal string or null".to_owned()),
        },
        "scan" => match &message["channels"] {
            Value::Null => Ok(Cmd::Scan(
                channels::BLOCKS.iter().map(|&(n, _)| n).collect(),
            )),
            Value::Array(list) => {
                let mut blocks = Vec::new();
                for item in list {
                    let raw = item.as_str().ok_or("channels must be strings")?;
                    let block = channels::canonical(raw)
                        .ok_or_else(|| format!("unknown DAB block {raw:?}"))?;
                    if !blocks.contains(&block) {
                        blocks.push(block);
                    }
                }
                if blocks.is_empty() {
                    Err("channels is empty".to_owned())
                } else {
                    Ok(Cmd::Scan(blocks))
                }
            }
            _ => Err("\"channels\" must be a list".to_owned()),
        },
        "scan_stop" => Ok(Cmd::ScanStop),
        other => Err(format!("unknown command {other:?}")),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A running welle-cli.
struct Welle {
    child: Child,
    web: SocketAddr,
}

impl Welle {
    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// The audio thread of the selected service.
struct AudioTask {
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    thread: JoinHandle<()>,
}

impl AudioTask {
    fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(socket) = self
            .socket
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        let _ = self.thread.join();
    }
}

/// One ensemble found by the scan.
#[derive(Debug, Clone)]
struct Found {
    channel: &'static str,
    freq_hz: u64,
    label: String,
    eid: String,
    ecc: String,
    services: usize,
    snr: f64,
}

impl Found {
    fn json(&self) -> Value {
        json!({
            "channel": self.channel, "freq_hz": self.freq_hz, "label": self.label,
            "eid": self.eid, "ecc": self.ecc, "services": self.services, "snr": self.snr,
        })
    }
}

struct Scan {
    plan: Vec<&'static str>,
    index: usize,
    found: Vec<Found>,
    return_to: Option<&'static str>,
    started_ms: u64,
}

/// Everything the engine needs from the outside.
pub struct Setup {
    /// Link to the host.
    pub link: Arc<Link>,
    /// The stand-in rtl_tcp server welle-cli reads its IQ from.
    pub fake: Arc<FakeRtl>,
    /// Drops incoming IQ while set (after a retune).
    pub muted: Arc<AtomicBool>,
    /// Block to start on.
    pub channel: Option<&'static str>,
    /// Centre frequency the device is on now (the host tuned it for the instance).
    pub current_freq_hz: Option<u64>,
    /// Service to play from the start.
    pub service: Option<u32>,
    /// Timing of the "is there an ensemble" decision.
    pub timing: Timing,
    /// Program to start (`welle-cli`).
    pub program: String,
}

/// The engine.
pub struct Engine {
    s: Setup,
    channel: Option<&'static str>,
    freq_hz: Option<u64>,
    welle: Option<Welle>,
    tuned_at: Instant,
    dwell: Dwell,
    announced: bool,
    mux: Option<Mux>,
    poll_failures: u32,
    selected: Option<u32>,
    audio: Option<AudioTask>,
    audio_retry_at: Instant,
    last_dls: HashMap<u32, String>,
    last_slide: HashMap<u32, i64>,
    scan: Option<Scan>,
    last_scan: Option<(u64, Vec<Found>)>,
    error: Option<String>,
    respawns: u32,
    /// Counts retunes, so a poll answered for the old block can be told from the new one.
    generation: u64,
}

impl Engine {
    /// A new engine, idle until [`Self::run`] tunes it.
    #[must_use]
    pub fn new(setup: Setup) -> Self {
        let dwell = Dwell::new(setup.timing);
        let (selected, freq_hz) = (setup.service, setup.current_freq_hz);
        Self {
            selected,
            s: setup,
            channel: None,
            freq_hz,
            welle: None,
            tuned_at: Instant::now(),
            dwell,
            announced: false,
            mux: None,
            poll_failures: 0,
            audio: None,
            audio_retry_at: Instant::now(),
            last_dls: HashMap::new(),
            last_slide: HashMap::new(),
            scan: None,
            last_scan: None,
            error: None,
            respawns: 0,
            generation: 0,
        }
    }

    /// Run until [`Cmd::Shutdown`] arrives or the channel closes.
    pub fn run(mut self, commands: &Receiver<Cmd>) {
        if let Some(channel) = self.s.channel {
            self.tune(channel);
        }
        self.send_status();
        loop {
            match commands.recv_timeout(POLL) {
                Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => {
                    self.command(cmd);
                    self.send_status();
                }
                Err(RecvTimeoutError::Timeout) => self.tick(),
            }
        }
        self.stop_all();
    }

    fn event(&self, kind: &str, data: &Value) {
        let _ = self.s.link.event(kind, data);
    }

    fn fail(&mut self, message: String) {
        if self.error.as_deref() != Some(message.as_str()) {
            self.event(
                "dab.error",
                &json!({"summary": message, "message": message}),
            );
            let _ = self.s.link.log(&message);
        }
        self.error = Some(message);
    }

    fn stop_audio(&mut self) {
        if let Some(task) = self.audio.take() {
            task.stop();
        }
    }

    fn stop_welle(&mut self) {
        if let Some(welle) = self.welle.take() {
            welle.stop();
        }
        self.s.fake.disconnect();
    }

    fn stop_all(&mut self) {
        self.stop_audio();
        self.stop_welle();
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Tune(channel) => {
                self.cancel_scan(false);
                if self.channel != Some(channel) {
                    self.selected = None;
                }
                self.tune(channel);
            }
            Cmd::Select(sid) => {
                self.stop_audio();
                self.selected = sid;
                self.audio_retry_at = Instant::now();
                if let Some(sid) = sid {
                    self.announce_play(sid);
                }
                self.tick_audio();
            }
            Cmd::Scan(plan) => self.start_scan(plan),
            Cmd::ScanStop => self.cancel_scan(true),
            Cmd::Shutdown => {}
        }
    }

    /// Retune the device to `channel` and start a fresh welle-cli on it.
    fn tune(&mut self, channel: &'static str) {
        let Some(freq) = channels::freq_of(channel) else {
            return;
        };
        self.stop_audio();
        self.stop_welle();
        if self.freq_hz != Some(freq) {
            self.s.muted.store(true, Ordering::SeqCst);
            if self.s.link.tune(freq).is_err() {
                return;
            }
            std::thread::sleep(SETTLE);
            self.s.muted.store(false, Ordering::SeqCst);
        }
        self.channel = Some(channel);
        self.freq_hz = Some(freq);
        self.generation += 1;
        self.respawns = 0;
        self.mux = None;
        self.poll_failures = 0;
        self.announced = false;
        self.dwell = Dwell::new(self.s.timing);
        self.last_dls.clear();
        self.last_slide.clear();
        self.tuned_at = Instant::now();
        self.spawn_welle();
    }

    fn spawn_welle(&mut self) {
        let Some(channel) = self.channel else { return };
        let web = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|l| l.local_addr()) {
            Ok(addr) => addr,
            Err(err) => return self.fail(format!("no free port for welle-cli: {err}")),
        };
        let spawned = Command::new(&self.s.program)
            .args([
                "-c",
                channel,
                "-F",
                &format!("rtl_tcp,127.0.0.1:{}", self.s.fake.port()),
                "-w",
                &web.port().to_string(),
                // No transmitter identification: saves CPU, the plugin does not show it.
                "-T",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        match spawned {
            Ok(mut child) => {
                if let Some(stderr) = child.stderr.take() {
                    let link = Arc::clone(&self.s.link);
                    std::thread::spawn(move || {
                        for line in BufReader::new(stderr).lines().take(LOG_LINES).flatten() {
                            let _ = link.log(&format!("{WELLE}: {line}"));
                        }
                    });
                }
                self.error = None;
                self.welle = Some(Welle {
                    child,
                    web: SocketAddr::from((Ipv4Addr::LOCALHOST, web.port())),
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.fail(format!(
                    "{} is not installed (Debian package welle.io)",
                    self.s.program
                ));
            }
            Err(err) => self.fail(format!("cannot start {}: {err}", self.s.program)),
        }
    }

    fn start_scan(&mut self, plan: Vec<&'static str>) {
        self.cancel_scan(false);
        self.stop_audio();
        let total = plan.len();
        self.event(
            "dab.scan",
            &json!({"summary": format!("Scan of {total} DAB blocks started"), "state": "started", "total": total, "found": 0}),
        );
        let return_to = self.channel;
        let first = plan[0];
        self.scan = Some(Scan {
            plan,
            index: 0,
            found: Vec::new(),
            return_to,
            started_ms: now_ms(),
        });
        self.selected = None;
        self.tune(first);
    }

    /// End the scan. With `restore` the receiver returns to the block the scan started on
    /// (stop button, end of the plan); without it the caller is about to tune elsewhere.
    fn cancel_scan(&mut self, restore: bool) {
        let Some(scan) = self.scan.take() else { return };
        let total = scan.plan.len();
        let found = scan.found.len();
        let state = if scan.index >= total {
            "finished"
        } else {
            "stopped"
        };
        let summary = format!(
            "Scan {state}: {found} ensemble(s) in {} of {total} blocks",
            scan.index.min(total)
        );
        self.event(
            "dab.scan",
            &json!({"summary": summary, "state": state, "total": total, "found": found}),
        );
        self.last_scan = Some((now_ms(), scan.found));
        if restore {
            match scan.return_to {
                Some(back) if self.channel != Some(back) => self.tune(back),
                None => {
                    self.stop_all();
                    self.channel = None;
                    self.mux = None;
                }
                Some(_) => {}
            }
        }
    }

    fn announce_play(&self, sid: u32) {
        let Some(mux) = &self.mux else { return };
        let Some(service) = mux.services.iter().find(|s| s.sid == sid) else {
            return;
        };
        let channel = self.channel.unwrap_or("");
        self.event(
            "dab.play",
            &json!({
                "summary": format!("{} on {channel}", service.label),
                "sid": service.sid_hex, "service": service.label, "channel": channel,
                "ensemble": mux.ensemble.label,
            }),
        );
    }

    fn poll(&mut self) -> Option<Mux> {
        let welle = self.welle.as_ref()?;
        let timeout = Duration::from_secs(2);
        let body = http::get(welle.web, "/mux.json", timeout)
            .map_err(|err| err.to_string())
            .and_then(|(head, body)| {
                if head.status == 200 {
                    Ok(body)
                } else {
                    Err(format!("HTTP {}", head.status))
                }
            })
            .and_then(|body| mux::parse(&body));
        match body {
            Ok(mux) => {
                self.poll_failures = 0;
                Some(mux)
            }
            Err(_) => {
                self.poll_failures += 1;
                None
            }
        }
    }

    fn tick(&mut self) {
        // welle-cli died on its own (bad IQ, crash): start it again, with a limit.
        if let Some(welle) = self.welle.as_mut()
            && !welle.running()
        {
            self.welle = None;
            self.stop_audio();
            self.respawns += 1;
            if self.respawns <= 5 {
                let _ = self
                    .s
                    .link
                    .log("welle-cli ended unexpectedly; restarting it");
                self.spawn_welle();
            } else {
                self.fail("welle-cli keeps crashing; select the block again to retry".to_owned());
            }
        }
        if self.welle.is_some() {
            if let Some(mux) = self.poll() {
                let generation = self.generation;
                if self.scan.is_some() || !self.announced {
                    self.judge(&mux);
                }
                // The judgement may have moved the scan on; the snapshot belongs to the old block.
                if generation == self.generation {
                    self.after_poll(&mux);
                    self.mux = Some(mux);
                }
            } else if self.poll_failures >= 20 {
                // The web interface never came up or went away.
                self.poll_failures = 0;
                self.fail("welle-cli does not answer".to_owned());
                self.stop_welle();
                self.stop_audio();
                self.advance_scan_on_failure();
            }
            self.tick_audio();
        }
        self.send_status();
    }

    fn advance_scan_on_failure(&mut self) {
        if self.scan.is_some() {
            self.next_block();
        }
    }

    /// Decide whether the tuned block has an ensemble; announce it; move the scan on.
    fn judge(&mut self, mux: &Mux) {
        let verdict = self.dwell.observe(self.tuned_at.elapsed(), mux);
        match verdict {
            Verdict::Wait => {}
            Verdict::Empty => {
                if self.scan.is_some() {
                    self.next_block();
                }
            }
            Verdict::Found => {
                if !self.announced {
                    self.announced = true;
                    self.announce_ensemble(mux);
                }
                if self.scan.is_some() {
                    self.record_found(mux);
                    self.next_block();
                }
            }
        }
    }

    fn found_of(&self, mux: &Mux) -> Option<Found> {
        let channel = self.channel?;
        Some(Found {
            channel,
            freq_hz: channels::freq_of(channel)?,
            label: mux.ensemble.label.clone(),
            eid: mux.ensemble.eid.clone(),
            ecc: mux.ensemble.ecc.clone(),
            services: mux.services.len(),
            snr: mux.snr,
        })
    }

    fn record_found(&mut self, mux: &Mux) {
        if let (Some(found), Some(scan)) = (self.found_of(mux), self.scan.as_mut()) {
            scan.found.push(found);
        }
    }

    fn announce_ensemble(&self, mux: &Mux) {
        let Some(found) = self.found_of(mux) else {
            return;
        };
        let services: Vec<Value> = mux
            .services
            .iter()
            .map(|s| {
                json!({
                    "sid": s.sid_hex, "label": s.label, "codec": s.codec.label(),
                    "bitrate": s.bitrate, "pty": s.pty,
                })
            })
            .collect();
        let mut data = found.json();
        data["summary"] = json!(format!(
            "{}: {} ({} services)",
            found.channel,
            if found.label.is_empty() {
                &found.eid
            } else {
                &found.label
            },
            found.services
        ));
        data["services"] = Value::Array(services);
        data["origin"] = json!(if self.scan.is_some() { "scan" } else { "tune" });
        self.event("dab.ensemble", &data);
    }

    /// Next block of the scan, or the end of it.
    fn next_block(&mut self) {
        let Some(scan) = self.scan.as_mut() else {
            return;
        };
        scan.index += 1;
        match scan.plan.get(scan.index).copied() {
            Some(block) => self.tune(block),
            None => self.cancel_scan(true),
        }
        self.send_status();
    }

    /// DLS changes and slides of the selected service.
    fn after_poll(&mut self, mux: &Mux) {
        let Some(sid) = self.selected else { return };
        let Some(service) = mux.services.iter().find(|s| s.sid == sid) else {
            return;
        };
        if let Some(text) = &service.dls
            && self.last_dls.get(&sid) != Some(text)
        {
            self.last_dls.insert(sid, text.clone());
            self.event(
                "dab.dls",
                &json!({
                    "summary": format!("{}: {text}", service.label),
                    "text": text, "sid": service.sid_hex, "service": service.label,
                    "channel": self.channel,
                }),
            );
        }
        if service.mot_changed != 0 && self.last_slide.get(&sid) != Some(&service.mot_changed) {
            self.last_slide.insert(sid, service.mot_changed);
            self.fetch_slide(service);
        }
    }

    fn fetch_slide(&self, service: &Service) {
        let Some(welle) = self.welle.as_ref() else {
            return;
        };
        // welle accepts the service id in decimal as well as in its own `0x...` spelling.
        let path = format!("/slide/{}", service.sid);
        let Ok((head, body)) = http::get(welle.web, &path, Duration::from_secs(3)) else {
            return;
        };
        if head.status != 200 || body.is_empty() || body.len() > MAX_SLIDE {
            return;
        }
        let mime = head
            .header("content-type")
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or("application/octet-stream")
            .trim();
        let key = format!("slide.{}", service.sid_hex.to_ascii_lowercase());
        let _ = self.s.link.media(&key, mime, &body);
    }

    /// Start (or restart after a drop) the audio thread of the selected service.
    fn tick_audio(&mut self) {
        if self
            .audio
            .as_ref()
            .is_some_and(|task| task.thread.is_finished())
        {
            if let Some(task) = self.audio.take() {
                task.stop();
            }
            self.audio_retry_at = Instant::now() + Duration::from_secs(2);
        }
        let (Some(sid), Some(welle), Some(mux)) = (self.selected, self.welle.as_ref(), &self.mux)
        else {
            return;
        };
        if self.audio.is_some() || Instant::now() < self.audio_retry_at || self.scan.is_some() {
            return;
        }
        let Some(service) = mux.services.iter().find(|s| s.sid == sid && s.has_stream) else {
            return;
        };
        self.audio = Some(start_audio(&self.s.link, welle.web, service));
    }

    fn send_status(&self) {
        let _ = self.s.link.status(&self.status());
    }

    fn status(&self) -> Value {
        let mux = self.mux.as_ref();
        let selected = self.selected;
        let services: Vec<Value> = mux
            .into_iter()
            .flat_map(|m| m.services.iter())
            .map(|s| {
                let playing = Some(s.sid) == selected;
                json!({
                    "sid": s.sid_hex, "label": s.label, "pty": s.pty, "language": s.language,
                    "codec": s.codec.label(), "bitrate": s.bitrate, "protection": s.protection,
                    "audio": s.has_stream,
                    "dls": if playing { s.dls.clone() } else { None },
                    "audio_errors": if playing { Some(s.audio_errors) } else { None },
                })
            })
            .collect();
        let ensemble = mux.filter(|m| m.ensemble.known()).map(
            |m| json!({"label": m.ensemble.label, "eid": m.ensemble.eid, "ecc": m.ensemble.ecc}),
        );
        let scan = self.scan.as_ref().map(|scan| {
            json!({
                "active": true,
                "index": scan.index,
                "total": scan.plan.len(),
                "channel": scan.plan.get(scan.index),
                "started_unix_ms": scan.started_ms,
                "found": scan.found.iter().map(Found::json).collect::<Vec<_>>(),
            })
        });
        let last_scan = self.last_scan.as_ref().map(|(finished, found)| {
            json!({"finished_unix_ms": finished, "found": found.iter().map(Found::json).collect::<Vec<_>>()})
        });
        json!({
            "v": 1,
            "mode": if self.scan.is_some() { "scanning" } else if self.channel.is_some() { "tuned" } else { "idle" },
            "channel": self.channel,
            "freq_hz": self.freq_hz,
            "receiver_running": self.welle.is_some(),
            "synced": mux.is_some_and(|m| m.ensemble.known()),
            "snr": mux.map(|m| m.snr),
            "fic_crc_errors": mux.map(|m| m.fic_crc_errors),
            "freq_correction_hz": mux.map(|m| m.freq_correction_hz),
            "iq_connected": self.s.fake.connected(),
            "ensemble": ensemble,
            "services": services,
            "selected": selected.map(|sid| format!("{sid:04X}")),
            "playing": self.audio.is_some(),
            "scan": scan,
            "last_scan": last_scan,
            "error": self.error,
        })
    }
}

fn start_audio(link: &Arc<Link>, web: SocketAddr, service: &Service) -> AudioTask {
    let stop = Arc::new(AtomicBool::new(false));
    let socket: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));
    let thread = {
        let (link, stop, socket) = (Arc::clone(link), Arc::clone(&stop), Arc::clone(&socket));
        let path = format!("/mp3/{}", service.sid);
        std::thread::spawn(move || {
            let Ok((head, reader)) = http::open(
                web,
                &path,
                Duration::from_secs(3),
                Some(Duration::from_secs(15)),
            ) else {
                return;
            };
            if head.status != 200 {
                return;
            }
            *socket
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                reader.get_ref().try_clone().ok();
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let result = audio::decode_stream(reader, |pcm| {
                !stop.load(Ordering::SeqCst) && link.audio(pcm).is_ok()
            });
            if let Err(err) = result
                && !stop.load(Ordering::SeqCst)
            {
                let _ = link.log(&format!("audio stream ended: {err}"));
            }
        })
    };
    AudioTask {
        stop,
        socket,
        thread,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn control_messages_are_parsed_strictly() {
        assert_eq!(
            parse_control(&json!({"cmd": "tune", "channel": "5c"})),
            Ok(Cmd::Tune("5C"))
        );
        assert!(parse_control(&json!({"cmd": "tune", "channel": "99Z"})).is_err());
        assert!(parse_control(&json!({"cmd": "tune"})).is_err());
        assert_eq!(
            parse_control(&json!({"cmd": "select", "service": "d210"})),
            Ok(Cmd::Select(Some(0xD210)))
        );
        assert_eq!(
            parse_control(&json!({"cmd": "select", "service": null})),
            Ok(Cmd::Select(None))
        );
        assert!(parse_control(&json!({"cmd": "select", "service": "zz"})).is_err());
        assert!(parse_control(&json!({"cmd": "select", "service": 5})).is_err());
        assert_eq!(
            parse_control(&json!({"cmd": "scan_stop"})),
            Ok(Cmd::ScanStop)
        );
        match parse_control(&json!({"cmd": "scan"})) {
            Ok(Cmd::Scan(all)) => assert_eq!(all.len(), 38),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_control(&json!({"cmd": "scan", "channels": ["5c", "5C", "12D"]})),
            Ok(Cmd::Scan(vec!["5C", "12D"]))
        );
        assert!(parse_control(&json!({"cmd": "scan", "channels": []})).is_err());
        assert!(parse_control(&json!({"cmd": "scan", "channels": ["nope"]})).is_err());
        assert!(parse_control(&json!({"cmd": "fly"})).is_err());
        assert!(parse_control(&json!({"x": 1})).is_err());
    }
}
