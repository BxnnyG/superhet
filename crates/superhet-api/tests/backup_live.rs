//! End-to-end tests of backup and restore over HTTP: the real `superhet-api` binary on a real
//! PostgreSQL (needs `DATABASE_URL`; skipped otherwise). No NATS involved.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;

use common::*;
use serde_json::{Value, json};

struct Raw {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

impl Raw {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// HTTP/1.0 with a binary body and a binary answer.
fn raw(
    port: u16,
    method: &str,
    path: &str,
    cookie: &str,
    extra: &[(&str, &str)],
    body: &[u8],
) -> Raw {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .expect("timeout");
    let mut head = format!(
        "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nCookie: {cookie}\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (n, v) in extra {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("write head");
    // The server may answer (and close) before the whole body is sent (size cap).
    let _ = stream.write_all(body);
    let mut all = Vec::new();
    let _ = stream.read_to_end(&mut all);
    let split = all
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response head");
    let headers = String::from_utf8_lossy(&all[..split]).to_string();
    let status = headers
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status");
    Raw {
        status,
        headers,
        body: all[split + 4..].to_vec(),
    }
}

fn export(port: u16, cookie: &str, body: &Value) -> Raw {
    raw(
        port,
        "POST",
        "/api/v1/admin/backup/export",
        cookie,
        &[("Content-Type", "application/json")],
        body.to_string().as_bytes(),
    )
}

fn import(port: u16, cookie: &str, query: &str, pass: Option<&str>, body: &[u8]) -> Raw {
    let extra: Vec<(&str, &str)> = pass
        .map(|p| ("X-Backup-Passphrase", p))
        .into_iter()
        .collect();
    raw(
        port,
        "POST",
        &format!("/api/v1/admin/backup/import{query}"),
        cookie,
        &extra,
        body,
    )
}

#[tokio::test]
async fn backup_api_round_trip_encryption_and_hostile_uploads() {
    let Some(db) = TestDb::create("backup").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (_api, port) = spawn_api(&db.url, |c| {
        c.env("SUPERHET_BACKUP_MAX_UPLOAD_MB", "1");
    });
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    let operator = make_user(port, &admin, "olivia", "operator");
    let pool = db.pool().await;
    sqlx::query("INSERT INTO superhet.profiles (name, data) VALUES ('Morning', '{}')")
        .execute(&pool)
        .await
        .expect("seed");

    // Not for operators.
    assert_eq!(export(port, &operator, &json!({})).status, 403);
    assert_eq!(
        import(port, &operator, "?dry_run=true", None, b"x").status,
        403
    );

    // Plain export is a zip; the answer names the file.
    let plain = export(port, &admin, &json!({"include_credentials": true}));
    assert_eq!(
        plain.status,
        200,
        "{}",
        String::from_utf8_lossy(&plain.body)
    );
    assert!(plain.body.starts_with(b"PK"));
    assert!(
        plain
            .headers
            .to_ascii_lowercase()
            .contains("content-disposition: attachment; filename=\"superhet-backup-")
    );

    // Sealed export: weak passphrase refused, strong one accepted, never readable as a zip.
    assert_eq!(
        export(port, &admin, &json!({"passphrase": "short"})).status,
        400
    );
    let sealed = export(
        port,
        &admin,
        &json!({"passphrase": "correct horse", "areas": ["profiles", "users"]}),
    );
    assert_eq!(sealed.status, 200);
    assert!(sealed.body.starts_with(b"SHBKENC1"));

    // Import: passphrase handling.
    let no_pass = import(port, &admin, "?dry_run=true", None, &sealed.body);
    assert_eq!(
        (
            no_pass.status,
            no_pass.json()["error"].as_str().map(str::to_owned)
        ),
        (400, Some("passphrase_required".into()))
    );
    let wrong = import(
        port,
        &admin,
        "?dry_run=true",
        Some("wrong horse!"),
        &sealed.body,
    );
    assert_eq!(
        (
            wrong.status,
            wrong.json()["error"].as_str().map(str::to_owned)
        ),
        (400, Some("wrong_passphrase".into()))
    );
    let mut tampered = sealed.body.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert_eq!(
        import(
            port,
            &admin,
            "?dry_run=true",
            Some("correct horse"),
            &tampered
        )
        .status,
        400
    );
    let dry = import(
        port,
        &admin,
        "?dry_run=true",
        Some("correct%20horse"),
        &sealed.body,
    );
    assert_eq!(dry.status, 200, "{}", String::from_utf8_lossy(&dry.body));
    assert_eq!(dry.json()["report"]["dry_run"], true);
    assert_eq!(dry.json()["report"]["areas"], json!(["users", "profiles"]));

    // Real import of the plain archive; our own account is the same, so nothing conflicts.
    sqlx::query("DELETE FROM superhet.profiles")
        .execute(&pool)
        .await
        .expect("wipe");
    let real = import(port, &admin, "?areas=profiles", None, &plain.body);
    assert_eq!(real.status, 200, "{}", String::from_utf8_lossy(&real.body));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM superhet.profiles")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(n, 1, "the profile is back");
    assert_eq!(one(port, &admin, "backup.import")["outcome"], "ok");
    assert!(
        audit(port, &admin, "action=backup.export")
            .iter()
            .all(|e| e["outcome"] == "ok")
    );

    // Hostile uploads.
    assert_eq!(
        import(port, &admin, "?dry_run=true", None, b"this is not a zip").status,
        400
    );
    assert_eq!(import(port, &admin, "?dry_run=true", None, b"").status, 400);
    let mut evil = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut evil));
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("manifest.json", o).expect("start");
        z.write_all(br#"{"format":1,"app_version":"x","schema_version":1,"created_at":"","created_by":"","includes_credentials":false,"areas":[],"files":[]}"#).expect("write");
        z.start_file("../../etc/cron.d/evil", o).expect("start");
        z.write_all(b"* * * * * root id").expect("write");
        z.finish().expect("finish");
    }
    let hostile = import(port, &admin, "?dry_run=true", None, &evil);
    assert_eq!(hostile.status, 400);
    assert!(
        hostile.json()["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unexpected entry")
    );
    // Unknown area, and an upload above the cap (1 MiB here).
    assert_eq!(
        import(
            port,
            &admin,
            "?dry_run=true&areas=nonsense",
            None,
            &plain.body
        )
        .status,
        400
    );
    let big = vec![0u8; 2 << 20];
    assert_eq!(
        import(port, &admin, "?dry_run=true", None, &big).status,
        413
    );

    // The overview lists the areas for the page.
    let overview = http(
        port,
        "GET",
        "/api/v1/admin/backup",
        &[("Cookie", &admin)],
        "",
    );
    assert_eq!(overview.status, 200);
    assert!(
        overview.json()["areas"]
            .as_array()
            .is_some_and(|a| a.len() >= 10)
    );
}

#[tokio::test]
async fn backup_needs_a_session() {
    let Some(db) = TestDb::create("backup_anon").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let (_api, port) = spawn_api(&db.url, |_| {});
    wait_ready(port, "api").await;
    for (method, path) in [
        ("GET", "/api/v1/admin/backup"),
        ("POST", "/api/v1/admin/backup/export"),
        ("POST", "/api/v1/admin/backup/import"),
    ] {
        assert_eq!(
            raw(port, method, path, "", &[], b"{}").status,
            401,
            "{path}"
        );
    }
}
