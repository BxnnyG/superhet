//! End-to-end test against a real NATS server; skipped unless `SUPERHET_TEST_NATS_URL` is set.
//!
//! Optional: `SUPERHET_TEST_NATS_USER` / `SUPERHET_TEST_NATS_PASSWORD` for the subscriber;
//! `SUPERHET_NATS_USER` / `SUPERHET_NATS_PASSWORD` are inherited by the daemon under test.
//!
//! The daemon is started first and the test waits for `/readyz`, so it also covers the
//! background connect path.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use superhet_bus::spectrum::SpectrumFrame;

const RATE: u32 = 1_024_000;
const TONE_HZ: f64 = 50_000.0;

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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

/// Status code of `GET /readyz`, or `None` when the daemon does not answer yet.
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

async fn connect(url: &str) -> async_nats::Client {
    let mut options = async_nats::ConnectOptions::new();
    if let (Ok(user), Ok(password)) = (
        std::env::var("SUPERHET_TEST_NATS_USER"),
        std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
    ) {
        options = options.user_and_password(user, password);
    }
    options.connect(url).await.expect("connect")
}

/// Spawn the daemon on a free port and wait until `/readyz` is 200.
async fn start_daemon(url: &str, device_id: &str, tone: &std::path::Path) -> Daemon {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("free port")
        .port();
    let daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_superhet-sdrd"))
            .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
            .env("SUPERHET_NATS_URL", url)
            .env("SUPERHET_DEVICE_ID", device_id)
            .env("SUPERHET_SPECTRUM_BINS", "1024")
            .env("SUPERHET_FFT_SIZE", "4096")
            .env(
                "SUPERHET_DEVICE",
                format!(
                    "file:{}?format=cu8&rate={RATE}&freq=433920000&block=4096",
                    tone.display()
                ),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sdrd"),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while readyz_status(port) != Some(200) {
        assert!(Instant::now() < deadline, "/readyz never became 200");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    daemon
}

#[tokio::test]
async fn spectrum_frames_flow_through_real_nats() {
    let Ok(url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
        eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let pid = std::process::id();
    let device_id = format!("it{pid}");
    let path = std::env::temp_dir().join(format!("superhet-sdrd-live-{pid}.cu8"));
    write_tone(&path);

    let client = connect(&url).await;
    let mut sub = client
        .subscribe(format!("sh.v1.dev.{device_id}.spectrum"))
        .await
        .expect("subscribe");
    client.flush().await.expect("flush");
    let _daemon = start_daemon(&url, &device_id, &path).await;

    let started = Instant::now();
    let mut frames = Vec::new();
    while started.elapsed() < Duration::from_secs(2) {
        if let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(500), sub.next()).await {
            frames.push(SpectrumFrame::decode(&msg.payload).expect("decode"));
        }
    }
    let _ = std::fs::remove_file(&path);

    // Default 25 fps; allow generous slack for scheduling and start-up.
    assert!(
        (30..=70).contains(&frames.len()),
        "got {} frames in 2 s",
        frames.len()
    );
    for frame in &frames {
        assert_eq!(frame.center_hz, 433_920_000);
        assert_eq!(frame.sample_rate, RATE);
        assert_eq!(frame.bins.len(), 1024);
        let peak = frame
            .bins
            .iter()
            .enumerate()
            .max_by_key(|(_, q)| **q)
            .map(|(i, _)| i)
            .expect("bins");
        assert_eq!(peak, 512 + 50);
    }
}

async fn request_json(
    client: &async_nats::Client,
    subject: String,
    body: &str,
) -> serde_json::Value {
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        client.request(subject, body.to_owned().into()),
    )
    .await
    .expect("request timed out")
    .expect("request failed");
    serde_json::from_slice(&reply.payload).expect("reply is json")
}

