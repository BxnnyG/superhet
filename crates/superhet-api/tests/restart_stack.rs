//! ROADMAP P1.5 criterion 9, second half: restarting `superhet-api` neither interrupts the
//! daemon nor loses audit events of system origin. Runs the same topology as the compose stack
//! (api + sdrd + worker on one NATS and one PostgreSQL) as processes. Skipped unless
//! `SUPERHET_TEST_NATS_URL` and `DATABASE_URL` are set; needs `cargo build -p superhet-sdrd
//! -p superhet-worker` first.

mod common;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::*;
use serde_json::json;
use superhet_bus::{
    audit::{Outcome, SystemEvent},
    subjects,
};

const RATE: u32 = 2_048_000;

fn spawn_with_nats(bin: std::path::PathBuf, url: &str, listen: u16) -> Command {
    let mut command = Command::new(bin);
    command
        .env("SUPERHET_LISTEN", format!("127.0.0.1:{listen}"))
        .env("SUPERHET_NATS_URL", url)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let (Ok(user), Ok(password)) = (
        std::env::var("SUPERHET_TEST_NATS_USER"),
        std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
    ) {
        command
            .env("SUPERHET_NATS_USER", user)
            .env("SUPERHET_NATS_PASSWORD", password);
    }
    command
}

async fn publish(url: &str, action: &str) {
    let options = match (
        std::env::var("SUPERHET_TEST_NATS_USER"),
        std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
    ) {
        (Ok(user), Ok(password)) => {
            async_nats::ConnectOptions::with_user_and_password(user, password)
        }
        _ => async_nats::ConnectOptions::new(),
    };
    let client = options.connect(url).await.expect("connect nats");
    let event = SystemEvent {
        v: 1,
        ts_ms: None,
        action: action.to_owned(),
        source: "restart-test".to_owned(),
        target: Some("dev0".to_owned()),
        outcome: Outcome::Error,
        details: Some(json!({"reason": "test"})),
    };
    client
        .publish(
            subjects::audit("restart-test").expect("subject"),
            serde_json::to_vec(&event).expect("json").into(),
        )
        .await
        .expect("publish");
    client.flush().await.expect("flush");
}

/// The worker only has `/healthz`.
async fn wait_healthy(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let up = tokio::task::spawn_blocking(move || {
            std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
        })
        .await
        .unwrap_or(false);
        if up {
            return;
        }
        assert!(Instant::now() < deadline, "worker never listened");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until the audit log holds a system row for `action` and return how many there are.
async fn system_rows(port: u16, admin: &str, action: &str) -> usize {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let rows = audit(port, admin, &format!("action={action}&actor_kind=system")).len();
        if rows > 0 || Instant::now() > deadline {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_api_restart_keeps_the_daemon_running_and_loses_no_system_events() {
    let (Ok(nats_url), Some(db)) = (
        std::env::var("SUPERHET_TEST_NATS_URL"),
        TestDb::create("restart").await,
    ) else {
        eprintln!("SUPERHET_TEST_NATS_URL / DATABASE_URL not set; skipping");
        return;
    };
    let pid = std::process::id();
    let device = format!("rs{pid}");
    let tone = std::env::temp_dir().join(format!("superhet-restart-{pid}.cu8"));
    std::fs::write(&tone, vec![128u8; 8 * 4096 * 2]).expect("fixture");

    let api_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_superhet-api"));
    let sdrd_bin = api_bin.with_file_name("superhet-sdrd");
    let worker_bin = api_bin.with_file_name("superhet-worker");
    for bin in [&sdrd_bin, &worker_bin] {
        assert!(
            bin.exists(),
            "{} not found: run `cargo build -p superhet-sdrd -p superhet-worker`",
            bin.display()
        );
    }
    let (sdrd_port, worker_port) = (free_port(), free_port());
    let _sdrd = Proc(
        spawn_with_nats(sdrd_bin, &nats_url, sdrd_port)
            .env("SUPERHET_DEVICE_ID", &device)
            .env(
                "SUPERHET_DEVICE",
                format!(
                    "file:{}?format=cu8&rate={RATE}&freq=100000000&block=4096",
                    tone.display()
                ),
            )
            .spawn()
            .expect("spawn sdrd"),
    );
    let spawn_api_proc = |db_url: &str| {
        spawn_api(db_url, |c| {
            c.env("SUPERHET_NATS_URL", &nats_url);
            if let (Ok(user), Ok(password)) = (
                std::env::var("SUPERHET_TEST_NATS_USER"),
                std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
            ) {
                c.env("SUPERHET_NATS_USER", user)
                    .env("SUPERHET_NATS_PASSWORD", password);
            }
        })
    };

    wait_ready(sdrd_port, "sdrd").await;
    let (api, port) = spawn_api_proc(&db.url);
    wait_ready(port, "api").await;
    // The API applies the migrations; the worker needs the schema.
    let _worker = Proc(
        spawn_with_nats(worker_bin, &nats_url, worker_port)
            .env("SUPERHET_DATABASE_URL", &db.url)
            .spawn()
            .expect("spawn worker"),
    );
    wait_healthy(worker_port).await;
    // Core NATS does not replay: give the worker a moment to subscribe.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let admin = setup_admin(port);

    // A system event while everything runs.
    publish(&nats_url, "restart.before").await;
    assert_eq!(system_rows(port, &admin, "restart.before").await, 1);
    let state = format!("/api/v1/devices/{device}/state");
    assert_eq!(req(port, "GET", &state, Some(&admin), None).status, 200);

    // The API goes away. The daemon keeps serving, and an event published meanwhile is
    // persisted by the worker.
    drop(api);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        readyz_status(sdrd_port),
        Some(200),
        "sdrd must not notice the api restart"
    );
    publish(&nats_url, "restart.during").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // The new API process sees both events, accepts the old session and reaches the daemon.
    let (_api, port) = spawn_api_proc(&db.url);
    wait_ready(port, "api").await;
    assert_eq!(system_rows(port, &admin, "restart.before").await, 1);
    assert_eq!(
        system_rows(port, &admin, "restart.during").await,
        1,
        "the event published while the api was down is in the log"
    );
    let status = req(port, "GET", "/api/v1/auth/status", Some(&admin), None).json();
    assert_eq!(status["authenticated"], true, "the session survives");
    assert_eq!(req(port, "GET", &state, Some(&admin), None).status, 200);
    assert_eq!(readyz_status(sdrd_port), Some(200));
    let _ = std::fs::remove_file(&tone);
}
