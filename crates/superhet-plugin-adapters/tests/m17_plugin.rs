//! The M17 adapter end to end: the real `superhet-plugin-adapter m17` binary against a fake host
//! (this test speaks the framed transport over a Unix socket).
//!
//! - With the real `m17-demod` (built by the Dockerfile stage `m17`, see
//!   `scripts/gen-m17-recording.sh`): a synthetic M17 voice stream (`tests/recordings/m17-voice.*`,
//!   made by `m17-mod` of the same project) must come out as a call between the right callsigns
//!   with decoded audio, both as recorded and after a trip through the production NFM channel.
//!   Without the tool these cases are skipped, unless `SUPERHET_REQUIRE_PLUGIN_TOOLS=1` (CI).
//! - With a stand-in tool (`tests/m17/fake-m17-demod`, needs `python3`): encrypted calls are
//!   muted, positions arrive, a vanished transmitter ends the call, a dying tool fails the
//!   plugin, a missing tool is reported.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use superhet_plugin_protocol::framed::{
    self, Frame, FrameDecoder, FrameKind, HostHello, PluginHello, SamplesHeader,
};
use superhet_plugin_protocol::{PROTOCOL_VERSION, Package};

fn plugin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/m17")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn tool_available(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
}

/// Whether the real `m17-demod` can be used; fails instead of skipping when CI requires it.
fn real_tool() -> bool {
    if tool_available("m17-demod") {
        return true;
    }
    assert!(
        std::env::var_os("SUPERHET_REQUIRE_PLUGIN_TOOLS").is_none(),
        "m17-demod is required (SUPERHET_REQUIRE_PLUGIN_TOOLS is set) but not on PATH"
    );
    eprintln!("SKIP m17: m17-demod is not installed");
    false
}

struct Host {
    stream: UnixStream,
    decoder: FrameDecoder,
    frames: Vec<Frame>,
}

impl Host {
    /// Read frames until `pred` holds for everything received so far.
    fn wait_for(&mut self, what: &str, secs: u64, pred: impl Fn(&[Frame]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut buf = vec![0u8; 64 * 1024];
        while !pred(&self.frames) {
            assert!(
                Instant::now() < deadline,
                "timeout waiting for {what}; events {:?}; status {:?}; logs {:?}",
                events(&self.frames),
                last_status(&self.frames),
                logs(&self.frames)
            );
            while let Some(frame) = self.decoder.next_frame().expect("frame") {
                self.frames.push(frame);
            }
            if pred(&self.frames) {
                return;
            }
            match self.stream.read(&mut buf) {
                Ok(0) => panic!(
                    "adapter closed the socket while waiting for {what}; events {:?}; logs {:?}",
                    events(&self.frames),
                    logs(&self.frames)
                ),
                Ok(n) => self.decoder.push(&buf[..n]),
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(err) => panic!("read: {err}"),
            }
        }
    }

    /// Keep reading for `ms` milliseconds (to prove that something does not happen).
    fn drain(&mut self, ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        let mut buf = vec![0u8; 64 * 1024];
        while Instant::now() < until {
            match self.stream.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => self.decoder.push(&buf[..n]),
                Err(_) => {}
            }
            while let Some(frame) = self.decoder.next_frame().expect("frame") {
                self.frames.push(frame);
            }
        }
    }
}

fn events(frames: &[Frame]) -> Vec<Value> {
    frames
        .iter()
        .filter(|f| f.kind == FrameKind::Event)
        .map(|f| serde_json::from_slice(&f.payload).expect("event json"))
        .collect()
}

