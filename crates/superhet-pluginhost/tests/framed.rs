//! The framed transport end to end: the host runs a fake plugin (tests/fixtures/fake_framed.py,
//! needs `python3`) that speaks the wire format over the instance socket. Covers the hello
//! exchange, samples in, events / audio / status / media out, control messages, retuning an
//! exclusive window, protocol violations and cleanup.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use superhet_plugin_protocol::Package;
use superhet_plugin_protocol::ctl::{InstanceInfo, InstanceState, audio_vfo_of};
use superhet_plugin_protocol::event::HostEvent;
use superhet_pluginhost::outputs::{AudioOut, AudioSpec, Outputs};
use superhet_pluginhost::supervisor::{
    Attachment, EventSink, InputHealth, InputRequest, Inputs, Policy, StartError, StartSpec,
    Supervisor,
};
use tokio::sync::{mpsc, watch};

const BLOCK: usize = 4096;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "superhet-framed-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An exclusive, framed DAB-like plugin around the fake.
fn fake_plugin(dir: &TempDir) -> Arc<Package> {
    let path = dir.0.join("plugin");
    std::fs::create_dir_all(&path).expect("plugin dir");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_framed.py"),
        path.join("fake_framed.py"),
    )
    .expect("copy fake");
    std::fs::write(
        path.join("plugin.toml"),
        r#"
[plugin]
id = "org.superhet.test.framed"
name = "framed"
version = "0.0.1"
protocol = 1
license = "AGPL-3.0-only"
type = "decoder"

[runtime]
transport = "framed"
command = ["python3", "-I", "fake_framed.py"]
heartbeat_timeout_s = 5

[input]
kind = "iq"
sample_rate = 2048000
format = "cu8"

[device]
exclusive = true
center_hz_min = 174928000
center_hz_max = 239200000
sample_rate = 2048000

[output.audio]
sample_rate = 48000
channels = 2
"#,
    )
    .expect("manifest");
    Arc::new(Package::load(&path).expect("package"))
}

#[derive(Default)]
struct Collect(Mutex<Vec<HostEvent>>);

impl EventSink for Collect {
    fn emit(&self, event: HostEvent) {
        self.0.lock().expect("lock").push(event);
    }
}

/// Records the audio the host would encode.
#[derive(Default)]
struct RecordedAudio {
    specs: Mutex<Vec<AudioSpec>>,
    bytes: Arc<Mutex<usize>>,
}

struct Tap(Arc<Mutex<usize>>);

impl AudioOut for Tap {
    fn push(&mut self, pcm: &[u8]) {
        *self.0.lock().expect("lock") += pcm.len();
    }
}

impl Outputs for RecordedAudio {
    fn open_audio(&self, spec: &AudioSpec) -> Option<Box<dyn AudioOut>> {
        self.specs.lock().expect("lock").push(spec.clone());
        Some(Box::new(Tap(Arc::clone(&self.bytes))))
    }
}

/// Feeds a steady stream of chunks and exposes the retune channel.
struct StreamInputs {
    requests: Mutex<Vec<InputRequest>>,
    retune: watch::Sender<u64>,
    released: Arc<AtomicBool>,
}

struct Released(Arc<AtomicBool>);

