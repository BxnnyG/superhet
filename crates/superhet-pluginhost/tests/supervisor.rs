//! Supervisor behaviour with real child processes: restart with backoff, the five-crashes rule,
//! invalid event handling, stop, watchdog and refusals. Needs `sh` and `python3` (for the
//! reference plugin).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use superhet_plugin_protocol::Package;
use superhet_plugin_protocol::ctl::{InstanceInfo, InstanceState};
use superhet_plugin_protocol::event::HostEvent;
use superhet_pluginhost::supervisor::{EventSink, Policy, StartError, StartSpec, Supervisor};

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

    let framed = shell_plugin(&dir, "true", "transport = \"framed\"");
    assert!(matches!(
        supervisor.start(framed, StartSpec::default()),
        Err(StartError::Unsupported(_))
    ));
    supervisor.shutdown().await;
}
