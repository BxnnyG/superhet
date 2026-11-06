//! The M17 plugin (`plugins/m17`, ADR-0018): a framed-transport adapter around `m17-demod` from
//! mobilinkd's m17-cxx-demod (GPL-3.0, a separate process; Codec2 inside is open and patent-free).
//!
//! ```text
//!  host (48 kHz FM discriminator audio, s16le, in `samples` frames) --> m17-demod -l  stdin
//!  host <-- audio (48 kHz mono) <-- resampler <-- 8 kHz decoded voice <-- m17-demod stdout
//!  host <-- event / status       <-- call tracker <-- link setup frames, EOS <-- m17-demod stderr
//! ```
//!
//! `lsf` parses what the tool prints, `call` turns it into call start/end events and the status
//! document and decides whether audio may be played (encrypted calls are muted). The process
//! plumbing here is tested by `tests/m17_plugin.rs` against a fake host and the real tool.

pub mod call;
pub mod lsf;

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use superhet_dsp::resample::Resampler;
use superhet_plugin_protocol::framed::{Frame, FrameKind, SamplesHeader};

use crate::dab::link::{self, Link};
use call::{Emit, Tracker};
use lsf::Line;

/// Interval of the plugin's heartbeat frames (the host's watchdog allows 15 s).
pub(crate) const HEARTBEAT_EVERY: Duration = Duration::from_secs(3);
/// Sample chunks waiting for the demodulator (about 2.5 s at the daemon's 20 ms chunks).
pub(crate) const INPUT_QUEUE: usize = 128;
/// How often the call tracker is asked whether the signal is gone.
pub(crate) const TICK: Duration = Duration::from_millis(100);
/// Audio that arrives without a call waits this long for the link setup frame the tool prints
/// just before it (two pipes, two threads).
const LSF_GRACE: Duration = Duration::from_millis(50);
/// The end-of-stream marker waits this long for the voice of the last frames (other pipe).
const EOS_GRACE: Duration = Duration::from_millis(80);
/// Unrecognised lines of the tool forwarded to the host log (it prints a lot of progress).
const MAX_LOGGED_LINES: usize = 20;

/// Instance configuration (`plugins/m17/schema/config.json`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Silence frames the Viterbi decoder is unsure about instead of playing noise (`-b`).
    pub noise_blanker: bool,
    /// The received baseband has the opposite polarity (`-i`).
    pub invert: bool,
    /// Do not play the (noise) audio of encrypted calls.
    pub mute_encrypted: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            noise_blanker: true,
            invert: false,
            mute_encrypted: true,
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
            let flag = || {
                value
                    .as_bool()
                    .ok_or_else(|| format!("config.{key} must be true or false"))
            };
            match key.as_str() {
                "noise_blanker" => config.noise_blanker = flag()?,
                "invert" => config.invert = flag()?,
                "mute_encrypted" => config.mute_encrypted = flag()?,
                other => return Err(format!("unknown config key {other:?}")),
            }
        }
        Ok(config)
    }

    /// Arguments of `m17-demod`.
    #[must_use]
    pub fn args(self) -> Vec<String> {
        let mut args = vec!["-l".to_owned()];
        if self.noise_blanker {
            args.push("-b".to_owned());
        }
        if self.invert {
            args.push("-i".to_owned());
        }
        args
    }
}

