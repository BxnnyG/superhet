//! End-to-end test of profiles, the tuning lock, free tuning, the state feed and the restore of
//! the daemon's state: real NATS server, real PostgreSQL, real `superhet-api` and
//! `superhet-sdrd` binaries (file device, no hardware).
//! Skipped unless `SUPERHET_TEST_NATS_URL` and `DATABASE_URL` are set (optional
//! `SUPERHET_TEST_NATS_USER` / `SUPERHET_TEST_NATS_PASSWORD`, also forwarded to both services).
//! The sdrd binary must have been built (`cargo build -p superhet-sdrd`, done by
//! `cargo test --workspace`); it is looked up next to the API binary.

mod common;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{Proc, free_port, http_async, wait_ready};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

const RATE: u32 = 1_024_000;
const CENTER: u64 = 433_920_000;

struct Stack {
    sdrd: Option<Proc>,
    _api: Proc,
    api_port: u16,
    device: String,
    nats: async_nats::Client,
    nats_url: String,
    tone: std::path::PathBuf,
    cookie: String,
    _db: common::TestDb,
}

impl Drop for Stack {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.tone);
    }
}

fn creds() -> Option<(String, String)> {
    match (
        std::env::var("SUPERHET_TEST_NATS_USER"),
        std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
    ) {
        (Ok(user), Ok(password)) => Some((user, password)),
        _ => None,
    }
}

fn sdrd_bin() -> std::path::PathBuf {
    let bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_superhet-api"))
        .with_file_name("superhet-sdrd");
    assert!(
        bin.exists(),
        "{} not found: run `cargo build -p superhet-sdrd` first",
        bin.display()
    );
    bin
}

/// Start an sdrd on the file device; returns the process and its HTTP port.
fn spawn_sdrd(url: &str, device: &str, tone: &std::path::Path) -> (Proc, u16) {
    let port = free_port();
    let mut command = Command::new(sdrd_bin());
    if let Some((user, password)) = creds() {
        command
            .env("SUPERHET_NATS_USER", user)
            .env("SUPERHET_NATS_PASSWORD", password);
    }
    let process = command
        .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
        .env("SUPERHET_NATS_URL", url)
        .env("SUPERHET_DEVICE_ID", device)
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
        .expect("spawn sdrd");
    (Proc(process), port)
}

