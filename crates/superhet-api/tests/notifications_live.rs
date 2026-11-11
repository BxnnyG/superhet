//! Notification rules, targets, delivery log and push subscriptions of the real `superhet-api`
//! binary on a real PostgreSQL (needs `DATABASE_URL`; skipped otherwise): validation, roles,
//! write-only secrets, audit without secrets, test sends queued for the worker.

mod common;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use common::*;
use serde_json::{Value, json};

const SECRET: &str = "whsec_SUPER-SECRET-VALUE";
const URL_TOKEN: &str = "T0K3N-IN-THE-URL";

fn spawn(db_url: &str) -> (Proc, u16) {
    spawn_api(db_url, |c| {
        c.env_remove("SUPERHET_NATS_URL");
    })
}

fn browser_keys() -> (String, String) {
    // Any 65-byte point starting with 0x04 and 16 auth bytes pass the shape check at subscribe time.
    let mut point = vec![4u8];
    point.extend((1..=64u8).collect::<Vec<_>>());
    (B64.encode(point), B64.encode([7u8; 16]))
}

#[tokio::test(flavor = "multi_thread")]
async fn targets_never_give_their_secrets_back() {
    let Some(db) = TestDb::create("notify_targets").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (_api, port) = spawn(&db.url);
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    let operator = make_user(port, &admin, "olga", "operator");

    // Operators cannot touch any of it.
    for (method, path) in [
        ("GET", "/api/v1/admin/notification-targets"),
        ("GET", "/api/v1/admin/notification-rules"),
        ("GET", "/api/v1/admin/notification-deliveries"),
    ] {
        assert_eq!(
            req(port, method, path, Some(&operator), None).status,
            403,
            "{path}"
        );
    }
    assert_eq!(
        req(
            port,
            "GET",
            "/api/v1/admin/notification-targets",
            None,
            None
        )
        .status,
        401
    );

    // Validation.
    let create = |body: Value| {
        req(
            port,
            "POST",
            "/api/v1/admin/notification-targets",
            Some(&admin),
            Some(&body),
        )
    };
    for body in [
        json!({"name": "x", "kind": "sms", "config": {}}),
        json!({"name": "x", "kind": "webhook", "config": {"url": "http://127.0.0.1:9/"}}),
        json!({"name": "x", "kind": "webhook", "config": {"url": "http://169.254.169.254/"}}),
        json!({"name": "x", "kind": "webhook", "config": {"url": "ftp://example.com/"}}),
        json!({"name": "x", "kind": "webhook", "config": {"url": "https://example.com/", "sign": true}}),
        json!({"name": "x", "kind": "email", "config": {"to": []}, "secret": "nope"}),
        json!({"name": "", "kind": "push", "config": {}}),
        json!({"name": "x", "kind": "push", "config": {}, "extra": 1}),
    ] {
        let reply = create(body.clone());
        assert_eq!(
            (reply.status, reply.json()["error"].clone()),
            (400, json!("invalid")),
            "{body}: {reply:?}"
        );
    }

    let url = format!("https://hooks.example.com/services/{URL_TOKEN}");
    let created = create(json!({
        "name": "Ops hook", "kind": "webhook",
        "config": {"url": url, "sign": true, "headers": [{"name": "Authorization", "value": "Bearer {{secret}}"}]},
        "secret": SECRET,
    }));
    assert_eq!(created.status, 201, "{created:?}");
    let target = created.json()["target"].clone();
    let id = target["id"].as_i64().expect("id");
    assert_eq!(target["has_secret"], true);
    assert_eq!(target["config"]["url_host"], "https://hooks.example.com");
    assert_eq!(
        create(json!({"name": "Ops hook", "kind": "push", "config": {}})).json()["error"],
        "name_taken"
    );

    // Nothing returned anywhere contains the secret or the URL path.
    let listed = req(
        port,
        "GET",
        "/api/v1/admin/notification-targets",
        Some(&admin),
        None,
    );
    for text in [created.body.clone(), listed.body.clone()] {
        assert!(
            !text.contains(SECRET) && !text.contains(URL_TOKEN),
            "{text}"
        );
    }

    // Updates keep the URL and the secret unless told otherwise.
    let kept = req(
        port,
        "PUT",
        &format!("/api/v1/admin/notification-targets/{id}"),
        Some(&admin),
        Some(&json!({"name": "Ops hook 2", "config": {"sign": true, "retries": 1}})),
    );
    assert_eq!(kept.status, 200, "{kept:?}");
    assert_eq!(kept.json()["target"]["has_secret"], true);
    assert_eq!(kept.json()["target"]["config"]["retries"], 1);
    // Clearing the secret while signing is on is refused; turning signing off first works.
    let cleared = req(
        port,
        "PUT",
        &format!("/api/v1/admin/notification-targets/{id}"),
        Some(&admin),
        Some(&json!({"name": "Ops hook 2", "config": {"sign": true}, "secret": null})),
    );
    assert_eq!(cleared.status, 400, "{cleared:?}");
    let off = req(
        port,
        "PUT",
        &format!("/api/v1/admin/notification-targets/{id}"),
        Some(&admin),
        Some(&json!({"name": "Ops hook 2", "config": {"sign": false}, "secret": null})),
    );
    assert_eq!(off.status, 200, "{off:?}");
    assert_eq!(off.json()["target"]["has_secret"], false);
    // The kind cannot change.
    let kind = req(
        port,
        "PUT",
        &format!("/api/v1/admin/notification-targets/{id}"),
        Some(&admin),
        Some(&json!({"name": "x", "kind": "email", "config": {"to": ["a@example.com"]}})),
    );
    assert_eq!(kind.status, 400);

    // The audit log names what happened but holds no secret and no URL.
    let events = audit(port, &admin, "");
    let text = serde_json::to_string(&events).expect("json");
    assert!(
        !text.contains(SECRET) && !text.contains(URL_TOKEN) && !text.contains("hooks.example.com"),
        "{text}"
    );
    assert_eq!(
        one(port, &admin, "notification.target.create")["target"],
        "Ops hook"
    );
    assert!(
        events
            .iter()
            .any(|e| e["action"] == "notification.target.update")
    );

    // A test send is queued for the worker and shows in the log as pending.
    let test = req(
        port,
        "POST",
        &format!("/api/v1/admin/notification-targets/{id}/test"),
        Some(&admin),
        None,
    );
    assert_eq!(test.status, 202, "{test:?}");
    let log = req(
        port,
        "GET",
        "/api/v1/admin/notification-deliveries",
        Some(&admin),
        None,
    )
    .json();
    assert_eq!(log["deliveries"][0]["status"], "pending");
    assert_eq!(log["deliveries"][0]["target_name"], "Ops hook 2");
    assert_eq!(log["deliveries"][0]["rule_name"], "(test)");
    assert_eq!(
        req(
            port,
            "GET",
            "/api/v1/admin/notification-deliveries?status=bogus",
            Some(&admin),
            None
        )
        .status,
        400
    );

    let gone = req(
        port,
        "DELETE",
        &format!("/api/v1/admin/notification-targets/{id}"),
        Some(&admin),
        None,
    );
    assert_eq!(gone.status, 204);
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/admin/notification-targets/{id}"),
            Some(&admin),
            None
        )
        .status,
        404
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_are_validated_stored_and_tested() {
    let Some(db) = TestDb::create("notify_rules").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (_api, port) = spawn(&db.url);
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    let target = req(
        port,
        "POST",
        "/api/v1/admin/notification-targets",
        Some(&admin),
        Some(&json!({"name": "mail", "kind": "email", "config": {"to": ["ops@example.com"]}})),
    )
    .json()["target"]["id"]
        .as_i64()
        .expect("target");

    let rule = |body: Value| {
        req(
            port,
            "POST",
            "/api/v1/admin/notification-rules",
            Some(&admin),
            Some(&body),
        )
    };
    let good = json!({
        "name": "Fire pager", "source": "org.superhet.decoder.pocsag", "kind": "pocsag.message",
        "conditions": [
            {"field": "data.address", "op": "equals", "value": [1234, 5678]},
            {"field": "value", "op": "regex", "value": "fire|brand"},
        ],
        "cooldown_secs": 120, "max_per_hour": 20,
        "quiet": {"start": "22:00", "end": "06:30", "tz": "Europe/Berlin"},
        "title_template": "{{rule}}: RIC {{data.address}}", "body_template": "{{data.text}}",
        "target_ids": [target],
    });
    for (field, value) in [
        (
            "conditions",
            json!([{"field": "x", "op": "regex", "value": "("}]),
        ),
        (
            "conditions",
            json!([{"field": "x", "op": "nope", "value": "a"}]),
        ),
        (
            "conditions",
            json!([{"field": "x", "op": "regex", "value": "a".repeat(300)}]),
        ),
        (
            "quiet",
            json!({"start": "25:00", "end": "06:00", "tz": "Europe/Berlin"}),
        ),
        (
            "quiet",
            json!({"start": "22:00", "end": "22:00", "tz": "Europe/Berlin"}),
        ),
        (
            "quiet",
            json!({"start": "22:00", "end": "06:00", "tz": "Mars/Olympus"}),
        ),
        ("title_template", json!("{{broken")),
        ("body_template", json!("{{ a | b }}")),
        ("target_ids", json!([424_242])),
        ("cooldown_secs", json!(-1)),
        ("max_per_hour", json!(0)),
        ("kind", json!("Not A Kind")),
        ("name", json!("")),
    ] {
        let mut body = good.clone();
        body[field] = value.clone();
        let reply = rule(body);
        assert_eq!(
            (reply.status, reply.json()["error"].clone()),
            (400, json!("invalid")),
            "{field}={value}: {reply:?}"
        );
    }

    let created = rule(good.clone());
    assert_eq!(created.status, 201, "{created:?}");
    let stored = created.json()["rule"].clone();
    let id = stored["id"].as_i64().expect("id");
    assert_eq!(
        stored["quiet"],
        json!({"start": "22:00", "end": "06:30", "tz": "Europe/Berlin"})
    );
    assert_eq!(stored["target_ids"], json!([target]));
    assert_eq!(stored["conditions"][0]["value"], json!([1234, 5678]));
    assert_eq!(rule(good.clone()).json()["error"], "name_taken");

    let mut changed = good.clone();
    changed["enabled"] = json!(false);
    changed["quiet"] = Value::Null;
    let updated = req(
        port,
        "PUT",
        &format!("/api/v1/admin/notification-rules/{id}"),
        Some(&admin),
        Some(&changed),
    );
    assert_eq!(updated.status, 200, "{updated:?}");
    assert_eq!(updated.json()["rule"]["enabled"], false);
    assert!(updated.json()["rule"]["quiet"].is_null());

    let test = req(
        port,
        "POST",
        &format!("/api/v1/admin/notification-rules/{id}/test"),
        Some(&admin),
        None,
    );
    assert_eq!(test.status, 202, "{test:?}");
    let log = req(
        port,
        "GET",
        "/api/v1/admin/notification-deliveries",
        Some(&admin),
        None,
    )
    .json();
    assert_eq!(log["deliveries"][0]["rule_name"], "Fire pager (test)");
    assert!(
        log["deliveries"][0]["title"]
            .as_str()
            .is_some_and(|t| t.starts_with("[test] Fire pager: RIC"))
    );

    // A rule without targets cannot be tested.
    let mut empty = good;
    empty["name"] = json!("lonely");
    empty["target_ids"] = json!([]);
    let lonely = rule(empty).json()["rule"]["id"].as_i64().expect("id");
    let none = req(
        port,
        "POST",
        &format!("/api/v1/admin/notification-rules/{lonely}/test"),
        Some(&admin),
        None,
    );
    assert_eq!(
        (none.status, none.json()["error"].clone()),
        (409, json!("no_targets"))
    );

    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/admin/notification-rules/{id}"),
            Some(&admin),
            None
        )
        .status,
        204
    );
    let rules = req(
        port,
        "GET",
        "/api/v1/admin/notification-rules",
        Some(&admin),
        None,
    )
    .json();
    assert_eq!(rules["rules"].as_array().map(Vec::len), Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn accounts_manage_their_own_push_subscription() {
    let Some(db) = TestDb::create("notify_push").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (_api, port) = spawn(&db.url);
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    let listener = make_user(port, &admin, "lena", "listener");
    let other = make_user(port, &admin, "otto", "listener");
    enable_guests(port, &admin, json!({}));

    // Guests and anonymous callers have no subscription.
    assert_eq!(req(port, "GET", "/api/v1/me/push", None, None).status, 401);
    if let Some(guest) = guest_session(port) {
        assert_eq!(
            req(port, "GET", "/api/v1/me/push", Some(&guest), None).status,
            403
        );
    }

    let me = req(port, "GET", "/api/v1/me/push", Some(&listener), None);
    assert_eq!(me.status, 200, "{me:?}");
    let public_key = me.json()["public_key"].as_str().expect("key").to_owned();
    assert_eq!(B64.decode(&public_key).map(|k| k.len()).ok(), Some(65));
    assert!(
        me.body.clone().find("private").is_none(),
        "no private key in the reply"
    );
    // The key is stable.
    assert_eq!(
        req(port, "GET", "/api/v1/me/push", Some(&other), None).json()["public_key"],
        public_key
    );

    let (p256dh, auth) = browser_keys();
    let subscribe = |cookie: &str, endpoint: &str| {
        req(
            port,
            "POST",
            "/api/v1/me/push/subscriptions",
            Some(cookie),
            Some(
                &json!({"endpoint": endpoint, "keys": {"p256dh": p256dh, "auth": auth}, "label": "Firefox"}),
            ),
        )
    };
    for bad in ["http://push.example/x", "https://127.0.0.1/x", "nonsense"] {
        assert_eq!(subscribe(&listener, bad).status, 400, "{bad}");
    }
    let bad_keys = req(
        port,
        "POST",
        "/api/v1/me/push/subscriptions",
        Some(&listener),
        Some(
            &json!({"endpoint": "https://push.example/x", "keys": {"p256dh": "AAAA", "auth": auth}}),
        ),
    );
    assert_eq!(bad_keys.status, 400);

    let made = subscribe(&listener, "https://push.example/endpoint-1");
    assert_eq!(made.status, 201, "{made:?}");
    let sub_id = made.json()["subscription"]["id"].as_i64().expect("id");
    assert_eq!(
        subscribe(&other, "https://push.example/endpoint-1").json()["error"],
        "endpoint_in_use"
    );
    let mine = req(port, "GET", "/api/v1/me/push", Some(&listener), None).json();
    assert_eq!(mine["subscriptions"][0]["label"], "Firefox");
    assert!(
        !serde_json::to_string(&mine)
            .expect("json")
            .contains("endpoint-1"),
        "the endpoint is not echoed"
    );
    assert_eq!(
        req(port, "GET", "/api/v1/me/push", Some(&other), None).json()["subscriptions"],
        json!([])
    );

    // Others cannot remove it, its owner can.
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/me/push/subscriptions/{sub_id}"),
            Some(&other),
            None
        )
        .status,
        404
    );
    assert_eq!(
        req(
            port,
            "DELETE",
            &format!("/api/v1/me/push/subscriptions/{sub_id}"),
            Some(&listener),
            None
        )
        .status,
        204
    );
}
