//! Supervisor behaviour with real child processes: restart with backoff, the five-crashes rule,
//! invalid event handling, stop, watchdog and refusals. Needs `sh` and `python3` (for the
//! reference plugin).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use superhet_plugin_protocol::Package;
use superhet_plugin_protocol::ctl::{InstanceInfo, InstanceState};
use superhet_plugin_protocol::event::HostEvent;
use superhet_pluginhost::supervisor::{
    Attachment, EventSink, InputHealth, InputRequest, Inputs, Policy, StartError, StartSpec,
    Supervisor,
};
use tokio::sync::mpsc;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("superhet-ph-{}", uuid_like()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// Write a plugin whose command is `sh -c <script>` and load it.
fn shell_plugin(dir: &TempDir, script: &str, extra_runtime: &str) -> Arc<Package> {
    shell_plugin_with(dir, script, extra_runtime, &[])
}

/// Like [`shell_plugin`], with extra files in the plugin directory.
fn shell_plugin_with(
    dir: &TempDir,
    script: &str,
    extra_runtime: &str,
    files: &[(&str, &str)],
) -> Arc<Package> {
    let name = uuid_like();
    let path = dir.0.join(&name);
    std::fs::create_dir_all(&path).expect("plugin dir");
    let manifest = format!(
        r#"
[plugin]
id = "org.superhet.test.shell"
name = "shell"
version = "0.0.1"
protocol = 1
license = "AGPL-3.0-only"
type = "decoder"

[runtime]
command = ["sh", "-c", {script:?}]
{extra_runtime}

[input]
kind = "audio"
sample_rate = 48000
format = "s16le"
"#
    );
    std::fs::write(path.join("plugin.toml"), manifest).expect("manifest");
    for (name, text) in files {
        std::fs::write(path.join(name), text).expect("plugin file");
    }
    Arc::new(Package::load(&path).expect("package"))
}

fn echo_plugin() -> Arc<Package> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/example-echo");
    Arc::new(Package::load(&dir).expect("example-echo"))
}

#[derive(Default)]
struct Collect(Mutex<Vec<HostEvent>>);

impl Collect {
    fn events(&self) -> Vec<HostEvent> {
        self.0.lock().expect("lock").clone()
    }
}

impl EventSink for Collect {
    fn emit(&self, event: HostEvent) {
        self.0.lock().expect("lock").push(event);
    }
}

fn fast_policy() -> Policy {
    Policy {
        max_crashes: 5,
        window: Duration::from_secs(300),
        backoff_base: Duration::from_millis(20),
        backoff_max: Duration::from_millis(80),
        stop_grace: Duration::from_millis(300),
    }
}

