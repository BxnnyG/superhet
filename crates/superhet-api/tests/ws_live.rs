//! End-to-end test: real NATS server, real `superhet-api` binary, WebSocket client.
//! Skipped unless `SUPERHET_TEST_NATS_URL` is set (optional: `SUPERHET_TEST_NATS_USER` /
//! `SUPERHET_TEST_NATS_PASSWORD`, also forwarded to the API under test).

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
    let device = format!("it{}", std::process::id());
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("free port")
        .port();

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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn api"));

    let deadline = Instant::now() + Duration::from_secs(15);
    while readyz_status(port) != Some(200) {
        assert!(Instant::now() < deadline, "/readyz never became 200");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/api/v1/ws/spectrum?dev={device}"
    ))
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
