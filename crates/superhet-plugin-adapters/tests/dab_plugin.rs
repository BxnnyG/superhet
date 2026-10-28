//! The DAB adapter end to end: the real `superhet-plugin-adapter dab` binary against a fake host
//! (this test speaks the framed transport over a Unix socket) and a fake `welle-cli`
//! (tests/dab/fake-welle-cli, needs `python3`). Covers the IQ path into the stand-in rtl_tcp
//! server, ensemble announcement, service selection with audio / DLS / slideshow, the block scan
//! with its retunes, and a clean shutdown.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use superhet_plugin_protocol::framed::{
    self, Frame, FrameDecoder, FrameKind, HostHello, MediaHeader, PluginHello, SamplesHeader,
    TuneRequest,
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

    fn control(&mut self, message: &Value) {
        self.send(FrameKind::Control, message.to_string().as_bytes());
    }

    /// Read frames until `pred` holds for one of everything received so far.
    fn wait_for(&mut self, what: &str, secs: u64, pred: impl Fn(&[Frame]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut buf = vec![0u8; 64 * 1024];
        while !pred(&self.frames) {
            assert!(
                Instant::now() < deadline,
                "timeout waiting for {what}; events {:?}; status {:?}; logs {:?}",
                events(&self.frames),
                last_status(&self.frames),
                self.frames
                    .iter()
                    .filter(|f| f.kind == FrameKind::Log)
                    .map(|f| String::from_utf8_lossy(&f.payload).into_owned())
                    .collect::<Vec<_>>()
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

fn tunes(frames: &[Frame]) -> Vec<u64> {
    frames
        .iter()
        .filter(|f| f.kind == FrameKind::Tune)
        .map(|f| {
            serde_json::from_slice::<TuneRequest>(&f.payload)
                .expect("tune")
                .freq_hz
        })
        .collect()
}

/// Every event the adapter sent satisfies the shipped manifest's events schema.
fn assert_events_match_the_manifest(frames: &[Frame]) {
    let package = Package::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/dab"))
        .expect("plugins/dab loads");
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

fn start(blocks: &str, freq_hz: u64) -> Rig {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "superhet-dab-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dab");
    let socket = dir.join("plugin.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let path = format!(
        "{}:{}",
        fixtures.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // The adapter looks the decoder up by name: put the fake in front of PATH under that name.
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    std::os::unix::fs::symlink(fixtures.join("fake-welle-cli"), bin.join("welle-cli"))
        .expect("symlink");
    let path = format!("{}:{path}", bin.display());
    let child = Command::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"))
        .arg("dab")
        .env_clear()
        .env("PATH", path)
        .env("SUPERHET_SOCKET", &socket)
        .env("SUPERHET_FREQ_HZ", freq_hz.to_string())
        .env(
            "SUPERHET_CONFIG",
            r#"{"timing":{"no_signal_ms":1200,"stable_ms":700,"max_ms":4000}}"#,
        )
        .env("FAKE_WELLE_BLOCKS", blocks)
        .env("FAKE_WELLE_DIR", &dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn adapter");
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
        plugin_id: "org.superhet.decoder.dab".into(),
        instance_id: "i-test".into(),
        device: Some("dev0".into()),
        freq_hz: Some(freq_hz),
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

#[test]
fn tuned_block_announces_the_ensemble_plays_a_service_and_forwards_dls_and_slide() {
    let mut rig = start("5C", 178_352_000);
    let host = &mut rig.host;

    // The block the instance started on: IQ reaches welle, the ensemble is announced once.
    host.wait_for("the ensemble event", 20, |f| {
        events(f).iter().any(|e| e["type"] == "dab.ensemble")
    });
    let all = events(&host.frames);
    let ensemble = all
        .iter()
        .find(|e| e["type"] == "dab.ensemble")
        .expect("event");
    assert_eq!(ensemble["data"]["channel"], "5C");
    assert_eq!(ensemble["data"]["label"], "DR Deutschland");
    assert_eq!(ensemble["data"]["eid"], "D210");
    assert_eq!(ensemble["data"]["origin"], "tune");
    assert_eq!(
        ensemble["data"]["services"].as_array().map(Vec::len),
        Some(3)
    );
    assert!(
        ensemble["data"]["summary"]
            .as_str()
            .expect("summary")
            .starts_with("5C: DR Deutschland")
    );
    assert!(
        tunes(&host.frames).is_empty(),
        "already on the block, no retune"
    );
    let iq = std::fs::read_to_string(rig.dir.join("iq-bytes")).expect("welle read IQ");
    assert!(iq.parse::<u64>().expect("number") > 0);

    let status = last_status(&host.frames).expect("status");
    assert_eq!(status["mode"], "tuned");
    assert_eq!(status["channel"], "5C");
    assert_eq!(status["synced"], true);
    assert_eq!(status["iq_connected"], true);
    assert_eq!(status["services"][0]["label"], "Deutschlandfunk");
    assert_eq!(status["services"][0]["codec"], "DAB+");
    assert_eq!(status["services"][0]["bitrate"], 72);
    assert_eq!(status["services"][2]["audio"], false);

    // Play a service: PCM frames, the DLS event, the slide as a media object.
    host.control(&json!({"cmd": "select", "service": "D210"}));
    host.wait_for("audio, dls and slide", 20, |f| {
        let audio: usize = f
            .iter()
            .filter(|f| f.kind == FrameKind::Audio)
            .map(|f| f.payload.len())
            .sum();
        audio > 48_000 * 4 / 2
            && events(f).iter().any(|e| e["type"] == "dab.dls")
            && f.iter().any(|f| f.kind == FrameKind::Media)
    });
    let all = events(&host.frames);
    let dls = all.iter().find(|e| e["type"] == "dab.dls").expect("dls");
    assert_eq!(dls["data"]["text"], "Nachrichten: Bundestag debattiert");
    assert_eq!(dls["data"]["sid"], "D210");
    assert!(
        all.iter()
            .any(|e| e["type"] == "dab.play" && e["data"]["service"] == "Deutschlandfunk")
    );
    let media = host
        .frames
        .iter()
        .find(|f| f.kind == FrameKind::Media)
        .expect("media");
    let (head, bytes) = MediaHeader::decode(&media.payload).expect("media header");
    assert_eq!(
        (head.key.as_str(), head.mime.as_str()),
        ("slide.d210", "image/png")
    );
    assert_eq!(&bytes[..4], b"\x89PNG");
    let status = last_status(&host.frames).expect("status");
    assert_eq!(status["selected"], "D210");
    assert_eq!(
        status["services"][0]["dls"],
        "Nachrichten: Bundestag debattiert"
    );
    // Audio frames are whole stereo frames of s16le.
    assert!(
        host.frames
            .iter()
            .filter(|f| f.kind == FrameKind::Audio)
            .all(|f| f.payload.len() % 4 == 0)
    );

    // A bad control message is answered with an error event, not a crash.
    host.control(&json!({"cmd": "tune", "channel": "99Z"}));
    host.wait_for("the error event", 5, |f| {
        events(f).iter().any(|e| e["type"] == "dab.error")
    });
    assert_events_match_the_manifest(&host.frames);
}

#[test]
fn a_scan_walks_the_blocks_retunes_and_comes_back() {
    let mut rig = start("6A,6C", 178_352_000);
    let host = &mut rig.host;
    host.wait_for("the first status", 10, |f| last_status(f).is_some());

    host.control(&json!({"cmd": "scan", "channels": ["6A", "6B", "6C"]}));
    host.wait_for("the scan to finish", 60, |f| {
        events(f)
            .iter()
            .any(|e| e["type"] == "dab.scan" && e["data"]["state"] == "finished")
    });
    host.wait_for("the return to the start block", 10, |f| {
        last_status(f).is_some_and(|s| s["mode"] != "scanning" && s["channel"] == "5C")
    });
    // Started on 5C: 6A -> 6B -> 6C -> back to 5C.
    assert_eq!(
        tunes(&host.frames),
        [181_936_000, 183_648_000, 185_360_000, 178_352_000]
    );
    let all = events(&host.frames);
    let found: Vec<&str> = all
        .iter()
        .filter(|e| e["type"] == "dab.ensemble" && e["data"]["origin"] == "scan")
        .filter_map(|e| e["data"]["channel"].as_str())
        .collect();
    assert_eq!(found, ["6A", "6C"], "6B is empty");
    let done = all
        .iter()
        .find(|e| e["type"] == "dab.scan" && e["data"]["state"] == "finished")
        .expect("finished");
    assert_eq!(done["data"]["found"], 2);
    assert_eq!(done["data"]["total"], 3);

    host.wait_for("the status after the scan", 10, |f| {
        last_status(f).is_some_and(|s| s["mode"] != "scanning" && s["channel"] == "5C")
    });
    let status = last_status(&host.frames).expect("status");
    assert_events_match_the_manifest(&host.frames);
    let last = &status["last_scan"]["found"];
    assert_eq!(last.as_array().map(Vec::len), Some(2));
    assert_eq!(last[0]["channel"], "6A");
    assert_eq!(last[0]["label"], "DR Deutschland");
    assert_eq!(last[0]["services"], 3);

    // Stopping the host's side ends the adapter and its decoder.
    drop(rig.child.stdin.take());
    let started = Instant::now();
    loop {
        if rig.child.try_wait().expect("wait").is_some() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "adapter did not exit"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
