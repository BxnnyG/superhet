//! End-to-end test of the device REST routes: real NATS server (JetStream enabled), real
//! `superhet-api` binary, and a mock `sdrd` answering control requests.
//! Skipped unless `SUPERHET_TEST_NATS_URL` and `DATABASE_URL` are set (optional
//! `SUPERHET_TEST_NATS_USER` / `SUPERHET_TEST_NATS_PASSWORD`, also forwarded to the API under
//! test). The test logs in as the admin it creates through the setup endpoint.

mod common;

/// Session cookie sent with every request of this test.
static COOKIE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{Value, json};

struct Api(Child);

impl Drop for Api {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, Value) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let mut request = format!(
        "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).expect("write");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let body = response.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

async fn request(
    port: u16,
    method: &'static str,
    path: String,
    headers: Vec<(&'static str, String)>,
    body: &'static str,
) -> (u16, Value) {
    tokio::task::spawn_blocking(move || {
        let mut headers: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        if let Some(cookie) = COOKIE.get() {
            headers.push(("Cookie", cookie.as_str()));
        }
        http(port, method, &path, &headers, body)
    })
    .await
    .expect("join")
}

fn readyz_status(port: u16) -> Option<u16> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .write_all(b"GET /readyz HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.split_whitespace().nth(1)?.parse().ok()
}

fn state_doc(device: &str, center_hz: u64) -> Value {
    json!({"v": 1, "device": device, "driver": "mock", "center_hz": center_hz,
           "sample_rate": 2_048_000, "gain": null, "ppm": 0, "bias_tee": false,
           "tuning_epoch": 7, "gains_db": []})
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_state_and_tune_roundtrip_over_real_nats() {
    let Ok(url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
        eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let Some(db) = common::TestDb::create("dev").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let pid = std::process::id();
    let (kv_dev, ctl_dev, silent_dev, ghost_dev) = (
        format!("kv{pid}"),
        format!("ctl{pid}"),
        format!("silent{pid}"),
        format!("ghost{pid}"),
    );
    let port = common::free_port();

    let mut options = async_nats::ConnectOptions::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_superhet-api"));
    if let (Ok(user), Ok(password)) = (
        std::env::var("SUPERHET_TEST_NATS_USER"),
        std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
    ) {
        command
            .env("SUPERHET_NATS_USER", &user)
            .env("SUPERHET_NATS_PASSWORD", &password);
        options = options.user_and_password(user, password);
    }
    let client = options.connect(url.as_str()).await.expect("connect");

    // State bucket with one device stored in KV.
    let js = async_nats::jetstream::new(client.clone());
    let store = match js.get_key_value("STATE").await {
        Ok(store) => store,
        Err(_) => js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: "STATE".into(),
                history: 1,
                ..Default::default()
            })
            .await
            .expect("create bucket"),
    };
    store
        .put(
            format!("dev.{kv_dev}"),
            state_doc(&kv_dev, 100_000_000).to_string().into(),
        )
        .await
        .expect("put state");

    // Mock sdrd for `ctl_dev`: records tune bodies, answers state and tune.
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let mut ctl = client
        .subscribe(format!("sh.v1.dev.{ctl_dev}.ctl.>"))
        .await
        .expect("subscribe");
    {
        let (client, seen, dev) = (client.clone(), Arc::clone(&seen), ctl_dev.clone());
        tokio::spawn(async move {
            while let Some(message) = ctl.next().await {
                let Some(reply) = message.reply.clone() else {
                    continue;
                };
                let answer = if message.subject.as_str().ends_with(".tune") {
                    let body: Value = serde_json::from_slice(&message.payload).expect("json");
                    seen.lock().expect("lock").push(body.clone());
                    match body["center_hz"].as_u64() {
                        Some(0) => {
                            json!({"v":1,"ok":false,"code":"invalid","error":"center_hz 0 out of range"})
                        }
                        Some(hz) => json!({"v":1,"ok":true,"state":state_doc(&dev, hz)}),
                        None => json!({"v":1,"ok":false,"code":"device","error":"boom"}),
                    }
                } else {
                    json!({"v":1,"ok":true,"state":state_doc(&dev, 99)})
                };
                client
                    .publish(reply, answer.to_string().into())
                    .await
                    .expect("reply");
            }
        });
    }
    // A daemon that receives but never answers.
    let mut silent = client
        .subscribe(format!("sh.v1.dev.{silent_dev}.ctl.>"))
        .await
        .expect("subscribe");
    tokio::spawn(async move { while silent.next().await.is_some() {} });
    client.flush().await.expect("flush");

    let _api = Api(command
        .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
        .env("SUPERHET_NATS_URL", &url)
        .env("SUPERHET_DATABASE_URL", &db.url)
        .env("SUPERHET_SETUP_TOKEN", common::SETUP_TOKEN)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn api"));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if readyz_status(port) == Some(200) {
            break;
        }
        assert!(Instant::now() < deadline, "/readyz never became 200");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    COOKIE
        .set(common::setup_admin(port))
        .expect("cookie set once");

    let json_ct = || vec![("Content-Type", "application/json".to_owned())];

    // GET served from KV.
    let (status, body) = request(
        port,
        "GET",
        format!("/api/v1/devices/{kv_dev}/state"),
        vec![],
        "",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, state_doc(&kv_dev, 100_000_000));

    // GET falls back to ctl.state when KV has no entry.
    let (status, body) = request(
        port,
        "GET",
        format!("/api/v1/devices/{ctl_dev}/state"),
        vec![],
        "",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, state_doc(&ctl_dev, 99));

    // POST tune: forwarded with the version injected; reply passed through.
    let (status, body) = request(
        port,
        "POST",
        format!("/api/v1/devices/{ctl_dev}/tune"),
        json_ct(),
        r#"{"center_hz":145500000}"#,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["state"]["center_hz"], 145_500_000);
    assert_eq!(
        seen.lock().expect("lock").last(),
        Some(&json!({"v":1,"center_hz":145_500_000}))
    );

    // Daemon-side validation error becomes 400, device failure 502.
    let tune = format!("/api/v1/devices/{ctl_dev}/tune");
    let (status, body) = request(port, "POST", tune.clone(), json_ct(), r#"{"center_hz":0}"#).await;
    assert_eq!(
        (status, body["code"].clone()),
        (400, "invalid".into()),
        "{body}"
    );
    let (status, body) = request(port, "POST", tune.clone(), json_ct(), r#"{"ppm":1}"#).await;
    assert_eq!(
        (status, body["code"].clone()),
        (502, "device".into()),
        "{body}"
    );

    // CSRF: a foreign Origin never reaches the daemon.
    let before = seen.lock().expect("lock").len();
    let mut headers = json_ct();
    headers.push(("Origin", "https://evil.example".to_owned()));
    let (status, _) = request(port, "POST", tune.clone(), headers, r#"{"center_hz":1}"#).await;
    assert_eq!(status, 403);
    // Same-origin browser requests pass.
    let mut headers = json_ct();
    headers.push(("Origin", format!("http://127.0.0.1:{port}")));
    let (status, _) = request(port, "POST", tune, headers, r#"{"center_hz":2}"#).await;
    assert_eq!(status, 200);
    assert_eq!(seen.lock().expect("lock").len(), before + 1);

    // Nobody serving the device: 503. Daemon that does not answer: 504.
    let (status, _) = request(
        port,
        "POST",
        format!("/api/v1/devices/{ghost_dev}/tune"),
        json_ct(),
        r#"{"center_hz":1}"#,
    )
    .await;
    assert_eq!(status, 503);
    let (status, _) = request(
        port,
        "GET",
        format!("/api/v1/devices/{ghost_dev}/state"),
        vec![],
        "",
    )
    .await;
    assert_eq!(status, 503);
    let started = Instant::now();
    let (status, _) = request(
        port,
        "POST",
        format!("/api/v1/devices/{silent_dev}/tune"),
        json_ct(),
        r#"{"center_hz":1}"#,
    )
    .await;
    assert_eq!(status, 504);
    assert!(started.elapsed() >= Duration::from_millis(1500));

    let _ = store.purge(format!("dev.{kv_dev}")).await;
}
