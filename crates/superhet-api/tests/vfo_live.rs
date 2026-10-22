//! End-to-end test of the VFO routes and the audio WebSocket: real NATS server, real
//! `superhet-api` and `superhet-sdrd` binaries (file device, no hardware).
//! Skipped unless `SUPERHET_TEST_NATS_URL` and `DATABASE_URL` are set (optional
//! `SUPERHET_TEST_NATS_USER` / `SUPERHET_TEST_NATS_PASSWORD`, also forwarded to both services). The sdrd binary must have been
//! built (`cargo build -p superhet-sdrd`, done by `cargo test --workspace`); it is looked up next
//! to the API binary.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use superhet_bus::audio::AudioFrame;
use tokio_tungstenite::tungstenite::Message;

const RATE: u32 = 1_024_000;
const CENTER: u64 = 433_920_000;
const TONE_HZ: f64 = 50_000.0;

struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

use common::free_port;

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

async fn wait_ready(port: u16, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while readyz_status(port) != Some(200) {
        assert!(Instant::now() < deadline, "{what} /readyz never became 200");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn write_tone(path: &std::path::Path) {
    let samples = 8 * 4096;
    let mut bytes = Vec::with_capacity(samples * 2);
    for i in 0..samples {
        let ph = std::f64::consts::TAU * TONE_HZ * i as f64 / f64::from(RATE);
        bytes.push((127.5 + 127.0 * ph.cos()).round() as u8);
        bytes.push((127.5 + 127.0 * ph.sin()).round() as u8);
    }
    std::fs::write(path, bytes).expect("write fixture");
}

/// A running stack: sdrd (file device) + API on a shared NATS server.
struct Stack {
    _sdrd: Proc,
    _api: Proc,
    api_port: u16,
    device: String,
    nats: async_nats::Client,
    tone: std::path::PathBuf,
    /// Session cookie (`name=value`) of the admin created for this stack.
    cookie: String,
    /// Dropped last: the api must be gone before its database is.
    _db: common::TestDb,
}

impl Drop for Stack {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.tone);
    }
}

async fn start_stack(url: &str, tag: &str) -> Option<Stack> {
    let db = common::TestDb::create(tag).await?;
    let pid = std::process::id();
    let device = format!("{tag}{pid}");
    let tone = std::env::temp_dir().join(format!("superhet-api-{tag}-{pid}.cu8"));
    write_tone(&tone);

    let api_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_superhet-api"));
    let sdrd_bin = api_bin.with_file_name("superhet-sdrd");
    assert!(
        sdrd_bin.exists(),
        "{} not found: run `cargo build -p superhet-sdrd` first",
        sdrd_bin.display()
    );

    let mut options = async_nats::ConnectOptions::new();
    let creds = match (
        std::env::var("SUPERHET_TEST_NATS_USER"),
        std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
    ) {
        (Ok(user), Ok(password)) => Some((user, password)),
        _ => None,
    };
    if let Some((user, password)) = &creds {
        options = options.user_and_password(user.clone(), password.clone());
    }
    let nats = options.connect(url).await.expect("connect");

    let (sdrd_port, api_port) = (free_port(), free_port());
    let mut sdrd = Command::new(sdrd_bin);
    let mut api = Command::new(api_bin);
    if let Some((user, password)) = &creds {
        for command in [&mut sdrd, &mut api] {
            command
                .env("SUPERHET_NATS_USER", user)
                .env("SUPERHET_NATS_PASSWORD", password);
        }
    }
    let sdrd = Proc(
        sdrd.env("SUPERHET_LISTEN", format!("127.0.0.1:{sdrd_port}"))
            .env("SUPERHET_NATS_URL", url)
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
    let api = Proc(
        api.env("SUPERHET_LISTEN", format!("127.0.0.1:{api_port}"))
            .env("SUPERHET_NATS_URL", url)
            .env("SUPERHET_DATABASE_URL", &db.url)
            .env("SUPERHET_SETUP_TOKEN", common::SETUP_TOKEN)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn api"),
    );
    wait_ready(sdrd_port, "sdrd").await;
    wait_ready(api_port, "api").await;
    let cookie = common::setup_admin(api_port);
    Some(Stack {
        _sdrd: sdrd,
        _api: api,
        api_port,
        device,
        nats,
        tone,
        cookie,
        _db: db,
    })
}

struct Reply {
    status: u16,
    headers: String,
    body: Value,
}

fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
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
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    Reply {
        status,
        headers: head.to_owned(),
        body: serde_json::from_str(body).unwrap_or(Value::Null),
    }
}

