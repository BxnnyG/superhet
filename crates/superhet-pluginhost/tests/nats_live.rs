//! Control API, state feed and event publishing over a real NATS server; skipped unless
//! `SUPERHET_TEST_NATS_URL` is set (optional `SUPERHET_TEST_NATS_USER` / `_PASSWORD`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};
use superhet_bus::subjects;
use superhet_pluginhost::ctl::{self, Host, QueueSink};
use superhet_pluginhost::registry::Registry;
use superhet_pluginhost::supervisor::{Policy, Supervisor};

async fn connect() -> Option<async_nats::Client> {
    let url = std::env::var("SUPERHET_TEST_NATS_URL").ok()?;
    let mut options = async_nats::ConnectOptions::new();
    if let (Ok(user), Ok(password)) = (
        std::env::var("SUPERHET_TEST_NATS_USER"),
        std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
    ) {
        options = options.user_and_password(user, password);
    }
    Some(options.connect(url).await.expect("connect to NATS"))
}

async fn request(client: &async_nats::Client, cmd: &str, body: Value) -> Value {
    let subject = subjects::plugin_ctl(cmd).expect("subject");
    let reply = tokio::time::timeout(
        Duration::from_secs(10),
        client.request(subject, body.to_string().into()),
    )
    .await
    .expect("reply in time")
    .expect("request");
    serde_json::from_slice(&reply.payload).expect("json reply")
}

#[tokio::test]
async fn control_state_and_events_over_nats() {
    let Some(client) = connect().await else {
        eprintln!("SUPERHET_TEST_NATS_URL not set, skipping");
        return;
    };
    let dir: PathBuf =
        std::env::temp_dir().join(format!("superhet-nats-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let registry = Arc::new(Registry::open(&dir).expect("registry"));
    let (sink, events) = QueueSink::new(64);
    let supervisor = Supervisor::new(Policy::default(), Arc::new(sink));
    let host = Host {
        registry,
        supervisor: supervisor.clone(),
    };
    let server = tokio::spawn(ctl::serve(client.clone(), host));
    let states = tokio::spawn(ctl::publish_state(client.clone(), supervisor.clone()));
    let publisher = tokio::spawn(ctl::publish_events(client.clone(), events));

    let mut state_sub = client.subscribe(subjects::PLUGIN_STATE).await.expect("sub");
    let mut event_sub = client
        .subscribe("sh.v1.events.org_superhet_test_emit.>")
        .await
        .expect("sub");
    client.flush().await.expect("flush");
    // Let the server task finish its own subscribe.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A tiny plugin that reports one event and then idles.
    let source = dir.join("src-emit");
    std::fs::create_dir_all(&source).expect("mkdir");
    std::fs::write(
        source.join("plugin.toml"),
        r#"
[plugin]
id = "org.superhet.test.emit"
name = "emit"
version = "0.0.1"
protocol = 1
license = "AGPL-3.0-only"
type = "decoder"

[runtime]
command = ["sh", "-c", "printf '{\"v\":1,\"type\":\"emit.hello\",\"data\":{\"n\":1}}\\n'; sleep 60"]

[input]
kind = "audio"
sample_rate = 48000
format = "s16le"
"#,
    )
    .expect("manifest");
    let installed = request(
        &client,
        "install",
        json!({"v": 1, "path": source.display().to_string()}),
    )
    .await;
    assert_eq!(installed["ok"], true, "{installed}");
    let listed = request(&client, "list", json!({"v": 1})).await;
    assert_eq!(listed["plugins"].as_array().map(Vec::len), Some(1));

    let started = request(
        &client,
        "start",
        json!({"v": 1, "plugin_id": "org.superhet.test.emit", "device": "dev0"}),
    )
    .await;
    assert_eq!(started["ok"], true, "{started}");
    let id = started["instance"]["id"].as_str().expect("id").to_owned();

    // The state feed announces the instance running.
    let mut running = false;
    while !running {
        let msg = tokio::time::timeout(Duration::from_secs(10), state_sub.next())
            .await
            .expect("state update")
            .expect("open");
        let info: Value = serde_json::from_slice(&msg.payload).expect("json");
        running = info["id"] == id.as_str() && info["state"] == "running";
    }

    // The event arrives enriched on the plugin's subject tree.
    let msg = tokio::time::timeout(Duration::from_secs(10), event_sub.next())
        .await
        .expect("event in time")
        .expect("open");
    assert_eq!(
        msg.subject.as_str(),
        "sh.v1.events.org_superhet_test_emit.emit.hello"
    );
    let event: Value = serde_json::from_slice(&msg.payload).expect("json");
    assert_eq!(event["instance_id"], id.as_str());
    assert_eq!(event["device"], "dev0");
    assert_eq!(event["data"]["n"], 1);

    let stopped = request(&client, "stop", json!({"v": 1, "id": id})).await;
    assert_eq!(stopped["instance"]["state"], "stopped", "{stopped}");

    server.abort();
    states.abort();
    publisher.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
