//! End-to-end tests of the ADS-B aircraft database and range statistics routes: the real
//! `superhet-api` binary on a real PostgreSQL (needs `DATABASE_URL`; skipped otherwise). The
//! import reads a fixture archive from disk; nothing touches the network.

mod common;

use std::io::Write;
use std::time::Duration;

use common::*;
use serde_json::{Value, json};
use superhet_db::adsb::{Activity, RangeCell, RangeUpdate};

const JSON: (&str, &str) = ("Content-Type", "application/json");

fn fixture_zip(path: &std::path::Path) {
    let file = std::fs::File::create(path).expect("create");
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in [
        (
            "aircrafts.json",
            r#"{"3C6444":["D-AIXA","A359","00"],"AE1234":["","H60","10"]}"#,
        ),
        (
            "types.json",
            r#"{"A359":["AIRBUS A-350-900","L2J","H"],"H60":["SIKORSKY UH-60","H2T","M"]}"#,
        ),
        (
            "operators.json",
            r#"{"DLH":["Lufthansa","Germany","LUFTHANSA"]}"#,
        ),
        ("dbversion.json", r#"{"version":530}"#),
        ("LICENSE", "# ODC Attribution License (ODC-By)\n"),
    ] {
        zip.start_file(name, options).expect("start");
        zip.write_all(body.as_bytes()).expect("write");
    }
    zip.finish().expect("finish");
}

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
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sh-adsb-live-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn aircraft_database_import_lookup_and_roles() {
    let Some(db) = TestDb::create("adsb_live_db").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let dir = scratch("db");
    let zip = dir.join("export.zip");
    fixture_zip(&zip);
    let (api, port) = spawn_api(&db.url, |c| {
        c.env("SUPERHET_AIRCRAFT_DB_FILE", &zip)
            .env("SUPERHET_AIRCRAFT_DB_DIR", dir.join("data"));
    });
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    let listener = make_user(port, &admin, "lena", "listener");
    let admin = Client {
        port,
        cookie: admin,
    };
    let listener = Client {
        port,
        cookie: listener,
    };

    assert_eq!(
        http(port, "GET", "/api/v1/adsb/aircraft-db", &[], "").status,
        401
    );
    let status = admin.send("GET", "/api/v1/adsb/aircraft-db", None).json();
    assert!(status["database"].is_null(), "{status}");
    assert_eq!(status["offer"]["licence"], "ODC-By 1.0");
    assert_eq!(status["job"]["state"], "idle");
    assert_eq!(
        status["offer"]["file"].as_str(),
        zip.to_str(),
        "the offline import file is shown"
    );

    // Only admins import.
    let denied = listener.send(
        "POST",
        "/api/v1/admin/adsb/aircraft-db",
        Some(json!({"source": "file"})),
    );
    assert_eq!(denied.status, 403);
    let bad = admin.send(
        "POST",
        "/api/v1/admin/adsb/aircraft-db",
        Some(json!({"source": "ftp"})),
    );
    assert_eq!(bad.status, 400);

    let started = admin.send(
        "POST",
        "/api/v1/admin/adsb/aircraft-db",
        Some(json!({"source": "file"})),
    );
    assert_eq!(started.status, 202, "{started:?}");
    let mut state = Value::Null;
    for _ in 0..100 {
        state = admin.send("GET", "/api/v1/adsb/aircraft-db", None).json();
        if matches!(state["job"]["state"].as_str(), Some("done" | "failed")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(state["job"]["state"], "done", "{state}");
    assert_eq!(state["database"]["aircraft"], 2);
    assert_eq!(state["database"]["licence"], "ODC-By 1.0");
    assert_eq!(state["database"]["version"], "530");
    assert_eq!(state["database"]["origin"], "file");
    one(port, &admin.cookie, "adsb.aircraft_db.update");

    // Everybody who sees decoders can look up.
    let found = listener
        .send(
            "GET",
            "/api/v1/adsb/lookup?icao=3c6444,ae1234,123456&operator=dlh,xyz",
            None,
        )
        .json();
    assert_eq!(found["aircraft"]["3c6444"]["reg"], "D-AIXA");
    assert_eq!(found["aircraft"]["3c6444"]["type_desc"], "AIRBUS A-350-900");
    assert_eq!(found["aircraft"]["3c6444"]["class"], "L2J");
    assert_eq!(found["aircraft"]["3c6444"]["military"], false);
    assert_eq!(found["aircraft"]["ae1234"]["military"], true);
    assert!(found["aircraft"].get("123456").is_none());
    assert_eq!(found["operators"]["DLH"]["name"], "Lufthansa");
    assert!(found["operators"].get("XYZ").is_none());

    for bad in [
        "/api/v1/adsb/lookup?icao=zzzzzz",
        "/api/v1/adsb/lookup?icao=12345",
        "/api/v1/adsb/lookup?operator=a-b",
    ] {
        assert_eq!(listener.send("GET", bad, None).status, 400, "{bad}");
    }

    // Removing it again.
    assert_eq!(
        listener
            .send("DELETE", "/api/v1/admin/adsb/aircraft-db", None)
            .status,
        403
    );
    assert_eq!(
        admin
            .send("DELETE", "/api/v1/admin/adsb/aircraft-db", None)
            .status,
        200
    );
    let state = admin.send("GET", "/api/v1/adsb/aircraft-db", None).json();
    assert!(state["database"].is_null());
    let found = admin
        .send("GET", "/api/v1/adsb/lookup?icao=3c6444", None)
        .json();
    assert!(found["aircraft"].as_object().is_some_and(|m| m.is_empty()));

    drop(api);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_import_needs_a_file_and_an_unknown_licence_is_not_claimed() {
    let Some(db) = TestDb::create("adsb_live_nofile").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let dir = scratch("nofile");
    let (api, port) = spawn_api(&db.url, |c| {
        c.env_remove("SUPERHET_AIRCRAFT_DB_FILE")
            .env("SUPERHET_AIRCRAFT_DB_DIR", dir.join("data"));
    });
    wait_ready(port, "api").await;
    let admin = Client {
        port,
        cookie: setup_admin(port),
    };
    let reply = admin.send(
        "POST",
        "/api/v1/admin/adsb/aircraft-db",
        Some(json!({"source": "file"})),
    );
    assert_eq!(reply.status, 400, "{reply:?}");

    // A directory without licence text: imported, but the licence is not claimed.
    let export = dir.join("export");
    std::fs::create_dir_all(&export).expect("dir");
    std::fs::write(
        export.join("aircrafts.json"),
        r#"{"3C6444":["D-AIXA","A359","00"]}"#,
    )
    .expect("write");
    drop(api);
    let (api, port) = spawn_api(&db.url, |c| {
        c.env("SUPERHET_AIRCRAFT_DB_FILE", &export)
            .env("SUPERHET_AIRCRAFT_DB_DIR", dir.join("data"));
    });
    wait_ready(port, "api").await;
    let admin = Client {
        port,
        cookie: login(port, "admin", ADMIN_PASSWORD)
            .session_cookie()
            .expect("login"),
    };
    assert_eq!(
        admin
            .send(
                "POST",
                "/api/v1/admin/adsb/aircraft-db",
                Some(json!({"source": "file"})),
            )
            .status,
        202
    );
    let mut state = Value::Null;
    for _ in 0..100 {
        state = admin.send("GET", "/api/v1/adsb/aircraft-db", None).json();
        if matches!(state["job"]["state"].as_str(), Some("done" | "failed")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(state["job"]["state"], "done", "{state}");
    assert!(
        state["database"]["licence"]
            .as_str()
            .is_some_and(|l| l.starts_with("unknown")),
        "{state}"
    );
    drop(api);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn range_statistics_are_served_and_reset_by_operators() {
    let Some(db) = TestDb::create("adsb_live_range").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (api, port) = spawn_api(&db.url, |_| {});
    wait_ready(port, "api").await;
    let admin_cookie = setup_admin(port);
    let operator = Client {
        port,
        cookie: make_user(port, &admin_cookie, "olga", "operator"),
    };
    let listener = Client {
        port,
        cookie: make_user(port, &admin_cookie, "lena", "listener"),
    };

    let empty = listener.send("GET", "/api/v1/adsb/range?device=dev0", None);
    assert_eq!(empty.status, 200, "{empty:?}");
    let empty = empty.json();
    assert!(empty["receiver"].is_null());
    assert_eq!(empty["cells"].as_array().map(Vec::len), Some(0));

    let pool = db.pool().await;
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("ms");
    let minute = now - now % 60_000;
    for (i, km) in [120.5f32, 200.0].into_iter().enumerate() {
        superhet_db::adsb::record_range(
            &pool,
            "dev0",
            &RangeUpdate {
                ts_ms: minute - 60_000 * i64::try_from(i).expect("small"),
                receiver: Some((51.0, 7.0)),
                cells: vec![RangeCell {
                    sector: 3,
                    band: 4,
                    max_km: km,
                    icao: "3c6444".into(),
                }],
                activity: Activity {
                    msgs_per_s: 100.0,
                    aircraft: 12,
                    with_position: 8,
                },
            },
        )
        .await
        .expect("record");
    }

    let range = listener
        .send("GET", "/api/v1/adsb/range?device=dev0&hours=2", None)
        .json();
    assert_eq!(range["receiver"]["lat"], 51.0);
    assert_eq!(range["sector_deg"], 5);
    assert_eq!(range["cells"], json!([[3, 4, 200.0, "3c6444"]]));
    assert_eq!(range["series"].as_array().map(Vec::len), Some(2));
    assert_eq!(range["series"][1][2], 12);

    for bad in [
        "/api/v1/adsb/range?device=a/b",
        "/api/v1/adsb/range?hours=0",
        "/api/v1/adsb/range?hours=100000",
    ] {
        assert_eq!(listener.send("GET", bad, None).status, 400, "{bad}");
    }

    // Reset: operators only, audited, and it forgets everything.
    assert_eq!(
        listener
            .send("DELETE", "/api/v1/adsb/range?device=dev0", None)
            .status,
        403
    );
    let reset = operator.send("DELETE", "/api/v1/adsb/range?device=dev0", None);
    assert_eq!(reset.status, 200, "{reset:?}");
    assert_eq!(reset.json()["removed_cells"], 1);
    let after = listener
        .send("GET", "/api/v1/adsb/range?device=dev0", None)
        .json();
    assert!(after["receiver"].is_null());
    assert_eq!(after["cells"].as_array().map(Vec::len), Some(0));
    assert_eq!(after["series"].as_array().map(Vec::len), Some(0));
    one(port, &admin_cookie, "adsb.range.reset");

    drop(api);
}
