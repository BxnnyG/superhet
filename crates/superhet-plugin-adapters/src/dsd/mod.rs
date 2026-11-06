//! The DMR / dPMR / NXDN plugin (`plugins/dsd`, `docs/plugins-dsd.md`): a framed-transport adapter
//! around `dsd-fme`, which the **user installs**. The decoder (and with it the AMBE+2 / AMBE vocoder
//! `dsd-fme` links) is not part of this repository, the image or any release, and the adapter never
//! downloads anything (ADR-0018, ADR-0005).
//!
//! ```text
//!  host (48 kHz FM discriminator audio, s16le, `samples` frames) --> dsd-fme -i -  stdin
//!  host <-- audio (48 kHz mono) <-- resampler <-- 8 kHz decoded voice <-- dsd-fme -o -  stdout
//!  host <-- event / status       <-- call tracker <-- log lines (sync, link control, terminators)
//!                                                 <-- dsd-fme stderr
//! ```
//!
//! Without the tool the plugin stays up and reports `tool.state = "missing"` in its status
//! document (the web UI shows "needs dsd-fme" and where to read how to install it); with a tool
//! that decodes but cannot produce audio, the events still flow.

pub mod call;
pub mod lines;

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use superhet_dsp::resample::Resampler;

use crate::dab::link::{self, Link};
use crate::m17::{HEARTBEAT_EVERY, INPUT_QUEUE, TICK, feed, handle, spawn, unix_ms};
use call::{Emit, Tracker};
use lines::{Line, Protocol};

/// Name of the tool, looked up on `PATH` and in `bin/` of the plugin directory.
pub const TOOL: &str = "dsd-fme";
/// Environment variable with the full path of the tool (the plugin host passes it through).
pub const PATH_ENV: &str = "SUPERHET_DSD_FME_PATH";
/// The end marker waits this long for the voice of the last frames (other pipe).
const EOS_GRACE: Duration = Duration::from_millis(80);
/// Unrecognised lines forwarded to the host log (the tool prints a lot).
const MAX_LOGGED_LINES: usize = 20;
/// Last lines of the tool's output kept for the error report when it dies.
const KEEP_LINES: usize = 6;

/// Instance configuration (`plugins/dsd/schema/config.json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `None` = let the tool detect the protocol.
    pub protocol: Option<Protocol>,
    /// Pass the decoded voice on. `false` runs the tool without audio output: events only.
    pub audio: bool,
    /// Do not play the audio of encrypted calls.
    pub mute_encrypted: bool,
    /// Overrides the tool's frame-type switch (`-fs`, `-fn`, ...), for builds that spell it
    /// differently.
    pub frame_flag: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            protocol: None,
            audio: true,
            mute_encrypted: true,
            frame_flag: None,
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
                "protocol" => {
                    config.protocol = match value.as_str() {
                        Some("auto") => None,
                        Some("dmr") => Some(Protocol::Dmr),
                        Some("dpmr") => Some(Protocol::Dpmr),
                        Some("nxdn") => Some(Protocol::Nxdn),
                        _ => return Err("config.protocol must be auto, dmr, dpmr or nxdn".into()),
                    }
                }
                "audio" => config.audio = flag()?,
                "mute_encrypted" => config.mute_encrypted = flag()?,
                "frame_flag" => {
                    let text = value.as_str().unwrap_or_default();
                    let ok = text.len() >= 3
                        && text.len() <= 4
                        && text.starts_with("-f")
                        && text[2..].chars().all(|c| c.is_ascii_alphanumeric());
                    if !ok {
                        return Err("config.frame_flag must look like -fs".into());
                    }
                    config.frame_flag = Some(text.to_owned());
                }
                other => return Err(format!("unknown config key {other:?}")),
            }
        }
        Ok(config)
    }

    /// Channels of the tool's voice output: DMR and auto write one channel per slot, the others
    /// are mono (`pulse_digi_out_channels` in `dsd_main.c`).
    pub fn output_channels(&self) -> usize {
        if let Some(flag) = &self.frame_flag {
            return if matches!(flag.as_str(), "-fa" | "-fs") {
                2
            } else {
                1
            };
        }
        match self.protocol {
            None | Some(Protocol::Dmr) => 2,
            Some(Protocol::Dpmr | Protocol::Nxdn) => 1,
        }
    }

    /// The tool's frame-type switch for the protocol (`dsd-fme -h`: `-fa` auto, `-fs` DMR, `-fm`
    /// dPMR, `-fn` NXDN 12.5 kHz).
    #[must_use]
    pub fn frame_switch(&self) -> String {
        if let Some(flag) = &self.frame_flag {
            return flag.clone();
        }
        match self.protocol {
            None => "-fa",
            Some(Protocol::Dmr) => "-fs",
            Some(Protocol::Dpmr) => "-fm",
            Some(Protocol::Nxdn) => "-fn",
        }
        .to_owned()
    }

    /// Arguments of `dsd-fme`: 48 kHz discriminator audio on stdin, decoded voice on stdout (or
    /// nowhere), the frame types of the protocol. No ncurses: the plain log on stderr is what the
    /// adapter reads.
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        let output = if self.audio { "-" } else { "null" };
        ["-i", "-", "-o", output]
            .into_iter()
            .map(str::to_owned)
            .chain([self.frame_switch()])
            .collect()
    }
}

