//! FT8, FT4 and WSPR through the WSJT-X command-line decoders (`jt9`, `wsprd`).
//!
//! These modes are not streamed: a station transmits during a fixed UTC time slot (FT8 15 s, FT4
//! 7.5 s, WSPR 2 min) and the decoder wants the audio of that one slot as a 12 kHz WAV file. The
//! plugin gets the audio of a USB receiver on stdin (`input.kind = audio`, 12 kHz `s16le`), cuts
//! it into UTC-aligned slots ([`slots`]), writes each slot to a WAV file in a work directory, runs
//! the decoder on it and turns its output lines into events ([`lines`]). The decoder runs on a
//! second thread so that reading stdin never stalls (the host drops chunks when stdin is not
//! read). `jt9` and `wsprd` are GPL-3.0 programs of WSJT-X that stay separate processes
//! (`docs/plugins-ft8.md`, ADR-0032).
//!
//! The system clock must be right (NTP); the decoders tolerate about 2 s of error.
//! `SUPERHET_FT8_STREAM_START_MS` replaces the wall clock by the sample count counted from that
//! UTC time in milliseconds: the adapter then decodes a recording as fast as it can be read
//! (offline replay, and the integration tests).

pub mod lines;
pub mod message;
pub mod slots;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use self::lines::SlotInfo;
use self::slots::{Slicer, Slot};

/// Sample rate the decoders work at; the manifest asks the host for exactly this.
pub const SAMPLE_RATE: u32 = 12_000;

/// What to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ft8,
    Ft4,
    Wspr,
}

impl Mode {
    fn slot_ms(self) -> u64 {
        match self {
            Self::Ft8 => 15_000,
            Self::Ft4 => 7_500,
            Self::Wspr => 120_000,
        }
    }

    /// How late a slot may start and still be decoded (the first transmission starts 0.5 s into
    /// an FT8 slot and 1 s into a WSPR slot).
    fn tolerance_ms(self) -> u64 {
        match self {
            Self::Ft8 | Self::Ft4 => 500,
            Self::Wspr => 1_000,
        }
    }

    fn tool(self) -> &'static str {
        match self {
            Self::Ft8 | Self::Ft4 => "jt9",
            Self::Wspr => "wsprd",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Ft8 => "ft8",
            Self::Ft4 => "ft4",
            Self::Wspr => "wspr",
        }
    }

    /// Longest a decoder run may take before it is killed.
    fn timeout(self) -> Duration {
        Duration::from_millis((self.slot_ms() * 2).clamp(20_000, 110_000))
    }
}

/// The instance configuration (`plugins/ft8/schema/config.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub mode: Mode,
    /// 1 (fast) to 3 (deepest search).
    pub depth: u8,
    /// Dial frequency in Hz, overriding the one the instance was started on.
    pub dial_hz: Option<u64>,
}

impl Config {
    /// Read and check the configuration.
    ///
    /// # Errors
    /// On an unknown key, an unknown mode or a depth outside 1 to 3.
    pub fn from_json(config: &Value) -> Result<Self, String> {
        let empty = serde_json::Map::new();
        let object = match config {
            Value::Null => &empty,
            Value::Object(object) => object,
            _ => return Err("the configuration must be an object".into()),
        };
        let mut out = Self {
            mode: Mode::Ft8,
            depth: 2,
            dial_hz: None,
        };
        for (key, value) in object {
            match key.as_str() {
                "mode" => {
                    out.mode = match value.as_str() {
                        Some("ft8") => Mode::Ft8,
                        Some("ft4") => Mode::Ft4,
                        Some("wspr") => Mode::Wspr,
                        _ => return Err("mode must be \"ft8\", \"ft4\" or \"wspr\"".into()),
                    };
                }
                "depth" => {
                    out.depth = value
                        .as_u64()
                        .filter(|d| (1..=3).contains(d))
                        .and_then(|d| u8::try_from(d).ok())
                        .ok_or("depth must be 1, 2 or 3")?;
                }
                "dial_hz" => {
                    out.dial_hz = Some(
                        value
                            .as_u64()
                            .filter(|hz| *hz > 0)
                            .ok_or("dial_hz must be a positive integer")?,
                    );
                }
                other => return Err(format!("unknown configuration key {other:?}")),
            }
        }
        Ok(out)
    }

    /// The tool's command line for the WAV file `wav` and the work directory `work`.
    #[must_use]
    pub fn args(&self, work: &Path, wav: &Path) -> Vec<String> {
        let dir = work.display().to_string();
        let mut args: Vec<String> = Vec::new();
        match self.mode {
            Mode::Ft8 | Mode::Ft4 => {
                let (flag, period) = if self.mode == Mode::Ft8 {
                    ("-8", "15")
                } else {
                    ("-5", "7")
                };
                args.extend(
                    [
                        flag,
                        "-p",
                        period,
                        "-d",
                        &self.depth.to_string(),
                        "-a",
                        &dir,
                        "-t",
                        &dir,
                    ]
                    .map(String::from),
                );
            }
            Mode::Wspr => {
                args.extend(["-a", &dir].map(String::from));
                if self.depth >= 3 {
                    args.push("-d".into());
                }
            }
        }
        args.push(wav.display().to_string());
        args
    }
}

