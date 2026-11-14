//! Export, wipe, import against a throwaway PostgreSQL (`DATABASE_URL`; skipped when unset).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use sqlx::PgPool;
use superhet_backup::archive::{Limits, open};
use superhet_backup::{Error, ExportOptions, ImportOptions, Manifest, crypto, export, import};

struct TestDb {
    admin: PgPool,
    name: String,
    pool: PgPool,
}

impl TestDb {
    async fn create() -> Option<Self> {
        let url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|u| !u.is_empty())?;
        let admin = superhet_db::connect(&url).await.expect("connect admin");
        let name = format!("sh_bk_{}_{}", std::process::id(), uuid_part());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("create db");
        let (base, _) = url.rsplit_once('/').expect("url with database");
        let pool = superhet_db::connect(&format!("{base}/{name}"))
            .await
            .expect("connect");
        superhet_db::migrate(&pool).await.expect("migrate");
        Some(Self { admin, name, pool })
    }

    async fn drop(self) {
        self.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {} WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin)
        .await
        .expect("drop db");
    }

    async fn sql(&self, q: &str) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(q.to_owned()))
            .execute(&self.pool)
            .await
            .unwrap_or_else(|e| panic!("{q}: {e}"));
    }

    async fn count(&self, table: &str) -> i64 {
        use sqlx::Row;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT count(*)::bigint AS n FROM superhet.{table}"
        )))
        .fetch_one(&self.pool)
        .await
        .expect("count")
        .get("n")
    }
}

fn uuid_part() -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    format!("{}", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

const SEED: &str = "
INSERT INTO superhet.users (username, password_hash, role, email) VALUES
  ('admin', '$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNoaGFzaA', 'admin', NULL),
  ('alice', '$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$YWxpY2VoYXNoYWxpY2VoYQ', 'operator', 'alice@example.org'),
  ('Bob_2', '$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$Ym9iaGFzaGJvYmhhc2g', 'listener', NULL);
INSERT INTO superhet.oidc_identities (user_id, issuer, subject)
  SELECT id, 'https://idp.example', 'sub-alice' FROM superhet.users WHERE username = 'alice';
INSERT INTO superhet.invites (token_hash, role, note, created_by_name, expires_at)
  VALUES (decode(repeat('ab', 32), 'hex'), 'listener', 'spare', 'admin', now() + interval '1 day');
INSERT INTO superhet.settings (key, value, updated_by) VALUES
  ('guest_policy', '{\"enabled\": true}', 'admin'), ('limits', '{\"max_sessions\": 5}', 'admin');
INSERT INTO superhet.device_policies (device, view_role, control_role, guests_visible, allowed_users)
  SELECT 'dev0', 'operator', 'admin', false, ARRAY[id] FROM superhet.users WHERE username = 'alice';
INSERT INTO superhet.bookmark_groups (name, position) VALUES ('Air', 0);
INSERT INTO superhet.bookmarks (name, freq_hz, mode, tags, group_id, favourite)
  SELECT 'Tower', 118000000, 'am', ARRAY['air','atc'], id, true FROM superhet.bookmark_groups;
INSERT INTO superhet.profiles (name, data) VALUES ('Morning', '{\"freq\": 100000000}');
INSERT INTO superhet.channel_plans (name, doc) VALUES ('PMR', '{\"channels\": [1,2,3]}');
INSERT INTO superhet.notification_targets (name, kind, config, secret) VALUES ('hook', 'webhook', '{\"url\":\"https://example.org/h\"}', 's3cret');
INSERT INTO superhet.notification_rules (name, source, kind) VALUES ('pager', 'plugin.x', 'msg');
INSERT INTO superhet.notification_rule_targets (rule_id, target_id)
  SELECT r.id, t.id FROM superhet.notification_rules r, superhet.notification_targets t;
INSERT INTO superhet.share_links (token_hash, token_hint, label, device, features)
  VALUES (decode(repeat('cd', 32), 'hex'), 'cdcd', 'friends', 'dev0', ARRAY['audio']);
INSERT INTO superhet.plugin_instances (id, plugin_id, device, config) VALUES ('p1', 'org.example.decoder', 'dev0', '{\"gain\": 3}');
INSERT INTO superhet.recordings (kind, source, state, stem, title, device, freq_hz, size_bytes)
  VALUES ('audio', 'manual', 'done', 'rec1-118000000hz', 'Tower', 'dev0', 118000000, 11),
         ('iq', 'manual', 'recording', 'rec2', 'Live', 'dev0', 100000000, 0);
INSERT INTO superhet.audit_events (actor_kind, actor_name, action, outcome) VALUES ('user', 'admin', 'tune', 'ok');
INSERT INTO superhet.decoder_events (ts, device, kind, value) VALUES (now(), 'dev0', 'dtmf', '1234');
";

async fn admin_id(db: &TestDb) -> i64 {
    use sqlx::Row;
    sqlx::query("SELECT id FROM superhet.users WHERE username = 'admin'")
        .fetch_one(&db.pool)
        .await
        .expect("admin")
        .get("id")
}

fn all_areas() -> Vec<String> {
    superhet_backup::AREAS
        .iter()
        .map(|a| a.id.to_owned())
        .collect()
}

fn rec_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sh-bk-rec-{tag}-{}", std::process::id()));
    fs::create_dir_all(dir.join("audio")).expect("dir");
    fs::create_dir_all(dir.join("iq")).expect("dir");
    dir
}