/// Where the tool is: `SUPERHET_DSD_FME_PATH` (a file, or a directory containing it), then `PATH`,
/// then `bin/dsd-fme` in the plugin directory (the host starts plugins there). `Err` lists what
/// was looked at.
///
/// # Errors
/// The places that were searched.
pub fn locate(
    env_path: Option<&str>,
    path_var: Option<&OsStr>,
    plugin_dir: &Path,
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
        searched.push(format!("{PATH_ENV}={} (no such file)", file.display()));
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
    let local = plugin_dir.join("bin").join(TOOL);
    if local.is_file() {
        return Ok(local);
    }
    searched.push(local.display().to_string());
    Err(searched)
}

/// The `tool` member of the status document.
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
            "dsd-fme is not installed. Superhet does not ship it (it carries the AMBE vocoder); \
             install it yourself, see docs/plugins-dsd.md."
        );
    }
    tool
}

struct Shared {
    link: Arc<Link>,
    tracker: Mutex<Tracker>,
    tool: Value,
    epoch: Instant,
    stopping: AtomicBool,
    tool_died: AtomicBool,
    /// Last lines of the tool's log, for the report when it dies.
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
        let _ = self.link.status(&tracker.status(&self.tool));
    }

    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Wakes the thread that reads the host socket.
        self.link.close();
    }
}

/// Close the plugin when the host closes stdin.
fn watch_stdin(on_close: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        let mut sink = [0u8; 64];
        while std::io::stdin().read(&mut sink).is_ok_and(|n| n > 0) {}
        on_close();
    });
}

