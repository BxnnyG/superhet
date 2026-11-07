//! The TETRA plugin (`plugins/tetra`, `docs/plugins-tetra.md`, ADR-0021): a framed-transport
//! adapter that demodulates the 25 kHz channel itself and hands the bits to `tetra-rx` of
//! osmo-tetra-sq5bpf (AGPL-3.0, a separate process, shipped in the image). Voice goes through the
//! ETSI speech codec **the user installs** (`SUPERHET_TETRA_CODEC_DIR`); the codec is not part of
//! this repository, the image or any release and the adapter never downloads or builds it
//! (ADR-0005, ADR-0018).
//!
//! ```text
//!  host (cf32 IQ, 144 kHz, `samples` frames) --> phy::Demod --> bits --> tetra-rx  stdin
//!  host <-- event / status <-- call::Tracker <-- UDP datagrams (TETRA_HACK_PORT) <-- tetra-rx
//!  host <-- audio (48 kHz) <-- resampler <-- sdecoder <-- cdecoder <-- voice blocks (same UDP)
//! ```
//!
//! Without the codec the plugin still decodes everything else and says so in its status
//! document (`codec.state = "missing"`; the web UI shows "voice needs the ETSI codec"). Without
//! `tetra-rx` it stays idle (`tool.state = "missing"`).

pub mod call;
pub mod phy;
pub mod tetmon;

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use superhet_dsp::resample::Resampler;

use crate::dab::link::{self, Link};
use crate::m17::{HEARTBEAT_EVERY, INPUT_QUEUE, TICK, handle, on_path, unix_ms};
use call::{Emit, Tracker, Voice};
use tetmon::{Message, parse_datagram, parse_sysinfo_line};

/// Name of the decoder, looked up on `PATH`.
pub const TOOL: &str = "tetra-rx";
/// Environment variable with the full path of the decoder (the plugin host passes it through).
pub const TOOL_PATH_ENV: &str = "SUPERHET_TETRA_RX_PATH";
/// Environment variable with the directory holding the user's ETSI codec binaries.
pub const CODEC_DIR_ENV: &str = "SUPERHET_TETRA_CODEC_DIR";
/// Channel decoder of the ETSI reference codec (EN 300 395-2).
pub const CHANNEL_DECODER: &str = "cdecoder";
/// Speech decoder of the ETSI reference codec.
pub const SPEECH_DECODER: &str = "sdecoder";
/// Voice blocks waiting for the codec (about 3 s).
const VOICE_QUEUE: usize = 64;
/// Last lines of the decoder's output kept for the error report when it dies.
const KEEP_LINES: usize = 6;
/// The status document is refreshed this often while in sync (carrier offset).
const STATUS_REFRESH: Duration = Duration::from_secs(5);

/// Instance configuration (`plugins/tetra/schema/config.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Decode voice when the codec is installed. `false`: signalling only.
    pub audio: bool,
    /// Do not play the audio of encrypted calls.
    pub mute_encrypted: bool,
    /// Conjugate the input (a receiver chain that mirrors the spectrum).
    pub invert: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            audio: true,
            mute_encrypted: true,
            invert: false,
        }
    }
}

impl Config {
    /// # Errors
    /// An unknown key or a value of the wrong type.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let mut config = Self::default();
        let Some(object) = value.as_object() else {
            return if value.is_null() {
                Ok(config)
            } else {
                Err("config must be an object".to_owned())
            };
        };
        for (key, value) in object {
            let flag = value
                .as_bool()
                .ok_or_else(|| format!("config.{key} must be true or false"));
            match key.as_str() {
                "audio" => config.audio = flag?,
                "mute_encrypted" => config.mute_encrypted = flag?,
                "invert" => config.invert = flag?,
                other => return Err(format!("unknown config key {other:?}")),
            }
        }
        Ok(config)
    }
}