/// Content hash per data file: two exports of equal state agree on every entry.
fn fingerprint(m: &Manifest) -> BTreeMap<String, (u64, String)> {
    m.files
        .iter()
        .map(|f| (f.table.clone(), (f.rows, f.sha256.clone())))
        .collect()
}

async fn make(
    db: &TestDb,
    creds: bool,
    areas: Vec<String>,
    rec: Option<PathBuf>,
) -> (std::fs::File, Manifest) {
    export(
        &db.pool,
        &ExportOptions {
            areas,
            include_credentials: creds,
            created_by: "test".into(),
            recordings_dir: rec,
        },
    )
    .await
    .expect("export")
}

async fn wipe(db: &TestDb, keep_admin: i64) {
    db.sql(&format!(
        "BEGIN; SELECT set_config('superhet.audit_maintenance', 'on', true); \
         DELETE FROM superhet.audit_events; DELETE FROM superhet.decoder_events; \
         DELETE FROM superhet.recordings; DELETE FROM superhet.share_links; DELETE FROM superhet.plugin_instances; \
         DELETE FROM superhet.notification_rules; DELETE FROM superhet.notification_targets; \
         DELETE FROM superhet.channel_plans; DELETE FROM superhet.profiles; \
         DELETE FROM superhet.bookmarks; DELETE FROM superhet.bookmark_groups; \
         DELETE FROM superhet.device_policies; DELETE FROM superhet.settings; \
         DELETE FROM superhet.invites; DELETE FROM superhet.users WHERE id <> {keep_admin}; COMMIT;"
    ))
    .await;
}

#[tokio::test]
async fn export_wipe_import_gives_the_same_install() {
    let Some(db) = TestDb::create().await else {
        return;
    };
    db.sql(SEED).await;
    let admin = admin_id(&db).await;
    let src = rec_dir("src");
    fs::write(src.join("audio/rec1-118000000hz.ogg"), b"OggS-bytes!").expect("write");

    let (file, before) = make(&db, true, all_areas(), Some(src.clone())).await;
    assert!(
        before
            .files
            .iter()
            .any(|f| f.table == "share_links" && f.rows == 1)
    );
    assert_eq!(
        before.blobs.len(),
        1,
        "only the finished recording's file travels"
    );
    let interrupted: String = {
        let mut opened = open(file, Limits::default()).expect("open");
        let mut b = opened.batches("recordings").expect("batches");
        let rows = b.next_batch(10).expect("rows").expect("some");
        rows.iter()
            .find(|r| r["stem"] == "rec2")
            .map(|r| r["state"].as_str().unwrap_or_default().to_owned())
            .unwrap_or_default()
    };
    assert_eq!(
        interrupted, "interrupted",
        "a running recording is restored as interrupted"
    );

    // Re-export (the first file was consumed), wipe, rehearse, apply.
    let (file, _) = make(&db, true, all_areas(), Some(src.clone())).await;
    wipe(&db, admin).await;
    assert_eq!(db.count("bookmarks").await, 0);
    let dst = rec_dir("dst");
    let mut opened = open(file, Limits::default()).expect("open");
    let opts = ImportOptions {
        areas: Vec::new(),
        overwrite_self: false,
        self_user_id: Some(admin),
        dry_run: true,
        recordings_dir: Some(dst.clone()),
    };
    let dry = import(&db.pool, &mut opened, &opts).await.expect("dry run");
    assert!(
        dry.dry_run
            && dry
                .tables
                .iter()
                .any(|t| t.table == "bookmarks" && t.inserted == 1)
    );
    assert_eq!(db.count("bookmarks").await, 0, "a dry run changes nothing");
    assert_eq!(db.count("users").await, 1);
    assert!(!dst.join("audio/rec1-118000000hz.ogg").exists());

    let real = import(
        &db.pool,
        &mut opened,
        &ImportOptions {
            dry_run: false,
            ..opts
        },
    )
    .await
    .expect("import");
    assert_eq!(real.files_restored, 1);
    assert_eq!(
        fs::read(dst.join("audio/rec1-118000000hz.ogg")).expect("file"),
        b"OggS-bytes!"
    );

    let (_, after) = make(&db, true, all_areas(), Some(dst.clone())).await;
    // The recording's state was normalised on the way out, so compare against the same rule.
    let mut want = fingerprint(&before);
    let got = fingerprint(&after);
    want.remove("recordings");
    let mut got_wo = got.clone();
    got_wo.remove("recordings");
    assert_eq!(want, got_wo, "every table equal after the round trip");
    assert_eq!(got["recordings"].0, 2);
    assert_eq!(before.blobs, after.blobs);

    for d in [src, dst] {
        let _ = fs::remove_dir_all(d);
    }
    db.drop().await;
}

