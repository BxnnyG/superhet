//! Client-reported device info and public address on the real `superhet-api` binary and a real
//! PostgreSQL (needs `DATABASE_URL`; skipped otherwise): off by default, the policy decides who
//! is asked, reports are validated, stored with the session and shown (public address in the
//! address mode of the session class), and the audit row keeps them.

mod common;

use common::*;
use serde_json::{Value, json};

const POLICY: &str = "/api/v1/admin/client-report-policy";
const REPORT: &str = "/api/v1/auth/client-report";

fn report(public_ip: &str) -> Value {
    json!({
        "platform": "Linux",
        "platform_version": "6.12.0",
        "architecture": "x86",
        "mobile": false,
        "brands": [{"brand": "Chromium", "version": "131.0.6778.86"}],
        "screen": {"width": 2560, "height": 1440, "dpr": 1.25},
        "languages": ["de-DE", "en"],
        "timezone": "Europe/Berlin",
        "touch_points": 0,
        "cpu_cores": 8,
        "memory_gb": 8,
        "connection": {"kind": "wifi", "effective_type": "4g"},
        "public_ip": public_ip,
        "public_ip_status": "ok"
    })
}

fn set_policy(port: u16, admin: &str, body: &Value) -> Reply {
    put(port, admin, POLICY, body)
}

