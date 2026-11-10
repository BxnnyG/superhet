//! Per-device access (ADR-0023) on the real `superhet-api` binary and a real PostgreSQL (needs
//! `DATABASE_URL`; skipped otherwise). Without a bus the gateways answer 503, so "not 403" on a
//! device route means the request got past the device check. The device list test also needs
//! `SUPERHET_TEST_NATS_URL` (JetStream).

mod common;

use common::*;
use serde_json::{Value, json};

/// The API without a bus, whatever `SUPERHET_NATS_URL` the environment carries.
fn spawn_plain_api(db_url: &str) -> (Proc, u16) {
    spawn_api(db_url, |c| {
        c.env_remove("SUPERHET_NATS_URL");
    })
}

fn user_id(port: u16, admin: &str, name: &str) -> i64 {
    let reply = req(port, "GET", "/api/v1/admin/users", Some(admin), None);
    assert_eq!(reply.status, 200, "{reply:?}");
    let body = reply.json();
    let users = body["users"].as_array().expect("users");
    users
        .iter()
        .find(|u| u["username"] == name)
        .and_then(|u| u["id"].as_i64())
        .unwrap_or_else(|| panic!("no user {name}: {body}"))
}

fn set_policy(port: u16, admin: &str, device: &str, policy: &Value) {
    let reply = put(
        port,
        admin,
        &format!("/api/v1/admin/device-policies/{device}"),
        policy,
    );
    assert_eq!(reply.status, 200, "{device}: {reply:?}");
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Caller {
    Guest,
    Lena,
    Lars,
    Olga,
    Admin,
}

const CALLERS: [Caller; 5] = [
    Caller::Guest,
    Caller::Lena,
    Caller::Lars,
    Caller::Olga,
    Caller::Admin,
];

struct Fixture {
    _db: TestDb,
    _api: Proc,
    port: u16,
    admin: String,
    cookies: Vec<(Caller, String)>,
}

impl Fixture {
    fn cookie(&self, who: Caller) -> &str {
        &self
            .cookies
            .iter()
            .find(|(c, _)| *c == who)
            .expect("caller")
            .1
    }
}

/// Five callers (guest, two listeners, one operator, the admin) and these devices:
///   dev0     no policy (the default: open)
///   ops      view and control from operator up
///   ro       everybody sees it, control from operator up
///   private  only the listener "lena" (and admins)
///   noguest  guests are kept out
async fn fixture(tag: &str) -> Option<Fixture> {
    let db = TestDb::create(tag).await?;
    let (api, port) = spawn_plain_api(&db.url);
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    let olga = make_user(port, &admin, "olga", "operator");
    let lars = make_user(port, &admin, "lars", "listener");
    let lena = make_user(port, &admin, "lena", "listener");
    enable_guests(port, &admin, json!({"show_decoder_events": true}));
    let guest = guest_session(port).expect("guest session");
    let lena_id = user_id(port, &admin, "lena");
    set_policy(
        port,
        &admin,
        "ops",
        &json!({"view_role": "operator", "control_role": "operator"}),
    );
    set_policy(port, &admin, "ro", &json!({"control_role": "operator"}));
    set_policy(
        port,
        &admin,
        "private",
        &json!({"allowed_users": [lena_id]}),
    );
    set_policy(port, &admin, "noguest", &json!({"guests_visible": false}));
    let cookies = vec![
        (Caller::Guest, guest),
        (Caller::Lena, lena),
        (Caller::Lars, lars),
        (Caller::Olga, olga),
        (Caller::Admin, admin.clone()),
    ];
    Some(Fixture {
        _db: db,
        _api: api,
        port,
        admin,
        cookies,
    })
}

/// Whether `who` may see / control `device` under the fixture's policies.
fn expected(who: Caller, device: &str, control: bool) -> bool {
    if who == Caller::Admin {
        return true;
    }
    let operator = who == Caller::Olga;
    let view = match device {
        "ops" => operator,
        "private" => who == Caller::Lena,
        "noguest" => who != Caller::Guest,
        _ => true,
    };
    let control_ok = match device {
        "ops" | "ro" => operator,
        _ => true,
    };
    view && (!control || control_ok)
}

macro_rules! fixture {
    ($tag:expr) => {{
        let Some(f) = fixture($tag).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        f
    }};
}

fn code_of(reply: &Reply) -> String {
    reply.json()["code"].as_str().unwrap_or("").to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_routes_follow_the_policy_per_role_and_device() {
    let f = fixture!("rest");
    let devices = ["dev0", "ops", "ro", "private", "noguest", "unlisted"];
    let mut checked = 0;
    for device in devices {
        for who in CALLERS {
            let cookie = Some(f.cookie(who));
            // View routes: state and the VFO list.
            for path in [
                format!("/api/v1/devices/{device}/state"),
                format!("/api/v1/devices/{device}/vfos"),
            ] {
                let reply = req(f.port, "GET", &path, cookie, None);
                let ctx = format!("GET {path} as {who:?}: {reply:?}");
                if expected(who, device, false) {
                    assert_eq!(reply.status, 503, "{ctx}");
                } else {
                    assert_eq!(reply.status, 403, "{ctx}");
                    assert_eq!(code_of(&reply), "device_forbidden", "{ctx}");
                }
                checked += 1;
            }
            // Opening a VFO needs control (listeners and guests may, where the device lets them).
            let path = format!("/api/v1/devices/{device}/vfos");
            let body = json!({"freq_hz": 100_000_000, "mode": "nfm"});
            let reply = req(f.port, "POST", &path, cookie, Some(&body));
            let ctx = format!("POST {path} as {who:?}: {reply:?}");
            if expected(who, device, true) {
                assert!(reply.status != 403, "{ctx}");
            } else {
                assert_eq!(reply.status, 403, "{ctx}");
                let want = if expected(who, device, false) {
                    "device_read_only"
                } else {
                    "device_forbidden"
                };
                assert_eq!(code_of(&reply), want, "{ctx}");
            }
            checked += 1;
            // Tuning needs the operator role *and* the device's control floor.
            let path = format!("/api/v1/devices/{device}/tune");
            let reply = req(
                f.port,
                "POST",
                &path,
                cookie,
                Some(&json!({"center_hz": 100_000_000})),
            );
            let ctx = format!("POST {path} as {who:?}: {reply:?}");
            let role_ok = matches!(who, Caller::Olga | Caller::Admin);
            if role_ok && expected(who, device, true) {
                assert!(reply.status != 403, "{ctx}");
            } else {
                assert_eq!(reply.status, 403, "{ctx}");
                if role_ok {
                    assert!(code_of(&reply).starts_with("device_"), "{ctx}");
                } else {
                    assert_eq!(reply.json()["error"], "forbidden", "{ctx}");
                }
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 6 * 5 * 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn websockets_follow_the_policy_before_the_upgrade() {
    let f = fixture!("ws");
    let status = |path: String, cookie: String| {
        let port = f.port;
        async move {
            let url = format!("ws://127.0.0.1:{port}{path}");
            match ws_connect_with_cookie(&url, &cookie).await {
                Err(tokio_tungstenite::tungstenite::Error::Http(r)) => r.status().as_u16(),
                other => panic!("expected an HTTP refusal for {url}, got {other:?}"),
            }
        }
    };
    for kind in ["spectrum", "audio", "state", "events"] {
        for device in ["dev0", "ops", "private", "noguest"] {
            for who in CALLERS {
                let path = format!("/api/v1/ws/{kind}?dev={device}");
                let got = status(path.clone(), f.cookie(who).to_owned()).await;
                let want = if expected(who, device, false) {
                    503
                } else {
                    403
                };
                assert_eq!(got, want, "{path} as {who:?}");
            }
        }
    }
    // Ids are compared as sent: an encoded spelling of a closed device is refused, not decoded.
    for path in [
        "/api/v1/ws/audio?dev=pr%69vate",
        "/api/v1/ws/audio?dev=dev0&dev=private",
        "/api/v1/ws/audio",
    ] {
        assert_eq!(
            status(path.into(), f.cookie(Caller::Lars).to_owned()).await,
            400,
            "{path}"
        );
    }
    let encoded = req(
        f.port,
        "GET",
        "/api/v1/devices/pr%69vate/state",
        Some(f.cookie(Caller::Lars)),
        None,
    );
    assert_eq!(encoded.status, 400, "{encoded:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_and_history_routes_leave_closed_devices_out() {
    let f = fixture!("plugins");
    // Starting an instance on a closed or read-only device is refused before the host is asked.
    let start = |who, device: &str| {
        req(
            f.port,
            "POST",
            "/api/v1/plugin-instances",
            Some(f.cookie(who)),
            Some(&json!({"plugin_id": "org.superhet.example.echo", "device": device})),
        )
    };
    for (who, device, want) in [
        (Caller::Olga, "ops", None),
        (Caller::Olga, "private", Some("device_forbidden")),
        (Caller::Olga, "ro", None),
        (Caller::Admin, "private", None),
    ] {
        let reply = start(who, device);
        match want {
            Some(code) => {
                assert_eq!(
                    (reply.status, code_of(&reply)),
                    (403, code.to_owned()),
                    "{reply:?}"
                );
            }
            None => assert!(reply.status != 403, "{who:?} {device}: {reply:?}"),
        }
    }
    // Decoder history: a `dev` filter on a closed device is refused; the unfiltered list and
    // the export simply leave such devices out (there is no history in this database, so only
    // the status can be told).
    for who in CALLERS {
        let closed = req(
            f.port,
            "GET",
            "/api/v1/decoder-events?dev=private",
            Some(f.cookie(who)),
            None,
        );
        if expected(who, "private", false) {
            assert_eq!(closed.status, 200, "{who:?}: {closed:?}");
        } else {
            assert_eq!(closed.status, 403, "{who:?}: {closed:?}");
        }
        for path in [
            "/api/v1/decoder-events",
            "/api/v1/decoder-events/export.csv",
        ] {
            let reply = req(f.port, "GET", path, Some(f.cookie(who)), None);
            assert_eq!(reply.status, 200, "{path} as {who:?}: {reply:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_policy_api_validates_audits_and_takes_effect_at_once() {
    let f = fixture!("api");
    let port = f.port;
    let lars = f.cookie(Caller::Lars).to_owned();
    let olga = f.cookie(Caller::Olga).to_owned();

    // Only admins read or write policies.
    for who in [Caller::Guest, Caller::Lars, Caller::Olga] {
        let c = f.cookie(who);
        for (method, path) in [
            ("GET", "/api/v1/admin/device-policies"),
            ("PUT", "/api/v1/admin/device-policies/dev0"),
            ("DELETE", "/api/v1/admin/device-policies/dev0"),
        ] {
            let reply = req(port, method, path, Some(c), Some(&json!({})));
            assert_eq!(reply.status, 403, "{method} {path} as {who:?}");
        }
    }

    let listed = req(
        port,
        "GET",
        "/api/v1/admin/device-policies",
        Some(&f.admin),
        None,
    );
    assert_eq!(listed.status, 200, "{listed:?}");
    let devices: Vec<String> = listed.json()["policies"]
        .as_array()
        .expect("policies")
        .iter()
        .map(|p| p["device"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(devices, ["noguest", "ops", "private", "ro"]);

    // Validation.
    for (path, body) in [
        ("dev0", json!({"view_role": "root"})),
        ("dev0", json!({"control_role": 3})),
        ("dev0", json!({"bogus": true})),
        ("dev0", json!({"allowed_users": [999_999]})),
        ("dev0", json!({"allowed_users": [0]})),
        ("a.b", json!({})),
    ] {
        let reply = put(
            port,
            &f.admin,
            &format!("/api/v1/admin/device-policies/{path}"),
            &body,
        );
        assert_eq!(reply.status, 400, "{path} {body}: {reply:?}");
    }

    // A change applies on the very next request (no cache wait) ...
    let probe = |cookie: &str| {
        req(
            port,
            "GET",
            "/api/v1/devices/dev0/state",
            Some(cookie),
            None,
        )
        .status
    };
    assert_eq!(probe(&lars), 503);
    set_policy(port, &f.admin, "dev0", &json!({"view_role": "operator"}));
    assert_eq!(probe(&lars), 403);
    assert_eq!(probe(&olga), 503);
    // ... a stricter value never loosens what other devices say ...
    assert_eq!(
        req(port, "GET", "/api/v1/devices/ro/state", Some(&lars), None).status,
        503
    );
    // ... and a reset brings back the open default.
    let reset = req(
        port,
        "DELETE",
        "/api/v1/admin/device-policies/dev0",
        Some(&f.admin),
        None,
    );
    assert_eq!(reset.status, 200, "{reset:?}");
    assert_eq!(reset.json()["was_set"], true);
    assert_eq!(probe(&lars), 503);
    let again = req(
        port,
        "DELETE",
        "/api/v1/admin/device-policies/dev0",
        Some(&f.admin),
        None,
    );
    assert_eq!(again.json()["was_set"], false);

    // Audit: the change, the reset and the refusals are written.
    let change = audit(port, &f.admin, "action=device.policy_change");
    assert!(change.len() >= 5, "{change:?}");
    assert_eq!(change[0]["target"], "dev0");
    assert_eq!(change[0]["details"]["view_role"], "operator");
    assert_eq!(one(port, &f.admin, "device.policy_reset")["target"], "dev0");
    // Refusals are coalesced and written a moment later.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let denied = audit(port, &f.admin, "action=access.denied");
        if denied
            .iter()
            .any(|e| e["details"]["device"] == "dev0" && e["details"]["need"] == "view")
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{denied:?}");
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn existing_installs_see_no_change_without_policies() {
    let Some(db) = TestDb::create("default").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (_api, port) = spawn_plain_api(&db.url);
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    let olga = make_user(port, &admin, "olga", "operator");
    let lars = make_user(port, &admin, "lars", "listener");
    enable_guests(port, &admin, json!({}));
    let guest = guest_session(port).expect("guest");
    let listed = req(
        port,
        "GET",
        "/api/v1/admin/device-policies",
        Some(&admin),
        None,
    );
    assert_eq!(listed.json()["policies"], json!([]));
    for cookie in [&guest, &lars, &olga, &admin] {
        for path in ["/api/v1/devices/dev0/state", "/api/v1/devices/rtl7/vfos"] {
            assert_eq!(
                req(port, "GET", path, Some(cookie), None).status,
                503,
                "{path}"
            );
        }
        let vfo = req(
            port,
            "POST",
            "/api/v1/devices/dev0/vfos",
            Some(cookie),
            Some(&json!({"freq_hz": 100_000_000, "mode": "nfm"})),
        );
        assert!(vfo.status != 403, "{vfo:?}");
    }
}

/// The device list needs a bus: it reads the device keys from the `STATE` bucket.
#[tokio::test(flavor = "multi_thread")]
async fn the_device_list_shows_each_caller_only_the_devices_it_may_use() {
    let (Some(nats_url), Some(db)) = (
        std::env::var("SUPERHET_TEST_NATS_URL")
            .ok()
            .filter(|u| !u.is_empty()),
        TestDb::create("list").await,
    ) else {
        eprintln!("DATABASE_URL or SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let client = async_nats::connect(&nats_url).await.expect("nats");
    let js = async_nats::jetstream::new(client);
    // The API reads the real bucket; make sure it exists and holds our devices.
    let store = match js.get_key_value("STATE").await {
        Ok(store) => store,
        Err(_) => js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: "STATE".into(),
                ..Default::default()
            })
            .await
            .expect("create STATE"),
    };
    let ids = ["dev0", "ops", "ro", "private", "noguest"];
    for id in ids {
        store
            .put(
                format!("dev.{id}"),
                json!({"device": id, "driver": "file", "center_hz": 1, "sample_rate": 2})
                    .to_string()
                    .into(),
            )
            .await
            .expect("put");
    }
    let (_api, port) = spawn_api(&db.url, |c| {
        c.env("SUPERHET_NATS_URL", &nats_url);
    });
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    let olga = make_user(port, &admin, "olga", "operator");
    let lars = make_user(port, &admin, "lars", "listener");
    let lena = make_user(port, &admin, "lena", "listener");
    enable_guests(port, &admin, json!({}));
    let guest = guest_session(port).expect("guest");
    let lena_id = user_id(port, &admin, "lena");
    set_policy(
        port,
        &admin,
        "ops",
        &json!({"view_role": "operator", "control_role": "operator"}),
    );
    set_policy(port, &admin, "ro", &json!({"control_role": "operator"}));
    set_policy(
        port,
        &admin,
        "private",
        &json!({"allowed_users": [lena_id]}),
    );
    set_policy(port, &admin, "noguest", &json!({"guests_visible": false}));

    let list = |cookie: &str| -> Vec<(String, bool)> {
        let reply = req(port, "GET", "/api/v1/devices", Some(cookie), None);
        assert_eq!(reply.status, 200, "{reply:?}");
        reply.json()["devices"]
            .as_array()
            .expect("devices")
            .iter()
            // Other tests may share the bus and its bucket.
            .filter(|d| ids.contains(&d["id"].as_str().unwrap_or("")))
            .map(|d| {
                assert_eq!(d["access"]["view"], true);
                (
                    d["id"].as_str().unwrap_or("").to_owned(),
                    d["access"]["control"] == true,
                )
            })
            .collect()
    };
    let names = |l: Vec<(String, bool)>| l.into_iter().map(|(d, _)| d).collect::<Vec<_>>();
    assert_eq!(
        names(list(&admin)),
        ["dev0", "noguest", "ops", "private", "ro"]
    );
    assert_eq!(names(list(&olga)), ["dev0", "noguest", "ops", "ro"]);
    assert_eq!(names(list(&lars)), ["dev0", "noguest", "ro"]);
    assert_eq!(names(list(&lena)), ["dev0", "noguest", "private", "ro"]);
    assert_eq!(names(list(&guest)), ["dev0", "ro"]);
    // Read-only shows up as a flag instead of a missing entry.
    assert_eq!(
        list(&lars),
        [
            ("dev0".to_owned(), true),
            ("noguest".to_owned(), true),
            ("ro".to_owned(), false)
        ]
    );
    assert!(list(&olga).iter().all(|(_, control)| *control));

    // A policy change that takes a device away closes the open sockets of those who lost it.
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::{Error as WsError, Message};
    let url = format!("ws://127.0.0.1:{port}/api/v1/ws/state?dev=dev0");
    let mut lars_socket = ws_connect_with_cookie(&url, &lars)
        .await
        .expect("lars may look at dev0");
    let mut olga_socket = ws_connect_with_cookie(&url, &olga)
        .await
        .expect("olga may look at dev0");
    set_policy(port, &admin, "dev0", &json!({"view_role": "operator"}));
    let closed = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            match lars_socket.next().await {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the listener's socket was not closed");
    match ws_connect_with_cookie(&url, &lars).await {
        Err(WsError::Http(r)) => assert_eq!(r.status().as_u16(), 403),
        other => panic!("expected a refusal, got {other:?}"),
    }
    // The operator keeps its socket and can reconnect.
    let still_open = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            match olga_socket.next().await {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(still_open.is_err(), "the operator's socket was closed");
    ws_connect_with_cookie(&url, &olga)
        .await
        .expect("olga reconnects");

    for id in ids {
        let _ = store.purge(format!("dev.{id}")).await;
    }
}
