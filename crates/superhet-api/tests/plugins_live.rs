//! End-to-end test of the plugin wiring: the reference plugin `example-echo` is installed through
//! the admin API, started on a VFO of a file-backed device and its events show up in the decoder
//! history. Real PostgreSQL and NATS (JetStream), real `superhet-api`, `superhet-sdrd`,
//! `superhet-pluginhost` and `superhet-worker` binaries; the plugin needs `python3` on the host
//! (the production image does not ship it; the test is skipped without it).
//!
//! Skipped unless `DATABASE_URL` and `SUPERHET_TEST_NATS_URL` are set. The other binaries must
//! have been built (`cargo build -p superhet-sdrd -p superhet-pluginhost -p superhet-worker`);
//! they are looked up next to the API binary.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{Proc, free_port, req, wait_ready};
use serde_json::{Value, json};

const PLUGIN: &str = "org.superhet.example.echo";
const RATE: u32 = 1_024_000;
const CENTER: u64 = 433_920_000;

fn write_tone(path: &Path) {
    let samples = 8 * 4096;
    let mut bytes = Vec::with_capacity(samples * 2);
    for i in 0..samples {
        let ph = std::f64::consts::TAU * 50_000.0 * i as f64 / f64::from(RATE);
        bytes.push((127.5 + 127.0 * ph.cos()).round() as u8);
        bytes.push((127.5 + 127.0 * ph.sin()).round() as u8);
    }
    std::fs::write(path, bytes).expect("write fixture");
}