#[tokio::test]
async fn credentials_are_only_exported_when_asked_and_missing_ones_lock_the_account() {
    let Some(db) = TestDb::create().await else {
        return;
    };
    db.sql(SEED).await;
    let admin = admin_id(&db).await;
    let (plain, m) = make(&db, false, Vec::new(), None).await;
    assert!(!m.includes_credentials);
    assert!(
        m.files
            .iter()
            .all(|f| f.table != "invites" && f.table != "notification_vapid")
    );
    let mut opened = open(plain, Limits::default()).expect("open");
    let rows = opened
        .batches("users")
        .expect("b")
        .next_batch(10)
        .expect("r")
        .expect("some");
    assert!(
        rows.iter().all(|r| r.get("password_hash").is_none()),
        "no hashes without the tick"
    );
    let hook = opened
        .batches("notification_targets")
        .expect("b")
        .next_batch(10)
        .expect("r")
        .expect("some");
    assert!(hook[0].get("secret").is_none());

    wipe(&db, admin).await;
    let opts = ImportOptions {
        self_user_id: Some(admin),
        ..ImportOptions::default()
    };
    let report = import(&db.pool, &mut opened, &opts).await.expect("import");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("without a password hash")),
        "{:?}",
        report.warnings
    );
    use sqlx::Row;
    let hash: String =
        sqlx::query("SELECT password_hash FROM superhet.users WHERE username = 'alice'")
            .fetch_one(&db.pool)
            .await
            .expect("alice")
            .get("password_hash");
    assert_eq!(hash, superhet_backup::LOCKED_HASH);
    // The identity link and device policy were re-attached to the new account ids.
    assert_eq!(db.count("oidc_identities").await, 1);
    let n: i64 =
        sqlx::query("SELECT cardinality(allowed_users)::bigint AS n FROM superhet.device_policies")
            .fetch_one(&db.pool)
            .await
            .expect("policy")
            .get("n");
    assert_eq!(n, 1);
    db.drop().await;
}

