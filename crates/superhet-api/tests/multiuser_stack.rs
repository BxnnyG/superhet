//! Multi-user behaviour against the real stack: `superhet-api` + `superhet-sdrd` (file device)
//! on a real NATS server and PostgreSQL. Skipped unless `SUPERHET_TEST_NATS_URL` and
//! `DATABASE_URL` are set. The sdrd binary must have been built (`cargo build -p superhet-sdrd`).

mod common;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

const RATE: u32 = 1_024_000;
const CENTER: u64 = 433_920_000;

struct Stack {
    _sdrd: Proc,
    _api: Proc,
    port: u16,
    device: String,
    tone: std::path::PathBuf,
    admin: String,
    _db: TestDb,
}

impl Drop for Stack {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.tone);
    }
}

async fn start_stack(tag: &str, sdrd_env: &[(&str, &str)]) -> Option<Stack> {
    let url = std::env::var("SUPERHET_TEST_NATS_URL").ok()?;
    let db = TestDb::create(tag).await?;
    let pid = std::process::id();
    let device = format!("{tag}{pid}");
    let tone = std::env::temp_dir().join(format!("superhet-mu-{tag}-{pid}.cu8"));
    let bytes: Vec<u8> = (0..8 * 4096)
        .flat_map(|i| {
            let ph = std::f64::consts::TAU * 50_000.0 * f64::from(i) / f64::from(RATE);
            [
                (127.5 + 127.0 * ph.cos()).round() as u8,
                (127.5 + 127.0 * ph.sin()).round() as u8,
            ]
        })
        .collect();
    std::fs::write(&tone, bytes).expect("fixture");

    let api_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_superhet-api"));
    let sdrd_bin = api_bin.with_file_name("superhet-sdrd");
    assert!(
        sdrd_bin.exists(),
        "{} not found: run `cargo build -p superhet-sdrd`",
        sdrd_bin.display()
    );
    let creds = match (
        std::env::var("SUPERHET_TEST_NATS_USER"),
        std::env::var("SUPERHET_TEST_NATS_PASSWORD"),
    ) {
        (Ok(user), Ok(password)) => Some((user, password)),
        _ => None,
    };
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
    for (k, v) in sdrd_env {
        sdrd.env(k, v);
    }
    let sdrd = Proc(
        sdrd.env("SUPERHET_LISTEN", format!("127.0.0.1:{sdrd_port}"))
            .env("SUPERHET_NATS_URL", &url)
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
            .env("SUPERHET_NATS_URL", &url)
            .env("SUPERHET_DATABASE_URL", &db.url)
            .env("SUPERHET_SETUP_TOKEN", SETUP_TOKEN)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn api"),
    );
    wait_ready(sdrd_port, "sdrd").await;
    wait_ready(api_port, "api").await;
    let admin = setup_admin(api_port);
    Some(Stack {
        _sdrd: sdrd,
        _api: api,
        port: api_port,
        device,
        tone,
        admin,
        _db: db,
    })
}

macro_rules! stack {
    ($tag:expr) => {
        stack!($tag, &[])
    };
    ($tag:expr, $env:expr) => {{
        match start_stack($tag, $env).await {
            Some(s) => s,
            None => {
                eprintln!("SUPERHET_TEST_NATS_URL / DATABASE_URL not set; skipping");
                return;
            }
        }
    }};
}

impl Stack {
    fn dev(&self, rest: &str) -> String {
        format!("/api/v1/devices/{}/{rest}", self.device)
    }

    fn get(&self, cookie: &str, rest: &str) -> Reply {
        req(self.port, "GET", &self.dev(rest), Some(cookie), None)
    }

    fn state(&self, cookie: &str) -> Value {
        let r = self.get(cookie, "state");
        assert_eq!(r.status, 200, "{r:?}");
        r.json()
    }

    fn create(&self, cookie: &str, freq: u64, mode: &str) -> Reply {
        req(
            self.port,
            "POST",
            &self.dev("vfos"),
            Some(cookie),
            Some(&json!({"freq_hz": freq, "mode": mode})),
        )
    }

    fn patch(&self, cookie: &str, id: u64, body: &Value) -> Reply {
        req(
            self.port,
            "PATCH",
            &self.dev(&format!("vfos/{id}")),
            Some(cookie),
            Some(body),
        )
    }

