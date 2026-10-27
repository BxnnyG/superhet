//! The audit log against a real PostgreSQL (`DATABASE_URL`, needs CREATE DATABASE and CREATE ROLE).

mod common;

use common::TestDb;
use serde_json::json;
use superhet_db::audit::{self, Filter, NewEvent};

fn ev(kind: &str, id: &str, name: &str, action: &str) -> NewEvent {
    NewEvent {
        ts_ms: None,
        actor_kind: kind.into(),
        actor_id: Some(id.into()),
        actor_name: Some(name.into()),
        role: Some("listener".into()),
        ip: Some("203.0.113.0".into()),
        peer_ip: Some("172.22.0.1".into()),
        user_agent: Some("ua".into()),
        action: action.into(),
        target: Some("dev0".into()),
        outcome: "ok".into(),
        details: json!({"k": 1}),
    }
}

macro_rules! db {
    ($tag:expr) => {{
        let Some(db) = TestDb::create($tag, true).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        db
    }};
}

async fn backdate(db: &TestDb, id: i64, days: i32) {
    let mut tx = db.pool.begin().await.expect("tx");
    sqlx::query("SELECT set_config('superhet.audit_maintenance', 'on', true)")
        .execute(&mut *tx)
        .await
        .expect("maintenance");
    sqlx::query(
        "UPDATE superhet.audit_events SET ts = now() - make_interval(days => $2) WHERE id = $1",
    )
    .bind(id)
    .bind(days)
    .execute(&mut *tx)
    .await
    .expect("backdate");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn insert_query_and_filters() {
    let db = db!("filters");
    let a = audit::insert(&db.pool, &ev("user", "1", "alice", "auth.login"))
        .await
        .expect("a");
    let mut denied = ev("guest", "g1", "guest", "access.denied");
    denied.outcome = "denied".into();
    denied.ip = Some("198.51.100.0".into());
    denied.role = None;
    let b = audit::insert(&db.pool, &denied).await.expect("b");
    let mut sys = ev("system", "sdrd", "sdrd", "device.error");
    sys.ip = None;
    sys.ts_ms = Some(1_700_000_000_000);
    let c = audit::insert(&db.pool, &sys).await.expect("c");
    assert!(a < b && b < c);

    let all = audit::query(&db.pool, &Filter::default(), 100)
        .await
        .expect("all");
    assert_eq!(
        all.iter().map(|e| e.id).collect::<Vec<_>>(),
        [c, b, a],
        "newest id first"
    );
    assert_eq!(all[0].ts_ms, 1_700_000_000_000);
    assert_eq!(
        all[2].data.ip.as_deref(),
        Some("203.0.113.0"),
        "host() without a mask"
    );
    assert_eq!(all[2].data.details, json!({"k": 1}));

    let ids = |events: Vec<audit::Event>| events.into_iter().map(|e| e.id).collect::<Vec<_>>();
    let by = |f: Filter| {
        let pool = db.pool.clone();
        async move { ids(audit::query(&pool, &f, 100).await.expect("q")) }
    };
    assert_eq!(
        by(Filter {
            actor_kind: Some("guest".into()),
            ..Filter::default()
        })
        .await,
        [b]
    );
    assert_eq!(
        by(Filter {
            actor: Some("ALI".into()),
            ..Filter::default()
        })
        .await,
        [a],
        "name part"
    );
    assert_eq!(
        by(Filter {
            actor: Some("g1".into()),
            ..Filter::default()
        })
        .await,
        [b],
        "exact id"
    );
    assert_eq!(
        by(Filter {
            role: Some("listener".into()),
            ..Filter::default()
        })
        .await,
        [c, a]
    );
    assert_eq!(
        by(Filter {
            action: Some("auth.login".into()),
            ..Filter::default()
        })
        .await,
        [a]
    );
    assert_eq!(
        by(Filter {
            action: Some("auth.*".into()),
            ..Filter::default()
        })
        .await,
        [a]
    );
    assert_eq!(
        by(Filter {
            action: Some("%".into()),
            ..Filter::default()
        })
        .await,
        Vec::<i64>::new(),
        "no wildcard injection"
    );
    assert_eq!(
        by(Filter {
            outcome: Some("denied".into()),
            ..Filter::default()
        })
        .await,
        [b]
    );
    assert_eq!(
        by(Filter {
            ip: Some("203.0.113.0".into()),
            ..Filter::default()
        })
        .await,
        [a]
    );
    assert_eq!(
        by(Filter {
            ip: Some("198.51.100.0/24".into()),
            ..Filter::default()
        })
        .await,
        [b]
    );
    assert_eq!(
        by(Filter {
            target: Some("DEV".into()),
            ..Filter::default()
        })
        .await,
        [c, b, a]
    );
    assert_eq!(
        by(Filter {
            to_ms: Some(1_750_000_000_000),
            from_ms: Some(1_600_000_000_000),
            ..Filter::default()
        })
        .await,
        [c]
    );
    assert_eq!(
        by(Filter {
            before_id: Some(c),
            ..Filter::default()
        })
        .await,
        [b, a]
    );
    assert!(
        audit::query(
            &db.pool,
            &Filter {
                ip: Some("nonsense".into()),
                ..Filter::default()
            },
            5
        )
        .await
        .is_err()
    );

    let page = audit::query(&db.pool, &Filter::default(), 1)
        .await
        .expect("page");
    assert_eq!(page.len(), 1);
    let forward = audit::export_page(&db.pool, &Filter::default(), 0, 2)
        .await
        .expect("export");
    assert_eq!(ids(forward), [a, b], "exports walk oldest first");
    let rest = audit::export_page(&db.pool, &Filter::default(), b, 10)
        .await
        .expect("export");
    assert_eq!(ids(rest), [c]);

    let stats = audit::stats(&db.pool).await.expect("stats");
    assert_eq!(stats.count, 3);
    assert_eq!(stats.oldest_ms, Some(1_700_000_000_000));
    assert!(stats.bytes > 0);
    db.drop().await;
}

#[tokio::test]
async fn rows_cannot_be_changed_without_the_maintenance_switch() {
    let db = db!("append");
    let id = audit::insert(&db.pool, &ev("user", "1", "alice", "auth.login"))
        .await
        .expect("a");
    for sql in [
        "UPDATE superhet.audit_events SET outcome = 'error'",
        "DELETE FROM superhet.audit_events",
        "TRUNCATE superhet.audit_events",
    ] {
        let err = sqlx::query(sql).execute(&db.pool).await.expect_err(sql);
        assert!(err.to_string().contains("append-only"), "{sql}: {err}");
    }
    assert_eq!(
        audit::query(&db.pool, &Filter::default(), 10)
            .await
            .expect("q")[0]
            .id,
        id
    );
    db.drop().await;
}

#[tokio::test]
async fn a_restricted_api_role_can_insert_and_select_but_not_update_or_delete() {
    let db = db!("role");
    let role = format!("sh_api_{}", db.name);
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE ROLE {role} LOGIN PASSWORD 'pw'"
    )))
    .execute(&db.pool)
    .await
    .expect("create role");
    for grant in [
        format!("GRANT USAGE ON SCHEMA superhet TO {role}"),
        format!("GRANT INSERT, SELECT ON superhet.audit_events TO {role}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(grant))
            .execute(&db.pool)
            .await
            .expect("grant");
    }
    let host_and_db = db.url.rsplit_once('@').expect("credentials in url").1;
    let api = superhet_db::connect(&format!("postgres://{role}:pw@{host_and_db}"))
        .await
        .expect("connect as api role");
    // INSERT/SELECT work (the identity sequence needs no extra grant for a GENERATED ALWAYS column).
    audit::insert(&api, &ev("user", "1", "alice", "auth.login"))
        .await
        .expect("insert as api");
    assert_eq!(
        audit::query(&api, &Filter::default(), 10)
            .await
            .expect("select")
            .len(),
        1
    );
    for sql in [
        "UPDATE superhet.audit_events SET outcome = 'error'",
        "DELETE FROM superhet.audit_events",
    ] {
        let err = sqlx::query(sql).execute(&api).await.expect_err(sql);
        assert!(
            err.to_string().contains("permission denied"),
            "{sql}: {err}"
        );
    }
    // Even with the trigger switch a role without privileges stays out.
    let mut conn = api.acquire().await.expect("conn");
    sqlx::query("SELECT set_config('superhet.audit_maintenance', 'on', false)")
        .execute(&mut *conn)
        .await
        .expect("set");
    assert!(
        sqlx::query("DELETE FROM superhet.audit_events")
            .execute(&mut *conn)
            .await
            .is_err()
    );
    drop(conn);
    api.close().await;
    db.drop().await;
    // Roles are cluster-wide and outlive the database.
    let admin = superhet_db::connect(&std::env::var("DATABASE_URL").expect("url"))
        .await
        .expect("admin");
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP ROLE {role}")))
        .execute(&admin)
        .await
        .expect("drop role");
}