// ---------- time ----------

/// Civil date and time (UTC) of `ms` since the Unix epoch: year, month, day, hour, minute, second.
#[must_use]
pub fn utc_parts(ms: u64) -> (u64, u64, u64, u64, u64, u64) {
    let secs = ms / 1000;
    let days = secs / 86_400;
    let rem = secs % 86_400;
    // Days since 1970-01-01 to a civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `2026-11-14T12:00:15Z`, with milliseconds (`...T12:00:07.500Z`) when there are any (FT4).
#[must_use]
pub fn iso_time(ms: u64) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(ms);
    let frac = if ms.is_multiple_of(1000) {
        String::new()
    } else {
        format!(".{:03}", ms % 1000)
    };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}{frac}Z")
}

/// The file name the WSJT-X tools expect: `YYMMDD_HHMMSS.wav` (`YYMMDD_HHMM.wav` for wsprd,
/// which takes the time of the transmission from it).
#[must_use]
pub fn wav_name(mode: Mode, ms: u64) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(ms);
    let y = y % 100;
    if mode == Mode::Wspr {
        format!("{y:02}{mo:02}{d:02}_{h:02}{mi:02}.wav")
    } else {
        format!("{y:02}{mo:02}{d:02}_{h:02}{mi:02}{s:02}.wav")
    }
}

/// A mono 16-bit WAV file of `samples`.
#[must_use]
pub fn wav_bytes(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX - 36);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

// ---------- running the tool ----------

/// True when `program` is an executable file somewhere on `PATH`.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// Samples at or below this peak are silence (a closed squelch, a gap): not worth a decoder run.
const SILENCE_PEAK: u16 = 16;

/// Decode one slot and return the events.
fn decode_slot(config: &Config, dial_hz: Option<u64>, work: &Path, slot: &Slot) -> Vec<Value> {
    if slot
        .samples
        .iter()
        .all(|s| s.unsigned_abs() <= SILENCE_PEAK)
    {
        return Vec::new();
    }
    let wav = work.join(wav_name(config.mode, slot.start_ms));
    if let Err(err) = std::fs::write(&wav, wav_bytes(&slot.samples, SAMPLE_RATE)) {
        eprintln!("cannot write {}: {err}", wav.display());
        return Vec::new();
    }
    let output = run_tool(config.mode, &config.args(work, &wav));
    let _ = std::fs::remove_file(&wav);
    let time = iso_time(slot.start_ms);
    let info = SlotInfo {
        mode: config.mode.name(),
        time: &time,
        dial_hz,
    };
    output
        .lines()
        .filter_map(|line| match config.mode {
            Mode::Wspr => lines::parse_wsprd(line, &info),
            Mode::Ft8 | Mode::Ft4 => lines::parse_jt9(line, &info),
        })
        .collect()
}

/// Run the decoder, return what it printed; a decoder that fails or hangs gives nothing (and a
/// line in the log), the next slot is tried again.
fn run_tool(mode: Mode, args: &[String]) -> String {
    let tool = mode.tool();
    let mut child = match Command::new(tool)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            eprintln!("cannot start {tool}: {err}");
            return String::new();
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        return String::new();
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut raw = Vec::new();
        let _ = stdout.read_to_end(&mut raw);
        let _ = tx.send(raw);
    });
    let started = Instant::now();
    match rx.recv_timeout(mode.timeout()) {
        Ok(raw) => {
            match child.wait() {
                Ok(status) if status.success() => {}
                Ok(status) => eprintln!("{tool} ended with {status}"),
                Err(err) => eprintln!("waiting for {tool}: {err}"),
            }
            String::from_utf8_lossy(&raw).into_owned()
        }
        Err(_) => {
            eprintln!(
                "{tool} did not finish within {} s: killed",
                started.elapsed().as_secs()
            );
            let _ = child.kill();
            let _ = child.wait();
            String::new()
        }
    }
}

/// A directory of our own below the temporary directory (the tools keep their FFT wisdom and
/// callsign hash table in it); removed again at the end.
struct WorkDir(PathBuf);