#[tokio::test]
async fn own_account_needs_confirmation_and_the_last_admin_cannot_be_lost() {
    let Some(db) = TestDb::create().await else {
        return;
    };
    db.sql(SEED).await;
    let admin = admin_id(&db).await;
    let (file, _) = make(&db, true, vec!["users".into()], None).await;
    db.sql("UPDATE superhet.users SET email = 'new@example.org' WHERE username = 'admin'")
        .await;
    let mut opened = open(file, Limits::default()).expect("open");
    let mut opts = ImportOptions {
        self_user_id: Some(admin),
        ..ImportOptions::default()
    };
    let report = import(&db.pool, &mut opened, &opts).await.expect("import");
    assert!(
        report
            .conflicts
            .iter()
            .any(|c| c.contains("your own account")),
        "{:?}",
        report.conflicts
    );
    use sqlx::Row;
    let email = |db: &TestDb| {
        let pool = db.pool.clone();
        async move {
            sqlx::query("SELECT email FROM superhet.users WHERE username = 'admin'")
                .fetch_one(&pool)
                .await
                .expect("row")
                .get::<Option<String>, _>("email")
        }
    };
    assert_eq!(
        email(&db).await.as_deref(),
        Some("new@example.org"),
        "kept without confirmation"
    );
    opts.overwrite_self = true;
    import(&db.pool, &mut opened, &opts).await.expect("import");
    assert_eq!(
        email(&db).await,
        None,
        "confirmed: the backup's version wins"
    );

    // An archive in which the admin is only a listener must not lock everybody out.
    db.sql("UPDATE superhet.users SET role = 'listener' WHERE username = 'admin'")
        .await;
    let (file, _) = make(&db, true, vec!["users".into()], None).await;
    db.sql("UPDATE superhet.users SET role = 'admin' WHERE username = 'admin'")
        .await;
    let mut opened = open(file, Limits::default()).expect("open");
    let err = import(&db.pool, &mut opened, &opts)
        .await
        .expect_err("no admin left");
    assert!(matches!(err, Error::Conflict(_)), "{err}");
    let role: String = sqlx::query("SELECT role FROM superhet.users WHERE username = 'admin'")
        .fetch_one(&db.pool)
        .await
        .expect("row")
        .get("role");
    assert_eq!(role, "admin", "the failed import rolled back");
    assert_eq!(db.count("users").await, 3);
    db.drop().await;
}

#[tokio::test]
async fn newer_schema_is_refused_and_older_is_accepted() {
    let Some(db) = TestDb::create().await else {
        return;
    };
    db.sql(SEED).await;
    let admin = admin_id(&db).await;
    let current = superhet_backup::schema_version(&db.pool)
        .await
        .expect("version");
    for (version, ok) in [(current + 1, false), (current - 1, true)] {
        let (file, mut manifest) = make(&db, false, vec!["profiles".into()], None).await;
        drop(file);
        manifest.schema_version = version;
        let mut w =
            superhet_backup::archive::Writer::new(superhet_backup::temp_file().expect("tmp"));
        w.begin("profiles", "profiles").expect("begin");
        w.row(r#"{"name":"Restored","data":{}}"#).expect("row");
        w.end().expect("end");
        let (file, _) = w.finish(manifest).expect("finish");
        let mut opened = open(file, Limits::default()).expect("open");
        let opts = ImportOptions {
            self_user_id: Some(admin),
            dry_run: true,
            ..ImportOptions::default()
        };
        let result = import(&db.pool, &mut opened, &opts).await;
        match (ok, result) {
            (false, Err(Error::Newer(m))) => assert!(m.contains("update Superhet"), "{m}"),
            (true, Ok(r)) => assert!(
                r.warnings.iter().any(|w| w.contains("older schema")),
                "{:?}",
                r.warnings
            ),
            (_, other) => panic!("version {version}: {other:?}"),
        }
    }
    db.drop().await;
}

#[tokio::test]
async fn encrypted_archive_round_trips_and_refuses_a_wrong_passphrase() {
    let Some(db) = TestDb::create().await else {
        return;
    };
    db.sql(SEED).await;
    let (mut file, _) = make(&db, true, Vec::new(), None).await;
    let mut sealed = superhet_backup::temp_file().expect("tmp");
    crypto::encrypt(&mut file, &mut sealed, "correct horse").expect("encrypt");
    use std::io::{Read, Seek};
    sealed.rewind().expect("rewind");
    let mut head = [0u8; 8];
    sealed.read_exact(&mut head).expect("head");
    assert!(crypto::is_encrypted(&head));
    for (pass, ok) in [("correct horse", true), ("wrong horse!", false)] {
        sealed.rewind().expect("rewind");
        let mut plain = superhet_backup::temp_file().expect("tmp");
        let result = crypto::decrypt(&mut sealed, &mut plain, pass, 1 << 30);
        assert_eq!(result.is_ok(), ok);
        if ok {
            plain.rewind().expect("rewind");
            assert!(open(plain, Limits::default()).is_ok());
        } else {
            assert!(matches!(result, Err(Error::WrongPassphrase)));
        }
    }
    db.drop().await;
}