fn kinds(frames: &[Frame]) -> Vec<String> {
    events(frames)
        .iter()
        .map(|e| e["type"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn logs(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .filter(|f| f.kind == FrameKind::Log)
        .map(|f| String::from_utf8_lossy(&f.payload).into_owned())
        .collect()
}

fn statuses(frames: &[Frame]) -> Vec<Value> {
    frames
        .iter()
        .filter(|f| f.kind == FrameKind::Status)
        .map(|f| serde_json::from_slice(&f.payload).expect("status json"))
        .collect()
}

fn last_status(frames: &[Frame]) -> Option<Value> {
    statuses(frames).pop()
}

fn audio_bytes(frames: &[Frame]) -> usize {
    frames
        .iter()
        .filter(|f| f.kind == FrameKind::Audio)
        .map(|f| f.payload.len())
        .sum()
}

/// RMS of the audio (s16le) over all frames, relative to full scale.
fn audio_rms(frames: &[Frame]) -> f64 {
    let mut sum = 0.0;
    let mut n = 0usize;
    for f in frames.iter().filter(|f| f.kind == FrameKind::Audio) {
        for pair in f.payload.as_chunks::<2>().0.iter() {
            let s = f64::from(i16::from_le_bytes(*pair)) / 32768.0;
            sum += s * s;
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { (sum / n as f64).sqrt() }
}

/// Every event satisfies the shipped manifest's events schema.
fn assert_events_match_the_manifest(frames: &[Frame]) {
    let package = Package::load(&plugin_dir()).expect("plugins/m17 loads");
    let all = events(frames);
    assert!(!all.is_empty());
    for event in all {
        package
            .validator
            .check_value(event.clone())
            .unwrap_or_else(|err| panic!("{event}: {err}"));
    }
}

struct Rig {
    host: Host,
    child: Child,
    dir: PathBuf,
    feeding: Arc<AtomicBool>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.feeding.store(false, Ordering::SeqCst);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Rig {
    /// Close the socket and stdin like the host does when it stops the instance, and return how
    /// the adapter ended.
    fn stop(&mut self) -> ExitStatus {
        self.feeding.store(false, Ordering::SeqCst);
        let _ = self.host.stream.shutdown(std::net::Shutdown::Both);
        drop(self.child.stdin.take());
        self.wait(10)
    }

    fn wait(&mut self, secs: u64) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(status) = self.child.try_wait().expect("wait") {
                return status;
            }
            assert!(Instant::now() < deadline, "the adapter does not end");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// How the tool is found: the real one on `PATH`, or the stand-in with its settings.
enum Tool<'a> {
    Real,
    Fake(&'a [(&'a str, &'a str)]),
    Missing,
}

/// Where the baseband comes from.
enum Input {
    /// This signal in 20 ms chunks, one chunk per `pace`, then low-level noise.
    Samples(Arc<Vec<u8>>, Duration),
    /// No samples at all.
    None,
}

fn start(tool: Tool, input: Input, config: &str) -> Rig {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "superhet-m17-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let socket = dir.join("plugin.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let system_path = std::env::var("PATH").unwrap_or_default();
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let mut command = Command::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"));
    command.arg("m17").env_clear();
    // `stdbuf` is looked up on PATH too; a PATH with only the tool's directory would lose it.
    let path = match tool {
        Tool::Real => system_path,
        Tool::Fake(env) => {
            std::os::unix::fs::symlink(
                fixtures().join("m17/fake-m17-demod"),
                bin.join("m17-demod"),
            )
            .expect("symlink");
            for (key, value) in env {
                command.env(key, value);
            }
            format!("{}:{system_path}", bin.display())
        }
        Tool::Missing => {
            // PATH without the directory of a real tool installed on this machine.
            let mut keep: Vec<String> = Vec::new();
            for dir in std::env::split_paths(&system_path) {
                if dir.join("m17-demod").is_file() {
                    continue;
                }
                keep.push(dir.display().to_string());
            }
            keep.join(":")
        }
    };
    let child = command
        .env("PATH", path)
        .env("SUPERHET_SOCKET", &socket)
        .env("SUPERHET_CONFIG", config)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn adapter");
    let (stream, _) = listener.accept().expect("adapter connects");
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("timeout");
    let mut host = Host {
        stream,
        decoder: FrameDecoder::new(),
        frames: Vec::new(),
    };
    let hello = HostHello {
        protocol: PROTOCOL_VERSION,
        plugin_id: "org.superhet.decoder.m17".into(),
        instance_id: "i-test".into(),
        device: Some("dev0".into()),
        freq_hz: Some(144_800_000),
        config: serde_json::Value::Null,
    };
    let wire = framed::encode(FrameKind::Hello, &serde_json::to_vec(&hello).expect("json"))
        .expect("encode");
    host.stream.write_all(&wire).expect("hello");
    host.wait_for("the plugin hello", 5, |f| {
        f.iter().any(|f| f.kind == FrameKind::Hello)
    });
    let reply: PluginHello =
        serde_json::from_slice(&host.frames.remove(0).payload).expect("plugin hello");
    assert_eq!(reply.protocol, PROTOCOL_VERSION);

    let feeding = Arc::new(AtomicBool::new(true));
    if let Input::Samples(signal, pace) = input {
        let mut writer = host.stream.try_clone().expect("clone");
        let feeding = Arc::clone(&feeding);
        std::thread::spawn(move || {
            let mut seq = 0u64;
            // 20 ms of 48 kHz s16le per frame, like the daemon's tap chunks.
            for chunk in signal.chunks(1920) {
                if !feeding.load(Ordering::SeqCst) {
                    return;
                }
                let header = SamplesHeader {
                    seq,
                    timestamp_ns: 0,
                    tuning_epoch: 0,
                };
                let wire =
                    framed::encode(FrameKind::Samples, &header.encode(chunk)).expect("encode");
                if writer.write_all(&wire).is_err() {
                    return;
                }
                seq += 1;
                std::thread::sleep(pace);
            }
            // Idle channel afterwards: a little noise keeps the real tool's detectors running.
            let mut state = 0x2545_F491_4F6C_DD1Du64;
            while feeding.load(Ordering::SeqCst) {
                let noise: Vec<u8> = (0..960)
                    .flat_map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        ((state >> 40) as i16 % 400).to_le_bytes()
                    })
                    .collect();
                let header = SamplesHeader {
                    seq,
                    timestamp_ns: 0,
                    tuning_epoch: 0,
                };
                let wire =
                    framed::encode(FrameKind::Samples, &header.encode(&noise)).expect("encode");
                if writer.write_all(&wire).is_err() {
                    return;
                }
                seq += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    }
    Rig {
        host,
        child,
        dir,
        feeding,
    }
}

/// `n` chunks of 20 ms silence-ish baseband for the stand-in tool (it only counts bytes).
fn blank(frames: usize) -> Arc<Vec<u8>> {
    Arc::new(vec![0u8; frames * 1920])
}

const LSF_CLEAR: &str = "SRC: DL1ABC, DEST: DB0XYZ, STR:V/V CAN:03, TYPE: 0x0185, NONCE: 0000000000000000000000000000, CRC: 167a";

fn python3() -> bool {
    tool_available("python3")
}

// ---------------------------------------------------------------- the real tool

fn recording() -> Arc<Vec<u8>> {
    Arc::new(std::fs::read(fixtures().join("recordings/m17-voice.sigmf-data")).expect("recording"))
}

fn assert_synthetic_call_decoded(host: &Host) {
    let all = events(&host.frames);
    let start = all
        .iter()
        .find(|e| e["type"] == "m17.call.start")
        .expect("a call start");
    assert_eq!(start["data"]["src"], "DL1ABC");
    assert_eq!(start["data"]["dst"], "DB0XYZ");
    assert_eq!(start["data"]["can"], 3);
    assert_eq!(start["data"]["mode"], "voice");
    assert_eq!(start["data"]["encrypted"], false);
    let end = all
        .iter()
        .find(|e| e["type"] == "m17.call.end")
        .expect("a call end");
    assert_eq!(end["data"]["call"], start["data"]["call"]);
    assert_eq!(end["data"]["src"], "DL1ABC");
    assert_eq!(end["data"]["reason"], "eos");
    // The decoder needs about a second to lock onto the clock of a call: at least 1.5 s of the
    // 3 s voice must have been decoded.
    let audio_ms = end["data"]["audio_ms"].as_u64().expect("audio_ms");
    assert!(audio_ms >= 1500, "only {audio_ms} ms decoded");
    let bytes = audio_bytes(&host.frames);
    assert!(
        bytes >= 48_000 * 2 * 3 / 2 * 9 / 10,
        "only {bytes} bytes of audio reached the host"
    );
    assert!(audio_rms(&host.frames) > 0.01, "the audio is silent");
    assert!(statuses(&host.frames).len() >= 3, "idle, call, over");
    let status = last_status(&host.frames).expect("status");
    assert_eq!(status["now"], Value::Null);
    assert_eq!(status["last"]["src"], "DL1ABC");
    assert_eq!(status["calls"], 1);
    assert_events_match_the_manifest(&host.frames);
}

#[test]
fn golden_synthetic_m17_stream_gives_callsigns_and_audio() {
    if !real_tool() {
        return;
    }
    let mut rig = start(
        Tool::Real,
        Input::Samples(recording(), Duration::from_millis(4)),
        "null",
    );
    rig.host.wait_for("the end of the call", 30, |f| {
        kinds(f).contains(&"m17.call.end".to_owned())
    });
    // Let the last audio arrive.
    rig.host.drain(300);
    assert_synthetic_call_decoded(&rig.host);
    assert!(rig.stop().success());
}

/// The recording as an FM transmission on a carrier, through the receiver's production NFM
/// channel (what the daemon's audio tap delivers): the plugin must cope with its filters and level.
#[test]
fn m17_decodes_after_the_production_nfm_channel() {
    use num_complex::Complex32;
    use superhet_dsp::channel::{Channel, ChannelConfig, Mode};

    if !real_tool() {
        return;
    }
    // The channel wants at least 100 kS/s: FM-modulate the baseband at 5 x 48 kHz.
    let rate = 240_000u32;
    let baseband: Vec<f32> = recording()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)) / 31_322.0)
        .collect();
    // +-3 symbols = 2.4 kHz deviation; the NFM channel's full scale is 5 kHz.
    let mut phase = 0.0f64;
    let mut iq: Vec<Complex32> = Vec::with_capacity(baseband.len() * 5);
    for (i, v) in baseband.iter().enumerate() {
        let next = baseband.get(i + 1).copied().unwrap_or(*v);
        for k in 0..5 {
            let x = *v + (next - *v) * (k as f32 / 5.0);
            phase += std::f64::consts::TAU * 2_400.0 * f64::from(x) / f64::from(rate);
            iq.push(Complex32::new(phase.cos() as f32, phase.sin() as f32));
        }
    }
    let mut channel = Channel::new(ChannelConfig {
        input_rate: rate,
        offset_hz: 0.0,
        mode: Mode::nfm(),
        bandwidth_hz: Some(12_500),
        squelch_dbfs: None,
    })
    .expect("channel");
    let mut audio = Vec::new();
    channel.process(&iq, &mut audio);
    assert!(
        audio.len() > 100_000,
        "the channel produced {}",
        audio.len()
    );
    let signal: Vec<u8> = audio
        .iter()
        .flat_map(|v| ((v * 32767.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes())
        .collect();

    let mut rig = start(
        Tool::Real,
        Input::Samples(Arc::new(signal), Duration::from_millis(4)),
        "null",
    );
    rig.host.wait_for("the end of the call", 30, |f| {
        kinds(f).contains(&"m17.call.end".to_owned())
    });
    rig.host.drain(300);
    assert_synthetic_call_decoded(&rig.host);
    assert!(rig.stop().success());
}

// ------------------------------------------------------------------ the stand-in tool

#[test]
fn a_call_is_announced_played_and_closed() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let mut rig = start(
        Tool::Fake(&[("FAKE_M17_LSF", LSF_CLEAR), ("FAKE_M17_FRAMES", "10")]),
        Input::Samples(blank(60), Duration::from_millis(5)),
        "null",
    );
    rig.host.wait_for("the end of the call", 20, |f| {
        kinds(f).contains(&"m17.call.end".to_owned())
    });
    rig.host.drain(200);
    let all = events(&rig.host.frames);
    assert_eq!(
        kinds(&rig.host.frames),
        ["m17.call.start", "m17.call.end"],
        "{all:?}"
    );
    assert_eq!(all[0]["data"]["src"], "DL1ABC");
    // The end marker and the audio of the last frame travel on different pipes.
    let audio_ms = all[1]["data"]["audio_ms"].as_u64().expect("audio_ms");
    assert!((360..=400).contains(&audio_ms), "{audio_ms} ms");
    assert_eq!(all[1]["data"]["reason"], "eos");
    // 10 frames x 40 ms of 48 kHz mono s16le = 38400 bytes; the resampler delays a few samples.
    let bytes = audio_bytes(&rig.host.frames);
    assert!((34_000..=38_400).contains(&bytes), "{bytes} bytes of audio");
    assert!(audio_rms(&rig.host.frames) > 0.05);
    let during = statuses(&rig.host.frames)
        .into_iter()
        .find(|s| s["now"]["src"] == "DL1ABC")
        .expect("a status with the call on the air");
    assert_eq!(during["now"]["dst"], "DB0XYZ");
    assert_eq!(during["now"]["call"], all[0]["data"]["call"]);
    assert_eq!(
        last_status(&rig.host.frames).expect("status")["now"],
        Value::Null
    );
    assert_events_match_the_manifest(&rig.host.frames);
    assert!(rig.stop().success());
}

#[test]
fn an_encrypted_call_is_reported_and_not_played() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    // Stream, voice, AES, CAN 5.
    let word: u16 = 1 | (2 << 1) | (2 << 3) | (5 << 7);
    let lsf = format!(
        "SRC: DL1ABC, DEST: ALL, STR:V/V CAN:05, TYPE: 0x{word:04x}, NONCE: 0102030405060708090a0b0c0d0e, CRC: 0000"
    );
    let mut rig = start(
        Tool::Fake(&[("FAKE_M17_LSF", &lsf), ("FAKE_M17_FRAMES", "8")]),
        Input::Samples(blank(60), Duration::from_millis(5)),
        "null",
    );
    rig.host.wait_for("the end of the call", 20, |f| {
        kinds(f).contains(&"m17.call.end".to_owned())
    });
    rig.host.drain(300);
    let all = events(&rig.host.frames);
    assert_eq!(all[0]["data"]["encrypted"], true);
    assert_eq!(all[0]["data"]["encryption"], "aes");
    assert_eq!(all[0]["data"]["can"], 5);
    assert_eq!(all[1]["data"]["muted"], true);
    assert_eq!(all[1]["data"]["audio_ms"], 0);
    assert_eq!(
        audio_bytes(&rig.host.frames),
        0,
        "encrypted audio is withheld"
    );
    assert_events_match_the_manifest(&rig.host.frames);
    assert!(rig.stop().success());
}

#[test]
fn encrypted_audio_plays_when_muting_is_switched_off() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let word: u16 = 1 | (2 << 1) | (1 << 3);
    let lsf = format!(
        "SRC: DL1ABC, DEST: ALL, STR:V/V CAN:00, TYPE: 0x{word:04x}, NONCE: 0000000000000000000000000000, CRC: 0000"
    );
    let mut rig = start(
        Tool::Fake(&[("FAKE_M17_LSF", &lsf), ("FAKE_M17_FRAMES", "8")]),
        Input::Samples(blank(60), Duration::from_millis(5)),
        r#"{"mute_encrypted": false}"#,
    );
    rig.host.wait_for("the end of the call", 20, |f| {
        kinds(f).contains(&"m17.call.end".to_owned())
    });
    rig.host.drain(200);
    assert!(audio_bytes(&rig.host.frames) > 20_000);
    assert!(rig.stop().success());
}

#[test]
fn a_position_in_the_meta_field_is_forwarded() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    // GNSS data: OpenRTX handheld, everything valid, 48.0 N 9.0 W (see the unit test in lsf.rs).
    let lat = (48.0 / 90.0 * 8_388_607.0_f64).round() as i32;
    let lon = (-9.0 / 180.0 * 8_388_607.0_f64).round() as i32;
    let mut meta = [0u8; 14];
    meta[0] = 0x12;
    meta[1] = 0xF0 | (3 << 1);
    meta[2] = 90;
    meta[3..6].copy_from_slice(&lat.to_be_bytes()[1..]);
    meta[6..9].copy_from_slice(&lon.to_be_bytes()[1..]);
    meta[9..11].copy_from_slice(&2200u16.to_be_bytes());
    meta[11] = 1;
    meta[12] = 0x90;
    let hex: String = meta.iter().map(|b| format!("{b:02x}")).collect();
    let word: u16 = 1 | (2 << 1) | (1 << 5);
    let lsf = format!(
        "SRC: DL1ABC/P, DEST: ALL, STR:V/V CAN:00, TYPE: 0x{word:04x}, NONCE: {hex}, CRC: 0000"
    );
    let mut rig = start(
        Tool::Fake(&[("FAKE_M17_LSF", &lsf), ("FAKE_M17_FRAMES", "5")]),
        Input::Samples(blank(40), Duration::from_millis(5)),
        "null",
    );
    rig.host.wait_for("the end of the call", 20, |f| {
        kinds(f).contains(&"m17.call.end".to_owned())
    });
    let all = events(&rig.host.frames);
    let position = &all[0]["data"]["position"];
    assert_eq!(all[0]["data"]["src"], "DL1ABC/P");
    assert!((position["lat"].as_f64().expect("lat") - 48.0).abs() < 1e-4);
    assert!((position["lon"].as_f64().expect("lon") + 9.0).abs() < 1e-4);
    assert_eq!(position["altitude_m"], 600.0);
    assert_eq!(position["station"], "handheld");
    assert_eq!(all[1]["data"]["position"], *position);
    assert_events_match_the_manifest(&rig.host.frames);
    assert!(rig.stop().success());
}

#[test]
fn a_vanished_transmitter_ends_the_call_by_timeout() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let mut rig = start(
        Tool::Fake(&[
            ("FAKE_M17_LSF", LSF_CLEAR),
            ("FAKE_M17_FRAMES", "5"),
            ("FAKE_M17_EOS", "0"),
        ]),
        Input::Samples(blank(40), Duration::from_millis(5)),
        "null",
    );
    let began = Instant::now();
    rig.host.wait_for("the end of the call", 20, |f| {
        kinds(f).contains(&"m17.call.end".to_owned())
    });
    assert!(
        began.elapsed() >= Duration::from_millis(1400),
        "ended too early: {:?}",
        began.elapsed()
    );
    let end = events(&rig.host.frames)
        .into_iter()
        .find(|e| e["type"] == "m17.call.end")
        .expect("end");
    assert_eq!(end["data"]["reason"], "lost");
    assert!(rig.stop().success());
}

#[test]
fn the_tool_gets_the_configured_switches() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let args = std::env::temp_dir().join(format!("superhet-m17-args-{}", std::process::id()));
    let args_path = args.display().to_string();
    let mut rig = start(
        Tool::Fake(&[
            ("FAKE_M17_LSF", LSF_CLEAR),
            ("FAKE_M17_ARGS", &args_path),
            ("FAKE_M17_FRAMES", "2"),
        ]),
        Input::Samples(blank(10), Duration::from_millis(5)),
        r#"{"noise_blanker": false, "invert": true}"#,
    );
    rig.host.wait_for("the end of the call", 20, |f| {
        kinds(f).contains(&"m17.call.end".to_owned())
    });
    assert_eq!(std::fs::read_to_string(&args).expect("args"), "-l -i");
    let _ = std::fs::remove_file(&args);
    assert!(rig.stop().success());
}

#[test]
fn a_dying_tool_fails_the_plugin_so_the_host_restarts_it() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let mut rig = start(
        Tool::Fake(&[
            ("FAKE_M17_LSF", LSF_CLEAR),
            ("FAKE_M17_FRAMES", "3"),
            ("FAKE_M17_DIE", "1"),
        ]),
        Input::Samples(blank(200), Duration::from_millis(5)),
        "null",
    );
    let status = rig.wait(20);
    assert!(!status.success(), "{status:?}");
    // The call that was going on is closed before the plugin ends.
    rig.host.drain(100);
    assert!(kinds(&rig.host.frames).contains(&"m17.call.end".to_owned()));
}