#[tokio::test]
async fn retention_prunes_guests_and_users_separately() {
    let db = db!("prune");
    let mut ids = Vec::new();
    for (kind, action, age) in [
        ("guest", "guest.old", 8),
        ("guest", "guest.fresh", 6),
        ("user", "user.old", 91),
        ("user", "user.mid", 80),
        ("system", "sys.old", 100),
        ("guest", "guest.ancient", 400),
    ] {
        let id = audit::insert(&db.pool, &ev(kind, "x", "x", action))
            .await
            .expect("insert");
        backdate(&db, id, age).await;
        ids.push((action, id));
    }
    let left = |events: Vec<audit::Event>| {
        let mut names: Vec<String> = events.into_iter().map(|e| e.data.action).collect();
        names.sort();
        names
    };
    // Defaults of ADR-0012: 90 days for accounts and system, 7 days for guests.
    let removed = audit::prune(&db.pool, 90, 7).await.expect("prune");
    assert_eq!(removed, 4);
    assert_eq!(
        left(
            audit::query(&db.pool, &Filter::default(), 10)
                .await
                .expect("q")
        ),
        ["guest.fresh", "user.mid"]
    );
    // 0 keeps a class forever.
    let id = audit::insert(&db.pool, &ev("guest", "g", "g", "guest.keep"))
        .await
        .expect("insert");
    backdate(&db, id, 500).await;
    assert_eq!(audit::prune(&db.pool, 90, 0).await.expect("prune"), 0);
    assert_eq!(audit::prune(&db.pool, 0, 7).await.expect("prune"), 1);
    db.drop().await;
}
