//! The TETRA adapter end to end: the real `superhet-plugin-adapter tetra` binary against a fake host
//! (this test speaks the framed transport over a Unix socket).
//!
//! The host feeds a real modulated signal: osmo-tetra's test burst (`tests/tetra/sync-burst.bits`)
//! as pi/4-DQPSK at 144 kHz, so the adapter's own demodulator runs. `tetra-rx` is a stand-in
//! (`tests/tetra/fake-tetra-rx`, needs `python3`) that checks it gets those bits and replays the
//! transcripts of `tests/tetra/transcripts` as the UDP datagrams the real tool sends. The ETSI codec
//! is the user's and never part of this repository: `tests/tetra/codec` holds stand-ins that turn
//! voice blocks into a sine. Covered: cell, calls and messages announced; voice played with the
//! codec and withheld for encrypted calls; the **missing-codec state** (everything but the audio
//! works, `codec.state = "missing"`); a broken codec; the tool's command line; a dying tool; and the
//! state without `tetra-rx` (`tool.state = "missing"`, the plugin stays up).

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use superhet_plugin_adapters::tetra::phy;

use serde_json::Value;
use superhet_plugin_protocol::framed::{
    self, Frame, FrameDecoder, FrameKind, HostHello, PluginHello, SamplesHeader,
};
use superhet_plugin_protocol::{PROTOCOL_VERSION, Package};

fn plugin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/tetra")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tetra")
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
    let package = Package::load(&plugin_dir()).expect("plugins/tetra loads");
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

/// How `tetra-rx` is available to the adapter.
enum Tool<'a> {
    /// The stand-in on `PATH`, replaying a transcript, with more environment.
    OnPath(&'a str, &'a [(&'a str, &'a str)]),
    /// The stand-in somewhere else, named by `SUPERHET_TETRA_RX_PATH` only.
    ByVariable(&'a str),
    /// Not installed anywhere; `Some` sets a (wrong) `SUPERHET_TETRA_RX_PATH`.
    Missing(Option<&'a str>),
}

/// The user's ETSI codec.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    /// Stand-ins for `cdecoder` and `sdecoder` in `SUPERHET_TETRA_CODEC_DIR`.
    Installed,
    /// `SUPERHET_TETRA_CODEC_DIR` is not set.
    NotInstalled,
    /// The programs are there and end at once.
    Broken,
}

