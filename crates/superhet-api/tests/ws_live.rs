//! End-to-end test: real NATS server, real PostgreSQL, real `superhet-api` binary, WebSocket
//! client with a session cookie. Skipped unless `SUPERHET_TEST_NATS_URL` and `DATABASE_URL` are
//! set (optional: `SUPERHET_TEST_NATS_USER` / `SUPERHET_TEST_NATS_PASSWORD`, also forwarded to
//! the API under test).

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use superhet_bus::spectrum::SpectrumFrame;
use tokio_tungstenite::tungstenite::Message;

struct Api(Child);

impl Drop for Api {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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

#[tokio::test]
async fn nats_frames_reach_websocket_client_cropped() {
    let Ok(url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
        eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let Some(db) = common::TestDb::create("ws").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let device = format!("it{}", std::process::id());
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
    let publisher = options.connect(url.as_str()).await.expect("connect");
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
    while readyz_status(port) != Some(200) {
        assert!(Instant::now() < deadline, "/readyz never became 200");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let cookie = common::setup_admin(port);
    let mut ws = common::ws_connect_with_cookie(
        &format!("ws://127.0.0.1:{port}/api/v1/ws/spectrum?dev={device}"),
        &cookie,
    )
    .await
    .expect("ws connect");
    ws.send(Message::text(
        r#"{"type":"view","start_hz":99900000,"end_hz":100100000,"bins":50}"#,
    ))
    .await
    .expect("send view");

    let mut db = vec![-90.0f32; 1000];
    db[501] = -10.0;
    let payload = SpectrumFrame::quantize(100_000_000, 1_000_000, 1, -100.0, 0.0, &db).encode();
    let subject = format!("sh.v1.dev.{device}.spectrum");
    for _ in 0..100 {
        publisher
            .publish(subject.clone(), payload.clone().into())
            .await
            .expect("publish");
        if let Ok(Some(Ok(Message::Binary(data)))) =
            tokio::time::timeout(Duration::from_millis(200), ws.next()).await
        {
            let frame = SpectrumFrame::decode(&data).expect("valid frame");
            if frame.bins.len() != 50 {
                continue; // frame sent before the view took effect
            }
            assert_eq!(frame.center_hz, 100_000_000);
            assert_eq!(frame.sample_rate, 200_000);
            assert!(frame.bins.iter().copied().max() >= Some(229), "peak lost");
            return;
        }
    }
    panic!("no cropped frame received");
}

#[tokio::test]
async fn narrow_view_leases_a_slice_and_zoom_frames_reach_the_client() {
    let Ok(url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
        eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let Some(db) = common::TestDb::create("wszoom").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let device = format!("zm{}", std::process::id());
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
    let bus = options.connect(url.as_str()).await.expect("connect");
    let mut lease = bus
        .subscribe(format!("sh.v1.dev.{device}.ctl.spectrum.zoom"))
        .await
        .expect("subscribe");
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
    while readyz_status(port) != Some(200) {
        assert!(Instant::now() < deadline, "/readyz never became 200");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let cookie = common::setup_admin(port);
    let mut ws = common::ws_connect_with_cookie(
        &format!("ws://127.0.0.1:{port}/api/v1/ws/spectrum?dev={device}"),
        &cookie,
    )
    .await
    .expect("ws connect");
    ws.send(Message::text(
        r#"{"type":"view","center_hz":100000000,"span_hz":100000,"bins":4096}"#,
    ))
    .await
    .expect("send view");

    // Full-band frames tell the gateway how wide the band is; the slice is leased afterwards.
    let full = SpectrumFrame::quantize(100_000_000, 1_000_000, 1, -100.0, 0.0, &[-90.0; 1000]);
    let mut zoom_db = vec![-90.0f32; 800];
    zoom_db[400] = -10.0;
    let zoom = SpectrumFrame::quantize(100_000_000, 125_000, 2, -100.0, 0.0, &zoom_db);
    let full_subject = format!("sh.v1.dev.{device}.spectrum");
    let zoom_subject = format!("sh.v1.dev.{device}.spectrum.zoom");

    let mut request = None;
    for _ in 0..100 {
        bus.publish(full_subject.clone(), full.encode().into())
            .await
            .expect("publish");
        if let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(100), lease.next()).await
        {
            request = Some(msg.payload.to_vec());
            break;
        }
    }
    assert_eq!(
        request.as_deref(),
        Some(br#"{"v":1,"center_hz":100000000,"span_hz":100000}"#.as_slice())
    );

    // Zoom frames from the daemon are cropped to the view and sent at their own resolution.
    for _ in 0..100 {
        bus.publish(zoom_subject.clone(), zoom.encode().into())
            .await
            .expect("publish");
        bus.publish(full_subject.clone(), full.encode().into())
            .await
            .expect("publish");
        if let Ok(Some(Ok(Message::Binary(data)))) =
            tokio::time::timeout(Duration::from_millis(200), ws.next()).await
        {
            let frame = SpectrumFrame::decode(&data).expect("valid frame");
            if frame.timestamp_us != 2 {
                continue; // a full-band frame sent before the first zoom frame arrived
            }
            // 100 kHz at 156.25 Hz per bin.
            assert!(frame.bins.len() >= 640, "{} bins", frame.bins.len());
            assert!(frame.sample_rate >= 100_000 && frame.sample_rate <= 100_200);
            return;
        }
    }
    panic!("no zoom frame received");
}