fn sessions(port: u16, admin: &str) -> Vec<Value> {
    let reply = req(port, "GET", "/api/v1/admin/sessions", Some(admin), None);
    assert_eq!(reply.status, 200, "{reply:?}");
    reply.json()["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_follow_the_policy_and_show_up_in_sessions_and_audit() {
    let Some(db) = TestDb::create("clientreport").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (_api, port) = spawn_api(&db.url, |c| {
        c.env_remove("SUPERHET_NATS_URL");
        c.env("SUPERHET_REVERSE_DNS", "false");
    });
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    enable_guests(port, &admin, json!({}));
    let guest = guest_session(port).expect("guest session");

    // Everything is off by default: nobody is asked, a report is refused.
    let policy = req(port, "GET", POLICY, Some(&admin), None).json();
    assert_eq!(policy["client_report"]["collect"], "off");
    assert_eq!(policy["client_report"]["ask_public_ip"], false);
    let status = req(port, "GET", "/api/v1/auth/status", Some(&admin), None).json();
    assert!(status["client_report"].is_null(), "{status}");
    let refused = req(
        port,
        "POST",
        REPORT,
        Some(&admin),
        Some(&report("198.51.100.7")),
    );
    assert_eq!(refused.status, 409, "{refused:?}");
    assert_eq!(refused.json()["error"], "disabled");

    // The service URL is validated (https only, no credentials).
    for bad in ["http://api.ipify.org", "https://u:p@x.example/", "nonsense"] {
        let reply = set_policy(
            port,
            &admin,
            &json!({"collect": "users", "ask_public_ip": true, "public_ip_service": bad}),
        );
        assert_eq!(reply.status, 400, "{bad}: {reply:?}");
    }
    assert_eq!(
        set_policy(port, &admin, &json!({"collect": "mostly"})).status,
        400
    );

    // Accounts only, with the public address.
    let ok = set_policy(
        port,
        &admin,
        &json!({"collect": "users", "ask_public_ip": true}),
    );
    assert_eq!(ok.status, 200, "{ok:?}");
    let status = req(port, "GET", "/api/v1/auth/status", Some(&admin), None).json();
    assert_eq!(
        status["client_report"]["public_ip_service"], "https://api.ipify.org?format=json",
        "{status}"
    );
    let guest_status = req(port, "GET", "/api/v1/auth/status", Some(&guest), None).json();
    assert!(guest_status["client_report"].is_null(), "{guest_status}");
    let anonymous = req(port, "GET", "/api/v1/auth/status", None, None).json();
    assert!(anonymous["guest"]["client_report"].is_null(), "{anonymous}");
    assert_eq!(
        req(
            port,
            "POST",
            REPORT,
            Some(&guest),
            Some(&report("198.51.100.8"))
        )
        .status,
        409,
        "guests are not asked under scope users"
    );

    // Validation: unknown fields, garbage, out-of-range values.
    for bad in [
        json!({"cookie": "x"}),
        json!({"public_ip": "not an ip"}),
        json!({"screen": {"width": 0, "height": 1, "dpr": 1}}),
        json!({"platform": "x".repeat(200)}),
    ] {
        let reply = req(port, "POST", REPORT, Some(&admin), Some(&bad));
        assert_eq!(reply.status, 400, "{bad}: {reply:?}");
    }

    // A good report is stored with the session and shown with the claimed public address.
    let ok = req(
        port,
        "POST",
        REPORT,
        Some(&admin),
        Some(&report("198.51.100.7")),
    );
    assert_eq!(ok.status, 204, "{ok:?}");
    let rows = sessions(port, &admin);
    let row = rows
        .iter()
        .find(|s| s["kind"] == "user")
        .expect("user session");
    let client = &row["client_report"];
    assert_eq!(client["platform"], "Linux", "{row}");
    assert_eq!(client["brands"][0]["brand"], "Chromium", "{row}");
    assert_eq!(client["screen"]["dpr"], 1.25, "{row}");
    assert_eq!(client["public_ip"], "198.51.100.7", "{row}");
    assert_eq!(client["public_ip_info"]["class"], "reserved", "{row}");
    assert!(client["reported_at_ms"].as_i64().unwrap_or(0) > 0, "{row}");
    // The server-seen address stays separate: it is what the connection really used.
    assert_eq!(row["ip"], "127.0.0.1", "{row}");
    let mine = req(port, "GET", "/api/v1/me/sessions", Some(&admin), None).json();
    assert_eq!(
        mine["sessions"][0]["client_report"]["public_ip"], "198.51.100.7",
        "{mine}"
    );

    // The audit row keeps the report for as long as audit retention allows.
    let event = one(port, &admin, "session.client_report");
    assert_eq!(event["details"]["client"]["platform"], "Linux", "{event}");
    assert_eq!(
        event["details"]["client"]["public_ip"], "198.51.100.7",
        "{event}"
    );

    // Not asking for the address drops it from reports that carry one.
    set_policy(
        port,
        &admin,
        &json!({"collect": "users", "ask_public_ip": false}),
    );
    let again = req(
        port,
        "POST",
        REPORT,
        Some(&admin),
        Some(&report("198.51.100.9")),
    );
    assert_eq!(again.status, 204);
    let rows = sessions(port, &admin);
    let client = &rows
        .iter()
        .find(|s| s["kind"] == "user")
        .expect("user session")["client_report"];
    assert!(client.get("public_ip").is_none(), "{client}");
    assert_eq!(client["platform"], "Linux");

    // Everybody, guests included: the guest's claimed address is truncated like their own.
    set_policy(
        port,
        &admin,
        &json!({"collect": "all", "ask_public_ip": true}),
    );
    let anonymous = req(port, "GET", "/api/v1/auth/status", None, None).json();
    assert_eq!(
        anonymous["guest"]["client_report"]["public_ip_service"],
        "https://api.ipify.org?format=json",
        "{anonymous}"
    );
    let ok = req(
        port,
        "POST",
        REPORT,
        Some(&guest),
        Some(&report("198.51.100.77")),
    );
    assert_eq!(ok.status, 204, "{ok:?}");
    let rows = sessions(port, &admin);
    let g = rows
        .iter()
        .find(|s| s["kind"] == "guest")
        .expect("guest session");
    assert_eq!(g["client_report"]["public_ip"], "198.51.100.0", "{g}");
    assert_eq!(g["client_report"]["timezone"], "Europe/Berlin", "{g}");
    let guest_events = audit(
        port,
        &admin,
        "action=session.client_report&actor_kind=guest",
    );
    assert_eq!(
        guest_events[0]["details"]["client"]["public_ip"], "198.51.100.0",
        "{guest_events:?}"
    );

    // With address storage off for guests the claimed address is not kept at all.
    let reply = put(
        port,
        &admin,
        "/api/v1/admin/audit-settings",
        &json!({"ip_mode_guest": "none"}),
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    let rows = sessions(port, &admin);
    let g = rows
        .iter()
        .find(|s| s["kind"] == "guest")
        .expect("guest session");
    assert!(g["client_report"].get("public_ip").is_none(), "{g}");
    assert_eq!(g["client_report"]["platform"], "Linux");

    // The policy change itself is audited.
    assert!(
        audit(port, &admin, "action=settings.change")
            .iter()
            .any(|e| e["target"] == "client_report")
    );

    // Reports never influence access: nothing about the claimed address appears in bans.
    let bans = req(port, "GET", "/api/v1/admin/bans", Some(&admin), None).json();
    assert!(bans["bans"].as_array().is_some_and(Vec::is_empty), "{bans}");
}