async fn wait_for(
    supervisor: &Supervisor,
    id: &str,
    what: &str,
    pred: impl Fn(&InstanceInfo) -> bool,
) -> InstanceInfo {
    for _ in 0..1000 {
        if let Some(info) = supervisor.get(id)
            && pred(&info)
        {
            return info;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timeout waiting for {what}: {:?}", supervisor.get(id));
}

#[tokio::test]
async fn five_crashes_in_the_window_end_in_error() {
    let dir = TempDir::new();
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    let mut states = supervisor.subscribe();
    let info = supervisor
        .start(shell_plugin(&dir, "exit 1", ""), StartSpec::default())
        .expect("start");
    let info = wait_for(&supervisor, &info.id, "error", |i| {
        i.state == InstanceState::Error
    })
    .await;
    assert_eq!(info.crashes_in_window, 5);
    // 5 crashes mean 4 restarts: the fifth crash is final.
    assert_eq!(info.restarts, 4);
    assert!(
        info.last_error
            .as_deref()
            .unwrap_or("")
            .contains("giving up"),
        "{:?}",
        info.last_error
    );
    // It stays down.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(supervisor.get(&info.id).expect("still listed").restarts, 4);

    // The state feed saw backoff phases with growing delays, then error.
    let mut delays = Vec::new();
    let mut last = None;
    while let Ok(update) = states.try_recv() {
        if update.state == InstanceState::Backoff {
            delays.push(update.retry_in_ms.expect("retry delay"));
        }
        last = Some(update.state);
    }
    assert_eq!(delays, [20, 40, 80, 80]);
    assert_eq!(last, Some(InstanceState::Error));
}

#[tokio::test]
async fn killed_process_is_restarted_with_backoff() {
    let dir = TempDir::new();
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    let info = supervisor
        .start(shell_plugin(&dir, "sleep 60", ""), StartSpec::default())
        .expect("start");
    let running = wait_for(&supervisor, &info.id, "running", |i| {
        i.state == InstanceState::Running
    })
    .await;
    let old_pid = running.pid.expect("pid");
    let status = std::process::Command::new("kill")
        .args(["-9", &old_pid.to_string()])
        .status()
        .expect("kill");
    assert!(status.success());
    let again = wait_for(&supervisor, &info.id, "restart", |i| {
        i.state == InstanceState::Running && i.pid != Some(old_pid)
    })
    .await;
    assert_eq!(again.restarts, 1);
    assert_eq!(again.crashes_in_window, 1);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn crashes_outside_the_window_do_not_add_up() {
    let dir = TempDir::new();
    let policy = Policy {
        max_crashes: 3,
        window: Duration::from_millis(60),
        backoff_base: Duration::from_millis(120),
        backoff_max: Duration::from_millis(120),
        stop_grace: Duration::from_millis(300),
    };
    let supervisor = Supervisor::new(policy, Arc::new(Collect::default()));
    let info = supervisor
        .start(shell_plugin(&dir, "exit 1", ""), StartSpec::default())
        .expect("start");
    let info = wait_for(&supervisor, &info.id, "six restarts", |i| i.restarts >= 6).await;
    assert_ne!(info.state, InstanceState::Error);
    assert!(info.crashes_in_window < 3);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn events_are_validated_counted_and_enriched() {
    let dir = TempDir::new();
    let sink = Arc::new(Collect::default());
    let supervisor = Supervisor::new(fast_policy(), sink.clone());
    let script = r#"printf '%s\n' '{"v":1,"type":"t.ok","data":{"n":1}}' 'garbage' '{"v":7,"type":"t.bad"}' '{"v":1,"type":"heartbeat"}'; sleep 60"#;
    let info = supervisor
        .start(
            shell_plugin(&dir, script, ""),
            StartSpec {
                device: Some("dev0".into()),
                vfo: None,
                freq_hz: Some(144_800_000),
                config: Value::Null,
            },
        )
        .expect("start");
    let info = wait_for(&supervisor, &info.id, "counters", |i| {
        i.events_ok == 1 && i.events_invalid == 2
    })
    .await;
    assert_eq!(info.state, InstanceState::Running);
    let events = sink.events();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.event_type, "t.ok");
    assert_eq!(event.plugin_id, "org.superhet.test.shell");
    assert_eq!(event.instance_id, info.id);
    assert_eq!(event.device.as_deref(), Some("dev0"));
    assert_eq!(event.freq_hz, Some(144_800_000));
    assert_eq!(event.data["n"], 1);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn plugins_get_a_scrubbed_environment() {
    let dir = TempDir::new();
    let sink = Arc::new(Collect::default());
    let supervisor = Supervisor::new(fast_policy(), sink.clone());
    // HOME is not on the pass-through list and must not reach the plugin; the instance variables
    // must.
    let script = r#"printf '{"v":1,"type":"env","data":{"nats":"%s","id":"%s","cfg":%s}}\n' "${HOME:-absent}" "$SUPERHET_INSTANCE_ID" "$SUPERHET_CONFIG"; sleep 60"#;
    let info = supervisor
        .start(
            shell_plugin(&dir, script, ""),
            StartSpec {
                config: json!({"k": 1}),
                ..StartSpec::default()
            },
        )
        .expect("start");
    wait_for(&supervisor, &info.id, "event", |i| i.events_ok == 1).await;
    let events = sink.events();
    assert_eq!(events[0].data["nats"], "absent");
    assert_eq!(events[0].data["id"], info.id.as_str());
    assert_eq!(events[0].data["cfg"], json!({"k": 1}));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn stop_ends_the_process_and_forgets_the_instance() {
    let dir = TempDir::new();
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));

    // Reference plugin: leaves on stdin EOF by itself.
    let echo = supervisor
        .start(echo_plugin(), StartSpec::default())
        .expect("start echo");
    wait_for(&supervisor, &echo.id, "running", |i| {
        i.state == InstanceState::Running
    })
    .await;
    let stopped = supervisor.stop(&echo.id).await.expect("stopped");
    assert_eq!(stopped.state, InstanceState::Stopped);
    assert!(supervisor.get(&echo.id).is_none());

    // A plugin that ignores stdin is killed after the grace period.
    let stubborn = supervisor
        .start(shell_plugin(&dir, "sleep 60", ""), StartSpec::default())
        .expect("start sleeper");
    let running = wait_for(&supervisor, &stubborn.id, "running", |i| {
        i.state == InstanceState::Running
    })
    .await;
    let pid = running.pid.expect("pid");
    let stopped = supervisor.stop(&stubborn.id).await.expect("stopped");
    assert_eq!(stopped.state, InstanceState::Stopped);
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists() || zombie_or_gone(pid),
        "process {pid} still alive"
    );
    assert!(supervisor.stop("i-unknown").await.is_none());
}

fn zombie_or_gone(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |s| s.contains(") Z"))
}

#[tokio::test]
async fn silent_plugin_is_killed_by_the_watchdog() {
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    let dir = TempDir::new();
    // A plugin that never writes, with a 1 s heartbeat timeout.
    let package = shell_plugin(&dir, "sleep 60", "heartbeat_timeout_s = 1");
    let info = supervisor
        .start(package, StartSpec::default())
        .expect("start");
    let info = wait_for(&supervisor, &info.id, "watchdog restart", |i| {
        i.restarts >= 1
    })
    .await;
    assert!(
        info.last_error.as_deref().unwrap_or("").contains("silent"),
        "{:?}",
        info.last_error
    );
    assert!(info.crashes_in_window >= 1);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn reference_plugin_stays_alive_through_heartbeats() {
    let dir = TempDir::new();
    // Same as the reference manifest but with a 2 s watchdog; the plugin beats once a second.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/example-echo");
    let copy = dir.0.join("echo");
    copy_dir(&src, &copy);
    let manifest = std::fs::read_to_string(copy.join("plugin.toml")).expect("manifest");
    std::fs::write(
        copy.join("plugin.toml"),
        manifest.replace(
            "transport = \"simple\"",
            "transport = \"simple\"\nheartbeat_timeout_s = 2",
        ),
    )
    .expect("write");
    let package = Arc::new(Package::load(&copy).expect("package"));
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    let info = supervisor
        .start(package, StartSpec::default())
        .expect("start");
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let now = supervisor.get(&info.id).expect("listed");
    assert_eq!((now.state, now.restarts), (InstanceState::Running, 0));
    supervisor.shutdown().await;
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read_dir") {
        let entry = entry.expect("entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).expect("copy");
        }
    }
}

#[tokio::test]
async fn error_instances_can_be_restarted() {
    let dir = TempDir::new();
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    let info = supervisor
        .start(shell_plugin(&dir, "exit 1", ""), StartSpec::default())
        .expect("start");
    wait_for(&supervisor, &info.id, "error", |i| {
        i.state == InstanceState::Error
    })
    .await;
    let restarted = supervisor.restart(&info.id).expect("restart");
    assert_eq!(restarted.crashes_in_window, 0);
    // It crashes through the whole sequence again.
    let again = wait_for(&supervisor, &info.id, "second error", |i| {
        i.state == InstanceState::Error && i.restarts >= 8
    })
    .await;
    assert_eq!(again.crashes_in_window, 5);
    // Running instances cannot be "restarted".
    let running = supervisor
        .start(shell_plugin(&dir, "sleep 60", ""), StartSpec::default())
        .expect("start");
    assert!(matches!(
        supervisor.restart(&running.id),
        Err(StartError::Conflict(_))
    ));
    assert!(matches!(
        supervisor.restart("i-nope"),
        Err(StartError::NotFound)
    ));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn refusals_and_exclusive_claims() {
    let dir = TempDir::new();
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    let exclusive = shell_plugin(&dir, "sleep 60", "");
    // Re-write as an exclusive IQ plugin.
    let path = exclusive.dir.clone();
    std::fs::write(
        path.join("plugin.toml"),
        r#"
[plugin]
id = "org.superhet.test.claim"
name = "claim"
version = "0.0.1"
protocol = 1
license = "AGPL-3.0-only"
type = "decoder"

[runtime]
command = ["sh", "-c", "sleep 60"]

[input]
kind = "iq"
sample_rate = 2400000
format = "cu8"

[device]
exclusive = true
center_hz = 1090000000
sample_rate = 2400000
"#,
    )
    .expect("write");
    let exclusive = Arc::new(Package::load(&path).expect("exclusive package"));
    let spec = |device: &str| StartSpec {
        device: Some(device.into()),
        ..StartSpec::default()
    };
    let first = supervisor
        .start(Arc::clone(&exclusive), spec("rtl0"))
        .expect("first claim");
    assert!(first.exclusive);
    match supervisor.start(Arc::clone(&exclusive), spec("rtl0")) {
        Err(StartError::Conflict(msg)) => assert!(msg.contains(&first.id), "{msg}"),
        other => panic!("expected conflict, got {other:?}"),
    }
    // Another device is free.
    supervisor
        .start(Arc::clone(&exclusive), spec("rtl1"))
        .expect("other device");

    supervisor.shutdown().await;
}

/// An input provider that records the request, sends fixed chunks and notes when it is released.
struct FakeInputs {
    requests: Mutex<Vec<InputRequest>>,
    chunks: Vec<Vec<u8>>,
    released: Arc<AtomicBool>,
}

struct Released(Arc<AtomicBool>);

impl Drop for Released {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Inputs for FakeInputs {
    fn attach(&self, request: &InputRequest, health: InputHealth) -> Option<Attachment> {
        self.requests.lock().expect("lock").push(request.clone());
        let (tx, rx) = mpsc::channel(16);
        for chunk in &self.chunks {
            tx.try_send(chunk.clone()).expect("queue");
        }
        let _ = health.status.send(Some("daemon says hello".into()));
        // The sender stays alive in the guard: the input stays open until the guard is dropped.
        Some(Attachment {
            retune: None,
            chunks: rx,
            guard: Box::new((tx, Released(Arc::clone(&self.released)))),
        })
    }
}

fn fake_inputs(chunks: Vec<Vec<u8>>) -> Arc<FakeInputs> {
    Arc::new(FakeInputs {
        requests: Mutex::new(Vec::new()),
        chunks,
        released: Arc::new(AtomicBool::new(false)),
    })
}

#[tokio::test]
async fn bound_instances_get_their_input_on_stdin_and_release_it_when_stopped() {
    let dir = TempDir::new();
    let inputs = fake_inputs(vec![b"hello ".to_vec(), b"plugin".to_vec()]);
    let supervisor = Supervisor::with_inputs(
        fast_policy(),
        Arc::new(Collect::default()),
        Arc::clone(&inputs) as Arc<dyn Inputs>,
    );
    // `cat` copies stdin into a file in the plugin directory until stdin is closed.
    let package = shell_plugin(&dir, "cat > fed.bin", "");
    let info = supervisor
        .start(
            Arc::clone(&package),
            StartSpec {
                device: Some("rtl0".into()),
                vfo: Some(3),
                ..StartSpec::default()
            },
        )
        .expect("start");
    assert_eq!(info.vfo, Some(3));
    let info = wait_for(&supervisor, &info.id, "input status", |i| {
        i.input_error.is_some()
    })
    .await;
    assert_eq!(info.input_error.as_deref(), Some("daemon says hello"));

    let request = inputs.requests.lock().expect("lock")[0].clone();
    assert_eq!(request.device, "rtl0");
    assert_eq!(request.tap.id, info.id);
    assert_eq!(request.tap.vfo, Some(3));
    assert_eq!(request.tap.rate, 48_000);
    assert!(request.claim.is_none());

    let fed = package.dir.join("fed.bin");
    for _ in 0..200 {
        if std::fs::read(&fed).is_ok_and(|b| b.len() == 12) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(std::fs::read(&fed).expect("fed"), b"hello plugin");

    assert!(!inputs.released.load(Ordering::SeqCst));
    supervisor.stop(&info.id).await.expect("stopped");
    assert!(inputs.released.load(Ordering::SeqCst), "input released");
}

#[tokio::test]
async fn bindings_are_checked_and_exclusive_plugins_claim_the_window() {
    let dir = TempDir::new();
    let inputs = fake_inputs(Vec::new());
    let supervisor = Supervisor::with_inputs(
        fast_policy(),
        Arc::new(Collect::default()),
        Arc::clone(&inputs) as Arc<dyn Inputs>,
    );
    // A VFO needs its device.
    let err = supervisor
        .start(
            shell_plugin(&dir, "sleep 60", ""),
            StartSpec {
                vfo: Some(1),
                ..StartSpec::default()
            },
        )
        .expect_err("vfo without device");
    assert!(matches!(err, StartError::Invalid(_)), "{err:?}");

    // A tunable exclusive plugin: the instance picks the centre inside the declared range.
    let exclusive = shell_plugin(&dir, "sleep 60", "");
    std::fs::write(
        exclusive.dir.join("plugin.toml"),
        r#"
[plugin]
id = "org.superhet.test.dab"
name = "dab"
version = "0.0.1"
protocol = 1
license = "AGPL-3.0-only"
type = "decoder"

[runtime]
command = ["sh", "-c", "sleep 60"]

[input]
kind = "iq"
sample_rate = 2048000
format = "cu8"

[device]
exclusive = true
center_hz_min = 174928000
center_hz_max = 239200000
sample_rate = 2048000
"#,
    )
    .expect("manifest");
    let exclusive = Arc::new(Package::load(&exclusive.dir).expect("package"));
    let on = |device: Option<&str>, freq_hz: Option<u64>| StartSpec {
        device: device.map(Into::into),
        freq_hz,
        ..StartSpec::default()
    };
    for bad in [
        on(None, Some(200_000_000)),
        on(Some("rtl0"), None),
        on(Some("rtl0"), Some(100_000_000)),
    ] {
        let err = supervisor
            .start(Arc::clone(&exclusive), bad)
            .expect_err("invalid binding");
        assert!(matches!(err, StartError::Invalid(_)), "{err:?}");
    }
    let info = supervisor
        .start(exclusive, on(Some("rtl0"), Some(190_640_000)))
        .expect("claim");
    wait_for(&supervisor, &info.id, "attached", |i| {
        i.input_error.is_some()
    })
    .await;
    let request = inputs.requests.lock().expect("lock")[0].clone();
    let claim = request.claim.expect("claim");
    assert_eq!(
        (claim.center_hz, claim.sample_rate),
        (190_640_000, Some(2_048_000))
    );
    assert_eq!(request.tap.center_hz, Some(190_640_000));
    assert_eq!(request.tap.vfo, None);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn stopping_kills_the_whole_process_group() {
    let dir = TempDir::new();
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    // The plugin starts a helper that would outlive it, like a wrapped decoder binary does.
    let package = shell_plugin(&dir, "sleep 300 & echo $! > helper.pid; wait", "");
    let info = supervisor
        .start(Arc::clone(&package), StartSpec::default())
        .expect("start");
    let pid_file = package.dir.join("helper.pid");
    let mut helper = None;
    for _ in 0..300 {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok())
        {
            helper = Some(pid);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let helper = helper.expect("helper pid");
    assert!(!zombie_or_gone(helper), "helper should be running");
    supervisor.stop(&info.id).await.expect("stopped");
    for _ in 0..200 {
        if zombie_or_gone(helper) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        zombie_or_gone(helper),
        "helper {helper} survived the plugin"
    );
}

#[tokio::test]
async fn reconfigure_restarts_with_the_new_config_and_keeps_the_instance() {
    let dir = TempDir::new();
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    let package = shell_plugin(&dir, "echo \"$SUPERHET_CONFIG\" >> cfg.log; sleep 300", "");
    let info = supervisor
        .start(
            Arc::clone(&package),
            StartSpec {
                config: json!({"n": 1}),
                ..StartSpec::default()
            },
        )
        .expect("start");
    let first = wait_for(&supervisor, &info.id, "running", |i| {
        i.state == InstanceState::Running
    })
    .await;
    let log = package.dir.join("cfg.log");
    let lines = || {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    for _ in 0..200 {
        if !lines().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let after = supervisor
        .reconfigure(&info.id, json!({"n": 2}))
        .expect("reconfigure");
    assert_eq!(after.id, info.id);
    assert_eq!(after.config, json!({"n": 2}));
    assert_eq!(after.reconfigures, 1);
    for _ in 0..300 {
        if lines().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(lines(), [r#"{"n":1}"#, r#"{"n":2}"#]);
    let running = wait_for(&supervisor, &info.id, "running again", |i| {
        i.state == InstanceState::Running && i.pid != first.pid
    })
    .await;
    // Not counted as a crash.
    assert_eq!(running.restarts, 0);
    assert_eq!(running.crashes_in_window, 0);
    assert_eq!(running.created_unix_ms, first.created_unix_ms);

    assert!(matches!(
        supervisor.reconfigure(&info.id, json!([1])),
        Err(StartError::Invalid(_))
    ));
    assert!(matches!(
        supervisor.reconfigure("i-nope", json!({})),
        Err(StartError::NotFound)
    ));
    supervisor.stop(&info.id).await.expect("stopped");
}

#[tokio::test]
async fn reconfigure_revives_an_instance_in_error() {
    let dir = TempDir::new();
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    let package = shell_plugin(
        &dir,
        "if echo \"$SUPERHET_CONFIG\" | grep -q bad; then exit 2; fi; sleep 300",
        "",
    );
    let info = supervisor
        .start(
            package,
            StartSpec {
                config: json!({"mode": "bad"}),
                ..StartSpec::default()
            },
        )
        .expect("start");
    wait_for(&supervisor, &info.id, "error", |i| {
        i.state == InstanceState::Error
    })
    .await;
    supervisor
        .reconfigure(&info.id, json!({"mode": "good"}))
        .expect("reconfigure");
    let running = wait_for(&supervisor, &info.id, "running", |i| {
        i.state == InstanceState::Running
    })
    .await;
    assert_eq!(running.crashes_in_window, 0);
    supervisor.stop(&info.id).await.expect("stopped");
}

#[tokio::test]
async fn reconfigure_refuses_a_config_that_breaks_the_schema_and_changes_nothing() {
    let dir = TempDir::new();
    let supervisor = Supervisor::new(fast_policy(), Arc::new(Collect::default()));
    let package = shell_plugin_with(
        &dir,
        "sleep 300",
        "\n[config]\nschema = \"config.json\"\n",
        &[(
            "config.json",
            r#"{"type":"object","properties":{"baud":{"type":"array","items":{"enum":[512,1200]}}},"additionalProperties":false}"#,
        )],
    );
    let info = supervisor
        .start(package, StartSpec::default())
        .expect("start");
    let first = wait_for(&supervisor, &info.id, "running", |i| {
        i.state == InstanceState::Running
    })
    .await;

    let Err(StartError::Config(issue)) =
        supervisor.reconfigure(&info.id, json!({"baud": [512, 700]}))
    else {
        panic!("a bad value must be refused");
    };
    assert_eq!(issue.field, "baud.1");
    let Err(StartError::Config(issue)) = supervisor.reconfigure(&info.id, json!({"bad": 1})) else {
        panic!("an unknown key must be refused");
    };
    assert_eq!(issue.field, "bad");

    // Nothing was applied: same config, no restart counted, same process.
    let now = supervisor.get(&info.id).expect("instance");
    assert_eq!(now.config, Value::Null);
    assert_eq!(now.reconfigures, 0);
    assert_eq!(now.pid, first.pid);

    // A valid one still goes through.
    let ok = supervisor
        .reconfigure(&info.id, json!({"baud": [1200]}))
        .expect("valid config");
    assert_eq!(ok.reconfigures, 1);
    supervisor.stop(&info.id).await.expect("stopped");
}
