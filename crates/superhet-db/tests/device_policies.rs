//! Device policies against a real PostgreSQL (`DATABASE_URL`; skipped otherwise): the migration,
//! the table constraints and the event filters that follow from a policy.

mod common;

use std::borrow::Cow;

use common::TestDb;
use serde_json::json;
use superhet_db::{
    decoder_events::{self, NewEvent, Query},
    device_policies::{self, Record},
};

macro_rules! db {
    ($tag:expr) => {{
        let Some(db) = TestDb::create($tag, true).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        db
    }};
}

fn policy(device: &str) -> Record {
    Record {
        device: device.into(),
        view_role: "operator".into(),
        control_role: "admin".into(),
        guests_visible: false,
        allowed_users: vec![3, 9],
    }
}

#[tokio::test]
async fn migration_15_adds_the_table_to_an_older_database_and_changes_nothing_else() {
    let Some(db) = TestDb::create("policy_upgrade", false).await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    // The schema of the release before this one: migrations 1..=14.
    let mut old = sqlx::migrate!("./migrations");
    let all = old.migrations.to_vec();
    old.migrations = Cow::Owned(all.into_iter().filter(|m| m.version <= 14).collect());
    old.create_schema("superhet");
    old.dangerous_set_table_name("superhet._sqlx_migrations");
    old.run(&db.pool).await.expect("old schema");
    let before: i32 = sqlx::query_scalar("SELECT version FROM superhet.schema_version")
        .fetch_one(&db.pool)
        .await
        .expect("version");
    assert_eq!(before, 14);

    superhet_db::migrate(&db.pool).await.expect("upgrade");
    superhet_db::migrate(&db.pool).await.expect("idempotent");
    let after: i32 = sqlx::query_scalar("SELECT version FROM superhet.schema_version")
        .fetch_one(&db.pool)
        .await
        .expect("version");
    assert!(
        after >= 15,
        "upgraded to at least migration 15, got {after}"
    );
    // No rows after the upgrade: every device keeps the open default.
    assert!(
        device_policies::list(&db.pool)
            .await
            .expect("list")
            .is_empty()
    );
    db.drop().await;
}

#[tokio::test]
async fn policies_round_trip_and_reset() {
    let db = db!("policy_rt");
    assert!(
        device_policies::list(&db.pool)
            .await
            .expect("list")
            .is_empty()
    );
    device_policies::put(&db.pool, &policy("rtl1"), "admin")
        .await
        .expect("put");
    device_policies::put(&db.pool, &policy("dev0"), "admin")
        .await
        .expect("put");
    let mut open = policy("rtl1");
    open.view_role = "listener".into();
    open.allowed_users.clear();
    device_policies::put(&db.pool, &open, "admin")
        .await
        .expect("replace");
    let all = device_policies::list(&db.pool).await.expect("list");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].device, "dev0");
    assert_eq!(all[0], policy("dev0"));
    assert_eq!(all[1], open);
    assert!(
        device_policies::delete(&db.pool, "rtl1")
            .await
            .expect("delete")
    );
    assert!(
        !device_policies::delete(&db.pool, "rtl1")
            .await
            .expect("again")
    );
    assert_eq!(
        device_policies::list(&db.pool).await.expect("list").len(),
        1
    );
    db.drop().await;
}

#[tokio::test]
async fn the_table_refuses_unknown_roles_and_oversized_lists() {
    let db = db!("policy_checks");
    let mut bad = policy("dev0");
    bad.view_role = "root".into();
    assert!(device_policies::put(&db.pool, &bad, "admin").await.is_err());
    let mut bad = policy("dev0");
    bad.control_role = "guest".into();
    assert!(device_policies::put(&db.pool, &bad, "admin").await.is_err());
    let mut bad = policy("dev0");
    bad.allowed_users = (0..201).collect();
    assert!(device_policies::put(&db.pool, &bad, "admin").await.is_err());
    let mut bad = policy("dev0");
    bad.device = String::new();
    assert!(device_policies::put(&db.pool, &bad, "admin").await.is_err());
    db.drop().await;
}

fn event(device: &str, value: &str) -> NewEvent {
    NewEvent {
        ts_ms: 1_700_000_000_000,
        device: device.into(),
        vfo: Some(1),
        freq_hz: Some(433_920_000),
        mode: Some("nfm".into()),
        kind: "dtmf".into(),
        source: "core".into(),
        value: value.into(),
        meta: json!({}),
    }
}

#[tokio::test]
async fn decoder_history_can_leave_out_or_keep_devices() {
    let db = db!("policy_events");
    for (device, value) in [("dev0", "1"), ("rtl1", "2"), ("rtl1", "3"), ("-", "4")] {
        decoder_events::insert(&db.pool, &event(device, value))
            .await
            .expect("insert");
    }
    let query = Query {
        limit: 100,
        exclude_devices: vec!["rtl1".into()],
        ..Query::default()
    };
    let rows = decoder_events::list(&db.pool, &query).await.expect("list");
    let mut devices: Vec<_> = rows.iter().map(|e| e.data.device.as_str()).collect();
    devices.sort_unstable();
    assert_eq!(devices, ["-", "dev0"]);
    let all = Query {
        limit: 100,
        ..Query::default()
    };
    assert_eq!(
        decoder_events::list(&db.pool, &all)
            .await
            .expect("all")
            .len(),
        4
    );

    // Clearing keeps what the caller may not touch.
    let removed = decoder_events::clear_except(&db.pool, &["rtl1".to_owned()])
        .await
        .expect("clear");
    assert_eq!(removed, 2);
    let left = decoder_events::list(&db.pool, &all).await.expect("left");
    assert_eq!(left.len(), 2);
    assert!(left.iter().all(|e| e.data.device == "rtl1"));
    db.drop().await;
}