pub(crate) fn on_path(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

/// Start the tool. `m17-demod` writes the decoded voice through C stdio, which buffers 4 KiB on a
/// pipe (a quarter of a second of audio): `stdbuf -o0` makes every frame leave at once.
pub(crate) fn spawn(program: &str, args: &[String]) -> Result<Child, String> {
    let Some(path) = on_path(program) else {
        return Err(format!("{program} is not installed (not found on PATH)"));
    };
    let attempt = |mut command: Command| {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    };
    if let Some(stdbuf) = on_path("stdbuf") {
        let mut command = Command::new(stdbuf);
        command.arg("-o0").arg(&path).args(args);
        if let Ok(child) = attempt(command) {
            return Ok(child);
        }
    }
    let mut command = Command::new(&path);
    command.args(args);
    attempt(command).map_err(|err| format!("cannot start {program}: {err}"))
}

struct Shared {
    link: Arc<Link>,
    tracker: Mutex<Tracker>,
    /// Signalled when the tracker learned about a call.
    call_changed: Condvar,
    epoch: Instant,
    /// Set when the adapter is going away (host closed, or the tool died).
    stopping: AtomicBool,
    /// The tool ended without being asked to.
    tool_died: AtomicBool,
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

    /// Send events and the new status. Called with the tracker lock held so the host sees the
    /// events in the order they happened.
    fn publish(&self, tracker: &Tracker, emits: &[Emit]) {
        if emits.is_empty() {
            return;
        }
        for emit in emits {
            let _ = self.link.event(emit.kind, &emit.data);
        }
        let _ = self.link.status(&tracker.status());
    }

    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Wakes the thread that reads the host socket.
        self.link.close();
    }
}

pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Run the plugin: connect to the host, serve until it goes away. `program` is the decoder to
/// start (`m17-demod`).
#[must_use]
pub fn run(program: &str, config: &Value, socket: &str) -> ExitCode {
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
    let mut child = match spawn(program, &config.args()) {
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
    let shared = Arc::new(Shared {
        link: Arc::clone(&link),
        tracker: Mutex::new(Tracker::new(config.mute_encrypted)),
        call_changed: Condvar::new(),
        epoch: Instant::now(),
        stopping: AtomicBool::new(false),
        tool_died: AtomicBool::new(false),
    });
    // Idle state for the first status document.
    let _ = link.status(&shared.tracker().status());

    // Samples to the tool, on their own thread so a slow tool never blocks the socket.
    let (input, input_rx) = mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE);
    let writer = std::thread::spawn(move || feed(stdin, &input_rx));
    let audio = {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || voice(stdout, &shared))
    };
    let lines = {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || watch_stderr(stderr, &shared))
    };
    // Heartbeats and the end of calls whose transmitter vanished.
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let mut last_beat = Instant::now();
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
                shared.publish(&tracker, &emits);
            }
        });
    }
    // The host closes stdin to ask the plugin to end.
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let mut sink = [0u8; 64];
            while std::io::stdin().read(&mut sink).is_ok_and(|n| n > 0) {}
            shared.stop();
        });
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
    let _ = writer.join();
    // The tool ends when its input does; give it a moment, then make sure.
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                break child.wait().ok();
            }
        }
    };
    let _ = audio.join();
    let _ = lines.join();
    {
        let mut tracker = shared.tracker();
        let emits = tracker.shutdown(shared.now());
        shared.publish(&tracker, &emits);
    }
    if !shared.tool_died.load(Ordering::SeqCst) {
        ExitCode::SUCCESS
    } else {
        // The tool died on its own: end with an error so the host restarts the plugin.
        eprintln!("{program} ended with {status:?}");
        ExitCode::FAILURE
    }
}

/// Samples from the host into the tool's stdin.
pub(crate) fn handle(frame: &Frame, input: &mpsc::SyncSender<Vec<u8>>) {
    if frame.kind != FrameKind::Samples {
        return;
    }
    if let Ok((_, samples)) = SamplesHeader::decode(&frame.payload) {
        match input.try_send(samples.to_vec()) {
            // Dropping on a full queue is the contract: a slow plugin loses samples.
            Ok(()) | Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {}
        }
    }
}

pub(crate) fn feed(mut stdin: ChildStdin, queue: &mpsc::Receiver<Vec<u8>>) {
    for chunk in queue {
        if stdin.write_all(&chunk).is_err() {
            return;
        }
    }
}