impl Stack {
    async fn rest(
        &self,
        method: &'static str,
        path: String,
        headers: Vec<(&'static str, &'static str)>,
        body: &'static str,
    ) -> Reply {
        let port = self.api_port;
        let cookie = self.cookie.clone();
        tokio::task::spawn_blocking(move || {
            let mut headers = headers;
            headers.push(("Cookie", cookie.as_str()));
            http(port, method, &path, &headers, body)
        })
        .await
        .expect("join")
    }

    async fn json_rest(&self, method: &'static str, path: String, body: &'static str) -> Reply {
        self.rest(
            method,
            path,
            vec![("Content-Type", "application/json")],
            body,
        )
        .await
    }

    fn vfos_path(&self) -> String {
        format!("/api/v1/devices/{}/vfos", self.device)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn vfo_rest_crud_against_a_real_sdrd() {
    let Ok(url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
        eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let Some(stack) = start_stack(&url, "vr").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let list = stack.vfos_path();

    // A fresh daemon starts with one VFO at the centre; remove it to start from an empty set.
    let reply = stack.rest("GET", list.clone(), vec![], "").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body["vfos"].as_array().map(Vec::len), Some(1));
    assert_eq!(reply.body["vfos"][0]["id"], 1);
    assert_eq!(reply.body["vfos"][0]["mode"], "nfm");
    let reply = stack.rest("DELETE", format!("{list}/1"), vec![], "").await;
    assert_eq!(reply.status, 204);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reply = stack.rest("GET", list.clone(), vec![], "").await;
        if reply.body == json!({"v":1,"vfos":[]}) {
            break;
        }
        assert!(Instant::now() < deadline, "default VFO still listed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Create: 201 with the VFO and a Location header.
    let reply = stack
        .json_rest(
            "POST",
            list.clone(),
            r#"{"freq_hz":433970000,"mode":"nfm","squelch_dbfs":-40}"#,
        )
        .await;
    assert_eq!(reply.status, 201, "{}", reply.body);
    assert_eq!(reply.body["vfo"]["id"], 2);
    assert_eq!(reply.body["vfo"]["status"], "active");
    assert_eq!(reply.body["vfo"]["squelch_dbfs"], -40.0);
    assert!(
        reply
            .headers
            .to_lowercase()
            .contains(&format!("location: {list}/2")),
        "{}",
        reply.headers
    );
    let reply = stack
        .json_rest("POST", list.clone(), r#"{"freq_hz":433900000,"mode":"am"}"#)
        .await;
    assert_eq!(
        (reply.status, reply.body["vfo"]["id"].clone()),
        (201, 3.into())
    );

    // The list comes from the state (KV is updated asynchronously).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reply = stack.rest("GET", list.clone(), vec![], "").await;
        assert_eq!(reply.status, 200);
        if reply.body["vfos"].as_array().is_some_and(|v| v.len() == 2) {
            assert_eq!(reply.body["v"], 1);
            assert_eq!(reply.body["vfos"][0]["mode"], "nfm");
            assert_eq!(reply.body["vfos"][1]["mode"], "am");
            break;
        }
        assert!(Instant::now() < deadline, "VFOs never showed up in state");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Partial update; null resets.
    let one = format!("{list}/2");
    let reply = stack
        .json_rest("PATCH", one.clone(), r#"{"freq_hz":433960000}"#)
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["vfo"]["freq_hz"], 433_960_000);
    assert_eq!(reply.body["vfo"]["squelch_dbfs"], -40.0);
    let reply = stack
        .json_rest("PATCH", one.clone(), r#"{"squelch_dbfs":null,"id":2}"#)
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["vfo"]["squelch_dbfs"], Value::Null);

    // Errors: daemon-side validation is 400, unknown ids are 404.
    for (method, path, body) in [
        ("PATCH", one.clone(), r#"{"mode":"ssb"}"#),
        ("PATCH", one.clone(), "{}"),
        ("PATCH", one.clone(), r#"{"id":3,"freq_hz":5}"#),
        ("PATCH", format!("{list}/x"), r#"{"freq_hz":5}"#),
        ("POST", list.clone(), r#"{"freq_hz":1,"mode":"ssb"}"#),
        ("POST", list.clone(), "nope"),
    ] {
        let reply = stack.json_rest(method, path.clone(), body).await;
        assert_eq!(
            (reply.status, reply.body["code"].clone()),
            (400, "invalid".into()),
            "{method} {path} {body}: {}",
            reply.body
        );
    }
    let reply = stack.rest("DELETE", format!("{list}/99"), vec![], "").await;
    assert_eq!(
        (reply.status, reply.body["code"].clone()),
        (404, "not_found".into())
    );
    let reply = stack
        .json_rest("PATCH", format!("{list}/99"), r#"{"freq_hz":5}"#)
        .await;
    assert_eq!(
        (reply.status, reply.body["code"].clone()),
        (404, "not_found".into())
    );

    // CSRF and content type.
    let evil = vec![("Origin", "https://evil.example")];
    for method in ["DELETE", "PATCH", "POST"] {
        let path = if method == "POST" {
            list.clone()
        } else {
            one.clone()
        };
        let mut headers = evil.clone();
        headers.push(("Content-Type", "application/json"));
        let reply = stack.rest(method, path, headers, "{}").await;
        assert_eq!(reply.status, 403, "{method}");
    }
    let reply = stack.rest("POST", list.clone(), vec![], "{}").await;
    assert_eq!(reply.status, 415);

    // Delete: 204 without body, then the id is gone.
    let reply = stack.rest("DELETE", format!("{list}/3"), vec![], "").await;
    assert_eq!((reply.status, reply.body), (204, Value::Null));
    let reply = stack.rest("DELETE", format!("{list}/3"), vec![], "").await;
    assert_eq!(reply.status, 404);
    let reply = stack
        .json_rest("PATCH", format!("{list}/3"), r#"{"freq_hz":5}"#)
        .await;
    assert_eq!(reply.status, 404);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reply = stack.rest("GET", list.clone(), vec![], "").await;
        if reply.body["vfos"].as_array().is_some_and(|v| v.len() == 1) {
            assert_eq!(reply.body["vfos"][0]["id"], 2);
            break;
        }
        assert!(Instant::now() < deadline, "deleted VFO still listed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Nobody serving the device: 503.
    let reply = stack
        .rest("GET", "/api/v1/devices/ghost/vfos".to_owned(), vec![], "")
        .await;
    assert_eq!(reply.status, 503);
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_ws_forwards_subscribed_vfos_only() {
    let Ok(url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
        eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let Some(stack) = start_stack(&url, "va").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let list = stack.vfos_path();
    // VFO 1 is the default one a fresh daemon creates; add VFO 2.
    let reply = stack
        .json_rest("POST", list.clone(), r#"{"freq_hz":433970000,"mode":"am"}"#)
        .await;
    assert_eq!(reply.status, 201, "{}", reply.body);
    assert_eq!(reply.body["vfo"]["id"], 2);

    // Control: VFO 2 really produces audio on the bus during the test.
    let mut bus_two = stack
        .nats
        .subscribe(format!("sh.v1.dev.{}.audio.2", stack.device))
        .await
        .expect("subscribe");

    let ws_url = format!(
        "ws://127.0.0.1:{}/api/v1/ws/audio?dev={}",
        stack.api_port, stack.device
    );
    let mut ws = common::ws_connect_with_cookie(&ws_url, &stack.cookie)
        .await
        .expect("ws");
    ws.send(Message::text(r#"{"type":"subscribe","vfos":[1]}"#))
        .await
        .expect("send");
    let ack = loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => break t.to_string(),
            Some(Ok(_)) => {}
            other => panic!("expected ack, got {other:?}"),
        }
    };
    assert_eq!(
        serde_json::from_str::<Value>(&ack).expect("json"),
        json!({"v":1,"type":"ack","vfos":[1]})
    );

    let mut frames = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while frames.len() < 50 && Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Binary(data)))) =
            tokio::time::timeout(Duration::from_millis(500), ws.next()).await
        {
            frames.push(AudioFrame::decode(&data).expect("valid AudioFrame v1"));
        }
    }
    assert!(frames.len() >= 10, "got {} frames", frames.len());
    for frame in &frames {
        assert_eq!(frame.vfo_id, 1, "frame of an unsubscribed VFO");
        assert_eq!(frame.sample_rate, 48_000);
    }
    for pair in frames.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq.wrapping_add(1), "gap in seq");
    }

    // Count what VFO 2 published meanwhile (the stream keeps flowing, so stop at a deadline).
    let mut bus_frames = 0;
    let window = Instant::now() + Duration::from_millis(300);
    while Instant::now() < window {
        if let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(100), bus_two.next()).await
        {
            bus_frames += 1;
        }
    }
    assert!(bus_frames >= 10, "VFO 2 produced only {bus_frames} frames");

    // Switch to VFO 2 only: VFO 1 frames stop (after the acknowledgement), VFO 2 frames flow.
    ws.send(Message::text(r#"{"type":"subscribe","vfos":[2]}"#))
        .await
        .expect("send");
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(_))) => break,
            Some(Ok(_)) => {}
            other => panic!("expected ack, got {other:?}"),
        }
    }
    let mut two = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    while two < 10 && Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Binary(data)))) =
            tokio::time::timeout(Duration::from_millis(500), ws.next()).await
        {
            let frame = AudioFrame::decode(&data).expect("valid AudioFrame v1");
            // Frames queued before the acknowledgement were consumed above; none may follow it.
            assert_eq!(frame.vfo_id, 2, "VFO 1 frame after unsubscribe");
            two += 1;
        }
    }
    assert!(two >= 10, "got {two} frames of VFO 2");
}