impl WorkDir {
    fn create() -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("superhet-ft8-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------- main loop ----------

/// Run the plugin: read samples from stdin until it closes, write events to stdout.
///
/// `dial_hz` is the frequency the instance was started on (`SUPERHET_FREQ_HZ`); `stream_start_ms`
/// replaces the wall clock (see the module documentation).
pub fn run(config: &Config, dial_hz: Option<u64>, stream_start_ms: Option<u64>) -> ExitCode {
    let tool = config.mode.tool();
    if !on_path(tool) {
        eprintln!(
            "cannot find {tool} on PATH: install WSJT-X (Debian: apt install wsjtx) or use the \
             Superhet image (docs/plugins-ft8.md)"
        );
        return ExitCode::from(127);
    }
    let work = match WorkDir::create() {
        Ok(work) => work,
        Err(err) => {
            eprintln!("cannot create a work directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let dial_hz = config.dial_hz.or(dial_hz);
    let (tx, rx) = mpsc::sync_channel::<Slot>(2);
    let decoder = {
        let config = config.clone();
        let dir = work.0.clone();
        std::thread::spawn(move || {
            let mut out = std::io::stdout().lock();
            for slot in rx {
                for event in decode_slot(&config, dial_hz, &dir, &slot) {
                    if writeln!(out, "{event}").and_then(|()| out.flush()).is_err() {
                        // The host closed our stdout: nobody listens any more.
                        return;
                    }
                }
            }
        })
    };

    let mut slicer = Slicer::new(
        SAMPLE_RATE,
        config.mode.slot_ms(),
        config.mode.tolerance_ms(),
    );
    let mut stdin = std::io::stdin().lock();
    let mut buf = vec![0u8; 64 * 1024];
    let mut carry: Option<u8> = None;
    let mut total_samples: u64 = 0;
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("reading samples: {err}");
                break;
            }
        };
        let mut samples = Vec::with_capacity(n / 2 + 1);
        let mut bytes = buf[..n].iter().copied();
        if let Some(low) = carry.take()
            && let Some(high) = bytes.next()
        {
            samples.push(i16::from_le_bytes([low, high]));
        }
        loop {
            match (bytes.next(), bytes.next()) {
                (Some(low), Some(high)) => samples.push(i16::from_le_bytes([low, high])),
                (Some(low), None) => carry = Some(low),
                _ => break,
            }
        }
        total_samples += samples.len() as u64;
        let now = match stream_start_ms {
            Some(start) => start + total_samples * 1000 / u64::from(SAMPLE_RATE),
            None => wall_ms(),
        };
        for slot in slicer.push(&samples, now) {
            if tx.try_send(slot).is_err() {
                // A decoder that cannot keep up must not hold the stream back.
                eprintln!("the decoder is too slow: a time slot was skipped");
            }
        }
    }
    drop(tx);
    let _ = decoder.join();
    drop(work);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_defaults_and_checks() {
        assert_eq!(
            Config::from_json(&Value::Null),
            Ok(Config {
                mode: Mode::Ft8,
                depth: 2,
                dial_hz: None
            })
        );
        let c = Config::from_json(&json!({"mode": "wspr", "depth": 3, "dial_hz": 14_095_600}))
            .expect("ok");
        assert_eq!(
            (c.mode, c.depth, c.dial_hz),
            (Mode::Wspr, 3, Some(14_095_600))
        );
        for bad in [
            json!({"mode": "jt65"}),
            json!({"depth": 0}),
            json!({"depth": 4}),
            json!({"dial_hz": 0}),
            json!({"dial": 1}),
            json!("ft8"),
        ] {
            assert!(Config::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn command_lines() {
        let work = Path::new("/w");
        let wav = Path::new("/w/261114_120015.wav");
        let ft8 = Config::from_json(&json!({"mode": "ft8", "depth": 3})).expect("ok");
        assert_eq!(
            ft8.args(work, wav).join(" "),
            "-8 -p 15 -d 3 -a /w -t /w /w/261114_120015.wav"
        );
        let ft4 = Config::from_json(&json!({"mode": "ft4", "depth": 1})).expect("ok");
        assert_eq!(
            ft4.args(work, wav).join(" "),
            "-5 -p 7 -d 1 -a /w -t /w /w/261114_120015.wav"
        );
        let wspr = Config::from_json(&json!({"mode": "wspr", "depth": 3})).expect("ok");
        assert_eq!(
            wspr.args(work, wav).join(" "),
            "-a /w -d /w/261114_120015.wav"
        );
        let wspr = Config::from_json(&json!({"mode": "wspr"})).expect("ok");
        assert_eq!(wspr.args(work, wav).join(" "), "-a /w /w/261114_120015.wav");
    }

    #[test]
    fn utc_formatting() {
        // 2026-11-14T12:00:15Z
        let ms = 1_794_657_615_000;
        assert_eq!(iso_time(ms), "2026-11-14T12:00:15Z");
        assert_eq!(wav_name(Mode::Ft8, ms), "261114_120015.wav");
        assert_eq!(wav_name(Mode::Wspr, ms), "261114_1200.wav");
        assert_eq!(iso_time(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_time(ms + 7_500), "2026-11-14T12:00:22.500Z");
        // A leap day.
        assert_eq!(iso_time(1_709_164_800_000), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn wav_header() {
        let wav = wav_bytes(&[1, -2, 3], 12_000);
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().expect("4")), 36 + 6);
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().expect("4")),
            12_000
        );
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().expect("4")), 6);
        assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), -2);
    }
}
