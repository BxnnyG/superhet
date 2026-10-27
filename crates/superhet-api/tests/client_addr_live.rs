//! Peer vs. client address of the real `superhet-api` binary on a real PostgreSQL (needs
//! `DATABASE_URL`; skipped otherwise): forwarding headers are honoured only from trusted proxies
//! (`SUPERHET_TRUSTED_PROXIES`), the peer address is stored next to the client address, the chain
//! lands in the details, guests are truncated and the enrichment shows up in the lists and exports.

mod common;

use common::*;
use serde_json::{Value, json};

const CHROME: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// A request from the loopback peer carrying `X-Forwarded-For: forwarded`.
fn via(port: u16, forwarded: &str, method: &str, path: &str, body: Option<&Value>) -> Reply {
    let mut headers: Vec<(&str, &str)> =
        vec![("User-Agent", CHROME), ("X-Forwarded-For", forwarded)];
    if body.is_some() {
        headers.push(("Content-Type", "application/json"));
    }
    let text = body.map(Value::to_string).unwrap_or_default();
    http(port, method, path, &headers, &text)
}

fn login_via(port: u16, forwarded: &str) -> Reply {
    let reply = via(
        port,
        forwarded,
        "POST",
        "/api/v1/auth/login",
        Some(&json!({"username": ADMIN_USER, "password": ADMIN_PASSWORD})),
    );
    assert_eq!(reply.status, 204, "{reply:?}");
    reply
}

#[tokio::test(flavor = "multi_thread")]
async fn a_trusted_proxy_network_yields_the_forwarded_client_and_keeps_the_peer() {
    let Some(db) = TestDb::create("clientaddr").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (_api, port) = spawn_api(&db.url, |c| {
        c.env_remove("SUPERHET_NATS_URL");
        c.env_remove("SUPERHET_TRUST_PROXY");
        c.env("SUPERHET_TRUSTED_PROXIES", "127.0.0.0/8");
        c.env("SUPERHET_REVERSE_DNS", "false");
    });
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    enable_guests(port, &admin, json!({}));

    // chain "spoofed, real": the rightmost entry that is not a trusted hop wins
    let cookie = login_via(port, "6.6.6.6, 8.8.4.4")
        .session_cookie()
        .expect("cookie");
    let event = audit(port, &admin, "action=auth.login")
        .into_iter()
        .find(|e| e["ip"] == "8.8.4.4")
        .expect("login event with the forwarded address");
    assert_eq!(event["peer_addr"], "127.0.0.1", "{event}");
    assert_eq!(event["ip_info"]["class"], "public", "{event}");
    assert_eq!(event["peer_info"]["class"], "loopback", "{event}");
    assert_eq!(event["ua_info"]["browser"], "Chrome", "{event}");
    assert_eq!(event["ua_info"]["browser_version"], "131", "{event}");
    assert_eq!(event["ua_info"]["os"], "Linux", "{event}");
    assert_eq!(
        event["details"]["forwarded"],
        json!(["6.6.6.6", "8.8.4.4"]),
        "{event}"
    );

    // live sessions show client and peer
    let sessions = req(port, "GET", "/api/v1/admin/sessions", Some(&admin), None).json();
    let row = sessions["sessions"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["ip"] == "8.8.4.4"))
        .expect("session with the client address")
        .clone();
    assert_eq!(row["peer_addr"], "127.0.0.1", "{row}");
    assert_eq!(row["ua_info"]["device"], "desktop", "{row}");
    let _ = cookie;

    // guests: both addresses truncated by the default guest mode, still classified
    let guest = via(port, "9.9.9.9", "POST", "/api/v1/auth/guest", None);
    assert_eq!(guest.status, 200, "{guest:?}");
    let sessions = req(port, "GET", "/api/v1/admin/sessions", Some(&admin), None).json();
    let g = sessions["sessions"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["kind"] == "guest"))
        .expect("guest")
        .clone();
    assert_eq!(g["ip"], "9.9.9.0", "{g}");
    assert_eq!(g["peer_addr"], "127.0.0.0", "{g}");
    assert_eq!(g["ip_info"]["class"], "public", "{g}");
    assert!(g["ip_info"].get("rdns").is_none(), "{g}");
    let start = one(port, &admin, "guest.session_start");
    assert_eq!(start["ip"], "9.9.9.0", "{start}");
    assert_eq!(start["details"]["forwarded"], json!(["9.9.9.0"]), "{start}");

    // exports carry the new columns
    let csv = req(
        port,
        "GET",
        "/api/v1/admin/audit/export?action=auth.login",
        Some(&admin),
        None,
    );
    assert_eq!(csv.status, 200);
    let text = csv.body.clone();
    let header = text.lines().next().unwrap_or_default();
    assert!(
        header.ends_with(
            "peer_addr,address_type,reverse_dns,country,city,asn,as_org,browser,os,device"
        ),
        "{header}"
    );
    assert!(text.contains(",127.0.0.1,public,"), "{text}");
    assert!(text.contains("Chrome 131,Linux,desktop"), "{text}");
    let json_export = req(
        port,
        "GET",
        "/api/v1/admin/audit/export?action=auth.login&format=json",
        Some(&admin),
        None,
    )
    .json();
    assert!(json_export["events"].as_array().is_some_and(|e| {
        e.iter()
            .any(|e| e["peer_addr"] == "127.0.0.1" && e["ip_info"]["class"] == "public")
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn forwarding_headers_from_an_untrusted_peer_are_ignored() {
    let Some(db) = TestDb::create("clientaddr_spoof").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    // The test client connects from 127.0.0.1, which is not in the trusted list.
    let (_api, port) = spawn_api(&db.url, |c| {
        c.env_remove("SUPERHET_NATS_URL");
        c.env_remove("SUPERHET_TRUST_PROXY");
        c.env("SUPERHET_TRUSTED_PROXIES", "10.0.0.0/8");
        c.env("SUPERHET_REVERSE_DNS", "false");
    });
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    login_via(port, "6.6.6.6");
    let events = audit(port, &admin, "action=auth.login");
    assert!(!events.is_empty());
    for e in &events {
        assert_eq!(e["ip"], "127.0.0.1", "{e}");
        assert_eq!(e["peer_addr"], "127.0.0.1", "{e}");
        assert!(e["details"].get("forwarded").is_none(), "{e}");
    }
}
