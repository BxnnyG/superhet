//! Multi-user behaviour of the real `superhet-api` binary on a real PostgreSQL (needs
//! `DATABASE_URL`; skipped otherwise): role matrix, guest mode, invites, login hardening and
//! the audit log. No NATS and no sdrd involved (see `multiuser_stack.rs` for those).

mod common;

use std::time::{Duration, Instant};

use common::*;
use serde_json::{Value, json};

/// The API without a bus, whatever `SUPERHET_NATS_URL` the environment (CI) carries: these tests
/// expect the gateways to answer 503.
fn spawn_plain_api(
    db_url: &str,
    configure: impl FnOnce(&mut std::process::Command),
) -> (Proc, u16) {
    spawn_api(db_url, |c| {
        c.env_remove("SUPERHET_NATS_URL");
        configure(c);
    })
}

macro_rules! start {
    ($tag:expr) => {
        start!($tag, |_| {})
    };
    ($tag:expr, $configure:expr) => {{
        let Some(db) = TestDb::create($tag).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        let (api, port) = spawn_plain_api(&db.url, $configure);
        wait_ready(port, "api").await;
        let admin = setup_admin(port);
        (db, api, port, admin)
    }};
}

/// `(method, path, access)` triples from the access table in the source, so the matrix always
/// covers exactly the routes the API declares.
fn declared_rules() -> Vec<(String, String, String)> {
    let source = include_str!("../src/access.rs");
    let table = &source[source.find("pub const RULES").expect("RULES")..];
    let table = &table[..table.find("];").expect("end of RULES")];
    let mut rules = Vec::new();
    for chunk in table.split("rule(\"").skip(1) {
        let (method, rest) = chunk.split_once('"').expect("method");
        let rest = rest
            .trim_start_matches(',')
            .trim_start()
            .trim_start_matches('"');
        let (path, rest) = rest.split_once('"').expect("path");
        let access = rest
            .trim_start_matches(',')
            .trim()
            .split(')')
            .next()
            .expect("access")
            .trim();
        rules.push((method.to_owned(), path.to_owned(), access.to_owned()));
    }
    rules
}

#[test]
fn the_access_table_is_readable_and_has_every_family() {
    let rules = declared_rules();
    assert!(rules.len() >= 50, "{} rules", rules.len());
    for family in [
        "/devices/",
        "/ws/",
        "/decoder-events",
        "/profiles",
        "/bookmarks",
        "/admin/",
        "/me/sessions",
    ] {
        assert!(rules.iter().any(|(_, p, _)| p.contains(family)), "{family}");
    }
    assert!(
        rules.iter().all(
            |(_, _, a)| ["Guest", "GuestDecoder", "Listener", "Operator", "Admin"]
                .contains(&a.as_str())
        ),
        "{rules:?}"
    );
}

/// Role of the caller in the matrix.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Who {
    Anonymous,
    Guest,
    Listener,
    Operator,
    Admin,
}