/// Where `tetra-rx` is: `SUPERHET_TETRA_RX_PATH` (a file, or a directory containing it), then
/// `PATH`. `Err` lists what was looked at.
///
/// # Errors
/// The places that were searched.
pub fn locate_tool(
    env_path: Option<&str>,
    path_var: Option<&OsStr>,
) -> Result<PathBuf, Vec<String>> {
    let mut searched = Vec::new();
    if let Some(given) = env_path.map(str::trim).filter(|p| !p.is_empty()) {
        let given = PathBuf::from(given);
        let file = if given.is_dir() {
            given.join(TOOL)
        } else {
            given
        };
        if file.is_file() {
            return Ok(file);
        }
        searched.push(format!("{TOOL_PATH_ENV}={} (no such file)", file.display()));
    }
    let mut on_path = false;
    if let Some(path) = path_var {
        for dir in std::env::split_paths(path) {
            on_path = true;
            let candidate = dir.join(TOOL);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    searched.push(if on_path {
        format!("{TOOL} on PATH")
    } else {
        "PATH (empty)".to_owned()
    });
    Err(searched)
}

/// The two codec programs in `SUPERHET_TETRA_CODEC_DIR`. `Err` lists what is missing.
///
/// # Errors
/// What was missing or not executable.
pub fn locate_codec(dir: Option<&str>) -> Result<(PathBuf, PathBuf), Vec<String>> {
    let Some(dir) = dir.map(str::trim).filter(|d| !d.is_empty()) else {
        return Err(vec![format!("{CODEC_DIR_ENV} is not set")]);
    };
    let dir = PathBuf::from(dir);
    if !dir.is_dir() {
        return Err(vec![format!(
            "{CODEC_DIR_ENV}={} (not a directory)",
            dir.display()
        )]);
    }
    let mut missing = Vec::new();
    let mut found = Vec::new();
    for name in [CHANNEL_DECODER, SPEECH_DECODER] {
        let file = dir.join(name);
        if is_executable(&file) {
            found.push(file);
        } else {
            missing.push(format!("{} (not an executable file)", file.display()));
        }
    }
    match (found.pop(), found.pop(), missing.is_empty()) {
        (Some(speech), Some(channel), true) => Ok((channel, speech)),
        _ => Err(missing),
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn tool_status(
    state: &str,
    path: Option<&Path>,
    searched: &[String],
    detail: Option<&str>,
) -> Value {
    let mut tool = json!({ "state": state, "name": TOOL });
    if let Some(path) = path {
        tool["path"] = json!(path.display().to_string());
    }
    if !searched.is_empty() {
        tool["searched"] = json!(searched);
    }
    if let Some(detail) = detail {
        tool["detail"] = json!(detail);
    }
    if state == "missing" {
        tool["message"] = json!(
            "tetra-rx (osmo-tetra) is not installed. The Superhet image contains it; outside the \
             image install it yourself, see docs/plugins-tetra.md."
        );
    }
    tool
}

fn codec_status(
    state: &str,
    dir: Option<&Path>,
    searched: &[String],
    detail: Option<&str>,
) -> Value {
    let mut codec = json!({ "state": state });
    if let Some(dir) = dir {
        codec["dir"] = json!(dir.display().to_string());
    }
    if !searched.is_empty() {
        codec["searched"] = json!(searched);
    }
    if let Some(detail) = detail {
        codec["detail"] = json!(detail);
    }
    if state == "missing" {
        codec["message"] = json!(
            "Voice needs the ETSI TETRA codec (cdecoder and sdecoder), which you install yourself: \
             it is not part of Superhet and is never downloaded by it. Signalling is decoded \
             without it. See docs/plugins-tetra.md."
        );
    }
    codec
}

struct Shared {
    link: Arc<Link>,
    tracker: Mutex<Tracker>,
    tool: Value,
    codec: Mutex<Value>,
    /// Blocks for the codec; `None` without a (working) codec.
    voice_tx: Mutex<Option<mpsc::SyncSender<Vec<u8>>>>,
    /// Carrier offset the demodulator measured, Hz.
    offset_hz: AtomicI32,
    epoch: Instant,
    stopping: AtomicBool,
    tool_died: AtomicBool,
    recent: Mutex<VecDeque<String>>,
}

impl Shared {
    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn tracker(&self) -> std::sync::MutexGuard<'_, Tracker> {
        self.tracker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn status_doc(&self, tracker: &Tracker) -> Value {
        let codec = self.codec.lock().map(|c| c.clone()).unwrap_or(Value::Null);
        tracker.status(
            &self.tool,
            &codec,
            Some(self.offset_hz.load(Ordering::Relaxed)),
        )
    }

    /// Send events and, when anything changed, the new status. Called with the tracker lock held
    /// so the host sees the events in the order they happened.
    fn publish(&self, tracker: &mut Tracker, emits: &[Emit]) {
        let dirty = tracker.take_dirty();
        if emits.is_empty() && !dirty {
            return;
        }
        for emit in emits {
            let _ = self.link.event(emit.kind, &emit.data);
        }
        let _ = self.link.status(&self.status_doc(tracker));
    }

    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.link.close();
    }

    fn codec_failed(&self, detail: &str) {
        if let Ok(mut codec) = self.codec.lock() {
            let dir = codec.get("dir").cloned();
            *codec = codec_status("failed", None, &[], Some(detail));
            if let Some(dir) = dir {
                codec["dir"] = dir;
            }
        }
        if let Ok(mut tx) = self.voice_tx.lock() {
            *tx = None;
        }
        let _ = self.link.log(&format!("codec stopped: {detail}"));
        let mut tracker = self.tracker();
        let status = self.status_doc(&tracker);
        tracker.take_dirty();
        let _ = self.link.status(&status);
    }
}

/// Run the plugin: connect to the host, serve until it goes away.
#[must_use]
pub fn run(config: &Value, socket: &str) -> ExitCode {
    let config = match Config::from_json(config) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let (link, mut incoming, _hello) = match link::connect(socket) {
        Ok(parts) => parts,
        Err(err) => {
            eprintln!("cannot reach the host at {socket}: {err}");
            return ExitCode::from(2);
        }
    };
    let link = Arc::new(link);
    let codec_dir = std::env::var(CODEC_DIR_ENV).ok();
    let codec_state = if config.audio {
        match locate_codec(codec_dir.as_deref()) {
            Ok(paths) => Ok(paths),
            Err(searched) => Err(codec_status("missing", None, &searched, None)),
        }
    } else {
        Err(codec_status("disabled", None, &[], None))
    };
    let program = match locate_tool(
        std::env::var(TOOL_PATH_ENV).ok().as_deref(),
        std::env::var_os("PATH").as_deref(),
    ) {
        Ok(program) => program,
        Err(searched) => {
            let codec = codec_state
                .as_ref()
                .map_or_else(Clone::clone, |_| codec_status("ready", None, &[], None));
            return idle_without_tool(&link, &mut incoming, &config, &searched, &codec);
        }
    };
    let socket_udp = match UdpSocket::bind(("127.0.0.1", 0)) {
        Ok(socket) => socket,
        Err(err) => {
            eprintln!("cannot open the datagram socket for {TOOL}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let port = socket_udp.local_addr().map_or(0, |a| a.port());
    let tool = tool_status("ready", Some(&program), &[], None);
    let mut child = match spawn_decoder(&program, port) {
        Ok(child) => child,
        Err(err) => {
            let _ = link.log(&err);
            eprintln!("{err}");
            return ExitCode::from(127);
        }
    };
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        return ExitCode::FAILURE;
    };
    let codec_json = match &codec_state {
        Ok((channel, _)) => codec_status("ready", channel.parent(), &[], None),
        Err(doc) => doc.clone(),
    };
    let shared = Arc::new(Shared {
        link: Arc::clone(&link),
        tracker: Mutex::new(Tracker::new(config.mute_encrypted)),
        tool,
        codec: Mutex::new(codec_json),
        voice_tx: Mutex::new(None),
        offset_hz: AtomicI32::new(0),
        epoch: Instant::now(),
        stopping: AtomicBool::new(false),
        tool_died: AtomicBool::new(false),
        recent: Mutex::new(VecDeque::new()),
    });
    let mut codec_children: Vec<Child> = Vec::new();
    let mut codec_threads = Vec::new();
    if let Ok((channel, speech)) = &codec_state {
        match start_codec(channel, speech, &shared) {
            Ok((tx, children, threads)) => {
                if let Ok(mut slot) = shared.voice_tx.lock() {
                    *slot = Some(tx);
                }
                codec_children = children;
                codec_threads = threads;
            }
            Err(err) => {
                if let Ok(mut codec) = shared.codec.lock() {
                    *codec = codec_status("failed", channel.parent(), &[], Some(&err));
                }
                let _ = link.log(&err);
            }
        }
    }
    {
        let mut tracker = shared.tracker();
        let _ = link.status(&shared.status_doc(&tracker));
        tracker.take_dirty();
    }

    let (input, input_rx) = mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE);
    let demodulator = {
        let shared = Arc::clone(&shared);
        let invert = config.invert;
        std::thread::spawn(move || demodulate(&input_rx, stdin, invert, &shared))
    };
    let datagrams = {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || listen(&socket_udp, &shared))
    };
    let console = {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || watch_stdout(stdout, &shared))
    };
    let errors = {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || watch_stderr(stderr, &shared))
    };
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let mut last_beat = Instant::now();
            let mut last_status = Instant::now();
            while !shared.stopping.load(Ordering::SeqCst) {
                std::thread::sleep(TICK);
                if last_beat.elapsed() >= HEARTBEAT_EVERY {
                    if shared.link.heartbeat().is_err() {
                        return;
                    }
                    last_beat = Instant::now();
                }
                let mut tracker = shared.tracker();
                let emits = tracker.tick(shared.now());
                if last_status.elapsed() >= STATUS_REFRESH {
                    last_status = Instant::now();
                    let _ = shared.link.status(&shared.status_doc(&tracker));
                }
                shared.publish(&mut tracker, &emits);
            }
        });
    }
    {
        let shared = Arc::clone(&shared);
        crate::dsd::watch_stdin(move || shared.stop());
    }

    loop {
        match incoming.read_frame() {
            Ok(Some(frame)) => handle(&frame, &input),
            Ok(None) => break,
            Err(err) => {
                if !shared.stopping.load(Ordering::SeqCst) {
                    eprintln!("host connection: {err}");
                }
                break;
            }
        }
    }
    shared.stopping.store(true, Ordering::SeqCst);
    drop(input);
    let _ = demodulator.join();
    if let Ok(mut tx) = shared.voice_tx.lock() {
        *tx = None;
    }
    let status = wait_or_kill(&mut child);
    for c in &mut codec_children {
        let _ = wait_or_kill(c);
    }
    let _ = datagrams.join();
    let _ = console.join();
    let _ = errors.join();
    for t in codec_threads {
        let _ = t.join();
    }
    {
        let mut tracker = shared.tracker();
        let emits = tracker.shutdown(shared.now());
        shared.publish(&mut tracker, &emits);
    }
    if shared.tool_died.load(Ordering::SeqCst) {
        let recent: Vec<String> = shared
            .recent
            .lock()
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default();
        eprintln!(
            "{TOOL} ended with {status:?}; last output: {}",
            recent.join(" | ")
        );
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn wait_or_kill(child: &mut Child) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                return child.wait().ok();
            }
        }
    }
}

