//! End-to-end tests of local authentication: the real `superhet-api` binary on a real
//! PostgreSQL (needs `DATABASE_URL`; skipped otherwise). No NATS involved.

mod common;

use std::io::{BufRead, BufReader};
use std::process::Stdio;

use common::*;
use serde_json::{Value, json};

const JSON: (&str, &str) = ("Content-Type", "application/json");

fn post(port: u16, path: &str, headers: &[(&str, &str)], body: Value) -> Reply {
    let mut all = vec![JSON];
    all.extend_from_slice(headers);
    http(port, "POST", path, &all, &body.to_string())
}

fn login(port: u16, user: &str, password: &str) -> Reply {
    post(
        port,
        "/api/v1/auth/login",
        &[],
        json!({"username": user, "password": password}),
    )
}

fn status(port: u16, cookie: Option<&str>) -> Value {
    let headers: Vec<(&str, &str)> = cookie.map(|c| ("Cookie", c)).into_iter().collect();
    let reply = http(port, "GET", "/api/v1/auth/status", &headers, "");
    assert_eq!(reply.status, 200, "{reply:?}");
    // The three fields of the original contract; the rest is covered by multiuser_live.
    let full = reply.json();
    json!({
        "setup_required": full["setup_required"],
        "authenticated": full["authenticated"],
        "username": full["username"],
    })
}

fn get(port: u16, path: &str, cookie: Option<&str>) -> Reply {
    let headers: Vec<(&str, &str)> = cookie.map(|c| ("Cookie", c)).into_iter().collect();
    http(port, "GET", path, &headers, "")
}

macro_rules! start {
    ($tag:expr) => {{
        let Some(db) = TestDb::create($tag).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        let (api, port) = spawn_api(&db.url, |_| {});
        wait_ready(port, "api").await;
        (db, api, port)
    }};
}