#[tokio::test]
async fn tune_over_ctl_updates_state_kv_and_spectrum_frames() {
    let Ok(url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
        eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let pid = std::process::id();
    let device_id = format!("tune{pid}");
    let path = std::env::temp_dir().join(format!("superhet-sdrd-tune-{pid}.cu8"));
    write_tone(&path);

    let client = connect(&url).await;
    let mut spectrum = client
        .subscribe(format!("sh.v1.dev.{device_id}.spectrum"))
        .await
        .expect("subscribe");
    let mut states = client
        .subscribe(format!("sh.v1.dev.{device_id}.state"))
        .await
        .expect("subscribe");
    client.flush().await.expect("flush");
    let _daemon = start_daemon(&url, &device_id, &path).await;
    let ctl = |cmd: &str| format!("sh.v1.dev.{device_id}.ctl.{cmd}");

    // Initial state is available through the request and carries the opening tuning.
    let reply = request_json(&client, ctl("state"), "").await;
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["state"]["device"], device_id.as_str());
    assert_eq!(reply["state"]["driver"], "file");
    assert_eq!(reply["state"]["center_hz"], 433_920_000);
    assert_eq!(reply["state"]["tuning_epoch"], 0);

    // A burst of retunes collapses into (about) one; the last value wins.
    let burst: Vec<_> = [
        100_000_000u64,
        101_000_000,
        102_000_000,
        103_000_000,
        145_500_000,
    ]
    .into_iter()
    .map(|hz| {
        let client = client.clone();
        let subject = ctl("tune");
        tokio::spawn(async move {
            request_json(&client, subject, &format!(r#"{{"v":1,"center_hz":{hz}}}"#)).await
        })
    })
    .collect();
    let mut replies = Vec::new();
    for handle in burst {
        replies.push(handle.await.expect("join"));
    }
    for reply in &replies {
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["state"]["center_hz"], 145_500_000, "{reply}");
    }
    let epoch = replies[0]["state"]["tuning_epoch"].as_u64().expect("epoch");
    assert!(
        (1..=2).contains(&epoch),
        "burst was not coalesced: epoch {epoch}"
    );

    // Spectrum frames switch to the new centre and never go back.
    let mut seen_new = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen_new < 10 {
        assert!(Instant::now() < deadline, "no frames with the new centre");
        if let Ok(Some(msg)) =
            tokio::time::timeout(Duration::from_millis(500), spectrum.next()).await
        {
            let frame = SpectrumFrame::decode(&msg.payload).expect("decode");
            if frame.center_hz == 145_500_000 {
                seen_new += 1;
                assert_eq!(frame.sample_rate, RATE);
            } else {
                assert_eq!(seen_new, 0, "old centre after the new one");
            }
        }
    }

    // The state subject saw the change.
    let mut published = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !published && Instant::now() < deadline {
        if let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(500), states.next()).await
        {
            let state: serde_json::Value = serde_json::from_slice(&msg.payload).expect("json");
            published = state["center_hz"] == 145_500_000 && state["tuning_epoch"] == epoch;
        }
    }
    assert!(published, "state subject never showed the new tuning");

    // So does the KV bucket (written asynchronously, hence polling).
    let js = async_nats::jetstream::new(client.clone());
    let key = format!("dev.{device_id}");
    let deadline = Instant::now() + Duration::from_secs(10);
    let stored = loop {
        let value = match js.get_key_value("STATE").await {
            Ok(store) => store.get(&key).await.ok().flatten(),
            Err(_) => None,
        };
        let state = value.and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok());
        if let Some(state) = state
            && state["center_hz"] == 145_500_000
        {
            break state;
        }
        assert!(Instant::now() < deadline, "KV never showed the new tuning");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(stored["device"], device_id.as_str());

    // Errors are reported, not applied.
    for (body, code) in [
        (r#"{"v":1,"center_hz":0}"#, "invalid"),
        ("not json", "invalid"),
        (r#"{"v":1,"sample_rate":500000}"#, "unsupported"),
        (r#"{"v":1,"gain":"auto"}"#, "unsupported"),
    ] {
        let reply = request_json(&client, ctl("tune"), body).await;
        assert_eq!(
            (reply["ok"].clone(), reply["code"].clone()),
            (false.into(), code.into()),
            "{body}: {reply}"
        );
        assert!(reply["error"].is_string());
    }
    let reply = request_json(&client, ctl("bogus"), "").await;
    assert_eq!(reply["ok"], false);
    let reply = request_json(&client, ctl("state"), "").await;
    assert_eq!(reply["state"]["center_hz"], 145_500_000);
    let _ = std::fs::remove_file(&path);
}

/// Receive audio frames until `count` arrived or `timeout` passed.
async fn recv_audio(
    sub: &mut async_nats::Subscriber,
    count: usize,
    timeout: Duration,
) -> Vec<superhet_bus::audio::AudioFrame> {
    let deadline = Instant::now() + timeout;
    let mut frames = Vec::new();
    while frames.len() < count && Instant::now() < deadline {
        if let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(200), sub.next()).await {
            frames.push(superhet_bus::audio::AudioFrame::decode(&msg.payload).expect("decode"));
        }
    }
    frames
}

/// Throw away whatever is buffered or arrives within `quiet`.
async fn drain_audio(sub: &mut async_nats::Subscriber, quiet: Duration) {
    while let Ok(Some(_)) = tokio::time::timeout(quiet, sub.next()).await {}
}

#[tokio::test]
async fn vfo_audio_flows_through_real_nats_and_follows_retunes() {
    use superhet_bus::audio::FRAME_SAMPLES;
    use superhet_bus::opus::Decoder;

    let Ok(url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
        eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let pid = std::process::id();
    let device_id = format!("vfo{pid}");
    let path = std::env::temp_dir().join(format!("superhet-sdrd-vfo-{pid}.cu8"));
    write_tone(&path);

    let client = connect(&url).await;
    let mut audio = client
        .subscribe(format!("sh.v1.dev.{device_id}.audio.*"))
        .await
        .expect("subscribe");
    client.flush().await.expect("flush");
    let _daemon = start_daemon(&url, &device_id, &path).await;
    let ctl = |cmd: &str| format!("sh.v1.dev.{device_id}.ctl.{cmd}");

    // No VFO, no audio; the state document lists none.
    let reply = request_json(&client, ctl("state"), "").await;
    assert_eq!(reply["state"]["vfos"], serde_json::json!([]));
    assert!(
        recv_audio(&mut audio, 1, Duration::from_millis(500))
            .await
            .is_empty()
    );

    // Create a VFO on the 50 kHz tone.
    let reply = request_json(
        &client,
        ctl("vfo.create"),
        r#"{"v":1,"freq_hz":433970000,"mode":"nfm","squelch_dbfs":-40}"#,
    )
    .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["vfo"]["id"], 1);
    assert_eq!(reply["vfo"]["status"], "active");
    assert_eq!(reply["vfo"]["offset_hz"], 50_000.0);
    assert_eq!(reply["vfo"]["bandwidth_hz"], 12_500);
    assert_eq!(reply["state"]["vfos"][0]["id"], 1);

    let frames = recv_audio(&mut audio, 30, Duration::from_secs(10)).await;
    assert!(frames.len() >= 25, "got {} frames", frames.len());
    for pair in frames.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq.wrapping_add(1));
        assert_eq!(
            pair[1].sample_pos,
            pair[0].sample_pos + FRAME_SAMPLES as u64
        );
        assert!(pair[1].capture_unix_ns >= pair[0].capture_unix_ns);
    }
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos() as u64;
    for frame in &frames {
        assert_eq!(frame.vfo_id, 1);
        assert_eq!(frame.sample_rate, 48_000);
        assert_eq!(frame.channels, 1);
        assert!(frame.capture_unix_ns <= now_ns);
        assert!(now_ns - frame.capture_unix_ns < 30_000_000_000);
        assert!(frame.power_dbfs > -20.0, "tone should read near 0 dBFS");
    }
    let mut decoder = Decoder::new().expect("decoder");
    let mut pcm = vec![0f32; FRAME_SAMPLES];
    let decoded = decoder
        .decode_float(&frames[0].payload, &mut pcm)
        .expect("opus decode");
    assert_eq!(decoded, FRAME_SAMPLES);

    // The state document (and its KV copy) shows the VFO.
    let reply = request_json(&client, ctl("state"), "").await;
    assert_eq!(reply["state"]["vfos"][0]["status"], "active");

    // Bad requests are rejected with the usual codes; the VFO stays untouched.
    for (cmd, body, code) in [
        (
            "vfo.create",
            r#"{"v":1,"freq_hz":1,"mode":"ssb"}"#,
            "invalid",
        ),
        ("vfo.create", "nope", "invalid"),
        ("vfo.update", r#"{"v":1,"id":99,"freq_hz":5}"#, "not_found"),
        ("vfo.update", r#"{"v":1,"id":1}"#, "invalid"),
        ("vfo.delete", r#"{"v":1,"id":99}"#, "not_found"),
        ("vfo.bogus", "{}", "invalid"),
    ] {
        let reply = request_json(&client, ctl(cmd), body).await;
        assert_eq!(
            (reply["ok"].clone(), reply["code"].clone()),
            (false.into(), code.into()),
            "{cmd} {body}: {reply}"
        );
    }

    // Partial update: only the given fields change.
    let reply = request_json(
        &client,
        ctl("vfo.update"),
        r#"{"v":1,"id":1,"squelch_dbfs":null,"bandwidth_hz":8000}"#,
    )
    .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["vfo"]["squelch_dbfs"], serde_json::Value::Null);
    assert_eq!(reply["vfo"]["bandwidth_hz"], 8000);
    assert_eq!(reply["vfo"]["freq_hz"], 433_970_000u64);
    assert_eq!(reply["vfo"]["mode"], "nfm");

    // Retune away: the VFO pauses and the audio stops ...
    let reply = request_json(&client, ctl("tune"), r#"{"v":1,"center_hz":440000000}"#).await;
    assert_eq!(reply["state"]["vfos"][0]["status"], "paused", "{reply}");
    drain_audio(&mut audio, Duration::from_millis(400)).await;
    assert!(
        recv_audio(&mut audio, 1, Duration::from_millis(700))
            .await
            .is_empty()
    );
    // ... and comes back with the offset recomputed.
    let reply = request_json(&client, ctl("tune"), r#"{"v":1,"center_hz":433920000}"#).await;
    assert_eq!(reply["state"]["vfos"][0]["status"], "active", "{reply}");
    let resumed = recv_audio(&mut audio, 10, Duration::from_secs(10)).await;
    assert!(resumed.len() >= 10, "audio did not resume");

    // Limit of eight VFOs.
    for n in 2..=8 {
        let reply = request_json(
            &client,
            ctl("vfo.create"),
            r#"{"v":1,"freq_hz":433920000,"mode":"am"}"#,
        )
        .await;
        assert_eq!(reply["vfo"]["id"], n, "{reply}");
    }
    let reply = request_json(
        &client,
        ctl("vfo.create"),
        r#"{"v":1,"freq_hz":433920000,"mode":"am"}"#,
    )
    .await;
    assert_eq!(
        (reply["ok"].clone(), reply["code"].clone()),
        (false.into(), "unsupported".into())
    );
    for n in 2..=8 {
        let reply = request_json(
            &client,
            ctl("vfo.delete"),
            &format!(r#"{{"v":1,"id":{n}}}"#),
        )
        .await;
        assert_eq!(reply["ok"], true, "{reply}");
    }

    // Delete the last VFO: audio ends and the state is empty again.
    let reply = request_json(&client, ctl("vfo.delete"), r#"{"v":1,"id":1}"#).await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["state"]["vfos"], serde_json::json!([]));
    drain_audio(&mut audio, Duration::from_millis(400)).await;
    assert!(
        recv_audio(&mut audio, 1, Duration::from_millis(700))
            .await
            .is_empty()
    );
    let _ = std::fs::remove_file(&path);
}