fn sibling_binary(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_superhet-api")).with_file_name(name);
    assert!(
        path.exists(),
        "{} not found: run `cargo build -p superhet-sdrd -p superhet-pluginhost -p superhet-worker` first",
        path.display()
    );
    path
}

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Spawn the plugin host on the shared database, bus and plugin directory.
fn spawn_pluginhost(nats: &str, db: &str, plugin_dir: &Path) -> Proc {
    let port = free_port();
    Proc(
        Command::new(sibling_binary("superhet-pluginhost"))
            .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
            .env("SUPERHET_NATS_URL", nats)
            .env("SUPERHET_DATABASE_URL", db)
            .env("SUPERHET_PLUGIN_DIR", plugin_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pluginhost"),
    )
}

/// Both tests start a plugin host on the same NATS server. The hosts answer the same
/// unqueued control subjects, so a request could be answered by the other test's host;
/// run the tests one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Poll `f` until it returns `Some`.
async fn eventually<T>(what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(value) = f() {
            return value;
        }
        assert!(Instant::now() < deadline, "timeout waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn events(port: u16, cookie: &str, query: &str) -> Vec<Value> {
    let reply = req(
        port,
        "GET",
        &format!("/api/v1/decoder-events?limit=1000&{query}"),
        Some(cookie),
        None,
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    reply.json()["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn example_echo_on_a_vfo_reaches_the_decoder_history() {
    let _serial = SERIAL.lock().await;
    let (Ok(nats_url), true) = (std::env::var("SUPERHET_TEST_NATS_URL"), python_available()) else {
        eprintln!("SUPERHET_TEST_NATS_URL not set or python3 missing; skipping");
        return;
    };
    let Some(db) = common::TestDb::create("plug").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let pid = std::process::id();
    let device = format!("plug{pid}");
    let tone = std::env::temp_dir().join(format!("superhet-plug-{pid}.cu8"));
    write_tone(&tone);
    let plugin_dir = std::env::temp_dir().join(format!("superhet-plug-{pid}-plugins"));
    let _ = std::fs::remove_dir_all(&plugin_dir);

    // The stack: sdrd (file device), API (migrates the database), worker, plugin host.
    let sdrd_port = free_port();
    let _sdrd = Proc(
        Command::new(sibling_binary("superhet-sdrd"))
            .env("SUPERHET_LISTEN", format!("127.0.0.1:{sdrd_port}"))
            .env("SUPERHET_NATS_URL", &nats_url)
            .env("SUPERHET_DEVICE_ID", &device)
            .env(
                "SUPERHET_DEVICE",
                format!(
                    "file:{}?format=cu8&rate={RATE}&freq={CENTER}&block=4096",
                    tone.display()
                ),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sdrd"),
    );
    let (_api, api) = {
        let (proc, port) = common::spawn_api(&db.url, |c| {
            c.env("SUPERHET_NATS_URL", &nats_url);
        });
        (proc, port)
    };
    wait_ready(sdrd_port, "sdrd").await;
    wait_ready(api, "api").await;
    let worker_port = free_port();
    let _worker = Proc(
        Command::new(sibling_binary("superhet-worker"))
            .env("SUPERHET_LISTEN", format!("127.0.0.1:{worker_port}"))
            .env("SUPERHET_NATS_URL", &nats_url)
            .env("SUPERHET_DATABASE_URL", &db.url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn worker"),
    );
    let mut pluginhost = spawn_pluginhost(&nats_url, &db.url, &plugin_dir);

    let admin = common::setup_admin(api);
    let operator = common::make_user(api, &admin, "olga", "operator");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/example-echo");

    // Roles: operators and admins list, only admins install and remove.
    let install_body = json!({"path": source});
    let denied = req(
        api,
        "POST",
        "/api/v1/plugins",
        Some(&operator),
        Some(&install_body),
    );
    assert_eq!(denied.status, 403, "{denied:?}");
    let denied = req(
        api,
        "DELETE",
        &format!("/api/v1/plugins/{PLUGIN}"),
        Some(&operator),
        None,
    );
    assert_eq!(denied.status, 403);

    // Install (admin). The plugin host may still be connecting to the bus.
    let installed = eventually("plugin install", 20, || {
        let reply = req(
            api,
            "POST",
            "/api/v1/plugins",
            Some(&admin),
            Some(&install_body),
        );
        match reply.status {
            201 => Some(reply),
            409 => Some(reply), // a retry after a slow first answer
            _ => None,
        }
    })
    .await;
    assert_eq!(
        installed.json()["plugin"]["manifest"]["plugin"]["id"],
        PLUGIN,
        "{installed:?}"
    );
    let listed = req(api, "GET", "/api/v1/plugins", Some(&operator), None);
    assert_eq!(listed.status, 200);
    assert_eq!(
        listed.json()["plugins"][0]["manifest"]["plugin"]["id"],
        PLUGIN
    );
    let again = req(
        api,
        "POST",
        "/api/v1/plugins",
        Some(&admin),
        Some(&install_body),
    );
    assert_eq!(again.status, 409, "same version twice");

    // Start on VFO 1 (the default VFO of a fresh daemon) as an operator.
    let start = json!({"plugin_id": PLUGIN, "device": device, "vfo": 1, "freq_hz": CENTER});
    let started = req(
        api,
        "POST",
        "/api/v1/plugin-instances",
        Some(&operator),
        Some(&start),
    );
    assert_eq!(started.status, 201, "{started:?}");
    let instance = started.json()["instance"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let bad = req(
        api,
        "POST",
        "/api/v1/plugin-instances",
        Some(&operator),
        Some(&json!({"plugin_id": PLUGIN, "vfo": 1})),
    );
    assert_eq!(bad.status, 400, "a vfo needs a device: {bad:?}");

    // The plugin's events land in the decoder history, tagged with the plugin as source.
    let filter = format!("source={PLUGIN}&kind=echo.block");
    let first = eventually("echo events in the history", 30, || {
        let found = events(api, &admin, &filter);
        (!found.is_empty()).then_some(found)
    })
    .await;
    let event = &first[0];
    assert_eq!(event["source"], PLUGIN);
    assert_eq!(event["kind"], "echo.block");
    assert_eq!(event["device"], device.as_str());
    assert_eq!(event["vfo"], 1);
    assert_eq!(event["freq_hz"], CENTER);
    assert_eq!(event["meta"]["instance"], instance.as_str());
    assert!(
        event["meta"]["data"]["bytes"].as_u64().unwrap_or(0) > 0,
        "{event}"
    );
    // Core detectors share the table; the source filter keeps them apart and the export has it.
    assert!(
        events(api, &admin, "source=core")
            .iter()
            .all(|e| e["source"] == "core")
    );
    let csv = req(
        api,
        "GET",
        "/api/v1/decoder-events/export.csv?source=org.superhet.example.echo",
        Some(&admin),
        None,
    );
    assert!(
        csv.body
            .lines()
            .next()
            .is_some_and(|h| h.ends_with(",source")),
        "{}",
        csv.body
    );
    assert!(csv.body.contains("echo.block"));

    // Instance state shows the input is flowing.
    let info = req(
        api,
        "GET",
        &format!("/api/v1/plugin-instances/{instance}"),
        Some(&operator),
        None,
    );
    assert_eq!(info.json()["instance"]["state"], "running");
    assert_eq!(info.json()["instance"]["vfo"], 1);
    assert_eq!(info.json()["instance"]["input_error"], Value::Null);

    // The instance survives a plugin host restart (same id, events keep coming).
    drop(pluginhost);
    let before = events(api, &admin, &filter).len();
    pluginhost = spawn_pluginhost(&nats_url, &db.url, &plugin_dir);
    eventually("instance restored", 30, || {
        let reply = req(
            api,
            "GET",
            &format!("/api/v1/plugin-instances/{instance}"),
            Some(&operator),
            None,
        );
        (reply.status == 200 && reply.json()["instance"]["state"] == "running").then_some(())
    })
    .await;
    eventually("new events after the restart", 30, || {
        (events(api, &admin, &filter).len() > before).then_some(())
    })
    .await;

    // A plugin cannot be removed while it runs; stop it, then remove it.
    let busy = req(
        api,
        "DELETE",
        &format!("/api/v1/plugins/{PLUGIN}"),
        Some(&admin),
        None,
    );
    assert_eq!(busy.status, 409, "{busy:?}");
    let stopped = req(
        api,
        "POST",
        &format!("/api/v1/plugin-instances/{instance}/stop"),
        Some(&operator),
        None,
    );
    assert_eq!(stopped.status, 200, "{stopped:?}");
    assert_eq!(stopped.json()["instance"]["state"], "stopped");
    let all = req(
        api,
        "GET",
        "/api/v1/plugin-instances",
        Some(&operator),
        None,
    );
    assert_eq!(all.json()["instances"], json!([]));
    let pool = db.pool().await;
    assert!(
        superhet_db::plugin_instances::list(&pool)
            .await
            .expect("list")
            .is_empty(),
        "a stopped instance is forgotten"
    );
    let removed = req(
        api,
        "DELETE",
        &format!("/api/v1/plugins/{PLUGIN}"),
        Some(&admin),
        None,
    );
    assert_eq!(removed.status, 204, "{removed:?}");
    assert_eq!(
        req(api, "GET", "/api/v1/plugins", Some(&admin), None).json()["plugins"],
        json!([])
    );

    // Every change was audited.
    for action in [
        "plugin.install",
        "plugin.start",
        "plugin.stop",
        "plugin.remove",
    ] {
        let reply = req(
            api,
            "GET",
            &format!("/api/v1/admin/audit?action={action}"),
            Some(&admin),
            None,
        );
        assert_eq!(reply.status, 200);
        assert!(
            reply.body.contains(action),
            "{action} not audited: {}",
            reply.body
        );
    }

    drop(pluginhost);
    let _ = std::fs::remove_file(&tone);
    let _ = std::fs::remove_dir_all(&plugin_dir);
}

async fn host_request(client: &async_nats::Client, cmd: &str, body: Value) -> Value {
    let reply = tokio::time::timeout(
        Duration::from_secs(10),
        client.request(format!("sh.v1.plugin.ctl.{cmd}"), body.to_string().into()),
    )
    .await
    .expect("reply in time");
    match reply {
        Ok(reply) => serde_json::from_slice(&reply.payload).expect("json reply"),
        // The host is still starting up.
        Err(err) => json!({"ok": false, "code": "bus", "error": err.to_string()}),
    }
}

async fn device_state(client: &async_nats::Client, device: &str) -> Value {
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        client.request(format!("sh.v1.dev.{device}.ctl.state"), "".into()),
    )
    .await
    .expect("reply in time")
    .expect("request");
    serde_json::from_slice::<Value>(&reply.payload).expect("json")["state"].clone()
}

/// An exclusive plugin takes the device: the daemon retunes, the plugin receives the whole
/// window and the previous tuning returns when the instance stops.
#[tokio::test(flavor = "multi_thread")]
async fn exclusive_plugin_claims_the_device_and_receives_the_window() {
    let _serial = SERIAL.lock().await;
    let Ok(nats_url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
        eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let pid = std::process::id();
    let device = format!("excl{pid}");
    let tone = std::env::temp_dir().join(format!("superhet-excl-{pid}.cu8"));
    write_tone(&tone);
    let plugin_dir = std::env::temp_dir().join(format!("superhet-excl-{pid}-plugins"));
    let source = std::env::temp_dir().join(format!("superhet-excl-{pid}-source"));
    let _ = std::fs::remove_dir_all(&plugin_dir);
    let _ = std::fs::remove_dir_all(&source);
    std::fs::create_dir_all(&source).expect("source dir");
    std::fs::write(
        source.join("plugin.toml"),
        format!(
            r#"
[plugin]
id = "org.superhet.test.window"
name = "window"
version = "0.0.1"
protocol = 1
license = "AGPL-3.0-only"
type = "decoder"

[runtime]
command = ["sh", "-c", "cat > fed.bin"]

[input]
kind = "iq"
sample_rate = {RATE}
format = "cu8"

[device]
exclusive = true
center_hz_min = 400000000
center_hz_max = 470000000
sample_rate = {RATE}
"#
        ),
    )
    .expect("manifest");

    let sdrd_port = free_port();
    let _sdrd = Proc(
        Command::new(sibling_binary("superhet-sdrd"))
            .env("SUPERHET_LISTEN", format!("127.0.0.1:{sdrd_port}"))
            .env("SUPERHET_NATS_URL", &nats_url)
            .env("SUPERHET_DEVICE_ID", &device)
            .env(
                "SUPERHET_DEVICE",
                format!(
                    "file:{}?format=cu8&rate={RATE}&freq={CENTER}&block=4096",
                    tone.display()
                ),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sdrd"),
    );
    wait_ready(sdrd_port, "sdrd").await;
    // No database: the host keeps the instance in memory only.
    let host_port = free_port();
    let _host = Proc(
        Command::new(sibling_binary("superhet-pluginhost"))
            .env("SUPERHET_LISTEN", format!("127.0.0.1:{host_port}"))
            .env("SUPERHET_NATS_URL", &nats_url)
            .env("SUPERHET_PLUGIN_DIR", &plugin_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pluginhost"),
    );
    let client = async_nats::connect(&nats_url).await.expect("connect");

    let installed = eventually_async("plugin install", 20, || async {
        let reply = host_request(&client, "install", json!({"v": 1, "path": source})).await;
        (reply["ok"] == true).then_some(reply)
    })
    .await;
    assert_eq!(installed["ok"], true);

    // The instance picks the centre inside the declared range.
    let started = host_request(
        &client,
        "start",
        json!({"v": 1, "plugin_id": "org.superhet.test.window", "device": device, "freq_hz": 434_500_000u64}),
    )
    .await;
    assert_eq!(started["ok"], true, "{started}");
    let id = started["instance"]["id"].as_str().expect("id").to_owned();
    assert_eq!(started["instance"]["exclusive"], true);

    eventually_async("device claimed", 20, || async {
        let state = device_state(&client, &device).await;
        (state["center_hz"] == 434_500_000 && state["locks"][0]["kind"] == "claim").then_some(())
    })
    .await;
    let fed = plugin_dir.join("org.superhet.test.window/0.0.1/fed.bin");
    eventually("window samples on stdin", 20, || {
        std::fs::metadata(&fed).ok().filter(|m| m.len() > 200_000)
    })
    .await;

    // A second claim on the device is refused by the host.
    let second = host_request(
        &client,
        "start",
        json!({"v": 1, "plugin_id": "org.superhet.test.window", "device": device, "freq_hz": 440_000_000u64}),
    )
    .await;
    assert_eq!(second["code"], "conflict", "{second}");

    let stopped = host_request(&client, "stop", json!({"v": 1, "id": id})).await;
    assert_eq!(stopped["instance"]["state"], "stopped");
    eventually_async("tuning restored", 20, || async {
        let state = device_state(&client, &device).await;
        (state["center_hz"] == CENTER && state["locks"] == json!([])).then_some(())
    })
    .await;

    let _ = std::fs::remove_file(&tone);
    let _ = std::fs::remove_dir_all(&plugin_dir);
    let _ = std::fs::remove_dir_all(&source);
}

/// Like [`eventually`], for async conditions.
async fn eventually_async<T, F, Fut>(what: &str, secs: u64, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(value) = f().await {
            return value;
        }
        assert!(Instant::now() < deadline, "timeout waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