impl Drop for Released {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Inputs for StreamInputs {
    fn attach(&self, request: &InputRequest, _health: InputHealth) -> Option<Attachment> {
        self.requests.lock().expect("lock").push(request.clone());
        let (tx, rx) = mpsc::channel(64);
        let feeder = tx.clone();
        let task = tokio::spawn(async move {
            loop {
                if feeder.send(vec![0x7f; BLOCK]).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        struct Guard(tokio::task::JoinHandle<()>, #[allow(dead_code)] Released);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        Some(Attachment {
            chunks: rx,
            guard: Box::new(Guard(task, Released(Arc::clone(&self.released)))),
            retune: Some(self.retune.clone()),
        })
    }
}

struct Rig {
    supervisor: Supervisor,
    events: Arc<Collect>,
    audio: Arc<RecordedAudio>,
    inputs: Arc<StreamInputs>,
    _dir: TempDir,
    package: Arc<Package>,
}

fn rig() -> Rig {
    let dir = TempDir::new();
    let package = fake_plugin(&dir);
    let events = Arc::new(Collect::default());
    let audio = Arc::new(RecordedAudio::default());
    let (retune, _) = watch::channel(0);
    let inputs = Arc::new(StreamInputs {
        requests: Mutex::new(Vec::new()),
        retune,
        released: Arc::new(AtomicBool::new(false)),
    });
    let supervisor = Supervisor::with_io(
        Policy {
            max_crashes: 5,
            window: Duration::from_secs(300),
            backoff_base: Duration::from_millis(20),
            backoff_max: Duration::from_millis(80),
            stop_grace: Duration::from_millis(500),
        },
        Arc::clone(&events) as Arc<dyn EventSink>,
        Arc::clone(&inputs) as Arc<dyn Inputs>,
        Arc::clone(&audio) as Arc<dyn Outputs>,
    );
    Rig {
        supervisor,
        events,
        audio,
        inputs,
        _dir: dir,
        package,
    }
}

fn spec(config: Value) -> StartSpec {
    StartSpec {
        device: Some("rtl0".into()),
        freq_hz: Some(178_352_000),
        config,
        ..StartSpec::default()
    }
}

async fn wait_until(what: &str, mut pred: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if pred() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timeout waiting for {what}");
}

async fn status_where(rig: &Rig, id: &str, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..1000 {
        if let Some(snapshot) = rig.supervisor.status(id)
            && pred(&snapshot.status)
        {
            return snapshot.status;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "timeout waiting for {what}: {:?}",
        rig.supervisor.status(id)
    );
}

#[tokio::test]
async fn a_framed_plugin_exchanges_hello_samples_events_audio_status_and_media() {
    let rig = rig();
    let info = rig
        .supervisor
        .start(Arc::clone(&rig.package), spec(json!({"mode": "test"})))
        .expect("start");
    assert_eq!(info.audio_vfo, Some(audio_vfo_of(&info.id)));
    assert_eq!(info.freq_hz, Some(178_352_000));

    // The plugin saw the host hello with ids and configuration, then got samples.
    let status = status_where(&rig, &info.id, "samples to arrive", |s| {
        s["bytes"].as_u64().unwrap_or(0) >= 4 * BLOCK as u64
    })
    .await;
    assert_eq!(status["hello"]["protocol"], 1);
    assert_eq!(status["hello"]["instance_id"], json!(info.id));
    assert_eq!(status["hello"]["device"], "rtl0");
    assert_eq!(status["hello"]["freq_hz"], 178_352_000);
    assert_eq!(status["hello"]["config"]["mode"], "test");
    assert_eq!(status["first_seq"], 0, "numbering starts at zero");

    // The claim was requested for the chosen block and the instance is running.
    let request = rig.inputs.requests.lock().expect("lock")[0].clone();
    assert_eq!(request.claim.expect("claim").center_hz, 178_352_000);

    // Events pass the envelope check and are enriched like stdout events.
    wait_until("the ready event", || {
        rig.events.0.lock().expect("lock").iter().any(|e| {
            e.event_type == "fake.ready"
                && e.instance_id == info.id
                && e.device.as_deref() == Some("rtl0")
                && e.freq_hz == Some(178_352_000)
        })
    })
    .await;

    // Audio reaches the output with the pseudo VFO of the instance.
    wait_until("audio", || {
        *rig.audio.bytes.lock().expect("lock") >= 4 * 960 * 4
    })
    .await;
    let spec = rig.audio.specs.lock().expect("lock")[0].clone();
    assert_eq!(
        (
            spec.device.as_str(),
            spec.vfo_id,
            spec.sample_rate,
            spec.channels
        ),
        ("rtl0", audio_vfo_of(&info.id), 48_000, 2)
    );

    // Status and media are served to the control API.
    let snapshot = rig.supervisor.status(&info.id).expect("snapshot");
    assert!(snapshot.updated_unix_ms > 0);
    assert_eq!(snapshot.media.len(), 1);
    assert_eq!(snapshot.media[0].key, "slide.one");
    assert_eq!(snapshot.media[0].mime, "image/png");
    assert_eq!(snapshot.media[0].revision, 1);
    let (mime, revision, bytes) = rig.supervisor.media(&info.id, "slide.one").expect("media");
    assert_eq!((mime.as_str(), revision), ("image/png", 1));
    assert_eq!(&bytes[..4], b"\x89PNG");
    assert!(rig.supervisor.media(&info.id, "nope").is_none());

    // Control messages reach the plugin and come back in its status.
    rig.supervisor
        .control(&info.id, &json!({"echo": "hello plugin"}))
        .expect("control");
    status_where(&rig, &info.id, "the echo", |s| s["echo"] == "hello plugin").await;
    assert!(matches!(
        rig.supervisor.control(&info.id, &json!("not an object")),
        Err(StartError::Invalid(_))
    ));
    assert!(matches!(
        rig.supervisor.control("i-nope", &json!({})),
        Err(StartError::NotFound)
    ));

    // Stopping ends the session, drops the stored state and releases the input.
    let stopped = rig.supervisor.stop(&info.id).await.expect("stop");
    assert_eq!(stopped.state, InstanceState::Stopped);
    wait_until("the input to be released", || {
        rig.inputs.released.load(Ordering::SeqCst)
    })
    .await;
    assert!(rig.supervisor.status(&info.id).is_none());
}

#[tokio::test]
async fn an_exclusive_plugin_may_move_its_window_inside_the_declared_range_only() {
    let rig = rig();
    let mut retuned = rig.inputs.retune.subscribe();
    let info = rig
        .supervisor
        .start(Arc::clone(&rig.package), spec(Value::Null))
        .expect("start");
    status_where(&rig, &info.id, "the session", |s| s["hello"].is_object()).await;

    // Outside Band III: refused, nothing moves.
    rig.supervisor
        .control(&info.id, &json!({"tune": 100_000_000u64}))
        .expect("control");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        rig.supervisor.get(&info.id).expect("info").freq_hz,
        Some(178_352_000)
    );

    // Inside: the claim follows, the instance reports the new frequency, events carry it.
    rig.supervisor
        .control(&info.id, &json!({"tune": 227_360_000u64}))
        .expect("control");
    tokio::time::timeout(Duration::from_secs(5), retuned.changed())
        .await
        .expect("retune signalled")
        .expect("watch open");
    assert_eq!(*retuned.borrow(), 227_360_000);
    assert_eq!(
        rig.supervisor.get(&info.id).expect("info").freq_hz,
        Some(227_360_000)
    );
    rig.supervisor.shutdown().await;
}

#[tokio::test]
async fn protocol_violations_and_a_wrong_hello_end_the_session_as_a_crash() {
    let rig = rig();
    // A plugin that speaks another protocol version never gets going.
    let info = rig
        .supervisor
        .start(Arc::clone(&rig.package), spec(json!({"hello_protocol": 2})))
        .expect("start");
    let failed = wait_failed(&rig, &info.id).await;
    assert!(
        failed
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("protocol 2"),
        "{failed:?}"
    );
    rig.supervisor.stop(&info.id).await.expect("stop");

    // A frame kind that only the host may send ends the session and the process is restarted.
    let info = rig
        .supervisor
        .start(Arc::clone(&rig.package), spec(Value::Null))
        .expect("start");
    status_where(&rig, &info.id, "the session", |s| s["hello"].is_object()).await;
    rig.supervisor
        .control(&info.id, &json!({"violate": true}))
        .expect("control");
    let failed = wait_failed(&rig, &info.id).await;
    assert!(
        failed
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("Samples"),
        "{failed:?}"
    );
    // After the backoff a fresh process comes up with a fresh session.
    status_where(&rig, &info.id, "a new session", |s| {
        s["hello"].is_object() && s["bytes"].as_u64().unwrap_or(0) > 0
    })
    .await;
    assert!(rig.supervisor.get(&info.id).expect("info").restarts >= 1);
    rig.supervisor.shutdown().await;
}

async fn wait_failed(rig: &Rig, id: &str) -> InstanceInfo {
    for _ in 0..1000 {
        if let Some(info) = rig.supervisor.get(id)
            && info.last_error.is_some()
        {
            return info;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the instance never failed: {:?}", rig.supervisor.get(id));
}

#[tokio::test]
async fn the_control_api_serves_status_media_and_control() {
    use superhet_plugin_protocol::ctl::MediaReplyHeader;
    use superhet_pluginhost::ctl::{Host, handle, handle_bytes};
    use superhet_pluginhost::registry::Registry;

    let rig = rig();
    let registry_dir = TempDir::new();
    let registry = Arc::new(Registry::open(&registry_dir.0).expect("registry"));
    registry.install(&rig.package.dir).expect("install");
    let host = Host {
        registry,
        supervisor: rig.supervisor.clone(),
        store: Default::default(),
    };
    let call = |cmd: &'static str, body: Value| {
        let host = host.clone();
        async move { handle(&host, cmd, body.to_string().as_bytes()).await }
    };

    let started = call(
        "start",
        json!({"v": 1, "plugin_id": "org.superhet.test.framed", "device": "rtl0", "freq_hz": 178_352_000u64}),
    )
    .await;
    assert_eq!(started["ok"], true, "{started}");
    let id = started["instance"]["id"].as_str().expect("id").to_owned();
    assert!(started["instance"]["audio_vfo"].is_u64());

    status_where(&rig, &id, "the session", |s| s["hello"].is_object()).await;
    let status = call("status", json!({"v": 1, "id": id})).await;
    assert_eq!(status["ok"], true);
    assert_eq!(status["status"]["hello"]["protocol"], 1);
    assert_eq!(status["media"][0]["key"], "slide.one");
    assert_eq!(status["media"][0]["size"], 8);

    let sent = call(
        "control",
        json!({"v": 1, "id": id, "message": {"echo": "via ctl"}}),
    )
    .await;
    assert_eq!(sent["ok"], true, "{sent}");
    status_where(&rig, &id, "the echo", |s| s["echo"] == "via ctl").await;

    let media = handle_bytes(
        &host,
        "media",
        json!({"v": 1, "id": id, "key": "slide.one"})
            .to_string()
            .as_bytes(),
    )
    .await;
    let (header, bytes) = MediaReplyHeader::decode(&media).expect("media reply");
    assert_eq!((header.mime.as_str(), header.revision), ("image/png", 1));
    assert_eq!(bytes, b"\x89PNGfake");
    let missing = handle_bytes(
        &host,
        "media",
        json!({"v": 1, "id": id, "key": "other"})
            .to_string()
            .as_bytes(),
    )
    .await;
    let missing: Value = serde_json::from_slice(&missing).expect("error reply is JSON");
    assert_eq!(missing["code"], "not_found");

    for (cmd, body) in [
        ("status", json!({"v": 1, "id": "i-nope"})),
        ("control", json!({"v": 1, "id": "i-nope", "message": {}})),
    ] {
        let reply = call(cmd, body).await;
        assert_eq!(reply["code"], "not_found", "{cmd}: {reply}");
    }
    rig.supervisor.shutdown().await;
}