/// Start `tetra-rx`: bits on stdin, datagrams to `port` on the loopback interface. The console is
/// piped for the system information line; `stdbuf -oL` makes it arrive line by line.
fn spawn_decoder(program: &Path, port: u16) -> Result<Child, String> {
    let attempt = |mut command: Command| {
        command
            .env("TETRA_HACK_PORT", port.to_string())
            .env("TETRA_HACK_IP", "127.0.0.1")
            .env("TETRA_HACK_RXID", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    };
    // -r: reassemble fragmented PDUs. No -e: encrypted PDUs are not interpreted.
    let args = ["-r", "/dev/stdin"];
    if let Some(stdbuf) = on_path("stdbuf") {
        let mut command = Command::new(stdbuf);
        command.arg("-oL").arg(program).args(args);
        if let Ok(child) = attempt(command) {
            return Ok(child);
        }
    }
    let mut command = Command::new(program);
    command.args(args);
    attempt(command).map_err(|err| format!("cannot start {TOOL}: {err}"))
}

/// IQ from the host to bits for the decoder.
fn demodulate(
    input: &mpsc::Receiver<Vec<u8>>,
    mut stdin: ChildStdin,
    invert: bool,
    shared: &Shared,
) {
    let mut demod = phy::Demod::new(invert);
    let (mut carry, mut iq, mut bits) = (Vec::new(), Vec::new(), Vec::new());
    for chunk in input {
        iq.clear();
        bits.clear();
        phy::parse_cf32(&mut carry, &chunk, &mut iq);
        demod.process(&iq, &mut bits);
        shared
            .offset_hz
            .store(demod.offset_hz().round() as i32, Ordering::Relaxed);
        if stdin.write_all(&bits).is_err() {
            break;
        }
    }
    // The input ended or the decoder is gone; either way its stdin is closed now.
}

/// Datagrams of the decoder: cell and call news, and voice.
fn listen(socket: &UdpSocket, shared: &Shared) {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let mut buf = vec![0u8; 4096];
    while !shared.stopping.load(Ordering::SeqCst) {
        let Ok((n, _)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let Some(message) = parse_datagram(&buf[..n]) else {
            continue;
        };
        let (now, wall) = (shared.now(), unix_ms());
        match message {
            Message::Voice { usage, block } => {
                let mut tracker = shared.tracker();
                let (action, emits) = tracker.voice(usage, now, wall);
                if action == Voice::Play {
                    let tx = shared.voice_tx.lock().ok().and_then(|tx| tx.clone());
                    if let Some(tx) = tx {
                        // A full queue drops the block: audio must not lag behind the air.
                        if !matches!(
                            tx.try_send(block),
                            Err(TrySendError::Full(_) | TrySendError::Disconnected(_))
                        ) {
                            tracker.played(usage);
                        }
                    }
                }
                shared.publish(&mut tracker, &emits);
            }
            other => {
                let mut tracker = shared.tracker();
                let emits = tracker.message(other, now, wall);
                shared.publish(&mut tracker, &emits);
            }
        }
    }
}

/// The decoder's console: the system information line carries the service details.
fn watch_stdout(stdout: impl Read, shared: &Shared) {
    let mut reader = BufReader::new(stdout);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&raw);
        if let Some(info) = parse_sysinfo_line(&text) {
            let mut tracker = shared.tracker();
            let emits = tracker.sysinfo(info, shared.now());
            shared.publish(&mut tracker, &emits);
        }
    }
    // The decoder closed its output: it is gone (or was asked to end).
    if !shared.stopping.load(Ordering::SeqCst) {
        shared.tool_died.store(true, Ordering::SeqCst);
        shared.stop();
    }
}