    fn delete(&self, cookie: &str, id: u64) -> Reply {
        req(
            self.port,
            "DELETE",
            &self.dev(&format!("vfos/{id}")),
            Some(cookie),
            None,
        )
    }

    fn tune(&self, cookie: &str, center: u64) -> Reply {
        req(
            self.port,
            "POST",
            &self.dev("tune"),
            Some(cookie),
            Some(&json!({"center_hz": center})),
        )
    }

    fn user_id(&self, cookie: &str) -> i64 {
        req(self.port, "GET", "/api/v1/auth/status", Some(cookie), None).json()["user_id"]
            .as_i64()
            .expect("user id")
    }

    async fn ws(&self, kind: &str, cookie: &str) -> Ws {
        let url = format!(
            "ws://127.0.0.1:{}/api/v1/ws/{kind}?dev={}",
            self.port, self.device
        );
        ws_connect_with_cookie(&url, cookie)
            .await
            .expect("ws connect")
    }

    async fn wait_vfo_count(&self, cookie: &str, wanted: usize, within: Duration) {
        let deadline = Instant::now() + within;
        loop {
            let n = self.state(cookie)["vfos"].as_array().map_or(0, Vec::len);
            if n == wanted {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected {wanted} VFOs, have {n}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_text(ws: &mut Ws) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                return serde_json::from_str(t.as_str()).expect("json");
            }
            Ok(Some(Ok(_))) => {}
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

async fn subscribe(ws: &mut Ws, vfos: &[u64]) -> Value {
    ws.send(Message::text(
        json!({"type": "subscribe", "vfos": vfos}).to_string(),
    ))
    .await
    .expect("send");
    next_text(ws).await
}

/// True once the socket reports the end of the stream (closed or reset) within `within`.
async fn closes_within(ws: &mut Ws, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Ok(None | Some(Err(_) | Ok(Message::Close(_)))) =
            tokio::time::timeout(Duration::from_millis(200), ws.next()).await
        {
            return true;
        }
    }
    false
}

fn vfos_of(state: &Value) -> Vec<Value> {
    state["vfos"].as_array().cloned().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn listeners_own_their_vfos_and_cannot_retune() {
    let s = stack!("own");
    let lars = make_user(s.port, &s.admin, "lars", "listener");
    let lena = make_user(s.port, &s.admin, "lena", "listener");
    let olga = make_user(s.port, &s.admin, "olga", "operator");
    let (lars_id, lena_id) = (s.user_id(&lars), s.user_id(&lena));

    // The default VFO is shared: nobody owns it, nobody sees an owner.
    let before = s.state(&lars);
    assert_eq!(vfos_of(&before).len(), 1);
    assert_eq!(vfos_of(&before)[0]["mine"], false);
    assert!(vfos_of(&before)[0].get("owner").is_none());

    // Lars creates a VFO; a client-supplied owner is ignored.
    let created = req(
        s.port,
        "POST",
        &s.dev("vfos"),
        Some(&lars),
        Some(&json!({"freq_hz": CENTER + 30_000, "mode": "nfm", "owner": "u:1"})),
    );
    assert_eq!(created.status, 201, "{created:?}");
    let lars_vfo = created.json()["vfo"]["id"].as_u64().expect("id");
    let lena_vfo = s.create(&lena, CENTER + 60_000, "nfm").json()["vfo"]["id"]
        .as_u64()
        .expect("id");

    let as_lars = s.state(&lars);
    let mine: Vec<(u64, bool)> = vfos_of(&as_lars)
        .iter()
        .map(|v| (v["id"].as_u64().expect("id"), v["mine"] == true))
        .collect();
    assert_eq!(mine, [(1, false), (lars_vfo, true), (lena_vfo, false)]);
    assert!(
        vfos_of(&as_lars).iter().all(|v| v.get("owner").is_none()),
        "listeners never see owners"
    );
    let as_olga = s.state(&olga);
    let owners: Vec<Option<String>> = vfos_of(&as_olga)
        .iter()
        .map(|v| v["owner"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        owners,
        [
            None,
            Some(format!("u:{lars_id}")),
            Some(format!("u:{lena_id}"))
        ]
    );

    // Not yours, not modifiable: another listener's VFO and the shared one alike.
    for id in [lars_vfo, 1] {
        let r = s.patch(&lena, id, &json!({"freq_hz": CENTER + 10_000}));
        assert_eq!(
            (r.status, r.json()["code"].clone()),
            (403, json!("not_owner")),
            "patch {id}"
        );
        let r = s.delete(&lena, id);
        assert_eq!(
            (r.status, r.json()["code"].clone()),
            (403, json!("not_owner")),
            "delete {id}"
        );
    }
    assert_eq!(s.patch(&lena, 999, &json!({"freq_hz": CENTER})).status, 404);
    assert_eq!(
        s.patch(&lars, lars_vfo, &json!({"freq_hz": CENTER + 31_000}))
            .status,
        200
    );
    // Operators and admins may change anybody's.
    assert_eq!(
        s.patch(&olga, lars_vfo, &json!({"freq_hz": CENTER + 32_000}))
            .status,
        200
    );
    assert_eq!(s.delete(&olga, lena_vfo).status, 204);
    assert_eq!(s.delete(&s.admin, lars_vfo).status, 204);

    // A listener's centre / rate / gain / lock requests are refused and change nothing.
    let epoch = before["tuning_epoch"].clone();
    for body in [
        json!({"center_hz": CENTER + 100_000}),
        json!({"sample_rate": 2_048_000}),
        json!({"gain": "auto"}),
        json!({"ppm": 3}),
    ] {
        let r = req(s.port, "POST", &s.dev("tune"), Some(&lars), Some(&body));
        assert_eq!(r.status, 403, "{body}");
    }
    assert_eq!(
        req(s.port, "POST", &s.dev("lock"), Some(&lars), None).status,
        403
    );
    assert_eq!(
        req(s.port, "DELETE", &s.dev("lock"), Some(&lars), None).status,
        403
    );
    let after = s.state(&s.admin);
    assert_eq!(
        (
            after["center_hz"].clone(),
            after["tuning_epoch"].clone(),
            after["sample_rate"].clone()
        ),
        (
            before["center_hz"].clone(),
            epoch,
            before["sample_rate"].clone()
        )
    );
    assert_eq!(after["locks"], json!([]));

    // Audio: own and shared VFOs yes, other people's and unknown ones no.
    let a = s.create(&lars, CENTER + 20_000, "nfm").json()["vfo"]["id"]
        .as_u64()
        .expect("id");
    let b = s.create(&lena, CENTER + 40_000, "nfm").json()["vfo"]["id"]
        .as_u64()
        .expect("id");
    let mut ws = s.ws("audio", &lars).await;
    assert_eq!(subscribe(&mut ws, &[a]).await["type"], "ack");
    assert_eq!(
        subscribe(&mut ws, &[a, 1]).await["type"],
        "ack",
        "shared VFOs are fine"
    );
    assert_eq!(
        subscribe(&mut ws, &[b]).await,
        json!({"v": 1, "type": "error", "error": "forbidden"})
    );
    assert_eq!(subscribe(&mut ws, &[a, b]).await["error"], "forbidden");
    assert_eq!(subscribe(&mut ws, &[9999]).await["error"], "forbidden");
    assert_eq!(subscribe(&mut ws, &[]).await["type"], "ack");
    let mut op_ws = s.ws("audio", &olga).await;
    assert_eq!(
        subscribe(&mut op_ws, &[a, b]).await["type"],
        "ack",
        "operators hear everything"
    );

    // Per-role quota from the admin settings: lars owns one VFO (a) -> room for one more.
    assert_eq!(
        put(
            s.port,
            &s.admin,
            "/api/v1/admin/limits",
            &json!({"listener_max_vfos": 2, "operator_max_vfos": 0})
        )
        .status,
        200
    );
    assert_eq!(s.create(&lars, CENTER + 21_000, "nfm").status, 201);
    let over = s.create(&lars, CENTER + 22_000, "nfm");
    assert_eq!(
        (over.status, over.json()["code"].clone()),
        (403, json!("vfo_limit"))
    );
    // ... counted per user, not per server.
    assert_eq!(s.create(&lena, CENTER + 41_000, "nfm").status, 201);
    assert_eq!(
        put(
            s.port,
            &s.admin,
            "/api/v1/admin/limits",
            &json!({"listener_max_vfos": 3})
        )
        .status,
        200
    );
    assert_eq!(s.create(&lars, CENTER + 22_000, "nfm").status, 201);
    // Operators have no quota by default.
    for i in 0..2 {
        assert_eq!(
            s.create(&olga, CENTER + 50_000 + i * 1000, "am").status,
            201
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_cap_is_configurable() {
    let s = stack!("cap", &[("SUPERHET_MAX_VFOS", "3")]);
    // The default VFO plus two more fill the daemon.
    assert_eq!(s.create(&s.admin, CENTER + 10_000, "nfm").status, 201);
    assert_eq!(s.create(&s.admin, CENTER + 20_000, "nfm").status, 201);
    let full = s.create(&s.admin, CENTER + 30_000, "nfm");
    assert_eq!(full.status, 400, "{full:?}");
    assert!(
        full.json()["error"]
            .as_str()
            .is_some_and(|e| e.contains("at most 3")),
        "{full:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn guests_are_held_to_the_policy_and_leave_nothing_behind() {
    let s = stack!("guest");
    let olga = make_user(s.port, &s.admin, "olga", "operator");
    enable_guests(
        s.port,
        &s.admin,
        json!({
            "allowed_modes": ["nfm", "am"],
            "freq_min_hz": CENTER - 100_000,
            "freq_max_hz": CENTER + 100_000,
            "max_vfos_per_guest": 1,
        }),
    );
    let guest = guest_session(s.port).expect("guest");

    // Allowed: an NFM channel inside the range.
    let ok = s.create(&guest, CENTER + 10_000, "nfm");
    assert_eq!(ok.status, 201, "{ok:?}");
    let id = ok.json()["vfo"]["id"].as_u64().expect("id");
    // Refused: second VFO, other mode, other frequency.
    let second = s.create(&guest, CENTER + 11_000, "nfm");
    assert_eq!(
        (second.status, second.json()["code"].clone()),
        (403, json!("vfo_limit"))
    );
    let usb = s.create(&guest, CENTER + 12_000, "usb");
    assert_eq!(
        (usb.status, usb.json()["code"].clone()),
        (403, json!("mode_not_allowed"))
    );
    let far = s.create(&guest, CENTER + 200_000, "nfm");
    assert_eq!(
        (far.status, far.json()["code"].clone()),
        (403, json!("frequency_not_allowed"))
    );
    // Changing one's own VFO is bound to the same rules.
    assert_eq!(
        s.patch(&guest, id, &json!({"mode": "wfm"})).json()["code"],
        "mode_not_allowed"
    );
    assert_eq!(
        s.patch(&guest, id, &json!({"freq_hz": CENTER + 150_000}))
            .json()["code"],
        "frequency_not_allowed"
    );
    assert_eq!(
        s.patch(
            &guest,
            id,
            &json!({"mode": "am", "freq_hz": CENTER + 20_000})
        )
        .status,
        200
    );
    // Not tuning, not the shared channel.
    assert_eq!(s.tune(&guest, CENTER + 50_000).status, 403);
    assert_eq!(s.patch(&guest, 1, &json!({"freq_hz": CENTER})).status, 403);

    // A guest sees no names, owners or lock holders.
    assert_eq!(
        req(s.port, "POST", &s.dev("lock"), Some(&olga), None).status,
        200
    );
    let view = s.state(&guest);
    assert!(vfos_of(&view).iter().all(|v| v.get("owner").is_none()));
    assert_eq!(
        vfos_of(&view).iter().filter(|v| v["mine"] == true).count(),
        1
    );
    assert_eq!(view["locks"][0]["kind"], "user");
    assert!(view["locks"][0].get("name").is_none() && view["locks"][0].get("id").is_none());
    let as_listener = s.state(&olga);
    assert_eq!(as_listener["locks"][0]["name"], "olga");

    // "Guests only without a lock": while the lock is held no new guest VFO, afterwards yes.
    assert_eq!(s.delete(&guest, id).status, 204);
    enable_guests(
        s.port,
        &s.admin,
        json!({"only_without_lock": true, "max_vfos_per_guest": 2, "allowed_modes": ["nfm", "am"]}),
    );
    let locked = s.create(&guest, CENTER + 10_000, "nfm");
    assert_eq!(
        (locked.status, locked.json()["code"].clone()),
        (403, json!("locked"))
    );
    assert_eq!(
        req(s.port, "DELETE", &s.dev("lock"), Some(&olga), None).status,
        200
    );
    let again = s.create(&guest, CENTER + 10_000, "nfm");
    assert_eq!(again.status, 201, "{again:?}");

    // Audio of other people's channels is closed to guests as well.
    let other = s.create(&olga, CENTER + 70_000, "nfm").json()["vfo"]["id"]
        .as_u64()
        .expect("id");
    let mut ws = s.ws("audio", &guest).await;
    assert_eq!(subscribe(&mut ws, &[other]).await["error"], "forbidden");

    // Leaving takes the guest's channels along (within the maintenance interval).
    let before = vfos_of(&s.state(&s.admin)).len();
    assert_eq!(
        req(s.port, "POST", "/api/v1/auth/logout", Some(&guest), None).status,
        204
    );
    s.wait_vfo_count(&s.admin, before - 1, Duration::from_secs(8))
        .await;
    assert!(vfos_of(&s.state(&s.admin)).iter().all(|v| {
        v.get("owner")
            .and_then(Value::as_str)
            .is_none_or(|o| !o.starts_with("g:"))
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lock_names_its_holder_and_only_admins_override_it() {
    let s = stack!("lock");
    let olga = make_user(s.port, &s.admin, "olga", "operator");
    let omar = make_user(s.port, &s.admin, "omar", "operator");
    let lars = make_user(s.port, &s.admin, "lars", "listener");
    let olga_id = s.user_id(&olga);

    assert_eq!(
        req(s.port, "POST", &s.dev("lock"), Some(&olga), None).status,
        200
    );
    let refused = s.tune(&omar, CENTER + 200_000);
    assert_eq!(refused.status, 409, "{refused:?}");
    let body = refused.json();
    assert_eq!(body["code"], "locked");
    assert_eq!(body["holders"][0]["name"], "olga");
    assert_eq!(body["holders"][0]["id"], format!("u:{olga_id}"));
    assert!(
        body["error"].as_str().is_some_and(|m| m.contains("olga")),
        "{body}"
    );
    // The refused request changed nothing.
    assert_eq!(s.state(&s.admin)["center_hz"], CENTER);
    // Listeners see who holds it too (accounts only).
    assert_eq!(s.state(&lars)["locks"][0]["name"], "olga");

    // Holders are per account: omar's unlock does not release olga's lock.
    assert_eq!(
        req(s.port, "POST", &s.dev("lock"), Some(&omar), None).status,
        200
    );
    assert_eq!(
        req(s.port, "DELETE", &s.dev("lock"), Some(&omar), None).status,
        200
    );
    assert_eq!(s.state(&s.admin)["locks"].as_array().map(Vec::len), Some(1));
    // `force` is for admins.
    for who in [&omar, &olga, &lars] {
        assert_eq!(
            req(
                s.port,
                "DELETE",
                &format!("{}?force=true", s.dev("lock")),
                Some(who),
                None
            )
            .status,
            403
        );
    }
    let forced = req(
        s.port,
        "DELETE",
        &format!("{}?force=true", s.dev("lock")),
        Some(&s.admin),
        None,
    );
    assert_eq!(forced.status, 200, "{forced:?}");
    assert_eq!(s.state(&s.admin)["locks"], json!([]));
    assert_eq!(s.tune(&omar, CENTER + 200_000).status, 200);
    // The audit trail tells who locked and who released.
    let locks = audit(s.port, &s.admin, "action=device.lock");
    assert_eq!(locks.len(), 2);
    assert_eq!(locks[1]["actor_name"], "olga");
    let unlocks = audit(s.port, &s.admin, "action=device.unlock");
    assert_eq!(unlocks.len(), 2);
    assert_eq!(unlocks[0]["details"]["force"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn kicking_closes_the_sockets_within_two_seconds() {
    let s = stack!("kick");
    let lars = make_user(s.port, &s.admin, "lars", "listener");
    enable_guests(s.port, &s.admin, json!({}));
    let guest = guest_session(s.port).expect("guest");
    let guest_vfo = s.create(&guest, CENTER + 10_000, "nfm");
    assert_eq!(guest_vfo.status, 201);
    let lars_vfo = s.create(&lars, CENTER + 20_000, "nfm").json()["vfo"]["id"]
        .as_u64()
        .expect("id");

    let mut lars_state = s.ws("state", &lars).await;
    let mut lars_audio = s.ws("audio", &lars).await;
    let mut guest_spectrum = s.ws("spectrum", &guest).await;
    assert_eq!(next_text(&mut lars_state).await["type"], "state");

    // The admin page sees both, with their connections and channels.
    let sessions = req(
        s.port,
        "GET",
        "/api/v1/admin/sessions",
        Some(&s.admin),
        None,
    )
    .json();
    let list = sessions["sessions"].as_array().expect("sessions");
    let lars_row = list.iter().find(|r| r["username"] == "lars").expect("lars");
    let kinds: Vec<&str> = lars_row["connections"]
        .as_array()
        .expect("connections")
        .iter()
        .filter_map(|c| c["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["state", "audio"]);
    assert_eq!(lars_row["vfos"][0]["id"], lars_vfo);
    assert_eq!(lars_row["vfos"][0]["freq_hz"], CENTER + 20_000);
    assert_eq!(lars_row["vfos"][0]["mode"], "nfm");
    assert_eq!(lars_row["user_agent"], UA);
    let guest_row = list.iter().find(|r| r["kind"] == "guest").expect("guest");
    assert_eq!(guest_row["connections"][0]["kind"], "spectrum");
    assert_eq!(guest_row["vfos"][0]["mode"], "nfm");
    assert!(
        list.iter()
            .find(|r| r["username"] == "admin")
            .is_some_and(|r| r["connections"] == json!([]))
    );

    // The connect events are in the log, once per session (not once per socket).
    let connects = audit(s.port, &s.admin, "action=session.connect&actor=lars");
    assert_eq!(connects.len(), 1, "{connects:?}");

    let started = Instant::now();
    let lars_sid = lars_row["sid"].as_str().expect("sid");
    assert_eq!(
        req(
            s.port,
            "DELETE",
            &format!("/api/v1/admin/sessions/{lars_sid}"),
            Some(&s.admin),
            None
        )
        .status,
        204
    );
    assert!(
        closes_within(&mut lars_state, Duration::from_secs(2)).await,
        "state socket still open"
    );
    assert!(
        closes_within(&mut lars_audio, Duration::from_secs(2)).await,
        "audio socket still open"
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(
        req(s.port, "GET", "/api/v1/auth/status", Some(&lars), None).json()["authenticated"],
        false
    );
    assert_eq!(s.get(&lars, "state").status, 401);

    let guest_sid = guest_row["sid"].as_str().expect("sid");
    assert_eq!(
        req(
            s.port,
            "DELETE",
            &format!("/api/v1/admin/sessions/{guest_sid}"),
            Some(&s.admin),
            None
        )
        .status,
        204
    );
    assert!(
        closes_within(&mut guest_spectrum, Duration::from_secs(2)).await,
        "guest socket still open"
    );
    // A kicked guest's channel is cleaned up; the user's stays (accounts keep their VFOs).
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let vfos = vfos_of(&s.state(&s.admin));
        if vfos
            .iter()
            .all(|v| v["owner"].as_str().is_none_or(|o| !o.starts_with("g:")))
        {
            assert!(vfos.iter().any(|v| v["id"] == lars_vfo));
            break;
        }
        assert!(Instant::now() < deadline, "guest VFO still there");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Disconnects are logged with their duration.
    let deadline = Instant::now() + Duration::from_secs(4);
    let disconnects = loop {
        let rows = audit(s.port, &s.admin, "action=session.disconnect&actor=lars");
        if !rows.is_empty() || Instant::now() > deadline {
            break rows;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(disconnects.len(), 1);
    assert!(disconnects[0]["details"]["duration_secs"].is_u64());
    // Kicking something that is gone is a 404; ids are validated.
    assert_eq!(
        req(
            s.port,
            "DELETE",
            &format!("/api/v1/admin/sessions/{lars_sid}"),
            Some(&s.admin),
            None
        )
        .status,
        404
    );
    assert_eq!(
        req(
            s.port,
            "DELETE",
            "/api/v1/admin/sessions/nonsense",
            Some(&s.admin),
            None
        )
        .status,
        400
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn device_actions_are_audited_and_drags_are_coalesced() {
    let s = stack!("devaudit");
    let olga = make_user(s.port, &s.admin, "olga", "operator");
    let lars = make_user(s.port, &s.admin, "lars", "listener");

    // A tuning drag: many requests, one row with the final value.
    for i in 1..=6u64 {
        assert_eq!(s.tune(&olga, CENTER + i * 1000).status, 200);
    }
    // VFO drag by a listener: create, many moves, delete.
    let id = s.create(&lars, CENTER + 10_000, "nfm").json()["vfo"]["id"]
        .as_u64()
        .expect("id");
    for i in 1..=5u64 {
        assert_eq!(
            s.patch(&lars, id, &json!({"freq_hz": CENTER + 10_000 + i * 100}))
                .status,
            200
        );
    }
    assert_eq!(s.delete(&lars, id).status, 204);
    // Denied: a listener tries to tune.
    assert_eq!(s.tune(&lars, CENTER).status, 403);
    // Profile load by an operator.
    let saved = req(
        s.port,
        "POST",
        &s.dev("profile/save"),
        Some(&olga),
        Some(&json!({"name": "audit"})),
    );
    assert_eq!(saved.status, 201, "{saved:?}");
    let load = req(
        s.port,
        "POST",
        &s.dev("profile/load"),
        Some(&olga),
        Some(&json!({"id": saved.json()["profile"]["id"]})),
    );
    assert_eq!(load.status, 200, "{load:?}");

    // Wait out the coalescing window.
    let deadline = Instant::now() + Duration::from_secs(8);
    while audit(s.port, &s.admin, "action=vfo.update").is_empty()
        || audit(s.port, &s.admin, "action=device.tune").is_empty()
    {
        assert!(Instant::now() < deadline, "coalesced rows never arrived");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let tune = one(s.port, &s.admin, "device.tune");
    assert_eq!(
        (
            tune["actor_name"].clone(),
            tune["role"].clone(),
            tune["target"].clone()
        ),
        (json!("olga"), json!("operator"), json!(s.device))
    );
    assert_eq!(
        tune["details"]["center_hz"],
        CENTER + 6000,
        "the final value wins"
    );
    assert_eq!(tune["details"]["merged"], 6);
    let update = one(s.port, &s.admin, "vfo.update");
    assert_eq!(update["actor_name"], "lars");
    assert_eq!(update["target"], format!("{}/{id}", s.device));
    assert_eq!(update["details"]["freq_hz"], CENTER + 10_500);
    assert_eq!(update["details"]["merged"], 5);
    assert_eq!(
        one(s.port, &s.admin, "vfo.create")["details"]["mode"],
        "nfm"
    );
    assert_eq!(
        one(s.port, &s.admin, "vfo.delete")["target"],
        format!("{}/{id}", s.device)
    );
    assert_eq!(one(s.port, &s.admin, "profile.load")["actor_name"], "olga");
    let denied = audit(s.port, &s.admin, "action=access.denied&actor=lars");
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0]["target"], "POST /api/v1/devices/{dev}/tune");
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_guests_expire_and_take_their_channel_along() {
    let s = stack!("idle");
    enable_guests(s.port, &s.admin, json!({"idle_timeout_secs": 30}));
    let guest = guest_session(s.port).expect("guest");
    assert_eq!(s.create(&guest, CENTER + 10_000, "nfm").status, 201);
    s.wait_vfo_count(&s.admin, 2, Duration::from_secs(2)).await;
    // The guest goes quiet. Asking for its state would count as activity, so watch the admin
    // list instead.
    let deadline = Instant::now() + Duration::from_secs(50);
    loop {
        let gone = {
            let list = req(
                s.port,
                "GET",
                "/api/v1/admin/sessions",
                Some(&s.admin),
                None,
            )
            .json();
            !list["sessions"]
                .as_array()
                .expect("sessions")
                .iter()
                .any(|r| {
                    r["kind"] == "guest" && r["vfos"].as_array().is_some_and(|v| !v.is_empty())
                })
        };
        if gone {
            break;
        }
        assert!(Instant::now() < deadline, "idle guest never expired");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert_eq!(
        req(s.port, "GET", "/api/v1/auth/status", Some(&guest), None).json()["authenticated"],
        false
    );
    s.wait_vfo_count(&s.admin, 1, Duration::from_secs(6)).await;
    let ended = audit(s.port, &s.admin, "action=guest.session_end");
    assert!(!ended.is_empty());
    assert_eq!(ended[0]["details"]["reason"], "idle");
}