/// Run the plugin: connect to the host, serve until it goes away. `plugin_dir` is the directory
/// the host started the plugin in.
#[must_use]
pub fn run(config: &Value, socket: &str, plugin_dir: &Path) -> ExitCode {
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
    let located = locate(
        std::env::var(PATH_ENV).ok().as_deref(),
        std::env::var_os("PATH").as_deref(),
        plugin_dir,
    );
    let program = match located {
        Ok(program) => program,
        Err(searched) => return idle_without_tool(&link, &mut incoming, &config, &searched),
    };
    let tool = tool_status("ready", Some(&program), &[], None);
    let mut child = match spawn(&program.display().to_string(), &config.args()) {
        Ok(child) => child,
        Err(err) => {
            let _ = link.log(&err);
            let _ = link.status(
                &json!({ "tool": tool_status("failed", Some(&program), &[], Some(&err)),
                "now": [], "last": null, "calls": 0 }),
            );
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
        tracker: Mutex::new(Tracker::new(config.mute_encrypted, config.protocol)),
        tool,
        epoch: Instant::now(),
        stopping: AtomicBool::new(false),
        tool_died: AtomicBool::new(false),
        recent: Mutex::new(VecDeque::new()),
    });
    {
        let mut tracker = shared.tracker();
        let _ = link.status(&tracker.status(&shared.tool));
        tracker.take_dirty();
    }

    let (input, input_rx) = mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE);
    let writer = std::thread::spawn(move || feed(stdin, &input_rx));
    let audio = {
        let shared = Arc::clone(&shared);
        let channels = config.output_channels();
        std::thread::spawn(move || voice(stdout, channels, &shared))
    };
    let log = {
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
                shared.publish(&mut tracker, &emits);
            }
        });
    }
    {
        let shared = Arc::clone(&shared);
        watch_stdin(move || shared.stop());
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
    let _ = log.join();
    {
        let mut tracker = shared.tracker();
        let emits = tracker.shutdown(shared.now());
        shared.publish(&mut tracker, &emits);
    }
    if shared.tool_died.load(Ordering::SeqCst) {
        // The tool died on its own: end with an error so the host restarts the plugin, and say
        // what the tool said last.
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

/// The tool is not installed: report it and wait to be stopped. The plugin does not exit, so the
/// host's supervisor does not restart it in a loop, and the status document tells the UI what to
/// show.
fn idle_without_tool(
    link: &Arc<Link>,
    incoming: &mut link::Incoming,
    config: &Config,
    searched: &[String],
) -> ExitCode {
    let _ = link.log(&format!(
        "{TOOL} is not installed (looked for: {}); see docs/plugins-dsd.md",
        searched.join(", ")
    ));
    let tracker = Tracker::new(config.mute_encrypted, config.protocol);
    let status = tracker.status(&tool_status("missing", None, searched, None));
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
                // A late joiner of the status document gets the state again now and then.
                if last_status.elapsed() >= Duration::from_secs(30) {
                    let _ = link.status(&status);
                    last_status = Instant::now();
                }
            }
        });
    }
    {
        let link = Arc::clone(link);
        watch_stdin(move || link.close());
    }
    // The samples the host sends have nowhere to go.
    while let Ok(Some(_)) = incoming.read_frame() {}
    stopping.store(true, Ordering::SeqCst);
    ExitCode::SUCCESS
}

/// Mixes the whole interleaved frames of `samples` down to mono and leaves a partial frame behind
/// for the next read. The idle DMR slot is silence, so a call keeps its level.
fn downmix(samples: &mut Vec<f32>, channels: usize) -> Vec<f32> {
    let whole = samples.len() - samples.len() % channels;
    let mono = samples[..whole]
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>().clamp(-1.0, 1.0))
        .collect();
    samples.drain(..whole);
    mono
}