/// Decoded voice (8 kHz s16le mono) to 48 kHz s16le for the host.
fn voice(mut stdout: impl Read, shared: &Shared) {
    let mut resampler = Resampler::new(8000.0, 48_000.0);
    let mut buf = vec![0u8; 8192];
    let mut carry: Option<u8> = None;
    let mut input: Vec<f32> = Vec::new();
    let mut output: Vec<f32> = Vec::new();
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        input.clear();
        let mut bytes = buf[..n].iter().copied();
        if let Some(low) = carry.take() {
            if let Some(high) = bytes.next() {
                input.push(f32::from(i16::from_le_bytes([low, high])) / 32768.0);
            } else {
                carry = Some(low);
            }
        }
        loop {
            match (bytes.next(), bytes.next()) {
                (Some(low), Some(high)) => {
                    input.push(f32::from(i16::from_le_bytes([low, high])) / 32768.0);
                }
                (Some(low), None) => {
                    carry = Some(low);
                    break;
                }
                _ => break,
            }
        }
        if input.is_empty() {
            continue;
        }
        let play = {
            let mut tracker = shared.tracker();
            if tracker.awaiting_call(shared.now()) {
                tracker = shared
                    .call_changed
                    .wait_timeout_while(tracker, LSF_GRACE, |t| t.awaiting_call(shared.now()))
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
            tracker.audio(input.len() as u64, shared.now())
        };
        if !play {
            // Keep the filter state in step with the stream, send nothing.
            resampler.reset();
            continue;
        }
        output.clear();
        resampler.process(&input, &mut output);
        let pcm: Vec<u8> = output
            .iter()
            .flat_map(|v| {
                let sample = (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
                sample.to_le_bytes()
            })
            .collect();
        if !pcm.is_empty() && shared.link.audio(&pcm).is_err() {
            break;
        }
    }
    // The tool closed its output: it is gone (or was asked to end).
    if !shared.stopping.load(Ordering::SeqCst) {
        shared.tool_died.store(true, Ordering::SeqCst);
        shared.stop();
    }
}

/// The tool's diagnostics: link setup frames start calls, `EOS` ends them.
fn watch_stderr(stderr: impl Read, shared: &Shared) {
    let mut reader = BufReader::new(stderr);
    let mut raw = Vec::new();
    let mut logged = 0usize;
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&raw);
        // Progress output uses carriage returns inside a line.
        for part in text.split(['\r', '\n']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match lsf::parse_line(part) {
                Some(Line::Lsf(frame)) => {
                    let mut tracker = shared.tracker();
                    let emits = tracker.lsf(*frame, shared.now(), unix_ms());
                    shared.publish(&tracker, &emits);
                    shared.call_changed.notify_all();
                }
                Some(Line::EndOfStream) => {
                    // The last frame's audio may still be on its way on the other pipe.
                    std::thread::sleep(EOS_GRACE);
                    let mut tracker = shared.tracker();
                    let emits = tracker.end_of_stream(shared.now());
                    shared.publish(&tracker, &emits);
                }
                None => {
                    // Known chatter of the tool: LICH progress, the odd note on packet frames.
                    if part != "LICH"
                        && !part.starts_with("LSF for reserved")
                        && logged < MAX_LOGGED_LINES
                    {
                        logged += 1;
                        let _ = shared.link.log(part);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_blank_noise_and_mute_encrypted_calls() {
        let config = Config::from_json(&Value::Null).expect("null is fine");
        assert_eq!(config, Config::default());
        assert_eq!(config.args(), ["-l", "-b"]);
    }

    #[test]
    fn config_switches_map_to_tool_arguments() {
        let config =
            Config::from_json(&json!({"noise_blanker": false, "invert": true})).expect("valid");
        assert_eq!(config.args(), ["-l", "-i"]);
        assert!(config.mute_encrypted);
    }

    #[test]
    fn bad_config_is_refused() {
        assert!(Config::from_json(&json!({"invert": "yes"})).is_err());
        assert!(Config::from_json(&json!({"gain": 2})).is_err());
        assert!(Config::from_json(&json!([1])).is_err());
    }
}
