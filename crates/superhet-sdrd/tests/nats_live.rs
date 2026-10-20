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
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("free port")
        .port();

    let mut options = async_nats::ConnectOptions::new();
    if let (Ok(user), Ok(password)) = (
        std::env::var("SUPERHET_TEST_NATS_USER"),
        std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
    ) {
        options = options.user_and_password(user, password);
    }
    let client = options.connect(url.as_str()).await.expect("connect");
    let mut sub = client
        .subscribe(format!("sh.v1.dev.{device_id}.spectrum"))
        .await
        .expect("subscribe");
    client.flush().await.expect("flush");

    let _daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_superhet-sdrd"))
            .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
            .env("SUPERHET_NATS_URL", &url)
            .env("SUPERHET_DEVICE_ID", &device_id)
            .env("SUPERHET_SPECTRUM_BINS", "1024")
            .env("SUPERHET_FFT_SIZE", "4096")
            .env(
                "SUPERHET_DEVICE",
                format!(
                    "file:{}?format=cu8&rate={RATE}&freq=433920000&block=4096",
                    path.display()
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
