//! End-to-end tests of the bookmark API: the real `superhet-api` binary on a real PostgreSQL
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
        let reply = self.send("GET", "/api/v1/bookmarks", None);
        assert_eq!(reply.status, 200, "{reply:?}");
        let body = reply.json();
        assert_eq!(body["v"], 1);
        body["bookmarks"].as_array().expect("array").clone()
    }

    fn create(&self, body: Value) -> Value {
        let reply = self.send("POST", "/api/v1/bookmarks", Some(body));
        assert_eq!(reply.status, 201, "{reply:?}");
        let created = reply.json();
        assert_eq!(
            reply.header("location").map(str::to_owned),
            Some(format!(
                "/api/v1/bookmarks/{}",
                created["id"].as_str().unwrap_or("")
            ))
        );
        created
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

fn code(reply: &Reply) -> Value {
    reply.json()["code"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn requires_a_session() {
    let (_db, _api, client) = start!("bm_auth");
    for (method, path) in [
        ("GET", "/api/v1/bookmarks"),
        ("POST", "/api/v1/bookmarks"),
        ("POST", "/api/v1/bookmarks/bulk"),
        (
            "PATCH",
            "/api/v1/bookmarks/0f8fad5b-d9cb-469f-a165-70867728950e",
        ),
        (
            "DELETE",
            "/api/v1/bookmarks/0f8fad5b-d9cb-469f-a165-70867728950e",
        ),
    ] {
        let reply = http(client.port, method, path, &[JSON], "{}");
        assert_eq!(reply.status, 401, "{method} {path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn crud_roundtrip() {
    let (_db, _api, client) = start!("bm_crud");
    assert!(client.list().is_empty());

    let high = client.create(json!({"name": "  Tower  ", "freq_hz": 118_700_000, "mode": "am"}));
    assert_eq!(high["name"], "Tower");
    assert_eq!(high["mode"], "am");
    for null in ["bandwidth_hz", "color", "notes"] {
        assert_eq!(high[null], Value::Null, "{null}");
    }
    assert_eq!(high["tags"], json!([]));
    let (created, updated) = (
        high["created_ms"].as_i64().expect("created_ms"),
        high["updated_ms"].as_i64().expect("updated_ms"),
    );
    assert!(created > 1_700_000_000_000 && created == updated);

    let low = client.create(json!({
        "name": "Radio", "freq_hz": 100_300_000, "mode": "wfm", "bandwidth_hz": 180_000,
        "tags": ["fm", " music ", "fm"], "color": "#AABBCC", "notes": "line1\nline2",
        // Read-only fields of an export are ignored.
        "id": "ignored", "created_ms": 1, "updated_ms": 2,
    }));
    assert_eq!(low["tags"], json!(["fm", "music"]));
    assert_eq!(low["color"], "#aabbcc");
    assert_eq!(low["notes"], "line1\nline2");
    assert_ne!(low["id"], "ignored");

    // Sorted by frequency.
    let all = client.list();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0]["id"], low["id"]);
    assert_eq!(all[1]["id"], high["id"]);

    // Partial update; null clears.
    let path = format!("/api/v1/bookmarks/{}", low["id"].as_str().expect("id"));
    std::thread::sleep(std::time::Duration::from_millis(5));
    let reply = client.send(
        "PATCH",
        &path,
        Some(
            json!({"freq_hz": 101_000_000, "color": null, "bandwidth_hz": 200_000, "mode": "nfm"}),
        ),
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    let patched = reply.json();
    assert_eq!(patched["freq_hz"], 101_000_000);
    assert_eq!(patched["color"], Value::Null);
    assert_eq!(patched["bandwidth_hz"], 200_000);
    assert_eq!(patched["mode"], "nfm");
    assert_eq!(patched["name"], "Radio");
    assert_eq!(patched["notes"], "line1\nline2");
    assert_eq!(patched["tags"], json!(["fm", "music"]));
    assert_eq!(patched["created_ms"], low["created_ms"]);
    assert!(patched["updated_ms"].as_i64() > low["updated_ms"].as_i64());

    let reply = client.send(
        "PATCH",
        &path,
        Some(json!({"mode": null, "bandwidth_hz": null, "notes": null, "tags": []})),
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    let cleared = reply.json();
    for null in ["mode", "bandwidth_hz", "notes", "color"] {
        assert_eq!(cleared[null], Value::Null, "{null}");
    }
    assert_eq!(cleared["tags"], json!([]));
    assert_eq!(cleared["freq_hz"], 101_000_000);

    // An empty patch is a no-op.
    assert_eq!(client.send("PATCH", &path, Some(json!({}))).status, 200);

    // Delete.
    let reply = client.send("DELETE", &path, None);
    assert_eq!(reply.status, 204, "{reply:?}");
    assert!(reply.body.is_empty());
    for (method, body) in [("DELETE", None), ("PATCH", Some(json!({"name": "x"})))] {
        let reply = client.send(method, &path, body);
        assert_eq!(reply.status, 404, "{method}");
        assert_eq!(code(&reply), "not_found");
    }
    assert_eq!(client.list().len(), 1);

    // Malformed ids are plain 404s.
    let reply = client.send("DELETE", "/api/v1/bookmarks/not-a-uuid", None);
    assert_eq!((reply.status, code(&reply)), (404, json!("not_found")));
}

#[tokio::test(flavor = "multi_thread")]
async fn validation() {
    let (_db, _api, client) = start!("bm_valid");
    let ok = json!({"name": "a", "freq_hz": 1});
    let with = |key: &str, value: Value| {
        let mut body = ok.clone();
        body[key] = value;
        body
    };
    let long_tags: Vec<String> = (0..17).map(|i| format!("t{i}")).collect();
    let bad = [
        json!({}),
        json!({"freq_hz": 1}),
        json!({"name": "a"}),
        with("name", json!("   ")),
        with("name", json!("x".repeat(101))),
        with("name", json!("bad\u{7}")),
        with("freq_hz", json!(0)),
        with("freq_hz", json!(10_000_000_001u64)),
        with("freq_hz", json!(1.5)),
        with("freq_hz", json!("1")),
        with("mode", json!("fm")),
        with("mode", json!("USB")),
        with("bandwidth_hz", json!(0)),
        with("bandwidth_hz", json!(10_000_001)),
        with("tags", json!(long_tags)),
        with("tags", json!([""])),
        with("tags", json!(["y".repeat(33)])),
        with("tags", json!("x")),
        with("color", json!("red")),
        with("color", json!("#12345")),
        with("color", json!("#12345g")),
        with("notes", json!("n".repeat(1001))),
        with("extra", json!(1)),
    ];
    for body in bad {
        let reply = client.send("POST", "/api/v1/bookmarks", Some(body.clone()));
        assert_eq!(reply.status, 400, "{body}");
        assert_eq!(code(&reply), "invalid", "{body}");
    }
    // Boundary values are accepted.
    for body in [
        json!({"name": "x".repeat(100), "freq_hz": 10_000_000_000u64, "bandwidth_hz": 10_000_000,
               "notes": "n".repeat(1000), "tags": vec!["z".repeat(32); 1]}),
        json!({"name": "ä".repeat(100), "freq_hz": 1, "bandwidth_hz": 1, "mode": "cw"}),
    ] {
        client.create(body);
    }
    for mode in ["wfm", "nfm", "am", "usb", "lsb", "cw"] {
        client.create(json!({"name": mode, "freq_hz": 5, "mode": mode}));
    }

    // Wrong content type, bad origin.
    let reply = http(
        client.port,
        "POST",
        "/api/v1/bookmarks",
        &[("Cookie", &client.cookie), ("Content-Type", "text/plain")],
        &ok.to_string(),
    );
    assert_eq!(reply.status, 415, "{reply:?}");
    let reply = http(
        client.port,
        "POST",
        "/api/v1/bookmarks",
        &[
            ("Cookie", &client.cookie),
            JSON,
            ("Origin", "https://evil.example"),
        ],
        &ok.to_string(),
    );
    assert_eq!(reply.status, 403, "{reply:?}");

    // Patch validation leaves the bookmark untouched.
    let id = client.create(ok.clone())["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let path = format!("/api/v1/bookmarks/{id}");
    for body in [
        json!({"name": null}),
        json!({"freq_hz": null}),
        json!({"freq_hz": 0}),
        json!({"mode": "fm"}),
        json!({"tags": null}),
        json!({"color": "x"}),
        json!({"id": "x"}),
    ] {
        let reply = client.send("PATCH", &path, Some(body.clone()));
        assert_eq!(
            (reply.status, code(&reply)),
            (400, json!("invalid")),
            "{body}"
        );
    }
    let stored = client.list();
    let item = stored.iter().find(|b| b["id"] == id).expect("stored");
    assert_eq!(
        (item["name"].clone(), item["freq_hz"].clone()),
        (json!("a"), json!(1))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bulk_import_is_atomic_and_can_replace() {
    let (_db, _api, client) = start!("bm_bulk");
    client.create(json!({"name": "old", "freq_hz": 1000}));

    // Append.
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"bookmarks": [
            {"name": "a", "freq_hz": 3000}, {"name": "b", "freq_hz": 2000, "mode": "usb"}
        ]})),
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.json(), json!({"v": 1, "created": 2}));
    let names: Vec<_> = client.list().iter().map(|b| b["name"].clone()).collect();
    assert_eq!(names, [json!("old"), json!("b"), json!("a")]);

    // One invalid item rejects everything, also with replace.
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"replace": true, "bookmarks": [
            {"name": "c", "freq_hz": 5}, {"name": "", "freq_hz": 6}
        ]})),
    );
    assert_eq!((reply.status, code(&reply)), (400, json!("invalid")));
    assert_eq!(client.list().len(), 3);

    // Replace, feeding an export back in.
    let export = client.list();
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"replace": true, "bookmarks": export})),
    );
    assert_eq!(reply.json(), json!({"v": 1, "created": 3}), "{reply:?}");
    let again = client.list();
    assert_eq!(again.len(), 3);
    assert_ne!(again[0]["id"], export[0]["id"], "ids are regenerated");

    // Replace with nothing clears the table.
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"replace": true, "bookmarks": []})),
    );
    assert_eq!(reply.json(), json!({"v": 1, "created": 0}));
    assert!(client.list().is_empty());

    // Not an object / unknown field.
    for body in [
        json!([]),
        json!({"bookmarks": [], "x": 1}),
        json!({"replace": true}),
    ] {
        let reply = client.send("POST", "/api/v1/bookmarks/bulk", Some(body));
        assert_eq!((reply.status, code(&reply)), (400, json!("invalid")));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn limit_of_10000_bookmarks() {
    let (db, _api, client) = start!("bm_limit");
    let items = |n: usize| -> Vec<Value> {
        (0..n)
            .map(|i| json!({"name": format!("b{i}"), "freq_hz": i + 1}))
            .collect()
    };

    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"bookmarks": items(10_000)})),
    );
    assert_eq!(
        reply.json(),
        json!({"v": 1, "created": 10_000}),
        "{reply:?}"
    );
    assert_eq!(client.list().len(), 10_000);

    let reply = client.send(
        "POST",
        "/api/v1/bookmarks",
        Some(json!({"name": "one too many", "freq_hz": 1})),
    );
    assert_eq!((reply.status, code(&reply)), (409, json!("limit")));
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"bookmarks": items(1)})),
    );
    assert_eq!((reply.status, code(&reply)), (409, json!("limit")));
    // Too many in one request, even when replacing.
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"replace": true, "bookmarks": items(10_001)})),
    );
    assert_eq!((reply.status, code(&reply)), (409, json!("limit")));
    assert_eq!(client.list().len(), 10_000, "failed writes change nothing");

    // Replacing a full table with a full set is fine.
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"replace": true, "bookmarks": items(10_000)})),
    );
    assert_eq!(reply.json(), json!({"v": 1, "created": 10_000}));

    // Direct constraint check: the schema rejects what the API would never send.
    let pool = db.pool().await;
    let bad =
        sqlx::query("INSERT INTO superhet.bookmarks (name, freq_hz, color) VALUES ('x', 1, 'red')")
            .execute(&pool)
            .await;
    assert!(bad.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn bulk_body_limit_is_8_mib() {
    let (_db, _api, client) = start!("bm_body");
    let padding = "x".repeat(8 * 1024 * 1024);
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"bookmarks": [], "pad": padding})),
    );
    assert_eq!(reply.status, 413, "{reply:?}");
    // Single create stays small.
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks",
        Some(json!({"name": "a", "freq_hz": 1, "notes": "n".repeat(20_000)})),
    );
    assert_eq!(reply.status, 413, "{reply:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn full_receiver_state_and_groups() {
    let (_db, _api, client) = start!("bm_v2");

    // v1-style bookmarks read back with neutral v2 fields.
    let plain = client.create(json!({"name": "plain", "freq_hz": 100_000_000}));
    assert_eq!(plain["squelch_auto"], false);
    assert_eq!(plain["favourite"], false);
    assert_eq!(plain["digital"], Value::Null);
    assert_eq!(plain["group_id"], Value::Null);
    assert_eq!(
        plain["scan"],
        json!({"include": false, "priority": null, "dwell_ms": null})
    );
    for null in ["squelch_db", "step_hz", "tone_mode", "ctcss_hz", "dcs_code"] {
        assert_eq!(plain[null], Value::Null, "{null}");
    }

    // Groups: create, duplicate (case-insensitive), order, rename.
    let group = |client: &Client, name: &str| -> Value {
        let reply = client.send(
            "POST",
            "/api/v1/bookmark-groups",
            Some(json!({"name": name})),
        );
        assert_eq!(reply.status, 201, "{reply:?}");
        reply.json()
    };
    let air = group(&client, "Airband");
    let tetra = group(&client, " TETRA ");
    assert_eq!(tetra["name"], "TETRA");
    assert_eq!(
        (air["position"].clone(), tetra["position"].clone()),
        (json!(0), json!(1))
    );
    let reply = client.send(
        "POST",
        "/api/v1/bookmark-groups",
        Some(json!({"name": "airband"})),
    );
    assert_eq!((reply.status, code(&reply)), (409, json!("duplicate")));
    for bad in [
        json!({"name": ""}),
        json!({"name": "x".repeat(61)}),
        json!({}),
    ] {
        let reply = client.send("POST", "/api/v1/bookmark-groups", Some(bad));
        assert_eq!((reply.status, code(&reply)), (400, json!("invalid")));
    }
    let (air_id, tetra_id) = (
        air["id"].as_str().expect("id").to_owned(),
        tetra["id"].as_str().expect("id").to_owned(),
    );
    let reply = client.send(
        "PUT",
        "/api/v1/bookmark-groups/order",
        Some(json!({"ids": [tetra_id, air_id]})),
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    let names: Vec<_> = reply.json()["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .map(|g| g["name"].clone())
        .collect();
    assert_eq!(names, [json!("TETRA"), json!("Airband")]);
    let reply = client.send(
        "PUT",
        "/api/v1/bookmark-groups/order",
        Some(json!({"ids": ["0f8fad5b-d9cb-469f-a165-70867728950e"]})),
    );
    assert_eq!((reply.status, code(&reply)), (400, json!("invalid")));
    let reply = client.send(
        "PATCH",
        &format!("/api/v1/bookmark-groups/{air_id}"),
        Some(json!({"name": "Aero"})),
    );
    assert_eq!(reply.json()["name"], "Aero");

    // A bookmark with the whole state.
    let full = client.create(json!({
        "name": "Dispatch", "freq_hz": 392_000_000, "mode": "nfm", "bandwidth_hz": 25_000,
        "squelch_db": -75, "squelch_auto": true, "step_hz": 25_000,
        "tone_mode": "dcs", "dcs_code": 23, "dcs_polarity": "R", "ctcss_hz": 88.5,
        "digital": {"protocol": "tetra", "timeslot": 3, "talkgroup": 262_001, "mcc": 262,
                    "mnc": 1, "decoder_plugin_id": "tetra-kit"},
        "group_id": tetra_id, "favourite": true,
        "scan": {"include": true, "priority": 2, "dwell_ms": 3000},
    }));
    assert_eq!(full["tone_mode"], "dcs");
    assert_eq!(full["dcs_code"], 23);
    assert_eq!(full["dcs_polarity"], "R");
    assert_eq!(
        full["ctcss_hz"],
        Value::Null,
        "tone fields follow the tone mode"
    );
    assert_eq!(full["digital"]["timeslot"], 3);
    assert_eq!(full["digital"]["talkgroup"], 262_001);
    assert_eq!(
        full["scan"],
        json!({"include": true, "priority": 2, "dwell_ms": 3000})
    );
    assert_eq!(full["group_id"], tetra_id);
    assert_eq!(full["squelch_db"], -75);
    assert_eq!(full["squelch_auto"], true);

    // Patch: switch the tone, clear digital and the group, keep the rest.
    let path = format!("/api/v1/bookmarks/{}", full["id"].as_str().expect("id"));
    let reply = client.send(
        "PATCH",
        &path,
        Some(
            json!({"tone_mode": "ctcss", "ctcss_hz": 123.0, "digital": null, "group_id": null,
                    "squelch_db": null, "scan": {"include": false}}),
        ),
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    let patched = reply.json();
    assert_eq!(patched["ctcss_hz"], 123.0);
    assert_eq!(patched["dcs_code"], Value::Null);
    assert_eq!(patched["digital"], Value::Null);
    assert_eq!(patched["group_id"], Value::Null);
    assert_eq!(patched["squelch_db"], Value::Null);
    assert_eq!(patched["favourite"], true);
    assert_eq!(patched["scan"]["include"], false);
    assert_eq!(patched["name"], "Dispatch");

    // Unknown group and read-only fields are rejected; nothing changes.
    for body in [
        json!({"group_id": "0f8fad5b-d9cb-469f-a165-70867728950e"}),
        json!({"tone_mode": "dcs"}),
        json!({"digital": {"protocol": "dmr", "timeslot": 3}}),
    ] {
        let reply = client.send("PATCH", &path, Some(body.clone()));
        assert_eq!(
            (reply.status, code(&reply)),
            (400, json!("invalid")),
            "{body}"
        );
    }
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks",
        Some(
            json!({"name": "x", "freq_hz": 1, "group_id": "0f8fad5b-d9cb-469f-a165-70867728950e"}),
        ),
    );
    assert_eq!((reply.status, code(&reply)), (400, json!("invalid")));

    // Export/import round trip keeps everything (group ids must still exist).
    let again = client.create(json!({"name": "g", "freq_hz": 7, "group_id": tetra_id}));
    let export = client.list();
    let reply = client.send(
        "POST",
        "/api/v1/bookmarks/bulk",
        Some(json!({"replace": true, "bookmarks": export})),
    );
    assert_eq!(reply.json(), json!({"v": 1, "created": 3}), "{reply:?}");
    let restored = client.list();
    let item = restored.iter().find(|b| b["name"] == "g").expect("g");
    assert_eq!(item["group_id"], again["group_id"]);

    // Deleting a group keeps its bookmarks, ungrouped.
    let reply = client.send(
        "DELETE",
        &format!("/api/v1/bookmark-groups/{tetra_id}"),
        None,
    );
    assert_eq!(reply.status, 204, "{reply:?}");
    let after = client.list();
    assert_eq!(after.len(), 3);
    assert!(after.iter().all(|b| b["group_id"] == Value::Null));
    let reply = client.send(
        "DELETE",
        &format!("/api/v1/bookmark-groups/{tetra_id}"),
        None,
    );
    assert_eq!((reply.status, code(&reply)), (404, json!("not_found")));
    let reply = client.send("GET", "/api/v1/bookmark-groups", None);
    assert_eq!(reply.json()["groups"].as_array().map(Vec::len), Some(1));
}