#[tokio::test(flavor = "multi_thread")]
async fn first_run_setup_login_logout_and_route_protection() {
    let (_db, _api, port) = start!("flow");

    assert_eq!(
        status(port, None),
        json!({"setup_required": true, "authenticated": false, "username": null})
    );

    // Deny by default: protected and unknown API paths answer 401, health probes stay open.
    for path in [
        "/api/v1/devices/rtl0/state",
        "/api/v1/devices/rtl0/vfos",
        "/api/v1/nonexistent",
    ] {
        let reply = get(port, path, None);
        assert_eq!(reply.status, 401, "{path}");
        assert_eq!(reply.json(), json!({"error": "unauthenticated"}));
    }
    assert_eq!(get(port, "/healthz", None).status, 200);
    let reply = post(
        port,
        "/api/v1/devices/rtl0/tune",
        &[],
        json!({"center_hz": 100_000_000}),
    );
    assert_eq!(reply.status, 401);

    // WebSocket upgrades are refused before the upgrade.
    for path in ["/api/v1/ws/spectrum?dev=rtl0", "/api/v1/ws/audio?dev=rtl0"] {
        match ws_connect_with_cookie(
            &format!("ws://127.0.0.1:{port}{path}"),
            "superhet_session=bogus",
        )
        .await
        {
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
                assert_eq!(r.status(), 401, "{path}")
            }
            other => panic!(
                "{path}: expected HTTP 401, got {:?}",
                other.map(|_| "connected")
            ),
        }
    }

    // Setup validation and the setup token.
    let bad_token = post(
        port,
        "/api/v1/auth/setup",
        &[],
        json!({"username": "admin", "password": ADMIN_PASSWORD, "setup_token": "nope"}),
    );
    assert_eq!(bad_token.status, 403);
    assert_eq!(bad_token.json(), json!({"error": "invalid_setup_token"}));
    for (user, pw) in [
        ("bad user", ADMIN_PASSWORD),
        ("admin", "short"),
        ("", ADMIN_PASSWORD),
    ] {
        let reply = post(
            port,
            "/api/v1/auth/setup",
            &[],
            json!({"username": user, "password": pw, "setup_token": SETUP_TOKEN}),
        );
        assert_eq!(reply.status, 400, "{user:?}/{pw:?}");
        assert_eq!(reply.json()["error"], "invalid");
        assert!(reply.json()["message"].is_string());
    }
    let wrong_type = http(
        port,
        "POST",
        "/api/v1/auth/setup",
        &[("Content-Type", "text/plain")],
        "{}",
    );
    assert_eq!(wrong_type.status, 415);
    let cross_site = post(
        port,
        "/api/v1/auth/setup",
        &[("Origin", "https://evil.example")],
        json!({"username": "admin", "password": ADMIN_PASSWORD, "setup_token": SETUP_TOKEN}),
    );
    assert_eq!(cross_site.status, 403);

    // Successful setup: 204 and a hardened cookie.
    let setup = post(
        port,
        "/api/v1/auth/setup",
        &[],
        json!({"username": "admin", "password": ADMIN_PASSWORD, "setup_token": SETUP_TOKEN}),
    );
    assert_eq!(setup.status, 204, "{setup:?}");
    let set_cookie = setup.set_cookie().expect("set-cookie").to_owned();
    for flag in [
        "superhet_session=",
        "Path=/",
        "HttpOnly",
        "SameSite=Strict",
        "Max-Age=2592000",
    ] {
        assert!(set_cookie.contains(flag), "{set_cookie} lacks {flag}");
    }
    assert!(
        !set_cookie.contains("Secure"),
        "plain http, auto mode: {set_cookie}"
    );
    let cookie = setup.session_cookie().expect("cookie");
    assert!(cookie.len() > "superhet_session=".len() + 40);

    assert_eq!(
        status(port, Some(&cookie)),
        json!({"setup_required": false, "authenticated": true, "username": "admin"})
    );
    assert_eq!(
        status(port, None),
        json!({"setup_required": false, "authenticated": false, "username": null})
    );

    // Setup is a one-shot.
    let again = post(
        port,
        "/api/v1/auth/setup",
        &[],
        json!({"username": "second", "password": ADMIN_PASSWORD, "setup_token": SETUP_TOKEN}),
    );
    assert_eq!(
        (again.status, again.json()),
        (409, json!({"error": "already_setup"}))
    );

    // With the session, protected routes are reachable (503: no NATS here), also over WS.
    let reply = get(port, "/api/v1/devices/rtl0/state", Some(&cookie));
    assert_ne!(reply.status, 401, "{reply:?}");
    let ws = ws_connect_with_cookie(
        &format!("ws://127.0.0.1:{port}/api/v1/ws/spectrum?dev=rtl0"),
        &cookie,
    )
    .await;
    // The upgrade passes the session check; without NATS it ends in 503, never 401.
    match ws {
        Ok(_) => {}
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status(), 503),
        Err(other) => panic!("ws with session: {other:?}"),
    }
    // ... but the Origin check still applies on top.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://127.0.0.1:{port}/api/v1/ws/spectrum?dev=rtl0")
        .into_client_request()
        .expect("request");
    request
        .headers_mut()
        .insert("Cookie", cookie.parse().expect("hdr"));
    request
        .headers_mut()
        .insert("Origin", "https://evil.example".parse().expect("hdr"));
    match tokio_tungstenite::connect_async(request).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status(), 403),
        other => panic!("expected 403, got {:?}", other.map(|_| "connected")),
    }

    // Login: wrong password, unknown user, right password (user name is case-insensitive).
    for (user, pw) in [("admin", "wrong password!!"), ("ghost", ADMIN_PASSWORD)] {
        let reply = login(port, user, pw);
        assert_eq!(reply.status, 401, "{user}");
        assert_eq!(reply.json(), json!({"error": "invalid_credentials"}));
        assert!(reply.set_cookie().is_none());
    }
    let cross_site = post(
        port,
        "/api/v1/auth/login",
        &[("Origin", "https://evil.example")],
        json!({"username": "admin", "password": ADMIN_PASSWORD}),
    );
    assert_eq!(cross_site.status, 403);
    let second = login(port, "ADMIN", ADMIN_PASSWORD);
    assert_eq!(second.status, 204);
    let second_cookie = second.session_cookie().expect("cookie");
    assert_ne!(second_cookie, cookie, "every login gets a fresh token");

    // Logout kills exactly that session and clears the cookie.
    let out = http(
        port,
        "POST",
        "/api/v1/auth/logout",
        &[("Cookie", &second_cookie)],
        "",
    );
    assert_eq!(out.status, 204);
    let cleared = out.set_cookie().expect("clearing cookie");
    assert!(
        cleared.starts_with("superhet_session=;") && cleared.contains("Max-Age=0"),
        "{cleared}"
    );
    assert_eq!(
        get(port, "/api/v1/devices/rtl0/state", Some(&second_cookie)).status,
        401
    );
    assert_ne!(
        get(port, "/api/v1/devices/rtl0/state", Some(&cookie)).status,
        401
    );
    // Logging out without a session is harmless.
    assert_eq!(
        http(port, "POST", "/api/v1/auth/logout", &[], "").status,
        204
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn password_change_revokes_other_sessions() {
    let (_db, _api, port) = start!("pw");
    let a = setup_admin(port);
    let b = login(port, ADMIN_USER, ADMIN_PASSWORD)
        .session_cookie()
        .expect("cookie");
    let new_password = "another very long passphrase";

    let change = |cookie: Option<&str>, current: &str, new: &str| {
        let headers: Vec<(&str, &str)> = cookie.map(|c| ("Cookie", c)).into_iter().collect();
        post(
            port,
            "/api/v1/auth/password",
            &headers,
            json!({"current_password": current, "new_password": new}),
        )
    };
    let reply = change(None, ADMIN_PASSWORD, new_password);
    assert_eq!(
        (reply.status, reply.json()),
        (401, json!({"error": "unauthenticated"}))
    );
    let reply = change(Some(&a), "not the password", new_password);
    assert_eq!(
        (reply.status, reply.json()),
        (401, json!({"error": "invalid_credentials"}))
    );
    assert_eq!(change(Some(&a), ADMIN_PASSWORD, "short").status, 400);
    // Nothing happened yet.
    assert_ne!(get(port, "/api/v1/devices/x/state", Some(&b)).status, 401);

    assert_eq!(change(Some(&a), ADMIN_PASSWORD, new_password).status, 204);
    assert_ne!(
        get(port, "/api/v1/devices/x/state", Some(&a)).status,
        401,
        "current session survives"
    );
    assert_eq!(
        get(port, "/api/v1/devices/x/state", Some(&b)).status,
        401,
        "others are revoked"
    );
    assert_eq!(
        login(port, ADMIN_USER, ADMIN_PASSWORD).status,
        401,
        "old password is dead"
    );
    assert_eq!(login(port, ADMIN_USER, new_password).status, 204);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_logins_are_rate_limited() {
    let (_db, _api, port) = start!("rate");
    setup_admin(port);
    for attempt in 1..=5 {
        let reply = login(port, ADMIN_USER, "wrong password!!");
        assert_eq!(reply.status, 401, "attempt {attempt}");
    }
    let blocked = login(port, ADMIN_USER, "wrong password!!");
    assert_eq!(
        (blocked.status, blocked.json()),
        (429, json!({"error": "rate_limited"}))
    );
    let retry: u64 = blocked
        .header("retry-after")
        .expect("Retry-After")
        .parse()
        .expect("seconds");
    assert!((1..=900).contains(&retry), "{retry}");
    // Even the right password is refused while blocked.
    assert_eq!(login(port, ADMIN_USER, ADMIN_PASSWORD).status, 429);
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_expire_idle_and_absolute() {
    let (db, _api, port) = start!("expiry");
    let idle = setup_admin(port);
    let absolute = login(port, ADMIN_USER, ADMIN_PASSWORD)
        .session_cookie()
        .expect("cookie");
    let live = login(port, ADMIN_USER, ADMIN_PASSWORD)
        .session_cookie()
        .expect("cookie");
    let pool = db.pool().await;
    let idx = |cookie: &str| -> Vec<u8> {
        use sha2::{Digest, Sha256};
        let token = cookie.trim_start_matches("superhet_session=");
        Sha256::digest(token.as_bytes()).to_vec()
    };
    // Only the digest is stored, never the token.
    let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT id FROM superhet.sessions")
        .fetch_all(&pool)
        .await
        .expect("ids");
    assert_eq!(stored.len(), 3);
    assert!(stored.contains(&idx(&live)));
    let (ip, agent): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT ip, user_agent FROM superhet.sessions LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(ip.as_deref(), Some("127.0.0.1"));
    assert!(agent.is_none(), "the raw test client sends no user agent");

    sqlx::query(
        "UPDATE superhet.sessions SET last_seen_at = now() - interval '8 days' WHERE id = $1",
    )
    .bind(idx(&idle))
    .execute(&pool)
    .await
    .expect("age idle");
    sqlx::query(
        "UPDATE superhet.sessions SET created_at = now() - interval '31 days' WHERE id = $1",
    )
    .bind(idx(&absolute))
    .execute(&pool)
    .await
    .expect("age absolute");
    assert_eq!(
        get(port, "/api/v1/devices/x/state", Some(&idle)).status,
        401
    );
    assert_eq!(
        get(port, "/api/v1/devices/x/state", Some(&absolute)).status,
        401
    );
    assert_ne!(
        get(port, "/api/v1/devices/x/state", Some(&live)).status,
        401
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM superhet.sessions")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 1, "expired sessions are deleted on access");
}

#[tokio::test(flavor = "multi_thread")]
async fn cookie_secure_policy() {
    let Some(db) = TestDb::create("secure").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    // Explicit true.
    let (_api, port) = spawn_api(&db.url, |c| {
        c.env("SUPERHET_COOKIE_SECURE", "true");
    });
    wait_ready(port, "api").await;
    let reply = login_or_setup(port);
    assert!(reply.set_cookie().expect("cookie").ends_with("; Secure"));
    drop(_api);

    // Auto: forwarded proto is ignored unless the proxy is trusted.
    let (_api, port) = spawn_api(&db.url, |_| {});
    wait_ready(port, "api").await;
    let spoofed = post(
        port,
        "/api/v1/auth/login",
        &[("X-Forwarded-Proto", "https")],
        json!({"username": ADMIN_USER, "password": ADMIN_PASSWORD}),
    );
    assert_eq!(spoofed.status, 204);
    assert!(!spoofed.set_cookie().expect("cookie").contains("Secure"));
    drop(_api);

    let (_api, port) = spawn_api(&db.url, |c| {
        c.env("SUPERHET_TRUST_PROXY", "true");
    });
    wait_ready(port, "api").await;
    let proxied = post(
        port,
        "/api/v1/auth/login",
        &[("X-Forwarded-Proto", "https")],
        json!({"username": ADMIN_USER, "password": ADMIN_PASSWORD}),
    );
    assert!(proxied.set_cookie().expect("cookie").ends_with("; Secure"));
    let direct = login(port, ADMIN_USER, ADMIN_PASSWORD);
    assert!(!direct.set_cookie().expect("cookie").contains("Secure"));
}

fn login_or_setup(port: u16) -> Reply {
    post(
        port,
        "/api/v1/auth/setup",
        &[],
        json!({"username": ADMIN_USER, "password": ADMIN_PASSWORD, "setup_token": SETUP_TOKEN}),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn generated_setup_token_is_logged_and_works() {
    let Some(db) = TestDb::create("token").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let port = free_port();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_superhet-api"));
    command
        .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
        .env("SUPERHET_DATABASE_URL", &db.url)
        .env_remove("SUPERHET_SETUP_TOKEN")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn");
    let stdout = child.stdout.take().expect("stdout");
    let _api = Proc(child);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some((_, rest)) = line.split_once("setup token: ") {
                let token = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let _ = tx.send((token, line));
                return;
            }
        }
    });
    let (token, line) = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("setup token in the log");
    assert!(token.len() >= 32, "{token}");
    assert!(line.contains("WARN"), "{line}");
    assert!(line.contains("docker compose logs superhet-api"), "{line}");
    wait_ready(port, "api").await;

    let wrong = post(
        port,
        "/api/v1/auth/setup",
        &[],
        json!({"username": ADMIN_USER, "password": ADMIN_PASSWORD, "setup_token": SETUP_TOKEN}),
    );
    assert_eq!(wrong.status, 403);
    let ok = post(
        port,
        "/api/v1/auth/setup",
        &[],
        json!({"username": ADMIN_USER, "password": ADMIN_PASSWORD, "setup_token": token}),
    );
    assert_eq!(ok.status, 204, "{ok:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_after_setup_requires_no_setup() {
    let Some(db) = TestDb::create("restart").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (api, port) = spawn_api(&db.url, |_| {});
    wait_ready(port, "api").await;
    let cookie = setup_admin(port);
    drop(api);
    let (_api, port) = spawn_api(&db.url, |_| {});
    wait_ready(port, "api").await;
    // Sessions are in PostgreSQL and survive a restart; the setup token is gone for good.
    assert_eq!(
        status(port, Some(&cookie)),
        json!({"setup_required": false, "authenticated": true, "username": "admin"})
    );
    let reply = login_or_setup(port);
    assert_eq!(reply.status, 409);
}
