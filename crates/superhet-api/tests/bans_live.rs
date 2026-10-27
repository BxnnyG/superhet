//! Address bans of the real `superhet-api` binary on a real PostgreSQL (needs `DATABASE_URL`;
//! skipped otherwise): enforcement before login, guest entry and WebSocket upgrades, networks,
//! expiry, kick on ban, lifting, audit and persistence across a restart. Client addresses are
//! simulated with `X-Forwarded-For` (`SUPERHET_TRUST_PROXY=true`).

mod common;

use std::time::Duration;

use common::*;
use serde_json::{Value, json};

fn spawn(db_url: &str) -> (Proc, u16) {
    spawn_api(db_url, |c| {
        c.env_remove("SUPERHET_NATS_URL");
        c.env("SUPERHET_TRUST_PROXY", "true");
    })
}

/// A request that appears to come from `ip`.
fn from(
    port: u16,
    ip: &str,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&Value>,
) -> Reply {
    let mut headers: Vec<(&str, &str)> = vec![("User-Agent", UA), ("X-Forwarded-For", ip)];
    if let Some(c) = cookie {
        headers.push(("Cookie", c));
    }
    if body.is_some() {
        headers.push(("Content-Type", "application/json"));
    }
    let text = body.map(Value::to_string).unwrap_or_default();
    http(port, method, path, &headers, &text)
}

fn ban(port: u16, admin: &str, body: &Value) -> Reply {
    req(port, "POST", "/api/v1/admin/bans", Some(admin), Some(body))
}