fn watch_stderr(stderr: impl Read, shared: &Shared) {
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }
        if let Ok(mut recent) = shared.recent.lock() {
            if recent.len() == KEEP_LINES {
                recent.pop_front();
            }
            recent.push_back(line);
        }
    }
}

/// The user's codec: `cdecoder` feeds `sdecoder` through a pipe. Blocks go in as the decoder sent
/// them; 8 kHz speech comes out and is resampled for the host.
type CodecStart = (
    mpsc::SyncSender<Vec<u8>>,
    Vec<Child>,
    Vec<std::thread::JoinHandle<()>>,
);

fn start_codec(channel: &Path, speech: &Path, shared: &Arc<Shared>) -> Result<CodecStart, String> {
    let spawn = |program: &Path, stdin: Stdio| {
        Command::new(program)
            .args(["/dev/stdin", "/dev/stdout"])
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("cannot start {}: {err}", program.display()))
    };
    let mut first = spawn(channel, Stdio::piped())?;
    let (Some(first_in), Some(first_out)) = (first.stdin.take(), first.stdout.take()) else {
        let _ = first.kill();
        return Err("no pipes to the channel decoder".to_owned());
    };
    let mut second = match spawn(speech, Stdio::from(first_out)) {
        Ok(child) => child,
        Err(err) => {
            let _ = first.kill();
            return Err(err);
        }
    };
    let Some(second_out) = second.stdout.take() else {
        let _ = first.kill();
        let _ = second.kill();
        return Err("no pipe from the speech decoder".to_owned());
    };
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(VOICE_QUEUE);
    let writer = std::thread::spawn(move || {
        let mut stdin = first_in;
        for block in rx {
            if stdin
                .write_all(&block)
                .and_then(|()| stdin.flush())
                .is_err()
            {
                return;
            }
        }
    });
    let reader = {
        let shared = Arc::clone(shared);
        std::thread::spawn(move || play(second_out, &shared))
    };
    Ok((tx, vec![first, second], vec![writer, reader]))
}

