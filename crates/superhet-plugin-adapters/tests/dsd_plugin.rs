//! The DMR / dPMR / NXDN adapter end to end: the real `superhet-plugin-adapter dsd` binary against a
//! fake host (this test speaks the framed transport over a Unix socket).
//!
//! `dsd-fme` is user-installed and never part of this repository (it carries the AMBE vocoder), so
//! the tool is a stand-in (`tests/dsd/fake-dsd-fme`, needs `python3`) that replays the recorded log
//! transcripts of `tests/dsd/transcripts` and writes a sine as "decoded voice". What is covered:
//! calls announced, played and closed; encrypted calls withheld; the tool's command line; a dying
//! tool; and the state without any tool installed (`tool.state = "missing"`, the plugin stays up).

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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/dsd")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dsd")
}

fn python3() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("python3").is_file()))
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

    /// Keep reading for `ms` milliseconds.
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

fn payloads(frames: &[Frame], kind: FrameKind) -> Vec<Value> {
    frames
        .iter()
        .filter(|f| f.kind == kind)
        .map(|f| serde_json::from_slice(&f.payload).expect("json"))
        .collect()
}

fn events(frames: &[Frame]) -> Vec<Value> {
    payloads(frames, FrameKind::Event)
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
    payloads(frames, FrameKind::Status)
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
        for pair in f.payload.as_chunks::<2>().0 {
            let s = f64::from(i16::from_le_bytes(*pair)) / 32768.0;
            sum += s * s;
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { (sum / n as f64).sqrt() }
}

/// Every event satisfies the shipped manifest's events schema.
fn assert_events_match_the_manifest(frames: &[Frame]) {
    let package = Package::load(&plugin_dir()).expect("plugins/dsd loads");
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
    /// Close the socket and stdin like the host does when it stops the instance.
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

/// How `dsd-fme` is available to the adapter.
enum Tool<'a> {
    /// The stand-in on `PATH`, replaying a transcript, with more environment.
    OnPath(&'a str, &'a [(&'a str, &'a str)]),
    /// The stand-in somewhere else, named by `SUPERHET_DSD_FME_PATH` only.
    ByVariable(&'a str),
    /// Not installed anywhere; `Some` sets a (wrong) `SUPERHET_DSD_FME_PATH`.
    Missing(Option<&'a str>),
}

fn start(tool: Tool, config: &str) -> Rig {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "superhet-dsd-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let socket = dir.join("plugin.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let fake = fixtures().join("fake-dsd-fme");
    let system_path = std::env::var("PATH").unwrap_or_default();
    // The system PATH without any dsd-fme a developer may have installed (and with `stdbuf`).
    let clean_path: String = std::env::split_paths(&system_path)
        .filter(|d| !d.join("dsd-fme").is_file())
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(":");
    let mut command = Command::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"));
    // The host starts plugins in their directory; a temp directory without `bin/dsd-fme` here.
    command.arg("dsd").env_clear().current_dir(&dir);
    let path = match tool {
        Tool::OnPath(transcript, env) => {
            std::os::unix::fs::symlink(&fake, bin.join("dsd-fme")).expect("symlink");
            command.env(
                "FAKE_DSD_TRANSCRIPT",
                fixtures().join("transcripts").join(transcript),
            );
            for (key, value) in env {
                command.env(key, value);
            }
            format!("{}:{clean_path}", bin.display())
        }
        Tool::ByVariable(transcript) => {
            let elsewhere = dir.join("elsewhere");
            std::fs::create_dir_all(&elsewhere).expect("dir");
            std::os::unix::fs::symlink(&fake, elsewhere.join("my-dsd")).expect("symlink");
            command.env(
                "FAKE_DSD_TRANSCRIPT",
                fixtures().join("transcripts").join(transcript),
            );
            command.env("SUPERHET_DSD_FME_PATH", elsewhere.join("my-dsd"));
            clean_path
        }
        Tool::Missing(variable) => {
            if let Some(variable) = variable {
                command.env("SUPERHET_DSD_FME_PATH", variable);
            }
            clean_path
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
        plugin_id: "org.superhet.decoder.dsd".into(),
        instance_id: "i-test".into(),
        device: Some("dev0".into()),
        freq_hz: Some(438_000_000),
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

    // 20 ms of 48 kHz s16le per frame, like the daemon's tap chunks: 80 chunks = 40 steps of the
    // stand-in, then it goes quiet.
    let feeding = Arc::new(AtomicBool::new(true));
    {
        let mut writer = host.stream.try_clone().expect("clone");
        let feeding = Arc::clone(&feeding);
        std::thread::spawn(move || {
            let chunk = vec![0u8; 1920];
            let mut seq = 0u64;
            while feeding.load(Ordering::SeqCst) {
                let header = SamplesHeader {
                    seq,
                    timestamp_ns: 0,
                    tuning_epoch: 0,
                };
                let wire =
                    framed::encode(FrameKind::Samples, &header.encode(&chunk)).expect("encode");
                if writer.write_all(&wire).is_err() {
                    return;
                }
                seq += 1;
                std::thread::sleep(Duration::from_millis(if seq < 80 { 5 } else { 20 }));
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

fn ended(frames: &[Frame], n: usize) -> bool {
    kinds(frames)
        .iter()
        .filter(|k| *k == "dsd.call.end")
        .count()
        >= n
}

// ------------------------------------------------------------------ with a tool

#[test]
fn dmr_calls_on_both_slots_are_announced_played_and_closed() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let args = std::env::temp_dir().join(format!("superhet-dsd-args-{}", std::process::id()));
    let mut rig = start(
        Tool::OnPath(
            "dmr-two-slots.log",
            &[("FAKE_DSD_ARGS", args.to_str().expect("utf8"))],
        ),
        r#"{"protocol": "dmr"}"#,
    );
    rig.host.wait_for("both calls to end", 20, |f| ended(f, 2));
    rig.host.drain(200);
    assert_eq!(
        kinds(&rig.host.frames),
        [
            "dsd.call.start",
            "dsd.call.start",
            "dsd.call.end",
            "dsd.call.end"
        ],
        "{:?}",
        events(&rig.host.frames)
    );
    let all = events(&rig.host.frames);
    let data = |i: usize| &all[i]["data"];
    assert_eq!(
        (data(0)["slot"].as_u64(), data(0)["src"].as_u64()),
        (Some(1), Some(3_101_234))
    );
    assert_eq!(
        (
            data(0)["dst"].as_u64(),
            data(0)["group"].as_bool(),
            data(0)["cc"].as_u64()
        ),
        (Some(91), Some(true), Some(1))
    );
    assert_eq!(
        (data(1)["slot"].as_u64(), data(1)["group"].as_bool()),
        (Some(2), Some(false))
    );
    assert_eq!(data(2)["reason"], "eos");
    assert!(
        data(2)["audio_ms"].as_u64().is_some_and(|ms| ms >= 80),
        "{}",
        data(2)
    );
    // Both slots were on the air at once: a status with two calls.
    assert!(
        statuses(&rig.host.frames)
            .iter()
            .any(|s| s["now"].as_array().is_some_and(|n| n.len() == 2)),
        "{:?}",
        statuses(&rig.host.frames)
    );
    // Voice reached the listener.
    assert!(
        audio_bytes(&rig.host.frames) > 8_000,
        "{}",
        audio_bytes(&rig.host.frames)
    );
    assert!(audio_rms(&rig.host.frames) > 0.05);
    let status = last_status(&rig.host.frames).expect("status");
    assert_eq!(status["now"], serde_json::json!([]));
    assert_eq!(
        (status["calls"].as_u64(), status["tool"]["state"].as_str()),
        (Some(2), Some("ready"))
    );
    assert!(
        status["tool"]["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("dsd-fme"))
    );
    assert_eq!(status["last"]["src"], 2_620_001);
    assert_events_match_the_manifest(&rig.host.frames);
    // The tool got stdin as input, stdout as the voice output and the DMR frame switch.
    assert_eq!(
        std::fs::read_to_string(&args).expect("args"),
        "-i - -o - -fs"
    );
    let _ = std::fs::remove_file(&args);
    assert!(rig.stop().success());
}

#[test]
fn an_encrypted_call_is_flagged_and_its_audio_withheld() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let mut rig = start(Tool::OnPath("dmr-encrypted.log", &[]), "null");
    rig.host.wait_for("the call to end", 20, |f| ended(f, 1));
    rig.host.drain(200);
    let all = events(&rig.host.frames);
    let end = &all[1]["data"];
    assert_eq!(all[1]["type"], "dsd.call.end");
    assert_eq!(
        (end["encrypted"].as_bool(), end["algorithm"].as_str()),
        (Some(true), Some("0x21"))
    );
    assert_eq!(end["muted"], true);
    // Three steps of voice, 120 ms: only what came before the algorithm was known may play.
    let audio_ms = end["audio_ms"].as_u64().expect("audio_ms");
    assert!(audio_ms < 120, "{audio_ms} ms played of an encrypted call");
    assert_events_match_the_manifest(&rig.host.frames);
    assert!(rig.stop().success());
}

#[test]
fn encrypted_audio_plays_when_muting_is_switched_off() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let mut rig = start(
        Tool::OnPath("dmr-encrypted.log", &[]),
        r#"{"mute_encrypted": false}"#,
    );
    rig.host.wait_for("the call to end", 20, |f| ended(f, 1));
    rig.host.drain(200);
    let end = &events(&rig.host.frames)[1]["data"];
    assert_eq!(end["audio_ms"], 120);
    assert!(end.get("muted").is_none());
    assert!(rig.stop().success());
}

#[test]
fn a_vanished_transmitter_ends_the_call_by_timeout() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let mut rig = start(Tool::OnPath("nxdn-and-dpmr.log", &[]), "null");
    rig.host.wait_for("both calls to end", 20, |f| ended(f, 2));
    let all = events(&rig.host.frames);
    assert_eq!(all[1]["data"]["reason"], "eos");
    assert_eq!(all[2]["data"]["mode"], "dpmr");
    assert_eq!(all[3]["data"]["reason"], "lost");
    assert_events_match_the_manifest(&rig.host.frames);
    assert!(rig.stop().success());
}

#[test]
fn without_audio_the_tool_gets_no_output_device_and_events_still_flow() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let args = std::env::temp_dir().join(format!("superhet-dsd-noaudio-{}", std::process::id()));
    let mut rig = start(
        Tool::OnPath(
            "dmr-two-slots.log",
            &[("FAKE_DSD_ARGS", args.to_str().expect("utf8"))],
        ),
        r#"{"audio": false, "protocol": "nxdn"}"#,
    );
    // The NXDN-only instance ignores the DMR transcript entirely; nothing may come out.
    rig.host.drain(1500);
    assert!(events(&rig.host.frames).is_empty());
    assert_eq!(audio_bytes(&rig.host.frames), 0);
    assert_eq!(
        std::fs::read_to_string(&args).expect("args"),
        "-i - -o null -fn"
    );
    let _ = std::fs::remove_file(&args);
    assert!(rig.stop().success());
}

#[test]
fn the_tool_can_be_named_by_the_environment_variable_alone() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let mut rig = start(Tool::ByVariable("dmr-two-slots.log"), "null");
    rig.host.wait_for("both calls to end", 20, |f| ended(f, 2));
    let status = last_status(&rig.host.frames).expect("status");
    assert_eq!(status["tool"]["state"], "ready");
    assert!(
        status["tool"]["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("my-dsd"))
    );
    assert!(rig.stop().success());
}

#[test]
fn a_dying_tool_fails_the_plugin_so_the_host_restarts_it() {
    if !python3() {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let mut rig = start(
        Tool::OnPath("dmr-two-slots.log", &[("FAKE_DSD_DIE", "1")]),
        "null",
    );
    let status = rig.wait(20);
    assert!(!status.success(), "{status:?}");
}

// ------------------------------------------------------------------ without a tool

#[test]
fn a_missing_tool_is_reported_in_the_status_and_the_plugin_stays_up() {
    let mut rig = start(Tool::Missing(None), "null");
    rig.host.wait_for("the missing-tool status", 5, |f| {
        last_status(f).is_some_and(|s| s["tool"]["state"] == "missing")
    });
    let status = last_status(&rig.host.frames).expect("status");
    assert_eq!(status["now"], serde_json::json!([]));
    assert_eq!(status["calls"], 0);
    let tool = &status["tool"];
    assert_eq!(tool["name"], "dsd-fme");
    assert!(
        tool["message"]
            .as_str()
            .is_some_and(|m| m.contains("docs/plugins-dsd.md"))
    );
    let searched = tool["searched"].as_array().expect("searched");
    assert!(
        searched
            .iter()
            .any(|s| s.as_str().is_some_and(|s| s.contains("PATH"))),
        "{searched:?}"
    );
    assert!(
        searched
            .iter()
            .any(|s| s.as_str().is_some_and(|s| s.ends_with("bin/dsd-fme"))),
        "{searched:?}"
    );
    assert!(
        logs(&rig.host.frames)
            .iter()
            .any(|l| l.contains("dsd-fme is not installed")),
        "{:?}",
        logs(&rig.host.frames)
    );
    // It does not exit (the supervisor would restart it in a loop), takes the samples the host
    // sends without a sound and ends cleanly when asked.
    rig.host.drain(1200);
    assert!(
        rig.child.try_wait().expect("wait").is_none(),
        "the plugin ended"
    );
    assert!(events(&rig.host.frames).is_empty());
    assert_eq!(audio_bytes(&rig.host.frames), 0);
    assert!(rig.stop().success());
}

#[test]
fn a_wrong_path_variable_is_named_in_the_report() {
    let mut rig = start(Tool::Missing(Some("/nonexistent/dsd-fme")), "null");
    rig.host.wait_for("the missing-tool status", 5, |f| {
        last_status(f).is_some_and(|s| s["tool"]["state"] == "missing")
    });
    let status = last_status(&rig.host.frames).expect("status");
    let searched = status["tool"]["searched"].as_array().expect("searched");
    assert!(
        searched.iter().any(|s| s
            .as_str()
            .is_some_and(|s| s.contains("SUPERHET_DSD_FME_PATH=/nonexistent/dsd-fme"))),
        "{searched:?}"
    );
    assert!(rig.stop().success());
}

#[test]
fn an_unknown_config_key_is_refused_at_start() {
    let dir = std::env::temp_dir().join(format!("superhet-dsd-cfg-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let status = Command::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"))
        .arg("dsd")
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
fn the_manifest_is_valid_framed_with_audio_and_asks_for_the_users_own_tool() {
    let package = superhet_plugin_protocol::contract::check_static(&plugin_dir())
        .unwrap_or_else(|problems| panic!("{problems:?}"));
    let manifest = &package.manifest;
    assert!(
        !manifest.is_exclusive(),
        "the plugin taps a VFO, it does not claim the device"
    );
    let audio = manifest.output.audio.as_ref().expect("audio output");
    assert_eq!((audio.sample_rate, audio.channels), (48_000, 1));
    let input = manifest.input.as_ref().expect("input");
    assert_eq!(input.sample_rate, 48_000);
    let notice = &manifest
        .install
        .as_ref()
        .expect("install")
        .acknowledge
        .as_ref()
        .expect("notice")
        .text;
    assert!(notice.contains("install yourself"), "{notice}");
    assert!(notice.contains("AMBE"));
}
