//! End-to-end tests of the channel plan API: the real `superhet-api` binary on a real PostgreSQL
//! (needs `DATABASE_URL`; skipped otherwise). No NATS involved.

mod common;

use common::*;
use serde_json::{Value, json};

const JSON: (&str, &str) = ("Content-Type", "application/json");

struct Client {
    port: u16,
    cookie: String,
}

impl Client {
    fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut headers = vec![("Cookie", self.cookie.as_str())];
        if !body.is_empty() {
            headers.push(JSON);
        }
        http(self.port, method, path, &headers, &body)
    }

    fn list(&self) -> Vec<Value> {
        let reply = self.send("GET", "/api/v1/channel-plans", None);
        assert_eq!(reply.status, 200, "{reply:?}");
        let body = reply.json();
        assert_eq!(body["v"], 1);
        body["plans"].as_array().expect("array").clone()
    }
}

macro_rules! start {
    ($tag:expr) => {{
        let Some(db) = TestDb::create($tag).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        let (api, port) = spawn_api(&db.url, |_| {});
        wait_ready(port, "api").await;
        let cookie = setup_admin(port);
        (db, api, Client { port, cookie })
    }};
}

const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn doc(name: &str) -> Value {
    json!({
        "v": 1, "name": name, "service": "Club", "regions": ["DE"], "licence": "licensed",
        "channels": [
            {"number": "1", "freq_hz": 145_500_000},
            {"number": "2", "freq_hz": 145_525_000, "ctcss_hz": 88.5},
        ],
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn requires_a_session() {
    let (_db, _api, client) = start!("cp_auth");
    for (method, path) in [
        ("GET", "/api/v1/channel-plans".to_owned()),
        ("POST", "/api/v1/channel-plans".to_owned()),
        ("PUT", format!("/api/v1/channel-plans/{ID}")),
        ("DELETE", format!("/api/v1/channel-plans/{ID}")),
    ] {
        let reply = http(client.port, method, &path, &[JSON], "{}");
        assert_eq!(reply.status, 401, "{method} {path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn crud_roundtrip() {
    let (_db, _api, client) = start!("cp_crud");
    assert!(client.list().is_empty());

    let reply = client.send("POST", "/api/v1/channel-plans", Some(doc("  Zebra club  ")));
    assert_eq!(reply.status, 201, "{reply:?}");
    let zebra = reply.json();
    let id = zebra["id"].as_str().expect("id").to_owned();
    assert_eq!(zebra["name"], "Zebra club");
    assert_eq!(zebra["doc"]["name"], "Zebra club");
    assert_eq!(zebra["doc"]["channels"][1]["ctcss_hz"], 88.5);
    assert_eq!(
        reply.header("location").map(str::to_owned),
        Some(format!("/api/v1/channel-plans/{id}"))
    );

    let reply = client.send("POST", "/api/v1/channel-plans", Some(doc("Alpha")));
    assert_eq!(reply.status, 201, "{reply:?}");

    // Sorted by name.
    let all = client.list();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0]["name"], "Alpha");
    assert_eq!(all[1]["id"], id.as_str());

    let reply = client.send(
        "PUT",
        &format!("/api/v1/channel-plans/{id}"),
        Some(doc("Renamed")),
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.json()["name"], "Renamed");
    assert_eq!(client.list()[1]["name"], "Renamed");

    let reply = client.send("DELETE", &format!("/api/v1/channel-plans/{id}"), None);
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(client.list().len(), 1);
    let reply = client.send("DELETE", &format!("/api/v1/channel-plans/{id}"), None);
    assert_eq!(reply.status, 404);
    let reply = client.send(
        "PUT",
        &format!("/api/v1/channel-plans/{ID}"),
        Some(doc("x")),
    );
    assert_eq!(reply.status, 404);
    let reply = client.send("DELETE", "/api/v1/channel-plans/not-a-uuid", None);
    assert_eq!(reply.status, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_invalid_documents() {
    let (_db, _api, client) = start!("cp_invalid");
    for bad in [
        json!({"v": 1, "name": "x"}),
        json!({"v": 2, "name": "x", "channels": [{"freq_hz": 1}]}),
        json!({"v": 1, "name": "", "channels": [{"freq_hz": 1}]}),
        json!({"v": 1, "name": "x", "channels": [{"freq_hz": "145.5"}]}),
        json!([]),
    ] {
        let reply = client.send("POST", "/api/v1/channel-plans", Some(bad.clone()));
        assert_eq!(reply.status, 400, "{bad}: {reply:?}");
        assert_eq!(reply.json()["code"], "invalid");
    }
    assert!(client.list().is_empty());
}