/// Speech from the codec (8 kHz s16le mono) to 48 kHz s16le for the host.
fn play(mut speech: impl Read, shared: &Shared) {
    let mut resampler = Resampler::new(8000.0, 48_000.0);
    let mut buf = vec![0u8; 4096];
    let mut carry: Option<u8> = None;
    let mut input: Vec<f32> = Vec::new();
    let mut output: Vec<f32> = Vec::new();
    loop {
        let n = match speech.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        input.clear();
        let mut bytes = carry.take().into_iter().chain(buf[..n].iter().copied());
        while let (Some(low), high) = (bytes.next(), bytes.next()) {
            match high {
                Some(high) => input.push(f32::from(i16::from_le_bytes([low, high])) / 32768.0),
                None => carry = Some(low),
            }
        }
        output.clear();
        resampler.process(&input, &mut output);
        let pcm: Vec<u8> = output
            .iter()
            .flat_map(|v| ((v * 32767.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes())
            .collect();
        if !pcm.is_empty() && shared.link.audio(&pcm).is_err() {
            return;
        }
    }
    if !shared.stopping.load(Ordering::SeqCst) {
        shared.codec_failed("the codec programs ended");
    }
}

/// `tetra-rx` is not installed: report it and wait to be stopped. The plugin does not exit, so
/// the host's supervisor does not restart it in a loop.
fn idle_without_tool(
    link: &Arc<Link>,
    incoming: &mut link::Incoming,
    config: &Config,
    searched: &[String],
    codec: &Value,
) -> ExitCode {
    let _ = link.log(&format!(
        "{TOOL} is not installed (looked for: {}); see docs/plugins-tetra.md",
        searched.join(", ")
    ));
    let tracker = Tracker::new(config.mute_encrypted);
    let status = tracker.status(&tool_status("missing", None, searched, None), codec, None);
    let _ = link.status(&status);
    let stopping = Arc::new(AtomicBool::new(false));
    {
        let link = Arc::clone(link);
        let stopping = Arc::clone(&stopping);
        std::thread::spawn(move || {
            let mut last_beat = Instant::now();
            let mut last_status = Instant::now();
            while !stopping.load(Ordering::SeqCst) {
                std::thread::sleep(TICK);
                if last_beat.elapsed() >= HEARTBEAT_EVERY {
                    if link.heartbeat().is_err() {
                        return;
                    }
                    last_beat = Instant::now();
                }
                if last_status.elapsed() >= Duration::from_secs(30) {
                    let _ = link.status(&status);
                    last_status = Instant::now();
                }
            }
        });
    }
    {
        let link = Arc::clone(link);
        crate::dsd::watch_stdin(move || link.close());
    }
    // The samples the host sends have nowhere to go.
    while let Ok(Some(_)) = incoming.read_frame() {}
    stopping.store(true, Ordering::SeqCst);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_play_clear_audio_and_keep_the_spectrum() {
        let config = Config::from_json(&Value::Null).expect("null is fine");
        assert_eq!(config, Config::default());
        assert!(config.audio && config.mute_encrypted && !config.invert);
        let config = Config::from_json(&json!({"audio": false, "invert": true})).expect("valid");
        assert!(!config.audio && config.invert);
    }

    #[test]
    fn bad_config_is_refused() {
        for bad in [
            json!({"audio": "yes"}),
            json!({"gain": 2}),
            json!({"codec": "/bin/sh"}),
            json!([1]),
        ] {
            assert!(Config::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_decoder_is_found_by_variable_then_path() {
        let root =
            std::env::temp_dir().join(format!("superhet-tetra-locate-{}", std::process::id()));
        let on_path = root.join("path");
        std::fs::create_dir_all(&on_path).expect("dir");
        let path_var = std::env::join_paths([&on_path]).expect("path");
        let err = locate_tool(None, Some(&path_var)).expect_err("nothing yet");
        assert_eq!(err.len(), 1);
        std::fs::write(on_path.join(TOOL), b"").expect("file");
        assert_eq!(locate_tool(None, Some(&path_var)), Ok(on_path.join(TOOL)));
        let explicit = root.join("custom-rx");
        std::fs::write(&explicit, b"").expect("file");
        assert_eq!(
            locate_tool(explicit.to_str(), Some(&path_var)),
            Ok(explicit.clone())
        );
        assert_eq!(
            locate_tool(Some("/nonexistent/rx"), Some(&path_var)),
            Ok(on_path.join(TOOL))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_codec_needs_both_programs_executable() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("superhet-tetra-codec-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("dir");
        let dir = root.to_str().map(str::to_owned);
        assert_eq!(locate_codec(None).expect_err("unset").len(), 1);
        assert_eq!(
            locate_codec(Some("/nonexistent"))
                .expect_err("no dir")
                .len(),
            1
        );
        assert_eq!(locate_codec(dir.as_deref()).expect_err("empty").len(), 2);
        for name in [CHANNEL_DECODER, SPEECH_DECODER] {
            std::fs::write(root.join(name), b"").expect("file");
        }
        // Present but not executable.
        assert_eq!(
            locate_codec(dir.as_deref())
                .expect_err("not executable")
                .len(),
            2
        );
        for name in [CHANNEL_DECODER, SPEECH_DECODER] {
            std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        assert_eq!(
            locate_codec(dir.as_deref()),
            Ok((root.join(CHANNEL_DECODER), root.join(SPEECH_DECODER)))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
