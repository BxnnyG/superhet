//! End-to-end tests of the decoder event API: the real `superhet-api` binary on a real
//! PostgreSQL (needs `DATABASE_URL`; skipped otherwise). No NATS involved.

mod common;

use common::*;
use serde_json::{Value, json};
use superhet_db::decoder_events::{self, NewEvent};

fn ev(ts_ms: i64, vfo: i32, kind: &str, value: &str) -> NewEvent {
    NewEvent {
        ts_ms,
        device: "rtl0".into(),
        vfo,
        freq_hz: Some(433_500_000),
        mode: Some("nfm".into()),
        kind: kind.into(),
        value: value.into(),
        meta: json!({}),
    }
}

fn get(port: u16, cookie: &str, path: &str) -> Reply {
    http(port, "GET", path, &[("Cookie", cookie)], "")
}

fn ids(reply: &Reply) -> Vec<String> {
    reply.json()["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|e| e["value"].as_str().unwrap_or("").to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn list_filter_page_export_clear() {
    let Some(db) = TestDb::create("devents_api").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (api, port) = spawn_api(&db.url, |_| {});
    wait_ready(port, "api").await;
    let cookie = setup_admin(port);
    let _keep = &api;

    // Unauthenticated access is refused.
    for path in [
        "/api/v1/decoder-events",
        "/api/v1/decoder-events/export.csv",
    ] {
        assert_eq!(http(port, "GET", path, &[], "").status, 401, "{path}");
    }
    assert_eq!(
        http(port, "DELETE", "/api/v1/decoder-events", &[], "").status,
        401
    );

    let pool = db.pool().await;
    let base = 1_700_000_000_000_i64;
    for (i, (vfo, kind, value)) in [
        (1, "dtmf", "123#"),
        (2, "ctcss", "88.5 Hz"),
        (1, "rds_rt", "=HYPERLINK(\"x\")"),
        (1, "dtmf", "*0"),
    ]
    .into_iter()
    .enumerate()
    {
        let ts = base + i64::try_from(i).expect("i") * 1000;
        decoder_events::insert(&pool, &ev(ts, vfo, kind, value))
            .await
            .expect("insert");
    }

    let all = get(port, &cookie, "/api/v1/decoder-events");
    assert_eq!(all.status, 200, "{all:?}");
    assert_eq!(all.json()["v"], 1);
    assert_eq!(all.json()["next_cursor"], Value::Null);
    assert_eq!(
        ids(&all),
        ["*0", "=HYPERLINK(\"x\")", "88.5 Hz", "123#"],
        "newest first"
    );

    let dtmf = get(
        port,
        &cookie,
        "/api/v1/decoder-events?kind=dtmf,ctcss&vfo=1",
    );
    assert_eq!(ids(&dtmf), ["*0", "123#"]);
    let range = get(
        port,
        &cookie,
        &format!(
            "/api/v1/decoder-events?from={}&to={}",
            base + 1000,
            base + 3000
        ),
    );
    assert_eq!(ids(&range), ["=HYPERLINK(\"x\")", "88.5 Hz"]);
    let search = get(port, &cookie, "/api/v1/decoder-events?q=88.5");
    assert_eq!(ids(&search), ["88.5 Hz"]);

    // Cursor paging.
    let page1 = get(port, &cookie, "/api/v1/decoder-events?limit=3");
    assert_eq!(ids(&page1).len(), 3);
    let cursor = page1.json()["next_cursor"]
        .as_str()
        .expect("cursor")
        .to_owned();
    let page2 = get(
        port,
        &cookie,
        &format!("/api/v1/decoder-events?limit=3&cursor={cursor}"),
    );
    assert_eq!(ids(&page2), ["123#"]);
    assert_eq!(page2.json()["next_cursor"], Value::Null);

    // Bad input.
    for bad in [
        "kind=bogus",
        "limit=0",
        "limit=5000",
        "cursor=nope",
        "vfo=x",
        "unknown=1",
    ] {
        let reply = get(port, &cookie, &format!("/api/v1/decoder-events?{bad}"));
        assert_eq!(reply.status, 400, "{bad}: {reply:?}");
    }

    // CSV: header, ISO time, formula defused.
    let csv = get(
        port,
        &cookie,
        "/api/v1/decoder-events/export.csv?kind=rds_rt",
    );
    assert_eq!(csv.status, 200);
    assert!(
        csv.header("content-type")
            .is_some_and(|c| c.starts_with("text/csv"))
    );
    let lines: Vec<&str> = csv.body.lines().collect();
    assert_eq!(
        lines[0],
        "time,ts_ms,device,vfo,freq_hz,mode,kind,value,meta"
    );
    assert!(
        lines[1].starts_with(
            "2023-11-14T22:13:22.000Z,1700000002000,rtl0,1,433500000,nfm,rds_rt,\"'=HYPERLINK"
        ),
        "{}",
        lines[1]
    );

    // Clear.
    let cleared = http(
        port,
        "DELETE",
        "/api/v1/decoder-events",
        &[("Cookie", cookie.as_str())],
        "",
    );
    assert_eq!(cleared.status, 200, "{cleared:?}");
    assert_eq!(cleared.json()["deleted"], 4);
    assert!(ids(&get(port, &cookie, "/api/v1/decoder-events")).is_empty());
}
