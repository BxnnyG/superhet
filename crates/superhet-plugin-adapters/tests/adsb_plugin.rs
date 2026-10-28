//! The ADS-B adapter end to end: the real `superhet-plugin-adapter adsb` binary against a fake host
//! (this test speaks the framed transport over a Unix socket) and a stand-in `readsb`
//! (tests/adsb/fake-readsb, needs `python3`) that serves recorded `aircraft.json` files in the
//! format of readsb 3.16. Covers the IQ path into the decoder, the live status document, the
//! rationed events (checked against the shipped manifest's schema), the decoder dying, and a clean
//! shutdown.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use superhet_plugin_protocol::framed::{
    self, Frame, FrameDecoder, FrameKind, HostHello, PluginHello, SamplesHeader,
};
use superhet_plugin_protocol::{PROTOCOL_VERSION, Package};

struct Host {
    stream: UnixStream,
    decoder: FrameDecoder,
    frames: Vec<Frame>,
}

impl Host {
    fn send(&mut self, kind: FrameKind, payload: &[u8]) {
        let wire = framed::encode(kind, payload).expect("encode");
        self.stream.write_all(&wire).expect("write");
    }

    /// Read frames until `pred` holds for everything received so far.
    fn wait_for(&mut self, what: &str, secs: u64, pred: impl Fn(&[Frame]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut buf = vec![0u8; 64 * 1024];
        while !pred(&self.frames) {
            assert!(
                Instant::now() < deadline,
                "timeout waiting for {what}; events {:?}; status {:?}",
                events(&self.frames),
                last_status(&self.frames),
            );
            while let Some(frame) = self.decoder.next_frame().expect("frame") {
                self.frames.push(frame);
            }
            if pred(&self.frames) {
                return;
            }
            match self.stream.read(&mut buf) {
                Ok(0) => panic!("adapter closed the socket while waiting for {what}"),
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
}

fn events(frames: &[Frame]) -> Vec<Value> {
    frames
        .iter()
        .filter(|f| f.kind == FrameKind::Event)
        .map(|f| serde_json::from_slice(&f.payload).expect("event json"))
        .collect()
}

fn last_status(frames: &[Frame]) -> Option<Value> {
    frames
        .iter()
        .rev()
        .find(|f| f.kind == FrameKind::Status)
        .map(|f| serde_json::from_slice(&f.payload).expect("status json"))
}

fn plugin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/adsb")
}

/// Every event the adapter sent satisfies the shipped manifest's events schema.
fn assert_events_match_the_manifest(frames: &[Frame]) {
    let package = Package::load(&plugin_dir()).expect("plugins/adsb loads");
    assert!(package.manifest.is_exclusive());
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

fn start(extra_env: &[(&str, &str)]) -> Rig {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "superhet-adsb-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/adsb");
    let socket = dir.join("plugin.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    // The adapter looks the decoder up by name: put the fake in front of PATH under that name.
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    std::os::unix::fs::symlink(fixtures.join("fake-readsb"), bin.join("readsb")).expect("symlink");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"));
    command
        .arg("adsb")
        .env_clear()
        .env("PATH", path)
        // The adapter keeps readsb's output directory under TMPDIR.
        .env("TMPDIR", &dir)
        .env("SUPERHET_SOCKET", &socket)
        .env(
            "SUPERHET_CONFIG",
            r#"{"lat":51.0,"lon":7.0,"event_interval_s":3600,"poll_ms":100}"#,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let child = command.spawn().expect("spawn adapter");
    let (stream, _) = listener.accept().expect("adapter connects");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("timeout");
    let mut host = Host {
        stream,
        decoder: FrameDecoder::new(),
        frames: Vec::new(),
    };
    let hello = HostHello {
        protocol: PROTOCOL_VERSION,
        plugin_id: "org.superhet.decoder.adsb".into(),
        instance_id: "i-test".into(),
        device: Some("dev0".into()),
        freq_hz: Some(1_090_000_000),
        config: Value::Null,
    };
    host.send(FrameKind::Hello, &serde_json::to_vec(&hello).expect("json"));
    host.wait_for("the plugin hello", 5, |f| {
        f.iter().any(|f| f.kind == FrameKind::Hello)
    });
    let reply: PluginHello =
        serde_json::from_slice(&host.frames.remove(0).payload).expect("plugin hello");
    assert_eq!(reply.protocol, PROTOCOL_VERSION);

    // A steady stream of IQ, as the daemon would deliver it.
    let feeding = Arc::new(AtomicBool::new(true));
    {
        let mut writer = host.stream.try_clone().expect("clone");
        let feeding = Arc::clone(&feeding);
        std::thread::spawn(move || {
            let mut seq = 0u64;
            while feeding.load(Ordering::SeqCst) {
                let header = SamplesHeader {
                    seq,
                    timestamp_ns: 0,
                    tuning_epoch: 0,
                };
                let wire = framed::encode(FrameKind::Samples, &header.encode(&[128u8; 8192]))
                    .expect("encode");
                if writer.write_all(&wire).is_err() {
                    return;
                }
                seq += 1;
                std::thread::sleep(Duration::from_millis(10));
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

fn icaos(frames: &[Frame]) -> Vec<String> {
    events(frames)
        .iter()
        .filter(|e| e["type"] == "adsb.aircraft")
        .filter_map(|e| e["data"]["icao"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn feeds_readsb_and_reports_the_picture_as_status_and_rationed_events() {
    let mut rig = start(&[]);
    let host = &mut rig.host;

    // First file: four valid aircraft, two of them with an identity or position.
    host.wait_for("the first picture", 20, |f| {
        last_status(f).is_some_and(|s| s["mode"] == "running" && s["total"] == 4)
    });
    let status = last_status(&host.frames).expect("status");
    assert_eq!(status["receiver"], json!({"lat": 51.0, "lon": 7.0}));
    assert_eq!(status["with_position"], 2);
    assert_eq!(status["messages"], 4821);
    assert_eq!(status["error"], Value::Null);
    let first = &status["aircraft"][0];
    assert_eq!(first["icao"], "3c6444");
    assert_eq!(first["callsign"], "DLH123");
    assert_eq!(first["alt_ft"], 35_000);
    assert_eq!(first["lat"], 51.12345);
    assert!(
        status["aircraft"]
            .as_array()
            .expect("list")
            .iter()
            .any(|a| a["icao"] == "406a3d" && a["on_ground"] == true)
    );

    // Once IQ reached the decoder the second file shows up: a callsign and a position for
    // 4ca7b2 (reported at once), the others gone from the picture.
    host.wait_for("the second picture", 20, |f| {
        last_status(f).is_some_and(|s| s["total"] == 2)
    });
    host.wait_for("the late announcement", 10, |f| {
        icaos(f).contains(&"4ca7b2".to_owned())
    });
    // Each aircraft is announced once (the interval is an hour): no flood from 10 Hz polling.
    assert_eq!(icaos(&host.frames), ["3c6444", "406a3d", "4ca7b2"]);
    let all = events(&host.frames);
    let dlh = all
        .iter()
        .find(|e| e["data"]["icao"] == "3c6444")
        .expect("dlh");
    assert_eq!(dlh["data"]["summary"], "DLH123 35000 ft 451 kt 123\u{b0}");
    assert_eq!(dlh["data"]["track_deg"], 123.5);
    assert_eq!(dlh["data"]["rssi"], -12.3);
    assert_events_match_the_manifest(&host.frames);

    // The decoder got the IQ and the command line: stdin, JSON directory, receiver position.
    let log: Value = serde_json::from_str(
        &std::fs::read_to_string(rig.dir.join("superhet-adsb-i-test/fake-readsb.log"))
            .expect("fake readsb log"),
    )
    .expect("log json");
    assert!(log["received"].as_u64().expect("bytes") > 0);
    let args: Vec<&str> = log["args"]
        .as_array()
        .expect("args")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(args.windows(2).any(|w| w == ["--ifile", "-"]));
    assert!(args.windows(2).any(|w| w == ["--iformat", "UC8"]));
    assert!(args.windows(2).any(|w| w == ["--lat", "51"]));
    assert!(args.contains(&"--quiet"));
    assert!(!args.iter().any(|a| a.contains("net")), "no network ports");

    // Closing the host's side ends the adapter, the decoder and removes the working directory.
    drop(rig.child.stdin.take());
    let started = Instant::now();
    let code = loop {
        if let Some(status) = rig.child.try_wait().expect("wait") {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "adapter did not exit"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(code.success(), "{code}");
    assert!(!rig.dir.join("superhet-adsb-i-test").exists());
}

#[test]
fn a_dying_decoder_is_reported_and_ends_the_adapter() {
    let mut rig = start(&[("FAKE_READSB_EXIT_AFTER_MS", "1500")]);
    let host = &mut rig.host;
    host.wait_for("the error event", 20, |f| {
        events(f).iter().any(|e| e["type"] == "adsb.error")
    });
    host.wait_for("the error status", 10, |f| {
        last_status(f).is_some_and(|s| s["mode"] == "error")
    });
    let status = last_status(&host.frames).expect("status");
    assert!(
        status["error"]
            .as_str()
            .is_some_and(|e| e.contains("readsb ended"))
    );
    assert_events_match_the_manifest(&host.frames);
    let started = Instant::now();
    let code = loop {
        if let Some(status) = rig.child.try_wait().expect("wait") {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "adapter did not exit"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        code.code(),
        Some(3),
        "the adapter passes readsb's status on"
    );
}

#[test]
fn a_bad_config_stops_the_plugin_before_it_connects() {
    for config in [
        r#"{"lat":51.0}"#,
        r#"{"lat":95.0,"lon":7.0}"#,
        r#"{"event_interval_s":1}"#,
        r#"{"gain":40}"#,
        "[]",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"))
            .arg("adsb")
            .env_clear()
            .env("SUPERHET_SOCKET", "/nonexistent/plugin.sock")
            .env("SUPERHET_CONFIG", config)
            .output()
            .expect("run");
        assert_eq!(output.status.code(), Some(2), "{config}");
        assert!(!output.stderr.is_empty(), "{config}");
    }
}