fn start(tool: Tool, codec: Codec, config: &str) -> Rig {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "superhet-tetra-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let socket = dir.join("plugin.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let fake = fixtures().join("fake-tetra-rx");
    let system_path = std::env::var("PATH").unwrap_or_default();
    // The system PATH without any tetra-rx a developer may have installed (and with `stdbuf`).
    let clean_path: String = std::env::split_paths(&system_path)
        .filter(|d| !d.join("tetra-rx").is_file())
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(":");
    let mut command = Command::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"));
    command.arg("tetra").env_clear().current_dir(&dir);
    let transcripts = fixtures().join("transcripts");
    let path = match tool {
        Tool::OnPath(transcript, env) => {
            std::os::unix::fs::symlink(&fake, bin.join("tetra-rx")).expect("symlink");
            command.env("FAKE_TETRA_TRANSCRIPT", transcripts.join(transcript));
            for (key, value) in env {
                command.env(key, value);
            }
            format!("{}:{clean_path}", bin.display())
        }
        Tool::ByVariable(transcript) => {
            let elsewhere = dir.join("elsewhere");
            std::fs::create_dir_all(&elsewhere).expect("dir");
            std::os::unix::fs::symlink(&fake, elsewhere.join("my-rx")).expect("symlink");
            command.env("FAKE_TETRA_TRANSCRIPT", transcripts.join(transcript));
            command.env("SUPERHET_TETRA_RX_PATH", elsewhere.join("my-rx"));
            clean_path
        }
        Tool::Missing(variable) => {
            if let Some(variable) = variable {
                command.env("SUPERHET_TETRA_RX_PATH", variable);
            }
            clean_path
        }
    };
    let codec_dir = dir.join("codec");
    match codec {
        Codec::Installed => {
            std::fs::create_dir_all(&codec_dir).expect("dir");
            // The stand-ins carry their own names in the repository (so nothing there looks like
            // the real programs) and are linked under the names the plugin looks for.
            for (stand_in, name) in [
                ("fake-channel-decoder", "cdecoder"),
                ("fake-speech-decoder", "sdecoder"),
            ] {
                std::os::unix::fs::symlink(
                    fixtures().join("codec").join(stand_in),
                    codec_dir.join(name),
                )
                .expect("symlink");
            }
            command.env("SUPERHET_TETRA_CODEC_DIR", &codec_dir);
        }
        Codec::Broken => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all(&codec_dir).expect("dir");
            for name in ["cdecoder", "sdecoder"] {
                let file = codec_dir.join(name);
                std::fs::write(&file, "#!/bin/sh\nexit 1\n").expect("script");
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
            command.env("SUPERHET_TETRA_CODEC_DIR", &codec_dir);
        }
        Codec::NotInstalled => {}
    }
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
        plugin_id: "org.superhet.decoder.tetra".into(),
        instance_id: "i-test".into(),
        device: Some("dev0".into()),
        freq_hz: Some(392_012_500),
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

    // The signal: osmo-tetra's test burst over and over as pi/4-DQPSK at 144 kHz with a carrier
    // offset, in 20 ms chunks like the daemon's tap, quickly at first so the stand-in gets going.
    let feeding = Arc::new(AtomicBool::new(true));
    {
        let mut writer = host.stream.try_clone().expect("clone");
        let feeding = Arc::clone(&feeding);
        let bits = burst_bits();
        std::thread::spawn(move || {
            let stream: Vec<u8> = bits.iter().copied().cycle().take(510 * 100).collect();
            let iq = phy::modulate(&stream, 300.0, 3, 0.02);
            let chunks: Vec<Vec<u8>> = iq
                .chunks(2880)
                .map(|c| {
                    c.iter()
                        .flat_map(|s| [s.re.to_le_bytes(), s.im.to_le_bytes()].concat())
                        .collect()
                })
                .collect();
            let mut seq = 0u64;
            while feeding.load(Ordering::SeqCst) {
                let chunk = &chunks[usize::try_from(seq).expect("seq") % chunks.len()];
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
                std::thread::sleep(Duration::from_millis(if seq < 120 { 5 } else { 20 }));
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

fn burst_bits() -> Vec<u8> {
    std::fs::read_to_string(fixtures().join("sync-burst.bits"))
        .expect("fixture")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .flat_map(str::bytes)
        .map(|b| b - b'0')
        .collect()
}

fn ended(frames: &[Frame], n: usize) -> bool {
    kinds(frames)
        .iter()
        .filter(|k| *k == "tetra.call.end")
        .count()
        >= n
}

fn temp_file(what: &str) -> PathBuf {
    std::env::temp_dir().join(format!("superhet-tetra-{what}-{}", std::process::id()))
}

macro_rules! need_python {
    () => {
        if !python3() {
            eprintln!("SKIP: python3 is not installed");
            return;
        }
    };
}

// ------------------------------------------------------------------ with a decoder and a codec

#[test]
fn a_group_call_is_announced_played_and_closed_and_the_decoder_gets_our_bits() {
    need_python!();
    let record = temp_file("record");
    let mut rig = start(
        Tool::OnPath(
            "group-call.udp",
            &[("FAKE_TETRA_RECORD", record.to_str().expect("utf8"))],
        ),
        Codec::Installed,
        "null",
    );
    rig.host.wait_for("the call to end", 30, |f| ended(f, 1));
    rig.host
        .wait_for("the voice", 10, |f| audio_bytes(f) > 20_000);
    rig.host.drain(300);
    assert_eq!(
        kinds(&rig.host.frames),
        [
            "tetra.cell",
            "tetra.cell",
            "tetra.cell",
            "tetra.call.start",
            "tetra.call.end"
        ],
        "{:?}",
        events(&rig.host.frames)
    );
    let all = events(&rig.host.frames);
    let cell = &all[2]["data"];
    assert_eq!(
        (
            cell["mcc"].as_u64(),
            cell["mnc"].as_u64(),
            cell["la"].as_u64()
        ),
        (Some(262), Some(42), Some(1))
    );
    assert_eq!(cell["encryption"], "none");
    assert_eq!(cell["dl_hz"], 392_012_500);
    let start = &all[3]["data"];
    assert_eq!(
        (
            start["dst"].as_u64(),
            start["src"].as_u64(),
            start["usage"].as_u64()
        ),
        (Some(9001), Some(2_630_001), Some(5))
    );
    let end = &all[4]["data"];
    assert_eq!(end["reason"], "release");
    assert_eq!(end["talker"], 2_630_002);
    assert_eq!(end["audio_ms"], end["voice_ms"]);
    assert!(
        end["voice_ms"].as_u64().is_some_and(|ms| ms >= 300),
        "{end}"
    );
    // Speech reached the listener.
    assert!(audio_rms(&rig.host.frames) > 0.05);
    let status = last_status(&rig.host.frames).expect("status");
    assert_eq!(status["now"], serde_json::json!([]));
    assert_eq!(status["calls"], 1);
    assert_eq!(status["tool"]["state"], "ready");
    assert_eq!(status["codec"]["state"], "ready");
    assert_eq!(status["cell"]["mcc"], 262);
    assert_eq!(status["signal"]["sync"], true);
    // The decoder's command line and environment, and the bits our demodulator made for it: the
    // training sequence and frequency correction field of the generator's burst.
    let recorded = std::fs::read_to_string(&record).expect("record");
    let mut lines = recorded.lines();
    assert_eq!(lines.next(), Some("-r /dev/stdin"));
    assert!(
        lines
            .next()
            .is_some_and(|l| l.contains("ip=127.0.0.1 rxid=0"))
    );
    let received = lines.next().expect("bits");
    let burst: String = burst_bits().iter().take(200).map(u8::to_string).collect();
    assert!(
        received.contains(&burst),
        "the decoder did not get the burst; first bits {}",
        &received[..received.len().min(120)]
    );
    let _ = std::fs::remove_file(&record);
    assert_events_match_the_manifest(&rig.host.frames);
    assert!(rig.stop().success());
}

#[test]
fn without_the_codec_everything_but_the_audio_works() {
    need_python!();
    let mut rig = start(
        Tool::OnPath("group-call.udp", &[]),
        Codec::NotInstalled,
        "null",
    );
    rig.host.wait_for("the call to end", 30, |f| ended(f, 1));
    rig.host.drain(500);
    assert_eq!(
        kinds(&rig.host.frames),
        [
            "tetra.cell",
            "tetra.cell",
            "tetra.cell",
            "tetra.call.start",
            "tetra.call.end"
        ]
    );
    let all = events(&rig.host.frames);
    let end = &all[4]["data"];
    assert_eq!(end["audio_ms"], 0);
    assert!(
        end["voice_ms"].as_u64().is_some_and(|ms| ms >= 300),
        "{end}"
    );
    assert_eq!(audio_bytes(&rig.host.frames), 0);
    let status = last_status(&rig.host.frames).expect("status");
    assert_eq!(status["tool"]["state"], "ready");
    assert_eq!(status["codec"]["state"], "missing");
    assert!(
        status["codec"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("ETSI") && m.contains("docs/plugins-tetra.md")),
        "{status}"
    );
    assert!(
        status["codec"]["searched"]
            .as_array()
            .is_some_and(|s| s.iter().any(|v| v
                .as_str()
                .is_some_and(|v| v.contains("SUPERHET_TETRA_CODEC_DIR")))),
        "{status}"
    );
    assert_events_match_the_manifest(&rig.host.frames);
    assert!(rig.stop().success());
}

#[test]
fn audio_off_does_not_start_the_codec() {
    need_python!();
    let mut rig = start(
        Tool::OnPath("group-call.udp", &[]),
        Codec::Installed,
        r#"{"audio": false}"#,
    );
    rig.host.wait_for("the call to end", 30, |f| ended(f, 1));
    rig.host.drain(300);
    assert_eq!(audio_bytes(&rig.host.frames), 0);
    let status = last_status(&rig.host.frames).expect("status");
    assert_eq!(status["codec"]["state"], "disabled");
    assert!(rig.stop().success());
}

#[test]
fn a_broken_codec_is_reported_and_the_events_keep_flowing() {
    need_python!();
    let mut rig = start(Tool::OnPath("group-call.udp", &[]), Codec::Broken, "null");
    rig.host.wait_for("the call to end", 30, |f| ended(f, 1));
    rig.host.wait_for("the codec failure", 10, |f| {
        last_status(f).is_some_and(|s| s["codec"]["state"] == "failed")
    });
    assert_eq!(audio_bytes(&rig.host.frames), 0);
    assert!(rig.stop().success());
}

#[test]
fn an_encrypted_call_is_flagged_and_its_voice_is_not_played() {
    need_python!();
    let mut rig = start(
        Tool::OnPath("encrypted-call.udp", &[]),
        Codec::Installed,
        "null",
    );
    rig.host.wait_for("the encrypted call", 30, |f| {
        last_status(f).is_some_and(|s| s["now"][0]["muted"] == true)
    });
    rig.host.drain(500);
    let all = events(&rig.host.frames);
    let start = all
        .iter()
        .find(|e| e["type"] == "tetra.call.start")
        .expect("call start");
    assert_eq!(start["data"]["encrypted"], true);
    assert_eq!(start["data"]["dst"], 9002);
    assert!(
        all.iter()
            .any(|e| e["type"] == "tetra.cell" && e["data"]["encryption"] == "air-interface")
    );
    assert_eq!(audio_bytes(&rig.host.frames), 0);
    assert_events_match_the_manifest(&rig.host.frames);
    assert!(rig.stop().success());
}

#[test]
fn encrypted_audio_plays_when_the_instance_says_so() {
    need_python!();
    let mut rig = start(
        Tool::OnPath("encrypted-call.udp", &[]),
        Codec::Installed,
        r#"{"mute_encrypted": false}"#,
    );
    rig.host.wait_for("audio", 30, |f| audio_bytes(f) > 10_000);
    assert!(rig.stop().success());
}

#[test]
fn neighbours_status_and_text_messages_arrive_once() {
    need_python!();
    let mut rig = start(
        Tool::OnPath("cell-and-messages.udp", &[]),
        Codec::NotInstalled,
        "null",
    );
    rig.host.wait_for("the text message", 30, |f| {
        kinds(f).iter().any(|k| k == "tetra.sds")
    });
    rig.host.drain(800);
    let k = kinds(&rig.host.frames);
    assert_eq!(
        k,
        ["tetra.cell", "tetra.neighbour", "tetra.status", "tetra.sds"],
        "{k:?}"
    );
    let status = last_status(&rig.host.frames).expect("status");
    assert_eq!(status["neighbours"].as_array().map(Vec::len), Some(1));
    assert_events_match_the_manifest(&rig.host.frames);
    assert!(rig.stop().success());
}

#[test]
fn the_cell_of_the_real_decoders_output_for_osmo_tetras_test_burst() {
    need_python!();
    let mut rig = start(
        Tool::OnPath("generator-sync.udp", &[]),
        Codec::NotInstalled,
        "null",
    );
    rig.host.wait_for("the cell", 30, |f| {
        kinds(f).iter().any(|k| k == "tetra.cell")
    });
    let all = events(&rig.host.frames);
    assert_eq!(all[0]["data"]["mcc"], 262);
    assert_eq!(all[0]["data"]["mnc"], 42);
    assert!(rig.stop().success());
}

#[test]
fn the_tool_can_be_named_by_the_environment_variable_alone() {
    need_python!();
    let mut rig = start(
        Tool::ByVariable("group-call.udp"),
        Codec::NotInstalled,
        "null",
    );
    rig.host.wait_for("the call to end", 30, |f| ended(f, 1));
    let status = last_status(&rig.host.frames).expect("status");
    assert!(
        status["tool"]["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("my-rx"))
    );
    assert!(rig.stop().success());
}

#[test]
fn a_dying_decoder_fails_the_plugin_so_the_host_restarts_it() {
    need_python!();
    let mut rig = start(
        Tool::OnPath("group-call.udp", &[("FAKE_TETRA_DIE", "1")]),
        Codec::NotInstalled,
        "null",
    );
    let status = rig.wait(30);
    assert!(!status.success(), "{status:?}");
}

// ------------------------------------------------------------------ without the decoder

#[test]
fn a_missing_decoder_is_reported_in_the_status_and_the_plugin_stays_up() {
    let mut rig = start(Tool::Missing(None), Codec::NotInstalled, "null");
    rig.host.wait_for("the missing-tool status", 5, |f| {
        last_status(f).is_some_and(|s| s["tool"]["state"] == "missing")
    });
    let status = last_status(&rig.host.frames).expect("status");
    assert_eq!(status["now"], serde_json::json!([]));
    assert_eq!(status["calls"], 0);
    assert_eq!(status["codec"]["state"], "missing");
    let tool = &status["tool"];
    assert_eq!(tool["name"], "tetra-rx");
    assert!(
        tool["message"]
            .as_str()
            .is_some_and(|m| m.contains("docs/plugins-tetra.md"))
    );
    assert!(
        logs(&rig.host.frames)
            .iter()
            .any(|l| l.contains("tetra-rx is not installed")),
        "{:?}",
        logs(&rig.host.frames)
    );
    rig.host.drain(1200);
    assert!(
        rig.child.try_wait().expect("wait").is_none(),
        "the plugin ended"
    );
    assert!(events(&rig.host.frames).is_empty());
    assert!(rig.stop().success());
}

#[test]
fn a_wrong_path_variable_is_named_in_the_report() {
    let mut rig = start(
        Tool::Missing(Some("/nonexistent/tetra-rx")),
        Codec::NotInstalled,
        "null",
    );
    rig.host.wait_for("the missing-tool status", 5, |f| {
        last_status(f).is_some_and(|s| s["tool"]["state"] == "missing")
    });
    let status = last_status(&rig.host.frames).expect("status");
    let searched = status["tool"]["searched"].as_array().expect("searched");
    assert!(
        searched.iter().any(|s| s
            .as_str()
            .is_some_and(|s| s.contains("SUPERHET_TETRA_RX_PATH=/nonexistent/tetra-rx"))),
        "{searched:?}"
    );
    assert!(rig.stop().success());
}

#[test]
fn an_unknown_config_key_is_refused_at_start() {
    let dir = std::env::temp_dir().join(format!("superhet-tetra-cfg-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"))
        .arg("tetra")
        .env_clear()
        .env("SUPERHET_SOCKET", dir.join("none.sock"))
        .env("SUPERHET_CONFIG", r#"{"codec": "/bin/sh"}"#)
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown config key"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_manifest_is_valid_framed_iq_with_audio_and_asks_for_the_users_own_codec() {
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
    assert_eq!(
        (input.sample_rate, input.bandwidth_hz),
        (phy::SAMPLE_RATE, Some(25_000))
    );
    let notice = &manifest
        .install
        .as_ref()
        .expect("install")
        .acknowledge
        .as_ref()
        .expect("notice")
        .text;
    assert!(notice.contains("install yourself"), "{notice}");
    assert!(notice.contains("ETSI"));
    assert!(notice.contains("TKG"));
    assert!(notice.contains("nothing is decrypted"));
}