fn assert_banned(reply: &Reply, ctx: &str) {
    assert_eq!(reply.status, 403, "{ctx}: {reply:?}");
    assert_eq!(reply.json()["error"], "banned", "{ctx}: {reply:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_banned_address_is_refused_before_authentication() {
    let Some(db) = TestDb::create("bans").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (_api, port) = spawn(&db.url);
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    enable_guests(port, &admin, json!({}));

    // Validation.
    for (body, status, code) in [
        (json!({}), 400, "invalid"),
        (json!({"network": "banana"}), 400, "invalid"),
        (json!({"network": "0.0.0.0/0"}), 400, "invalid"),
        (json!({"network": "10.0.0.0/7"}), 400, "invalid"),
        (
            json!({"network": "203.0.113.7", "session": "g.abc"}),
            400,
            "invalid",
        ),
        (
            json!({"network": "203.0.113.7", "expires_in_minutes": 0}),
            400,
            "invalid",
        ),
        (
            json!({"network": "203.0.113.7", "expires_in_minutes": 999_999_999}),
            400,
            "invalid",
        ),
        (json!({"network": "127.0.0.0/8"}), 409, "would_ban_self"),
        (json!({"session": "g.nobody"}), 404, "not_found"),
    ] {
        let reply = ban(port, &admin, &body);
        assert_eq!(
            (reply.status, reply.json()["error"].clone()),
            (status, json!(code)),
            "{body}: {reply:?}"
        );
    }

    // A host ban with expiry and reason.
    let created = ban(
        port,
        &admin,
        &json!({"network": "203.0.113.7", "reason": "spraying", "expires_in_minutes": 60}),
    );
    assert_eq!(created.status, 201, "{created:?}");
    let created = created.json();
    let id = created["ban"]["id"].as_i64().expect("ban id");
    assert_eq!(created["ban"]["network"], "203.0.113.7/32");
    assert_eq!(created["ban"]["state"], "active");
    assert_eq!(created["ban"]["reason"], "spraying");
    assert!(created["ban"]["expires_at_ms"].as_i64().is_some());
    assert_eq!(created["kicked"], 0);
    let again = ban(port, &admin, &json!({"network": "203.0.113.7"}));
    assert_eq!(
        (again.status, again.json()["error"].clone()),
        (409, json!("already_banned"))
    );

    // Everything under /api is refused for that address, before authentication.
    let credentials = json!({"username": ADMIN_USER, "password": ADMIN_PASSWORD});
    for (method, path, body) in [
        ("GET", "/api/v1/auth/status", None),
        ("POST", "/api/v1/auth/login", Some(&credentials)),
        ("POST", "/api/v1/auth/guest", None),
        ("GET", "/api/v1/auth/invites/whatever", None),
        ("GET", "/api/v1/devices/dev0/state", None),
    ] {
        assert_banned(
            &from(port, "203.0.113.7", method, path, None, body),
            &format!("{method} {path}"),
        );
    }
    // Even a valid session does not help, and the reply names the deadline but not the reason.
    let with_session = from(
        port,
        "203.0.113.7",
        "GET",
        "/api/v1/auth/status",
        Some(&admin),
        None,
    );
    assert_banned(&with_session, "valid session");
    assert!(with_session.json()["expires_at_ms"].as_i64().is_some());
    assert!(!with_session.body.contains("spraying"));
    // Health probes and the UI shell are not gated; other addresses are unaffected.
    assert_eq!(
        from(port, "203.0.113.7", "GET", "/healthz", None, None).status,
        200
    );
    let other = from(
        port,
        "203.0.113.8",
        "GET",
        "/api/v1/auth/status",
        None,
        None,
    );
    assert_eq!(other.status, 200, "{other:?}");
    // The WebSocket upgrade is gated as well.
    {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!("ws://127.0.0.1:{port}/api/v1/ws/spectrum?dev=dev0")
            .into_client_request()
            .expect("request");
        request
            .headers_mut()
            .insert("X-Forwarded-For", "203.0.113.7".parse().expect("header"));
        request
            .headers_mut()
            .insert("Cookie", admin.parse().expect("header"));
        match tokio_tungstenite::connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
                assert_eq!(r.status().as_u16(), 403);
            }
            other => panic!("expected an HTTP refusal, got {other:?}"),
        }
    }

    // Listing; non-admins cannot see or change bans.
    let listed = req(port, "GET", "/api/v1/admin/bans", Some(&admin), None).json();
    assert_eq!(listed["bans"].as_array().map(Vec::len), Some(1));
    let listener = make_user(port, &admin, "lars", "listener");
    for (method, path) in [
        ("GET", "/api/v1/admin/bans"),
        ("DELETE", &format!("/api/v1/admin/bans/{id}")),
    ] {
        assert_eq!(req(port, method, path, Some(&listener), None).status, 403);
    }

    // Lifting restores access at once; history shows it.
    let lifted = req(
        port,
        "DELETE",
        &format!("/api/v1/admin/bans/{id}"),
        Some(&admin),
        None,
    );
    assert_eq!(lifted.status, 204, "{lifted:?}");
    assert_eq!(
        from(
            port,
            "203.0.113.7",
            "GET",
            "/api/v1/auth/status",
            None,
            None
        )
        .status,
        200
    );
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/admin/bans/{id}"),
            Some(&admin),
            None
        )
        .status,
        404,
        "a lifted ban cannot be lifted again"
    );
    let active = req(port, "GET", "/api/v1/admin/bans", Some(&admin), None).json();
    assert_eq!(active["bans"].as_array().map(Vec::len), Some(0));
    let all = req(
        port,
        "GET",
        "/api/v1/admin/bans?state=all",
        Some(&admin),
        None,
    )
    .json();
    assert_eq!(all["bans"][0]["state"], "lifted");
    assert_eq!(all["bans"][0]["lifted_by"], ADMIN_USER);
    assert_eq!(
        req(
            port,
            "GET",
            "/api/v1/admin/bans?state=banana",
            Some(&admin),
            None
        )
        .status,
        400
    );

    // Audit: creation and lifting by the admin, the refusals coalesced into denied rows.
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let created = one(port, &admin, "ban.create");
    assert_eq!(created["target"], "203.0.113.7/32");
    assert_eq!(created["actor_name"], ADMIN_USER);
    assert_eq!(created["details"]["reason"], "spraying");
    assert_eq!(one(port, &admin, "ban.lift")["target"], "203.0.113.7/32");
    let denied = audit(port, &admin, "action=access.banned");
    assert!(!denied.is_empty(), "refusals are audited");
    assert!(denied.iter().all(|e| e["outcome"] == "denied"));
}