/// Decoded voice (8 kHz s16le, `channels` interleaved) to 48 kHz s16le mono for the host.
fn voice(mut stdout: impl Read, channels: usize, shared: &Shared) {
    let mut resampler = Resampler::new(8000.0, 48_000.0);
    let mut buf = vec![0u8; 8192];
    let mut carry: Option<u8> = None;
    let mut pending: Vec<f32> = Vec::new();
    let mut output: Vec<f32> = Vec::new();
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut bytes = carry.take().into_iter().chain(buf[..n].iter().copied());
        while let (Some(low), high) = (bytes.next(), bytes.next()) {
            match high {
                Some(high) => pending.push(f32::from(i16::from_le_bytes([low, high])) / 32768.0),
                None => carry = Some(low),
            }
        }
        let input = downmix(&mut pending, channels);
        if input.is_empty() {
            continue;
        }
        let play = shared.tracker().audio(input.len() as u64, shared.now());
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

/// The tool's log: link control starts calls, terminators and silence end them.
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
            let part = lines::clean(part);
            if part.is_empty() {
                continue;
            }
            if let Ok(mut recent) = shared.recent.lock() {
                if recent.len() == KEEP_LINES {
                    recent.pop_front();
                }
                recent.push_back(part.clone());
            }
            match lines::parse_line(&part) {
                Some(line @ Line::End { .. }) => {
                    // The last frame's audio may still be on its way on the other pipe.
                    std::thread::sleep(EOS_GRACE);
                    let mut tracker = shared.tracker();
                    let emits = tracker.line(line, shared.now(), unix_ms());
                    shared.publish(&mut tracker, &emits);
                }
                Some(line) => {
                    let mut tracker = shared.tracker();
                    let emits = tracker.line(line, shared.now(), unix_ms());
                    shared.publish(&mut tracker, &emits);
                }
                None => {
                    // Mention what the parser skipped, a few lines only: it helps to tell a
                    // tool whose log looks different from one that is merely quiet.
                    if logged < MAX_LOGGED_LINES && !part.starts_with("Sync:") {
                        logged += 1;
                        let _ = shared.link.log(&part);
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
    fn defaults_detect_the_protocol_and_play_clear_audio() {
        let config = Config::from_json(&Value::Null).expect("null is fine");
        assert_eq!(config, Config::default());
        assert_eq!(config.args(), ["-i", "-", "-o", "-", "-fa"]);
        assert_eq!(config.output_channels(), 2);
        assert!(config.mute_encrypted);
    }

    #[test]
    fn the_protocol_picks_the_frame_switch() {
        for (name, flag) in [
            ("dmr", "-fs"),
            ("dpmr", "-fm"),
            ("nxdn", "-fn"),
            ("auto", "-fa"),
        ] {
            let config = Config::from_json(&json!({ "protocol": name })).expect("valid");
            assert_eq!(config.frame_switch(), flag, "{name}");
        }
        let config =
            Config::from_json(&json!({"protocol": "dmr", "frame_flag": "-fr"})).expect("valid");
        assert_eq!(config.args().last().map(String::as_str), Some("-fr"));
    }

    #[test]
    fn without_audio_the_tool_gets_no_output_device() {
        let config = Config::from_json(&json!({"audio": false})).expect("valid");
        assert_eq!(config.args(), ["-i", "-", "-o", "null", "-fa"]);
    }

    #[test]
    fn bad_config_is_refused() {
        for bad in [
            json!({"protocol": "tetra"}),
            json!({"audio": "yes"}),
            json!({"frame_flag": "-o /etc/passwd"}),
            json!({"frame_flag": "--help"}),
            json!({"gain": 2}),
            json!([1]),
        ] {
            assert!(Config::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_tool_is_found_by_variable_path_or_plugin_dir_in_that_order() {
        let root = std::env::temp_dir().join(format!("superhet-dsd-locate-{}", std::process::id()));
        let (on_path, plugin) = (root.join("path"), root.join("plugin"));
        std::fs::create_dir_all(&on_path).expect("dir");
        std::fs::create_dir_all(plugin.join("bin")).expect("dir");
        let path_var = std::env::join_paths([&on_path]).expect("path");
        let none = |env: Option<&str>| locate(env, Some(&path_var), &plugin);
        assert_eq!(none(None).expect_err("nothing yet").len(), 2);

        std::fs::write(plugin.join("bin").join(TOOL), b"").expect("file");
        assert_eq!(none(None), Ok(plugin.join("bin").join(TOOL)));
        std::fs::write(on_path.join(TOOL), b"").expect("file");
        assert_eq!(none(None), Ok(on_path.join(TOOL)));
        let explicit = root.join("custom-dsd");
        std::fs::write(&explicit, b"").expect("file");
        assert_eq!(none(explicit.to_str()), Ok(explicit.clone()));
        // A directory works too; a bad variable is reported but does not hide the rest.
        assert_eq!(none(on_path.to_str()), Ok(on_path.join(TOOL)));
        assert_eq!(none(Some("/nonexistent/dsd-fme")), Ok(on_path.join(TOOL)));
        let _ = std::fs::remove_dir_all(&root);

        let err = locate(Some("/nonexistent/dsd-fme"), None, &root).expect_err("missing");
        assert!(err[0].contains("/nonexistent/dsd-fme"), "{err:?}");
        assert_eq!(err.len(), 3);
    }

    #[test]
    fn stereo_voice_is_mixed_to_mono_across_reads() {
        let mut pending = vec![0.25, 0.0, 0.5];
        assert_eq!(downmix(&mut pending, 2), vec![0.25]);
        assert_eq!(pending, vec![0.5]);
        pending.push(0.25);
        assert_eq!(downmix(&mut pending, 2), vec![0.75]);
        assert!(pending.is_empty());
        let mut mono = vec![0.5, -0.5];
        assert_eq!(downmix(&mut mono, 1), vec![0.5, -0.5]);
    }
}