#[test]
fn a_missing_tool_is_reported_and_fails_the_plugin() {
    let mut rig = start(Tool::Missing, Input::None, "null");
    let status = rig.wait(10);
    assert_eq!(status.code(), Some(127));
    rig.host.drain(100);
    assert!(
        logs(&rig.host.frames)
            .iter()
            .any(|l| l.contains("m17-demod is not installed")),
        "{:?}",
        logs(&rig.host.frames)
    );
}

#[test]
fn an_unknown_config_key_is_refused_at_start() {
    let dir = std::env::temp_dir().join(format!("superhet-m17-cfg-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let status = Command::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"))
        .arg("m17")
        .env_clear()
        .env("SUPERHET_SOCKET", dir.join("none.sock"))
        .env("SUPERHET_CONFIG", r#"{"gain": 3}"#)
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert_eq!(status.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&status.stderr).contains("unknown config key"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_manifest_is_valid_and_framed_with_audio() {
    let package = superhet_plugin_protocol::contract::check_static(&plugin_dir())
        .unwrap_or_else(|problems| panic!("{problems:?}"));
    let manifest = &package.manifest;
    assert!(
        !manifest.is_exclusive(),
        "M17 taps a VFO, it does not claim the device"
    );
    let audio = manifest.output.audio.as_ref().expect("audio output");
    assert_eq!((audio.sample_rate, audio.channels), (48_000, 1));
    let input = manifest.input.as_ref().expect("input");
    assert_eq!(input.sample_rate, 48_000);
}