fn allowed(access: &str, who: Who, guests_see_decoder: bool) -> bool {
    match (access, who) {
        (_, Who::Anonymous) => false,
        ("Guest", _) => true,
        ("GuestDecoder", Who::Guest) => guests_see_decoder,
        ("GuestDecoder", _) => true,
        (_, Who::Guest) => false,
        ("Listener", _) => true,
        ("Operator", Who::Listener) => false,
        ("Operator", _) => true,
        ("Admin", Who::Admin) => true,
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn role_matrix_over_every_declared_route() {
    let (_db, _api, port, admin) = start!("matrix");
    let operator = make_user(port, &admin, "olga", "operator");
    let listener = make_user(port, &admin, "lars", "listener");
    enable_guests(port, &admin, json!({}));
    let guest = guest_session(port).expect("guest session");

    let rules = declared_rules();
    let callers = [
        (Who::Anonymous, None),
        (Who::Guest, Some(guest.as_str())),
        (Who::Listener, Some(listener.as_str())),
        (Who::Operator, Some(operator.as_str())),
        (Who::Admin, Some(admin.as_str())),
    ];
    let mut checked = 0;
    for guests_see_decoder in [false, true] {
        // The one policy switch that changes the matrix (decoder history for guests).
        enable_guests(
            port,
            &admin,
            json!({"show_decoder_events": guests_see_decoder}),
        );
        for (method, pattern, access) in &rules {
            let path = pattern
                .replace("{dev}", "dev0")
                .replace("{id}", "1")
                .replace("{sid}", "u.zz");
            // ws/events and the rest of the table are all plain GET/POST/... requests here; the
            // bodies are invalid on purpose so an allowed call never changes anything.
            let body = ["POST", "PUT", "PATCH"]
                .contains(&method.as_str())
                .then(|| json!({"bogus": 1}));
            for (who, cookie) in callers {
                let reply = req(port, method, &path, cookie, body.as_ref());
                let ctx = format!(
                    "{method} {path} as {who:?} (guest decoder: {guests_see_decoder}): {reply:?}"
                );
                if who == Who::Anonymous {
                    assert_eq!(reply.status, 401, "{ctx}");
                } else if allowed(access, who, guests_see_decoder) {
                    assert!(reply.status != 401 && reply.status != 403, "{ctx}");
                } else {
                    assert_eq!(reply.status, 403, "{ctx}");
                    assert_eq!(reply.json()["error"], "forbidden", "{ctx}");
                }
                checked += 1;
            }
        }
    }
    assert!(checked >= 2 * 5 * 50);

    // An undeclared path is a plain 404 for signed-in callers, 401 for the rest.
    assert_eq!(
        req(port, "GET", "/api/v1/nothing/here", Some(&admin), None).status,
        404
    );
    assert_eq!(
        req(port, "GET", "/api/v1/nothing/here", None, None).status,
        401
    );

    // WebSocket upgrades go through the same check, before the upgrade. Without a bus the
    // gateways themselves answer 503, which proves the request got past the access check.
    enable_guests(port, &admin, json!({"show_decoder_events": false}));
    let ws_status = |path: &'static str, cookie: &str| {
        let url = format!("ws://127.0.0.1:{port}{path}");
        let cookie = cookie.to_owned();
        async move {
            match ws_connect_with_cookie(&url, &cookie).await {
                Err(tokio_tungstenite::tungstenite::Error::Http(r)) => r.status().as_u16(),
                other => panic!("expected an HTTP refusal for {url}, got {other:?}"),
            }
        }
    };
    for path in [
        "/api/v1/ws/spectrum?dev=dev0",
        "/api/v1/ws/audio?dev=dev0",
        "/api/v1/ws/state?dev=dev0",
        "/api/v1/ws/events?dev=dev0",
    ] {
        assert_eq!(ws_status(path, "").await, 401, "{path} anonymous");
        for (who, cookie) in [
            ("listener", &listener),
            ("operator", &operator),
            ("admin", &admin),
        ] {
            assert_eq!(ws_status(path, cookie).await, 503, "{path} as {who}");
        }
    }
    assert_eq!(ws_status("/api/v1/ws/spectrum?dev=dev0", &guest).await, 503);
    assert_eq!(ws_status("/api/v1/ws/audio?dev=dev0", &guest).await, 503);
    assert_eq!(
        ws_status("/api/v1/ws/events?dev=dev0", &guest).await,
        403,
        "decoder feed hidden from guests"
    );
    enable_guests(port, &admin, json!({"show_decoder_events": true}));
    assert_eq!(ws_status("/api/v1/ws/events?dev=dev0", &guest).await, 503);
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_mode_is_off_by_default_and_limited_when_on() {
    let (_db, _api, port, admin) = start!("guest");

    // Off: no endpoint, no session, and the status page says so.
    assert_eq!(
        req(port, "POST", "/api/v1/auth/guest", None, None).status,
        404
    );
    let status = req(port, "GET", "/api/v1/auth/status", None, None).json();
    assert_eq!(status["guest"]["enabled"], false);
    let forged = "superhet_guest=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    assert_eq!(
        req(
            port,
            "GET",
            "/api/v1/devices/dev0/state",
            Some(forged),
            None
        )
        .status,
        401
    );
    let policy = req(
        port,
        "GET",
        "/api/v1/admin/guest-policy",
        Some(&admin),
        None,
    )
    .json();
    assert_eq!(policy["guest_policy"]["enabled"], false);
    assert_eq!(
        policy["guest_policy"]["allowed_modes"],
        json!(["wfm", "nfm", "am"])
    );
    assert_eq!(policy["guest_policy"]["show_decoder_events"], false);

    // On, with tight limits.
    enable_guests(
        port,
        &admin,
        json!({"max_guests": 2, "max_guests_per_ip": 1}),
    );
    let status = req(port, "GET", "/api/v1/auth/status", None, None).json();
    assert_eq!(status["guest"]["enabled"], true);
    assert!(
        status["guest"]["privacy_text"]
            .as_str()
            .is_some_and(|t| t.contains("logged"))
    );
    let first = guest_session(port).expect("first guest");
    let me = req(port, "GET", "/api/v1/auth/status", Some(&first), None).json();
    assert_eq!(
        (
            me["authenticated"].clone(),
            me["kind"].clone(),
            me["role"].clone()
        ),
        (json!(true), json!("guest"), json!("listener"))
    );
    assert!(me["username"].is_null() && me["user_id"].is_null());
    // The same browser keeps its session; another one from the same address is refused.
    let again = req(port, "POST", "/api/v1/auth/guest", Some(&first), None);
    assert_eq!(again.status, 200);
    assert!(again.set_cookie().is_none());
    let second = req(port, "POST", "/api/v1/auth/guest", None, None);
    assert_eq!(
        (second.status, second.json()["error"].clone()),
        (429, json!("too_many_guests"))
    );

    // A guest reaches the listener surface and nothing else.
    for (method, path) in [
        ("GET", "/api/v1/admin/users"),
        ("GET", "/api/v1/admin/audit"),
        ("GET", "/api/v1/profiles"),
        ("GET", "/api/v1/bookmarks"),
        ("POST", "/api/v1/devices/dev0/tune"),
        ("POST", "/api/v1/devices/dev0/lock"),
        ("GET", "/api/v1/me/sessions"),
        ("GET", "/api/v1/decoder-events"),
    ] {
        let body = (method == "POST").then(|| json!({"center_hz": 100_000_000u64}));
        assert_eq!(
            req(port, method, path, Some(&first), body.as_ref()).status,
            403,
            "{method} {path}"
        );
    }
    // They show up in the live session list, with the truncated address.
    let sessions = req(port, "GET", "/api/v1/admin/sessions", Some(&admin), None).json();
    let guest_row = sessions["sessions"]
        .as_array()
        .expect("list")
        .iter()
        .find(|s| s["kind"] == "guest")
        .expect("guest listed")
        .clone();
    assert_eq!(guest_row["ip"], "127.0.0.0");
    assert_eq!(guest_row["user_agent"], UA);

    // Switching guest mode off ends their sessions at once.
    assert_eq!(
        put(
            port,
            &admin,
            "/api/v1/admin/guest-policy",
            &json!({"enabled": false})
        )
        .status,
        200
    );
    assert_eq!(
        req(port, "GET", "/api/v1/auth/status", Some(&first), None).json()["authenticated"],
        false
    );
    assert_eq!(
        req(port, "POST", "/api/v1/auth/guest", None, None).status,
        404
    );

    // Capacity: with room for one guest the next one is turned away.
    enable_guests(
        port,
        &admin,
        json!({"max_guests": 1, "max_guests_per_ip": 5}),
    );
    let one = guest_session(port).expect("guest");
    let full = req(port, "POST", "/api/v1/auth/guest", None, None);
    assert_eq!(
        (full.status, full.json()["error"].clone()),
        (503, json!("guests_full"))
    );
    // Logging out frees the slot.
    let out = req(port, "POST", "/api/v1/auth/logout", Some(&one), None);
    assert_eq!(out.status, 204);
    assert!(guest_session(port).is_some());

    // The policy itself is validated.
    for bad in [
        json!({"enabled": true, "allowed_modes": ["tetra"]}),
        json!({"enabled": true, "idle_timeout_secs": 5}),
        json!({"enabled": true, "max_vfos_per_guest": 0}),
        json!({"enabled": true, "freq_min_hz": 10, "freq_max_hz": 5}),
        json!({"enabled": true, "nonsense": 1}),
    ] {
        assert_eq!(
            put(port, &admin, "/api/v1/admin/guest-policy", &bad).status,
            400,
            "{bad}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn invites_create_exactly_the_invited_role_once() {
    let (db, _api, port, admin) = start!("invite");
    let made = req(
        port,
        "POST",
        "/api/v1/admin/invites",
        Some(&admin),
        Some(&json!({"role": "operator", "note": "for dave", "expires_in_hours": 2})),
    );
    assert_eq!(made.status, 201, "{made:?}");
    let token = made.json()["token"].as_str().expect("token").to_owned();
    assert_eq!(made.json()["path"], format!("/invite/{token}"));

    // The token is shown once; the database holds only its hash.
    let pool = db.pool().await;
    let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT token_hash FROM superhet.invites")
        .fetch_all(&pool)
        .await
        .expect("hashes");
    assert_eq!(stored.len(), 1);
    assert!(
        !stored[0]
            .windows(token.len().min(32))
            .any(|w| w == token.as_bytes())
    );
    let listed = req(port, "GET", "/api/v1/admin/invites", Some(&admin), None).json();
    assert!(!listed.to_string().contains(&token));
    assert_eq!(listed["invites"][0]["state"], "open");

    let preview = req(
        port,
        "GET",
        &format!("/api/v1/auth/invites/{token}"),
        None,
        None,
    );
    assert_eq!(
        (preview.status, preview.json()["role"].clone()),
        (200, json!("operator"))
    );
    assert_eq!(
        req(port, "GET", "/api/v1/auth/invites/not-a-token", None, None).status,
        404
    );

    let accept = |name: &str, pw: &str| {
        req(
            port,
            "POST",
            "/api/v1/auth/invites/accept",
            None,
            Some(&json!({"token": token, "username": name, "password": pw})),
        )
    };
    // Bad input does not burn the invite.
    assert_eq!(accept("x y", "long enough passphrase!").status, 400);
    assert_eq!(accept("dave", "short").status, 400);
    assert_eq!(accept("ADMIN", "long enough passphrase!").status, 409);
    let ok = accept("dave", "long enough passphrase!");
    assert_eq!(ok.status, 204, "{ok:?}");
    let dave = ok.session_cookie().expect("logged in by the invite");
    let me = req(port, "GET", "/api/v1/auth/status", Some(&dave), None).json();
    assert_eq!(
        (
            me["role"].clone(),
            me["username"].clone(),
            me["must_change_password"].clone()
        ),
        (json!("operator"), json!("dave"), json!(false))
    );
    // Single use.
    assert_eq!(accept("erin", "long enough passphrase!").status, 404);
    assert_eq!(
        req(
            port,
            "GET",
            &format!("/api/v1/auth/invites/{token}"),
            None,
            None
        )
        .status,
        404
    );

    // Revocation and expiry.
    let revoked = req(
        port,
        "POST",
        "/api/v1/admin/invites",
        Some(&admin),
        Some(&json!({"role": "listener"})),
    )
    .json();
    let id = revoked["invite"]["id"].as_i64().expect("id");
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/admin/invites/{id}"),
            Some(&admin),
            None
        )
        .status,
        204
    );
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/admin/invites/{id}"),
            Some(&admin),
            None
        )
        .status,
        404
    );
    let t2 = revoked["token"].as_str().expect("token");
    assert_eq!(
        req(
            port,
            "GET",
            &format!("/api/v1/auth/invites/{t2}"),
            None,
            None
        )
        .status,
        404
    );
    let expiring = req(
        port,
        "POST",
        "/api/v1/admin/invites",
        Some(&admin),
        Some(&json!({"role": "admin"})),
    )
    .json();
    sqlx::query(
        "UPDATE superhet.invites SET expires_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(expiring["invite"]["id"].as_i64().expect("id"))
    .execute(&pool)
    .await
    .expect("backdate");
    let t3 = expiring["token"].as_str().expect("token");
    assert_eq!(
        req(
            port,
            "GET",
            &format!("/api/v1/auth/invites/{t3}"),
            None,
            None
        )
        .status,
        404
    );
    let states: Vec<String> = req(port, "GET", "/api/v1/admin/invites", Some(&admin), None).json()
        ["invites"]
        .as_array()
        .expect("list")
        .iter()
        .map(|i| i["state"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(states, ["expired", "revoked", "used"]);

    // Invite parameters are validated.
    for bad in [
        json!({"role": "root"}),
        json!({"role": "admin", "expires_in_hours": 0}),
        json!({"role": "admin", "expires_in_hours": 1000}),
        json!({"role": "admin", "x": 1}),
    ] {
        assert_eq!(
            req(
                port,
                "POST",
                "/api/v1/admin/invites",
                Some(&admin),
                Some(&bad)
            )
            .status,
            400,
            "{bad}"
        );
    }
    // An operator cannot hand out invites.
    assert_eq!(
        req(
            port,
            "POST",
            "/api/v1/admin/invites",
            Some(&dave),
            Some(&json!({"role": "listener"}))
        )
        .status,
        403
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn user_management_and_the_last_admin_rule() {
    let (_db, _api, port, admin) = start!("users");
    let bob_cookie = make_user(port, &admin, "bob", "listener");
    let users = req(port, "GET", "/api/v1/admin/users", Some(&admin), None).json();
    let rows = users["users"].as_array().expect("users");
    assert_eq!(rows.len(), 2);
    let admin_id = rows
        .iter()
        .find(|u| u["username"] == "admin")
        .expect("admin")["id"]
        .as_i64()
        .expect("id");
    let bob_id = rows.iter().find(|u| u["username"] == "bob").expect("bob")["id"]
        .as_i64()
        .expect("id");
    assert_eq!(
        rows.iter()
            .find(|u| u["username"] == "admin")
            .expect("admin")["role"],
        "admin"
    );
    assert_eq!(
        req(
            port,
            "GET",
            &format!("/api/v1/admin/users/{bob_id}"),
            Some(&admin),
            None
        )
        .json()["user"]["sessions"],
        1
    );

    // Duplicate names (case-insensitive) and bad input.
    let dup = req(
        port,
        "POST",
        "/api/v1/admin/users",
        Some(&admin),
        Some(&json!({"username": "BOB", "role": "listener"})),
    );
    assert_eq!(
        (dup.status, dup.json()["error"].clone()),
        (409, json!("username_taken"))
    );
    for bad in [
        json!({"username": "a b", "role": "listener"}),
        json!({"username": "ok", "role": "root"}),
        json!({"username": "ok", "role": "listener", "password": "short"}),
    ] {
        assert_eq!(
            req(
                port,
                "POST",
                "/api/v1/admin/users",
                Some(&admin),
                Some(&bad)
            )
            .status,
            400,
            "{bad}"
        );
    }

    // The only admin can be neither demoted, disabled nor deleted.
    let url = format!("/api/v1/admin/users/{admin_id}");
    for body in [json!({"role": "operator"}), json!({"disabled": true})] {
        let r = req(port, "PATCH", &url, Some(&admin), Some(&body));
        assert_eq!(
            (r.status, r.json()["error"].clone()),
            (409, json!("last_admin")),
            "{body}"
        );
    }
    let r = req(port, "DELETE", &url, Some(&admin), None);
    assert_eq!(
        (r.status, r.json()["error"].clone()),
        (409, json!("last_admin"))
    );
    assert_eq!(
        req(port, "PATCH", &url, Some(&admin), Some(&json!({}))).status,
        400
    );

    // Promote bob; now the first admin may step down, and the second is the last one.
    let promoted = req(
        port,
        "PATCH",
        &format!("/api/v1/admin/users/{bob_id}"),
        Some(&admin),
        Some(&json!({"role": "admin"})),
    );
    assert_eq!(promoted.status, 200, "{promoted:?}");
    // The role applies to the very next request of bob's existing session.
    assert_eq!(
        req(port, "GET", "/api/v1/admin/users", Some(&bob_cookie), None).status,
        200
    );
    assert_eq!(
        req(
            port,
            "PATCH",
            &url,
            Some(&admin),
            Some(&json!({"role": "listener"}))
        )
        .status,
        200
    );
    assert_eq!(
        req(port, "GET", "/api/v1/admin/users", Some(&admin), None).status,
        403,
        "demoted at once"
    );
    let bob_url = format!("/api/v1/admin/users/{bob_id}");
    for body in [json!({"role": "operator"}), json!({"disabled": true})] {
        assert_eq!(
            req(port, "PATCH", &bob_url, Some(&bob_cookie), Some(&body)).status,
            409,
            "{body}"
        );
    }
    assert_eq!(
        req(port, "DELETE", &bob_url, Some(&bob_cookie), None).status,
        409
    );

    // Disabling revokes sessions; the correct password then fails like an unknown user.
    assert_eq!(
        req(
            port,
            "PATCH",
            &url,
            Some(&bob_cookie),
            Some(&json!({"role": "operator"}))
        )
        .status,
        200
    );
    let mallory = make_user(port, &bob_cookie, "mallory", "operator");
    let mallory_id =
        req(port, "GET", "/api/v1/auth/status", Some(&mallory), None).json()["user_id"]
            .as_i64()
            .expect("id");
    let disable = req(
        port,
        "PATCH",
        &format!("/api/v1/admin/users/{mallory_id}"),
        Some(&bob_cookie),
        Some(&json!({"disabled": true})),
    );
    assert_eq!(disable.status, 200, "{disable:?}");
    assert_eq!(
        req(port, "GET", "/api/v1/auth/status", Some(&mallory), None).json()["authenticated"],
        false
    );
    let right = login(port, "mallory", &format!("mallory-{ADMIN_PW2}"));
    let unknown = login(port, "nobody-here", "whatever long enough password");
    assert_eq!(
        (right.status, right.body.clone()),
        (401, unknown.body.clone())
    );
    // Enabling brings the account back.
    assert_eq!(
        req(
            port,
            "PATCH",
            &format!("/api/v1/admin/users/{mallory_id}"),
            Some(&bob_cookie),
            Some(&json!({"disabled": false}))
        )
        .status,
        200
    );
    assert_eq!(
        login(port, "mallory", &format!("mallory-{ADMIN_PW2}")).status,
        204
    );

    // Password reset revokes sessions and demands a new password.
    let reset = req(
        port,
        "POST",
        &format!("/api/v1/admin/users/{mallory_id}/password"),
        Some(&bob_cookie),
        None,
    );
    assert_eq!(reset.status, 200, "{reset:?}");
    let temp = reset.json()["password"]
        .as_str()
        .expect("generated")
        .to_owned();
    assert_eq!(
        req(port, "GET", "/api/v1/auth/status", Some(&mallory), None).json()["authenticated"],
        false
    );
    let again = login(port, "mallory", &temp)
        .session_cookie()
        .expect("login with the temporary password");
    assert_eq!(
        req(port, "GET", "/api/v1/auth/status", Some(&again), None).json()["must_change_password"],
        true
    );

    // Deleting removes the account and its sessions.
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/admin/users/{mallory_id}"),
            Some(&bob_cookie),
            None
        )
        .status,
        204
    );
    assert_eq!(
        req(port, "GET", "/api/v1/auth/status", Some(&again), None).json()["authenticated"],
        false
    );
    assert_eq!(login(port, "mallory", &temp).status, 401);
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/admin/users/{mallory_id}"),
            Some(&bob_cookie),
            None
        )
        .status,
        404
    );

    // Users see and revoke their own sessions.
    let second = login(port, "bob", &format!("bob-{ADMIN_PW2}"))
        .session_cookie()
        .expect("second login");
    let mine = req(port, "GET", "/api/v1/me/sessions", Some(&second), None).json();
    let list = mine["sessions"].as_array().expect("sessions");
    assert_eq!(list.len(), 2);
    let current = list.iter().find(|s| s["current"] == true).expect("current");
    let other = list.iter().find(|s| s["current"] == false).expect("other");
    assert!(current.get("user_id").is_none());
    let sid = other["sid"].as_str().expect("sid");
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/me/sessions/{sid}"),
            Some(&second),
            None
        )
        .status,
        204
    );
    assert_eq!(
        req(port, "GET", "/api/v1/auth/status", Some(&bob_cookie), None).json()["authenticated"],
        false
    );
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/me/sessions/{sid}"),
            Some(&second),
            None
        )
        .status,
        404
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn login_hardening_limits_addresses_and_names() {
    let (_db, _api, port, admin) = start!("harden", |c| {
        c.env("SUPERHET_TRUST_PROXY", "true");
    });
    let _ = make_user(port, &admin, "lars", "listener");
    let from = |ip: &str, user: &str, pw: &str| {
        http(
            port,
            "POST",
            "/api/v1/auth/login",
            &[
                ("Content-Type", "application/json"),
                ("X-Forwarded-For", ip),
            ],
            &json!({"username": user, "password": pw}).to_string(),
        )
    };
    let good = format!("lars-{ADMIN_PW2}");

    // One address: five failures against different names block it, even with a good password.
    for i in 0..5 {
        assert_eq!(
            from("198.51.100.1", &format!("victim{i}"), "wrong password here").status,
            401
        );
    }
    let blocked = from("198.51.100.1", "lars", &good);
    assert_eq!(
        (blocked.status, blocked.json()["error"].clone()),
        (429, json!("rate_limited"))
    );
    assert!(blocked.header("retry-after").is_some());

    // One name, many addresses: ten failures lock that name for everybody, others are fine.
    for i in 0..10 {
        let ip = format!("203.0.113.{}", 10 + i);
        assert_eq!(from(&ip, "Lars", "wrong password here").status, 401);
    }
    assert_eq!(
        from("203.0.113.200", "lars", &good).status,
        429,
        "the name is locked"
    );
    assert_eq!(
        from("203.0.113.200", "admin", ADMIN_PASSWORD).status,
        204,
        "other names are not"
    );

    // Disabled accounts: same answer as an unknown user, and the same cost class (a password
    // verification happens either way, so there is no cheap early exit to time).
    let _ = make_user(port, &admin, "quentin", "listener");
    let quentin_id = req(port, "GET", "/api/v1/admin/users", Some(&admin), None).json()["users"]
        .as_array()
        .expect("users")
        .iter()
        .find(|u| u["username"] == "quentin")
        .expect("quentin")["id"]
        .as_i64()
        .expect("id");
    let off = req(
        port,
        "PATCH",
        &format!("/api/v1/admin/users/{quentin_id}"),
        Some(&admin),
        Some(&json!({"disabled": true})),
    );
    assert_eq!(off.status, 200);
    let median = |user: &str, pw: &str, ip_base: u8| {
        let mut times: Vec<Duration> = (0..3u8)
            .map(|i| {
                let start = Instant::now();
                let r = from(&format!("192.0.2.{}", ip_base + i), user, pw);
                assert_eq!(r.status, 401);
                start.elapsed()
            })
            .collect();
        times.sort();
        times[1]
    };
    let unknown_time = median("nobody-at-all", "irrelevant password!", 20);
    let disabled_time = median("quentin", &format!("quentin-{ADMIN_PW2}"), 30);
    let (fast, slow) = if disabled_time < unknown_time {
        (disabled_time, unknown_time)
    } else {
        (unknown_time, disabled_time)
    };
    assert!(
        fast * 3 > slow,
        "disabled {disabled_time:?} vs unknown {unknown_time:?}"
    );
    let a = from("192.0.2.99", "quentin", &format!("quentin-{ADMIN_PW2}"));
    let b = from("192.0.2.98", "nobody-at-all", "irrelevant password!");
    assert_eq!((a.status, a.body), (b.status, b.body));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_audit_log_records_each_action_once_with_actor_role_ip_and_agent() {
    let (_db, _api, port, admin) = start!("audit");
    // Setup itself.
    let setup = one(port, &admin, "auth.setup");
    assert_eq!(
        (
            setup["actor_name"].clone(),
            setup["role"].clone(),
            setup["outcome"].clone()
        ),
        (json!("admin"), json!("admin"), json!("ok"))
    );

    // Login, failed login, logout.
    assert_eq!(login(port, "admin", "not the password!!").status, 401);
    let again = login(port, "admin", ADMIN_PASSWORD)
        .session_cookie()
        .expect("cookie");
    assert_eq!(
        req(port, "POST", "/api/v1/auth/logout", Some(&again), None).status,
        204
    );
    let failed = one(port, &admin, "auth.login_failed");
    assert_eq!(
        (
            failed["outcome"].clone(),
            failed["actor_name"].clone(),
            failed["actor_id"].clone()
        ),
        (json!("denied"), json!("admin"), Value::Null)
    );
    assert_eq!(failed["details"]["reason"], "bad_credentials");
    let login_row = one(port, &admin, "auth.login");
    assert_eq!(login_row["actor_kind"], "user");
    assert_eq!(login_row["role"], "admin");
    assert_eq!(login_row["ip"], "127.0.0.1");
    assert_eq!(login_row["user_agent"], UA);
    assert_eq!(login_row["outcome"], "ok");
    one(port, &admin, "auth.logout");

    // User and invite management.
    let eve = make_user(port, &admin, "eve", "listener");
    let created = one(port, &admin, "user.create");
    assert_eq!(
        (
            created["target"].clone(),
            created["details"]["role"].clone()
        ),
        (json!("eve"), json!("listener"))
    );
    let eve_id = req(port, "GET", "/api/v1/auth/status", Some(&eve), None).json()["user_id"]
        .as_i64()
        .expect("id");
    let eve_url = format!("/api/v1/admin/users/{eve_id}");
    assert_eq!(
        req(
            port,
            "PATCH",
            &eve_url,
            Some(&admin),
            Some(&json!({"role": "operator"}))
        )
        .status,
        200
    );
    let change = one(port, &admin, "user.role_change");
    assert_eq!(
        (
            change["details"]["from"].clone(),
            change["details"]["to"].clone()
        ),
        (json!("listener"), json!("operator"))
    );
    assert_eq!(
        req(
            port,
            "PATCH",
            &eve_url,
            Some(&admin),
            Some(&json!({"disabled": true}))
        )
        .status,
        200
    );
    one(port, &admin, "user.disable");
    assert_eq!(
        req(
            port,
            "PATCH",
            &eve_url,
            Some(&admin),
            Some(&json!({"disabled": false}))
        )
        .status,
        200
    );
    one(port, &admin, "user.enable");
    assert_eq!(
        req(
            port,
            "POST",
            &format!("{eve_url}/password"),
            Some(&admin),
            Some(&json!({"password": "a brand new long password"}))
        )
        .status,
        204
    );
    let reset = one(port, &admin, "user.password_reset");
    assert!(
        !reset.to_string().contains("brand new"),
        "passwords never reach the log"
    );
    let admin_id = req(port, "GET", "/api/v1/auth/status", Some(&admin), None).json()["user_id"]
        .as_i64()
        .expect("id");
    // A refused change is audited as denied (and is not a second success row).
    assert_eq!(
        req(
            port,
            "PATCH",
            &format!("/api/v1/admin/users/{admin_id}"),
            Some(&admin),
            Some(&json!({"role": "listener"}))
        )
        .status,
        409
    );
    let denied = audit(port, &admin, "action=user.role_change&outcome=denied");
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0]["details"]["reason"], "last_admin");
    assert_eq!(audit(port, &admin, "action=user.role_change").len(), 2);
    let invite = req(
        port,
        "POST",
        "/api/v1/admin/invites",
        Some(&admin),
        Some(&json!({"role": "listener"})),
    )
    .json();
    one(port, &admin, "invite.create");
    let invite_id = invite["invite"]["id"].as_i64().expect("id");
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/admin/invites/{invite_id}"),
            Some(&admin),
            None
        )
        .status,
        204
    );
    one(port, &admin, "invite.revoke");
    let invite2 = req(
        port,
        "POST",
        "/api/v1/admin/invites",
        Some(&admin),
        Some(&json!({"role": "listener"})),
    )
    .json();
    let accepted = req(
        port,
        "POST",
        "/api/v1/auth/invites/accept",
        None,
        Some(
            &json!({"token": invite2["token"], "username": "fay", "password": "long enough passphrase!"}),
        ),
    );
    assert_eq!(accepted.status, 204);
    assert_eq!(one(port, &admin, "invite.accept")["actor_name"], "fay");
    assert_eq!(
        req(port, "DELETE", &eve_url, Some(&admin), None).status,
        204
    );
    let deleted = one(port, &admin, "user.delete");
    assert_eq!(deleted["target"], "eve");
    // Eve's earlier rows stay, with the name pseudonymised.
    let eve_rows = audit(port, &admin, &format!("actor={eve_id}"));
    assert!(!eve_rows.is_empty());
    assert!(
        eve_rows
            .iter()
            .all(|e| e["actor_name"] == format!("deleted-user-{eve_id}")),
        "{eve_rows:?}"
    );

    // Settings, guests, kick, denied access, export.
    assert_eq!(
        put(
            port,
            &admin,
            "/api/v1/admin/limits",
            &json!({"listener_max_vfos": 3, "operator_max_vfos": 0})
        )
        .status,
        200
    );
    assert_eq!(
        put(
            port,
            &admin,
            "/api/v1/admin/audit-settings",
            &json!({"retention_user_days": 60})
        )
        .status,
        200
    );
    let settings = audit(port, &admin, "action=settings.change");
    assert_eq!(settings.len(), 2);
    enable_guests(port, &admin, json!({"max_guests": 3}));
    let policy = one(port, &admin, "guest.policy_change");
    assert!(
        policy["details"]["changed"]
            .as_array()
            .expect("changed")
            .iter()
            .any(|k| k == "enabled")
    );
    let guest = guest_session(port).expect("guest");
    let started = one(port, &admin, "guest.session_start");
    assert_eq!(
        (
            started["actor_kind"].clone(),
            started["role"].clone(),
            started["ip"].clone()
        ),
        (json!("guest"), json!("listener"), json!("127.0.0.0"))
    );
    // Guests are denied the admin surface; repeated attempts are coalesced into one row.
    for _ in 0..3 {
        assert_eq!(
            req(port, "GET", "/api/v1/admin/users", Some(&guest), None).status,
            403
        );
    }
    let sessions = req(port, "GET", "/api/v1/admin/sessions", Some(&admin), None).json();
    let sid = sessions["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|s| s["kind"] == "guest")
        .expect("guest")["sid"]
        .as_str()
        .expect("sid")
        .to_owned();
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/admin/sessions/{sid}"),
            Some(&admin),
            None
        )
        .status,
        204
    );
    assert_eq!(
        one(port, &admin, "session.kick")["details"]["kind"],
        "guest"
    );
    assert_eq!(
        req(port, "GET", "/api/v1/auth/status", Some(&guest), None).json()["authenticated"],
        false
    );

    let deadline = Instant::now() + Duration::from_secs(6);
    let denied_rows = loop {
        let rows = audit(port, &admin, "action=access.denied");
        if !rows.is_empty() || Instant::now() > deadline {
            break rows;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(
        denied_rows.len(),
        1,
        "three refusals in a burst, one row: {denied_rows:?}"
    );
    assert_eq!(denied_rows[0]["details"]["merged"], 3);
    assert_eq!(denied_rows[0]["outcome"], "denied");
    assert_eq!(denied_rows[0]["target"], "GET /api/v1/admin/users");

    let csv = req(
        port,
        "GET",
        "/api/v1/admin/audit/export?format=csv&action=user.*",
        Some(&admin),
        None,
    );
    assert_eq!(csv.status, 200);
    assert!(
        csv.header("content-type")
            .is_some_and(|t| t.starts_with("text/csv"))
    );
    assert!(
        csv.header("content-disposition")
            .is_some_and(|d| d.contains("attachment"))
    );
    let lines: Vec<&str> = csv.body.trim_end().split("\r\n").collect();
    assert!(lines[0].starts_with("id,time,actor_kind"));
    let in_table = audit(port, &admin, "action=user.*");
    assert_eq!(lines.len() - 1, in_table.len(), "CSV matches the table");
    let json_export = req(
        port,
        "GET",
        "/api/v1/admin/audit/export?format=json&action=user.*",
        Some(&admin),
        None,
    )
    .json();
    assert_eq!(
        json_export["events"].as_array().map(Vec::len),
        Some(in_table.len())
    );
    // The export is audited itself, once per export.
    assert_eq!(audit(port, &admin, "action=audit.export").len(), 2);
    assert_eq!(
        audit(port, &admin, "action=audit.export")[1]["details"]["format"],
        "csv"
    );

    // Filters: by role, outcome, ip network, time range, paging.
    assert!(!audit(port, &admin, "role=admin&outcome=ok").is_empty());
    assert!(audit(port, &admin, "ip=127.0.0.0/8").len() >= 5);
    assert!(audit(port, &admin, "ip=10.0.0.0/8").is_empty());
    assert!(audit(port, &admin, "from_ms=4102444800000").is_empty());
    let page1 = req(
        port,
        "GET",
        "/api/v1/admin/audit?limit=5",
        Some(&admin),
        None,
    )
    .json();
    assert_eq!(page1["events"].as_array().map(Vec::len), Some(5));
    let next = page1["next_before_id"].as_i64().expect("cursor");
    let page2 = req(
        port,
        "GET",
        &format!("/api/v1/admin/audit?limit=5&before_id={next}"),
        Some(&admin),
        None,
    )
    .json();
    assert!(page2["events"][0]["id"].as_i64().expect("id") < next);
    for bad in [
        "limit=0",
        "limit=5000",
        "outcome=maybe",
        "ip=banana",
        "actor_kind=robot",
        "bogus=1",
        "format=xml",
    ] {
        let path = if bad.starts_with("format") {
            "/api/v1/admin/audit/export"
        } else {
            "/api/v1/admin/audit"
        };
        assert_eq!(
            req(port, "GET", &format!("{path}?{bad}"), Some(&admin), None).status,
            400,
            "{bad}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stored_addresses_follow_the_proxy_header_only_when_trusted_and_the_ip_mode() {
    // Trusted proxy: the forwarded address is stored (full for users, truncated for guests).
    {
        let (_db, _api, port, admin) = start!("ip_trust", |c| {
            c.env("SUPERHET_TRUST_PROXY", "true");
        });
        let reply = http(
            port,
            "POST",
            "/api/v1/auth/login",
            &[
                ("Content-Type", "application/json"),
                ("X-Forwarded-For", "203.0.113.77"),
                ("User-Agent", UA),
            ],
            &json!({"username": "admin", "password": ADMIN_PASSWORD}).to_string(),
        );
        assert_eq!(reply.status, 204);
        assert_eq!(one(port, &admin, "auth.login")["ip"], "203.0.113.77");
        enable_guests(port, &admin, json!({}));
        let guest = http(
            port,
            "POST",
            "/api/v1/auth/guest",
            &[("X-Forwarded-For", "2001:db8:abcd:1234:5678::9")],
            "",
        );
        assert_eq!(guest.status, 200);
        assert_eq!(
            one(port, &admin, "guest.session_start")["ip"],
            "2001:db8:abcd::",
            "IPv6 guests are cut to /48"
        );
        let guest4 = http(
            port,
            "POST",
            "/api/v1/auth/guest",
            &[("X-Forwarded-For", "198.51.100.9")],
            "",
        );
        assert_eq!(guest4.status, 200);
        let starts = audit(port, &admin, "action=guest.session_start");
        assert!(
            starts.iter().any(|e| e["ip"] == "198.51.100.0"),
            "IPv4 guests are cut to /24: {starts:?}"
        );
    }
    // No trusted proxy: the header is ignored and the real peer is stored; the admin switches
    // the modes.
    {
        let (_db, _api, port, admin) = start!("ip_plain");
        let reply = http(
            port,
            "POST",
            "/api/v1/auth/login",
            &[
                ("Content-Type", "application/json"),
                ("X-Forwarded-For", "203.0.113.77"),
            ],
            &json!({"username": "admin", "password": ADMIN_PASSWORD}).to_string(),
        );
        assert_eq!(reply.status, 204);
        assert_eq!(
            audit(port, &admin, "action=auth.login")[0]["ip"],
            "127.0.0.1"
        );
        let settings = put(
            port,
            &admin,
            "/api/v1/admin/audit-settings",
            &json!({"ip_mode_user": "truncated", "ip_mode_guest": "none"}),
        );
        assert_eq!(settings.status, 200, "{settings:?}");
        assert_eq!(login(port, "admin", ADMIN_PASSWORD).status, 204);
        let rows = audit(port, &admin, "action=auth.login");
        assert_eq!(
            (rows[0]["ip"].clone(), rows[1]["ip"].clone()),
            (json!("127.0.0.0"), json!("127.0.0.1"))
        );
        enable_guests(port, &admin, json!({}));
        assert!(guest_session(port).is_some());
        assert_eq!(one(port, &admin, "guest.session_start")["ip"], Value::Null);
        // Settings are validated and readable, with the table statistics for the admin page.
        assert_eq!(
            put(
                port,
                &admin,
                "/api/v1/admin/audit-settings",
                &json!({"retention_user_days": 99999})
            )
            .status,
            400
        );
        assert_eq!(
            put(
                port,
                &admin,
                "/api/v1/admin/audit-settings",
                &json!({"ip_mode_user": "sometimes"})
            )
            .status,
            400
        );
        let got = req(
            port,
            "GET",
            "/api/v1/admin/audit-settings",
            Some(&admin),
            None,
        )
        .json();
        assert_eq!(got["audit_settings"]["ip_mode_guest"], "none");
        assert_eq!(got["audit_settings"]["retention_guest_days"], 7);
        assert!(got["stats"]["count"].as_i64().expect("count") > 3);
        assert!(
            got["stats"]["oldest_ms"].is_i64()
                && got["stats"]["bytes"].as_i64().expect("bytes") > 0
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_never_changes_or_deletes_audit_rows() {
    let (db, _api, port, admin) = start!("append");
    let before = audit(port, &admin, "").len();
    assert!(before >= 1);
    let pool = db.pool().await;
    for sql in [
        "UPDATE superhet.audit_events SET outcome = 'error'",
        "DELETE FROM superhet.audit_events",
    ] {
        let err = sqlx::query(sql).execute(&pool).await.expect_err(sql);
        assert!(err.to_string().contains("append-only"), "{err}");
    }
    assert!(audit(port, &admin, "").len() >= before);
    // Old password change flows still work with the new status fields.
    let changed = req(
        port,
        "POST",
        "/api/v1/auth/password",
        Some(&admin),
        Some(&json!({"current_password": ADMIN_PASSWORD, "new_password": ADMIN_PW2})),
    );
    assert_eq!(changed.status, 204);
    assert_eq!(one(port, &admin, "auth.password_change")["outcome"], "ok");
}