async fn start_stack(url: &str, tag: &str) -> Option<Stack> {
    let db = common::TestDb::create(tag).await?;
    let pid = std::process::id();
    let device = format!("{tag}{pid}");
    let tone = std::env::temp_dir().join(format!("superhet-api-{tag}-{pid}.cu8"));
    std::fs::write(&tone, vec![128u8; 8 * 4096 * 2]).expect("write fixture");

    let mut options = async_nats::ConnectOptions::new();
    if let Some((user, password)) = creds() {
        options = options.user_and_password(user, password);
    }
    let nats = options.connect(url).await.expect("connect");

    let (sdrd, sdrd_port) = spawn_sdrd(url, &device, &tone);
    let (api, api_port) = common::spawn_api(&db.url, |command| {
        command.env("SUPERHET_NATS_URL", url);
        if let Some((user, password)) = creds() {
            command
                .env("SUPERHET_NATS_USER", user)
                .env("SUPERHET_NATS_PASSWORD", password);
        }
    });
    wait_ready(sdrd_port, "sdrd").await;
    wait_ready(api_port, "api").await;
    let cookie = common::setup_admin(api_port);
    Some(Stack {
        sdrd: Some(sdrd),
        _api: api,
        api_port,
        device,
        nats,
        nats_url: url.to_owned(),
        tone,
        cookie,
        _db: db,
    })
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Stack {
    async fn rest(&self, method: &'static str, path: String, body: Option<Value>) -> Reply {
        let mut headers = vec![("Cookie".to_owned(), self.cookie.clone())];
        let body = match body {
            Some(body) => {
                headers.push(("Content-Type".into(), "application/json".into()));
                body.to_string()
            }
            None => String::new(),
        };
        let reply = http_async(self.api_port, method, path, headers, body).await;
        Reply {
            status: reply.status,
            body: reply.json(),
            headers: reply.headers,
        }
    }

    fn dev(&self, tail: &str) -> String {
        format!("/api/v1/devices/{}/{tail}", self.device)
    }

    async fn state(&self) -> Value {
        let reply = self.rest("GET", self.dev("state"), None).await;
        assert_eq!(reply.status, 200, "{:?}", reply.body);
        reply.body
    }

    async fn tune(&self, body: Value) -> Reply {
        self.rest("POST", self.dev("tune"), Some(body)).await
    }

    async fn vfo(&self, freq_hz: u64) -> Reply {
        self.rest(
            "POST",
            self.dev("vfos"),
            Some(json!({"freq_hz": freq_hz, "mode": "nfm", "squelch_dbfs": -45})),
        )
        .await
    }

    /// Delete every VFO (the daemon creates one on a fresh start).
    async fn clear_vfos(&self) {
        for vfo in self.state().await["vfos"].as_array().expect("vfos") {
            let path = self.dev(&format!("vfos/{}", vfo["id"]));
            assert_eq!(self.rest("DELETE", path, None).await.status, 204);
        }
    }

    /// Poll the live state until `check` holds (the KV copy lags the reply by a moment).
    async fn wait_state(&self, what: &str, check: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = self.state().await;
            if check(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {state}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn header<'a>(reply: &'a Reply, name: &str) -> Option<&'a str> {
    reply
        .headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// What a profile covers, ids/epoch/windows aside.
/// The VFO list as the daemon stores it: the per-viewer `mine` flag is added by the API only.
fn without_mine(vfos: &Value) -> Value {
    let mut vfos = vfos.clone();
    for vfo in vfos.as_array_mut().into_iter().flatten() {
        if let Some(object) = vfo.as_object_mut() {
            object.remove("mine");
        }
    }
    vfos
}

fn essence(state: &Value) -> Value {
    let vfos: Vec<Value> = state["vfos"]
        .as_array()
        .expect("vfos")
        .iter()
        .map(|v| {
            let mut v = v.clone();
            v["id"] = Value::Null;
            // Ownership differs between VFOs created by a user and those a profile restores.
            if let Some(object) = v.as_object_mut() {
                object.remove("owner");
                object.remove("mine");
            }
            v
        })
        .collect();
    json!({
        "center_hz": state["center_hz"],
        "sample_rate": state["sample_rate"],
        "gain": state["gain"],
        "ppm": state["ppm"],
        "bias_tee": state["bias_tee"],
        "vfos": vfos,
        "locks": state["locks"].as_array().map(Vec::len),
    })
}

/// A stack exactly as the daemon starts it, including its default VFO.
macro_rules! fresh_stack_or_skip {
    ($tag:expr) => {{
        let Ok(url) = std::env::var("SUPERHET_TEST_NATS_URL") else {
            eprintln!("SUPERHET_TEST_NATS_URL not set; skipping");
            return;
        };
        let Some(stack) = start_stack(&url, $tag).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        stack
    }};
}

/// A stack whose default VFO is deleted, so a test starts from an empty VFO set.
macro_rules! stack_or_skip {
    ($tag:expr) => {{
        let stack = fresh_stack_or_skip!($tag);
        stack.clear_vfos().await;
        stack
    }};
}

#[tokio::test(flavor = "multi_thread")]
async fn profile_save_load_round_trip_and_crud() {
    let stack = stack_or_skip!("pr");

    // Dress the device: a centre, three VFOs (one outside the window makes the centre follow).
    assert_eq!(
        stack
            .tune(json!({"center_hz": 433_500_000u64}))
            .await
            .status,
        200
    );
    assert_eq!(stack.vfo(433_600_000).await.status, 201);
    let wfm = stack
        .rest(
            "POST",
            stack.dev("vfos"),
            Some(json!({"freq_hz": 433_300_000u64, "mode": "wfm", "bandwidth_hz": 150_000})),
        )
        .await;
    assert_eq!(wfm.status, 201, "{:?}", wfm.body);
    let saved_state = stack.state().await;
    assert_eq!(saved_state["vfos"].as_array().map(Vec::len), Some(2));

    // Save it.
    let saved = stack
        .rest(
            "POST",
            stack.dev("profile/save"),
            Some(json!({"name": "  Test band  "})),
        )
        .await;
    assert_eq!(saved.status, 201, "{:?}", saved.body);
    let profile = saved.body["profile"].clone();
    let id = profile["id"].as_str().expect("id").to_owned();
    assert_eq!(profile["name"], "Test band");
    assert_eq!(profile["data"]["v"], 1);
    assert_eq!(profile["data"]["center_hz"], 433_500_000u64);
    assert_eq!(profile["data"]["sample_rate"], RATE);
    assert_eq!(profile["data"]["vfos"].as_array().map(Vec::len), Some(2));
    assert_eq!(profile["data"]["vfos"][0]["squelch_dbfs"], -45.0);
    assert_eq!(profile["data"]["lock"], false);
    assert_eq!(
        header(&saved, "location"),
        Some(format!("/api/v1/profiles/{id}").as_str())
    );

    // Change everything: centre, VFO set.
    assert_eq!(
        stack
            .tune(json!({"center_hz": 145_000_000u64}))
            .await
            .status,
        200
    );
    for vfo in saved_state["vfos"].as_array().expect("vfos") {
        let path = stack.dev(&format!("vfos/{}", vfo["id"]));
        assert_eq!(stack.rest("DELETE", path, None).await.status, 204);
    }
    assert_eq!(stack.vfo(145_100_000).await.status, 201);
    assert_ne!(essence(&stack.state().await), essence(&saved_state));

    // Load: everything is back.
    let loaded = stack
        .rest("POST", stack.dev("profile/load"), Some(json!({"id": id})))
        .await;
    assert_eq!(loaded.status, 200, "{:?}", loaded.body);
    assert_eq!(loaded.body["ok"], true);
    assert_eq!(essence(&loaded.body["state"]), essence(&saved_state));
    let after = stack
        .wait_state("restored state in KV", |s| {
            essence(s) == essence(&saved_state)
        })
        .await;
    assert_eq!(after["vfos"][0]["status"], "active");

    // CRUD.
    let list = stack.rest("GET", "/api/v1/profiles".into(), None).await;
    assert_eq!(list.status, 200);
    assert_eq!(list.body["profiles"].as_array().map(Vec::len), Some(1));
    let one = stack
        .rest("GET", format!("/api/v1/profiles/{id}"), None)
        .await;
    assert_eq!(one.body["profile"], profile);

    let data = profile["data"].clone();
    let dup = stack
        .rest(
            "POST",
            "/api/v1/profiles".into(),
            Some(json!({"name": "test BAND", "data": data})),
        )
        .await;
    assert_eq!(
        (dup.status, dup.body["code"].as_str()),
        (409, Some("conflict"))
    );
    let other = stack
        .rest(
            "POST",
            "/api/v1/profiles".into(),
            Some(json!({"name": "Other", "data": data})),
        )
        .await;
    assert_eq!(other.status, 201);
    let other_id = other.body["profile"]["id"].as_str().expect("id").to_owned();
    let rename_clash = stack
        .rest(
            "PUT",
            format!("/api/v1/profiles/{other_id}"),
            Some(json!({"name": "TEST band", "data": data})),
        )
        .await;
    assert_eq!(rename_clash.status, 409);
    let renamed = stack
        .rest(
            "PUT",
            format!("/api/v1/profiles/{id}"),
            Some(json!({"name": "Renamed", "data": data})),
        )
        .await;
    assert_eq!(renamed.status, 200);
    assert_eq!(renamed.body["profile"]["name"], "Renamed");
    assert_eq!(renamed.body["profile"]["created_at"], profile["created_at"]);

    // Invalid and unknown.
    for bad in [
        json!({"name": "x"}),
        json!({"name": "x", "data": {"v": 1, "center_hz": 0, "sample_rate": 2}}),
        json!({"name": "x", "data": {"v": 1, "center_hz": 1, "sample_rate": 2}, "extra": 1}),
    ] {
        let reply = stack
            .rest("POST", "/api/v1/profiles".into(), Some(bad.clone()))
            .await;
        assert_eq!(reply.status, 400, "{bad}");
    }
    let unknown = "00000000-0000-0000-0000-000000000000";
    for (method, body) in [
        ("GET", None),
        ("PUT", Some(json!({"name": "x", "data": data}))),
        ("DELETE", None),
    ] {
        let reply = stack
            .rest(method, format!("/api/v1/profiles/{unknown}"), body)
            .await;
        assert_eq!(reply.status, 404, "{method}");
    }
    let reply = stack
        .rest(
            "POST",
            stack.dev("profile/load"),
            Some(json!({"id": unknown})),
        )
        .await;
    assert_eq!(reply.status, 404);
    // A profile the device cannot take (the file replay has a fixed rate) is a 400.
    let mut unsupported = data.clone();
    unsupported["sample_rate"] = 2_048_000.into();
    let bad = stack
        .rest(
            "POST",
            "/api/v1/profiles".into(),
            Some(json!({"name": "Fast", "data": unsupported})),
        )
        .await;
    let bad_id = bad.body["profile"]["id"].as_str().expect("id").to_owned();
    let reply = stack
        .rest(
            "POST",
            stack.dev("profile/load"),
            Some(json!({"id": bad_id})),
        )
        .await;
    assert_eq!(
        (reply.status, reply.body["code"].as_str()),
        (400, Some("unsupported"))
    );
    assert_eq!(essence(&stack.state().await), essence(&saved_state));

    for id in [&id, &other_id, &bad_id] {
        assert_eq!(
            stack
                .rest("DELETE", format!("/api/v1/profiles/{id}"), None)
                .await
                .status,
            204
        );
    }
    let list = stack.rest("GET", "/api/v1/profiles".into(), None).await;
    assert_eq!(list.body["profiles"].as_array().map(Vec::len), Some(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn profile_save_overwrite() {
    let stack = stack_or_skip!("ow");
    let save = |body: Value| stack.rest("POST", stack.dev("profile/save"), Some(body));

    assert_eq!(stack.vfo(433_600_000).await.status, 201);
    let first = save(json!({"name": "Spot"})).await;
    assert_eq!(first.status, 201, "{:?}", first.body);
    let profile = first.body["profile"].clone();
    let id = profile["id"].as_str().expect("id").to_owned();
    assert_eq!(profile["data"]["vfos"].as_array().map(Vec::len), Some(1));

    // Without the flag (absent or false) a name clash stays a 409 and nothing changes.
    for body in [
        json!({"name": "spot"}),
        json!({"name": "spot", "overwrite": false}),
    ] {
        let clash = save(body).await;
        assert_eq!(
            (clash.status, clash.body["code"].as_str()),
            (409, Some("conflict"))
        );
    }

    // Change the device, then overwrite: same id and created_at, fresh snapshot.
    assert_eq!(stack.vfo(433_700_000).await.status, 201);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let replaced = save(json!({"name": "SPOT", "overwrite": true})).await;
    assert_eq!(replaced.status, 200, "{:?}", replaced.body);
    let new = &replaced.body["profile"];
    assert_eq!(new["id"], id.as_str());
    assert_eq!(new["created_at"], profile["created_at"]);
    assert!(new["updated_at"].as_str() > profile["updated_at"].as_str());
    assert_eq!(new["data"]["vfos"].as_array().map(Vec::len), Some(2));
    assert_eq!(header(&replaced, "location"), None);
    let list = stack.rest("GET", "/api/v1/profiles".into(), None).await;
    assert_eq!(list.body["profiles"].as_array().map(Vec::len), Some(1));

    // With nothing to overwrite the flag simply creates.
    let created = save(json!({"name": "Fresh", "overwrite": true})).await;
    assert_eq!(created.status, 201, "{:?}", created.body);
    assert!(header(&created, "location").is_some());

    // Validation still applies.
    let bad = save(json!({"name": "  ", "overwrite": true})).await;
    assert_eq!(bad.status, 400);
    let junk = save(json!({"name": "x", "overwrite": "yes"})).await;
    assert_eq!(junk.status, 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn lock_and_free_tuning_over_rest() {
    let stack = stack_or_skip!("lk");

    // Free tuning: a VFO far outside the window makes the centre follow.
    let created = stack.vfo(CENTER + 3_000_000).await;
    assert_eq!(created.status, 201, "{:?}", created.body);
    assert_eq!(created.body["retune"], "applied");
    assert_eq!(created.body["vfo"]["status"], "active");
    let center = created.body["state"]["center_hz"].as_u64().expect("center");
    assert_eq!(center, CENTER + 3_000_000 - 217_600); // quarter of 85 % of 1.024 MS/s
    let id = created.body["vfo"]["id"].clone();

    // Lock: tuning is refused with the holders, VFOs still move inside the window.
    let lock = stack.rest("POST", stack.dev("lock"), None).await;
    assert_eq!(lock.status, 200, "{:?}", lock.body);
    assert_eq!(lock.body["state"]["locks"][0]["kind"], "user");
    let again = stack.rest("POST", stack.dev("lock"), None).await;
    assert_eq!(
        again.body["state"]["locks"].as_array().map(Vec::len),
        Some(1)
    );

    let refused = stack.tune(json!({"center_hz": 100_000_000u64})).await;
    assert_eq!(refused.status, 409);
    assert_eq!(refused.body["code"], "locked");
    assert_eq!(refused.body["holders"][0]["kind"], "user");
    assert_eq!(refused.body["holders"][0]["id"], "u:1");
    assert_eq!(refused.body["holders"][0]["name"], "admin");
    let repeated = stack.tune(json!({"center_hz": center})).await;
    assert_eq!(
        repeated.status, 200,
        "repeating the current centre changes nothing"
    );

    let patch = stack
        .rest(
            "PATCH",
            stack.dev(&format!("vfos/{id}")),
            Some(json!({"freq_hz": center + 50_000})),
        )
        .await;
    assert_eq!(
        (patch.status, patch.body["retune"].as_str()),
        (200, Some("none"))
    );
    let away = stack
        .rest(
            "PATCH",
            stack.dev(&format!("vfos/{id}")),
            Some(json!({"freq_hz": center + 5_000_000})),
        )
        .await;
    assert_eq!(
        (away.status, away.body["retune"].as_str()),
        (200, Some("locked"))
    );
    assert_eq!(away.body["vfo"]["status"], "paused");
    assert_eq!(away.body["state"]["center_hz"].as_u64(), Some(center));

    // A profile that moves the tuning is refused, too.
    let data = json!({"v": 1, "center_hz": 100_000_000u64, "sample_rate": RATE});
    let profile = stack
        .rest(
            "POST",
            "/api/v1/profiles".into(),
            Some(json!({"name": "Elsewhere", "data": data})),
        )
        .await;
    let profile_id = profile.body["profile"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let refused = stack
        .rest(
            "POST",
            stack.dev("profile/load"),
            Some(json!({"id": profile_id})),
        )
        .await;
    assert_eq!(
        (refused.status, refused.body["code"].as_str()),
        (409, Some("locked"))
    );

    // Release: tuning works again, and DELETE is idempotent.
    let released = stack.rest("DELETE", stack.dev("lock"), None).await;
    assert_eq!(released.status, 200);
    assert_eq!(
        released.body["state"]["locks"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        stack.rest("DELETE", stack.dev("lock"), None).await.status,
        200
    );
    assert_eq!(
        stack
            .tune(json!({"center_hz": 100_000_000u64}))
            .await
            .status,
        200
    );

    // Loading a profile with the lock flag takes the lock; foreign origins cannot touch it.
    let mut locked_data = data.clone();
    locked_data["lock"] = json!(true);
    let profile = stack
        .rest(
            "POST",
            "/api/v1/profiles".into(),
            Some(json!({"name": "Locked", "data": locked_data})),
        )
        .await;
    let locked_id = profile.body["profile"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let loaded = stack
        .rest(
            "POST",
            stack.dev("profile/load"),
            Some(json!({"id": locked_id})),
        )
        .await;
    assert_eq!(loaded.status, 200, "{:?}", loaded.body);
    assert_eq!(loaded.body["state"]["locks"][0]["kind"], "user");

    let foreign = http_async(
        stack.api_port,
        "POST",
        stack.dev("lock"),
        vec![
            ("Cookie".into(), stack.cookie.clone()),
            ("Origin".into(), "https://evil.example".into()),
        ],
        String::new(),
    )
    .await;
    assert_eq!(foreign.status, 403);
}

/// Next JSON text frame from the state feed.
async fn next_state_frame<S>(socket: &mut S) -> Value
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("state frame in time")
            .expect("open")
            .expect("frame");
        if let Message::Text(text) = message {
            return serde_json::from_str(text.as_str()).expect("json");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn state_feed_pushes_every_change() {
    let stack = stack_or_skip!("sw");
    let url = format!(
        "ws://127.0.0.1:{}/api/v1/ws/state?dev={}",
        stack.api_port, stack.device
    );

    // No session, no feed.
    assert!(
        common::ws_connect_with_cookie(&url, "session=nope")
            .await
            .is_err()
    );
    let mut socket = common::ws_connect_with_cookie(&url, &stack.cookie)
        .await
        .expect("connect");

    // The current state comes first.
    let first = next_state_frame(&mut socket).await;
    assert_eq!(
        (first["v"].clone(), first["type"].clone()),
        (1.into(), "state".into())
    );
    assert_eq!(first["state"]["center_hz"], CENTER);
    assert_eq!(first["state"]["vfos"].as_array().map(Vec::len), Some(0));

    // Every kind of change arrives without polling.
    let checks: [fn(&Value) -> bool; 4] = [
        |s| s["vfos"].as_array().is_some_and(|v| v.len() == 1),
        |s| s["locks"].as_array().is_some_and(|l| l.len() == 1),
        |s| s["locks"].as_array().is_some_and(Vec::is_empty),
        |s| s["center_hz"] == 100_000_000u64,
    ];
    let actions = [
        stack.vfo(CENTER + 10_000).await,
        stack.rest("POST", stack.dev("lock"), None).await,
        stack.rest("DELETE", stack.dev("lock"), None).await,
        stack.tune(json!({"center_hz": 100_000_000u64})).await,
    ];
    for reply in &actions {
        assert!(reply.status < 300, "{:?}", reply.body);
    }
    for (index, check) in checks.iter().enumerate() {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let frame = next_state_frame(&mut socket).await;
            if check(&frame["state"]) {
                break;
            }
            assert!(Instant::now() < deadline, "change {index} never arrived");
        }
    }

    // An unknown device has no state yet, but the feed stays open and quiet.
    let idle_url = format!(
        "ws://127.0.0.1:{}/api/v1/ws/state?dev=nobody",
        stack.api_port
    );
    let mut idle = common::ws_connect_with_cookie(&idle_url, &stack.cookie)
        .await
        .expect("connect");
    assert!(
        tokio::time::timeout(Duration::from_millis(500), idle.next())
            .await
            .is_err()
    );
    // Invalid device ids are refused before the upgrade.
    assert!(
        common::ws_connect_with_cookie(
            &format!("ws://127.0.0.1:{}/api/v1/ws/state?dev=a.b", stack.api_port),
            &stack.cookie
        )
        .await
        .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sdrd_restores_its_state_after_a_restart() {
    let mut stack = stack_or_skip!("rs");

    // Build a state worth keeping: tuning, two VFOs (ids 3 and 4), the user lock.
    assert_eq!(
        stack
            .tune(json!({"center_hz": 433_700_000u64}))
            .await
            .status,
        200
    );
    assert_eq!(stack.vfo(433_750_000).await.status, 201);
    assert_eq!(stack.vfo(433_800_000).await.status, 201);
    assert_eq!(stack.vfo(433_850_000).await.status, 201);
    assert_eq!(
        stack.rest("DELETE", stack.dev("vfos/2"), None).await.status,
        204
    );
    assert_eq!(
        stack.rest("POST", stack.dev("lock"), None).await.status,
        200
    );
    let before = stack
        .wait_state("settled state", |s| {
            s["vfos"].as_array().is_some_and(|v| v.len() == 2)
                && s["locks"].as_array().is_some_and(|l| l.len() == 1)
        })
        .await;

    // Wait until the snapshot in KV has caught up, then kill the daemon without ceremony.
    let store = async_nats::jetstream::new(stack.nats.clone())
        .get_key_value("STATE")
        .await
        .expect("bucket");
    let key = format!("restore.{}", stack.device);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(saved) = store.get(&key).await.expect("kv get") {
            let saved: Value = serde_json::from_slice(&saved).expect("json");
            if saved["locks"].as_array().is_some_and(|l| l.len() == 1)
                && saved["vfos"] == without_mine(&before["vfos"])
            {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "restore snapshot never caught up"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(stack.sdrd.take());

    // A fresh daemon starts from the file's own centre ... and comes back with the old state.
    let (sdrd, port) = spawn_sdrd(&stack.nats_url, &stack.device, &stack.tone);
    stack.sdrd = Some(sdrd);
    wait_ready(port, "restarted sdrd").await;
    let after = stack
        .wait_state("restored state", |s| s["center_hz"] == 433_700_000u64)
        .await;
    assert_eq!(after["locks"], before["locks"]);
    assert_eq!(after["vfos"], before["vfos"]);
    let ids: Vec<u64> = after["vfos"]
        .as_array()
        .expect("vfos")
        .iter()
        .map(|v| v["id"].as_u64().expect("id"))
        .collect();
    assert_eq!(ids, [3, 4]);
    // Locked after the restart, too, and new VFOs continue after the highest id.
    let refused = stack.tune(json!({"center_hz": 100_000_000u64})).await;
    assert_eq!(refused.status, 409);
    let created = stack.vfo(433_720_000).await;
    assert_eq!(created.body["vfo"]["id"], 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn loading_a_profile_twice_does_not_stack_vfos() {
    let stack = stack_or_skip!("lt");
    assert_eq!(stack.vfo(433_950_000).await.status, 201);
    assert_eq!(stack.vfo(433_980_000).await.status, 201);
    let saved = stack
        .rest(
            "POST",
            stack.dev("profile/save"),
            Some(json!({"name": "twice"})),
        )
        .await;
    assert_eq!(saved.status, 201, "{:?}", saved.body);
    let id = saved.body["profile"]["id"].clone();
    let saved_state = stack.state().await;

    // Strays the profile does not know about.
    assert_eq!(stack.vfo(433_900_000).await.status, 201);
    assert_eq!(stack.vfo(433_910_000).await.status, 201);
    assert_eq!(stack.vfo(433_920_000).await.status, 201);

    for round in 0..3 {
        let loaded = stack
            .rest("POST", stack.dev("profile/load"), Some(json!({"id": id})))
            .await;
        assert_eq!(loaded.status, 200, "{:?}", loaded.body);
        assert_eq!(
            loaded.body["state"]["vfos"].as_array().map(Vec::len),
            Some(2),
            "round {round}"
        );
        assert_eq!(
            essence(&loaded.body["state"]),
            essence(&saved_state),
            "round {round}"
        );
    }
    let after = stack
        .wait_state("replaced VFO set", |s| essence(s) == essence(&saved_state))
        .await;
    assert_eq!(after["vfos"].as_array().map(Vec::len), Some(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_device_has_one_default_vfo_that_stays_deleted() {
    let mut stack = fresh_stack_or_skip!("dv");

    // The file device sits at 433.92 MHz: NFM at the centre, nothing was created by anyone.
    let state = stack
        .wait_state("default vfo", |s| {
            s["vfos"].as_array().is_some_and(|v| v.len() == 1)
        })
        .await;
    let vfo = &state["vfos"][0];
    assert_eq!(vfo["id"], 1);
    assert_eq!(vfo["freq_hz"], CENTER);
    assert_eq!(vfo["mode"], "nfm");
    assert_eq!(vfo["status"], "active");

    // Delete it, and let the snapshot catch up before the daemon is killed.
    assert_eq!(
        stack.rest("DELETE", stack.dev("vfos/1"), None).await.status,
        204
    );
    let store = async_nats::jetstream::new(stack.nats.clone())
        .get_key_value("STATE")
        .await
        .expect("bucket");
    let key = format!("restore.{}", stack.device);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(saved) = store.get(&key).await.expect("kv get") {
            let saved: Value = serde_json::from_slice(&saved).expect("json");
            if saved["vfos"] == json!([]) {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "restore snapshot never caught up"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(stack.sdrd.take());

    // After a restart the user's empty set is respected: no new default VFO.
    let (sdrd, port) = spawn_sdrd(&stack.nats_url, &stack.device, &stack.tone);
    stack.sdrd = Some(sdrd);
    wait_ready(port, "restarted sdrd").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(stack.state().await["vfos"], json!([]));
}