#[tokio::test(flavor = "multi_thread")]
async fn networks_ban_kick_guests_and_survive_a_restart() {
    let Some(db) = TestDb::create("bans_net").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (api, port) = spawn(&db.url);
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    enable_guests(
        port,
        &admin,
        json!({"max_guests": 10, "max_guests_per_ip": 5}),
    );

    // A guest from 198.51.100.4 enters; banning that live session bans its exact address and
    // ends the session.
    let entered = from(
        port,
        "198.51.100.4",
        "POST",
        "/api/v1/auth/guest",
        None,
        None,
    );
    assert_eq!(entered.status, 200, "{entered:?}");
    let guest_cookie = entered.session_cookie().expect("guest cookie");
    let sessions = req(port, "GET", "/api/v1/admin/sessions", Some(&admin), None).json();
    let sid = sessions["sessions"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["kind"] == "guest"))
        .and_then(|s| s["sid"].as_str())
        .expect("guest sid")
        .to_owned();
    let created = ban(
        port,
        &admin,
        &json!({"session": sid, "reason": "kick & ban"}),
    );
    assert_eq!(created.status, 201, "{created:?}");
    let created = created.json();
    assert_eq!(created["ban"]["network"], "198.51.100.4/32");
    assert!(created["ban"]["expires_at_ms"].is_null(), "permanent");
    assert_eq!(created["kicked"], 1);
    let sessions = req(port, "GET", "/api/v1/admin/sessions", Some(&admin), None).json();
    assert!(
        sessions["sessions"]
            .as_array()
            .is_some_and(|s| s.iter().all(|s| s["kind"] != "guest")),
        "the guest session is gone: {sessions}"
    );
    assert_banned(
        &from(
            port,
            "198.51.100.4",
            "GET",
            "/api/v1/auth/status",
            Some(&guest_cookie),
            None,
        ),
        "kicked guest",
    );

    // A /24 covers its hosts (mapped IPv6 clients included) and, with the default kick, ends
    // the account sessions that were opened from it; the rest of the world is unaffected.
    make_user(port, &admin, "olga", "operator");
    let login = from(
        port,
        "192.0.2.55",
        "POST",
        "/api/v1/auth/login",
        None,
        Some(&json!({"username": "olga", "password": format!("olga-{ADMIN_PW2}")})),
    );
    assert_eq!(login.status, 204, "{login:?}");
    let remote = login.session_cookie().expect("operator cookie");
    assert_eq!(
        from(
            port,
            "192.0.3.1",
            "GET",
            "/api/v1/auth/status",
            Some(&remote),
            None
        )
        .json()["role"],
        "operator"
    );
    let net = ban(port, &admin, &json!({"network": "192.0.2.77/24"}));
    assert_eq!(net.status, 201, "{net:?}");
    assert_eq!(net.json()["ban"]["network"], "192.0.2.0/24");
    assert_eq!(net.json()["kicked"], 1);
    assert_eq!(
        from(
            port,
            "192.0.3.1",
            "GET",
            "/api/v1/auth/status",
            Some(&remote),
            None
        )
        .json()["authenticated"],
        false,
        "the session opened from the banned network was revoked"
    );
    assert_banned(
        &from(
            port,
            "192.0.2.200",
            "GET",
            "/api/v1/auth/status",
            None,
            None,
        ),
        "inside the /24",
    );
    assert_banned(
        &from(
            port,
            "::ffff:192.0.2.9",
            "GET",
            "/api/v1/auth/status",
            None,
            None,
        ),
        "mapped",
    );
    assert_eq!(
        from(port, "192.0.3.1", "GET", "/api/v1/auth/status", None, None).status,
        200
    );

    // An expired row is ignored after a restart, the others still apply: the cache is
    // rebuilt from the database.
    let pool = db.pool().await;
    sqlx::query(
        "INSERT INTO superhet.ip_bans (network, created_by_name, expires_at) \
         VALUES ('203.0.113.99/32', 'test', now() - interval '1 minute')",
    )
    .execute(&pool)
    .await
    .expect("insert expired ban");
    drop(api);
    let (_api, port) = spawn(&db.url);
    wait_ready(port, "api").await;
    assert_banned(
        &from(
            port,
            "192.0.2.200",
            "GET",
            "/api/v1/auth/status",
            None,
            None,
        ),
        "after restart",
    );
    assert_banned(
        &from(
            port,
            "198.51.100.4",
            "GET",
            "/api/v1/auth/status",
            None,
            None,
        ),
        "host after restart",
    );
    assert_eq!(
        from(
            port,
            "203.0.113.99",
            "GET",
            "/api/v1/auth/status",
            None,
            None
        )
        .status,
        200,
        "expired ban"
    );
    // The expired row is listed as expired in the history, not among the active bans.
    let all = req(
        port,
        "GET",
        "/api/v1/admin/bans?state=all",
        Some(&admin),
        None,
    )
    .json();
    assert!(
        all["bans"]
            .as_array()
            .is_some_and(|b| b.iter().any(|b| b["state"] == "expired"))
    );
    let active = req(port, "GET", "/api/v1/admin/bans", Some(&admin), None).json();
    assert_eq!(active["bans"].as_array().map(Vec::len), Some(2));
}
