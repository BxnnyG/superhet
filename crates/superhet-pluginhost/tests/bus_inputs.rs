//! `BusInputs` against a real NATS server playing the device daemon; skipped unless
//! `SUPERHET_TEST_NATS_URL` is set (optional `SUPERHET_TEST_NATS_USER` / `_PASSWORD`).
//!
//! A plain tap (no claim) must keep asking the daemon for its lease, and a claim must follow a
//! retune of its centre at once.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;
use superhet_bus::tap::{ClaimAcquire, TapFormat, TapKind, TapOpen, VERSION};
use superhet_pluginhost::inputs::BusInputs;
use superhet_pluginhost::supervisor::{InputHealth, InputRequest, Inputs};

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

fn tap(id: &str, kind: TapKind, center_hz: Option<u64>) -> TapOpen {
    TapOpen {
        v: VERSION,
        id: id.to_owned(),
        kind,
        vfo: (kind != TapKind::Window).then_some(1),
        rate: 2_048_000,
        format: TapFormat::Cu8,
        center_hz,
    }
}

fn health() -> InputHealth {
    InputHealth {
        dropped: Arc::new(AtomicU64::new(0)),
        status: tokio::sync::watch::channel(None).0,
    }
}

#[tokio::test]
async fn bus_inputs_refresh_the_tap_and_move_the_claim() {
    let Some(client) = connect().await else {
        eprintln!("SUPERHET_TEST_NATS_URL not set, skipping");
        return;
    };
    let device = format!("dev{}", std::process::id());
    let mut taps = client
        .subscribe(format!("sh.v1.dev.{device}.ctl.tap.open"))
        .await
        .expect("sub");
    let mut claims = client
        .subscribe(format!("sh.v1.dev.{device}.ctl.claim.acquire"))
        .await
        .expect("sub");
    // The daemon: acknowledges everything.
    let responder = {
        let client = client.clone();
        let device = device.clone();
        tokio::spawn(async move {
            let mut all = client
                .subscribe(format!("sh.v1.dev.{device}.ctl.>"))
                .await
                .expect("sub");
            while let Some(message) = all.next().await {
                if let Some(reply) = message.reply {
                    let _ = client
                        .publish(reply, json!({"v": 1, "ok": true}).to_string().into())
                        .await;
                }
            }
        })
    };
    client.flush().await.expect("flush");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let inputs = BusInputs::new(client.clone());

    // A VFO tap: no claim, no retune handle, but the lease request must still arrive.
    let plain = InputRequest {
        device: device.clone(),
        tap: tap("i-plain", TapKind::Audio, None),
        claim: None,
    };
    let attachment = inputs.attach(&plain, health()).expect("attach");
    assert!(attachment.retune.is_none());
    let first = tokio::time::timeout(Duration::from_secs(10), taps.next())
        .await
        .expect("a tap lease request")
        .expect("message");
    assert!(String::from_utf8_lossy(&first.payload).contains("i-plain"));
    drop(attachment);

    // A claim: moving the centre re-acquires at once with the new centre.
    let window = InputRequest {
        device: device.clone(),
        tap: tap("i-claim", TapKind::Window, Some(178_352_000)),
        claim: Some(ClaimAcquire {
            v: VERSION,
            id: "i-claim".into(),
            center_hz: 178_352_000,
            sample_rate: Some(2_048_000),
        }),
    };
    let attachment = inputs.attach(&window, health()).expect("attach");
    let retune = attachment.retune.clone().expect("claims can be moved");
    let first = tokio::time::timeout(Duration::from_secs(10), claims.next())
        .await
        .expect("claim")
        .expect("message");
    assert!(String::from_utf8_lossy(&first.payload).contains("178352000"));
    retune.send(227_360_000).expect("retune");
    let moved = tokio::time::timeout(Duration::from_secs(3), claims.next())
        .await
        .expect("a claim at the new centre without waiting for the lease tick")
        .expect("message");
    assert!(String::from_utf8_lossy(&moved.payload).contains("227360000"));
    drop(attachment);
    responder.abort();
}
