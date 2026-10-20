//! Auth queries against a real PostgreSQL; runs only when `DATABASE_URL` is set. Every test
//! works in its own throwaway database so the single-admin constraint cannot collide.

use std::time::Duration;

use superhet_db::{
    PgPool,
    auth::{self, Lifetimes},
};

const LIFETIMES: Lifetimes = Lifetimes {
    idle: Duration::from_secs(7 * 24 * 3600),
    absolute: Duration::from_secs(30 * 24 * 3600),
    touch_interval: Duration::from_secs(60),
};

struct TestDb {
    admin: PgPool,
    name: String,
    pool: PgPool,
}

impl TestDb {
    async fn create(tag: &str) -> Option<Self> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let admin = superhet_db::connect(&url).await.expect("connect admin");
        let name = format!("sh_test_{tag}_{}", std::process::id());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {name}"
        )))
        .execute(&admin)
        .await
        .expect("drop stale");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("create db");
        let (base, _) = url.rsplit_once('/').expect("url with database");
        let pool = superhet_db::connect(&format!("{base}/{name}"))
            .await
            .expect("connect test db");
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
}

async fn backdate(pool: &PgPool, id: &[u8], column: &str, interval: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE superhet.sessions SET {column} = now() - interval '{interval}' WHERE id = $1"
    )))
    .bind(id)
    .execute(pool)
    .await
    .expect("backdate");
}

async fn session_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM superhet.sessions")
        .fetch_one(pool)
        .await
        .expect("count")
}

#[tokio::test]
async fn single_admin_and_lookup() {
    let Some(db) = TestDb::create("admin").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    assert!(!auth::has_user(&db.pool).await.expect("has_user"));
    let id = auth::create_admin(&db.pool, "Admin", "$argon2id$x")
        .await
        .expect("create")
        .expect("inserted");
    assert!(auth::has_user(&db.pool).await.expect("has_user"));
    // A second account is refused, whatever its name (single-user system).
    assert!(
        auth::create_admin(&db.pool, "other", "h")
            .await
            .expect("create")
            .is_none()
    );
    let found = auth::find_user_by_name(&db.pool, "aDMIN")
        .await
        .expect("find")
        .expect("case-insensitive");
    assert_eq!((found.id, found.username.as_str()), (id, "Admin"));
    assert!(
        auth::find_user_by_name(&db.pool, "nobody")
            .await
            .expect("find")
            .is_none()
    );
    db.drop().await;
}

#[tokio::test]
async fn session_lifetimes_touch_and_pruning() {
    let Some(db) = TestDb::create("sessions").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let pool = &db.pool;
    let user = auth::create_admin(pool, "admin", "h")
        .await
        .expect("create")
        .expect("inserted");
    let (fresh, idle, old, keep) = ([1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]);
    for id in [&fresh, &idle, &old, &keep] {
        auth::create_session(pool, id, user, Some("test-agent"), Some("127.0.0.1"))
            .await
            .expect("session");
    }

    let s = auth::find_active_session(pool, &fresh, LIFETIMES)
        .await
        .expect("find")
        .expect("active");
    assert_eq!(
        (s.user_id, s.username.as_str(), s.needs_touch),
        (user, "admin", false)
    );

    // Touching is only advised after the refresh interval, and resets it.
    backdate(pool, &fresh, "last_seen_at", "2 minutes").await;
    assert!(
        auth::find_active_session(pool, &fresh, LIFETIMES)
            .await
            .expect("find")
            .expect("active")
            .needs_touch
    );
    auth::touch_session(pool, &fresh).await.expect("touch");
    assert!(
        !auth::find_active_session(pool, &fresh, LIFETIMES)
            .await
            .expect("find")
            .expect("active")
            .needs_touch
    );

    // Idle (7 d sliding) and absolute (30 d) expiry; the expired row is removed on access.
    backdate(pool, &idle, "last_seen_at", "8 days").await;
    backdate(pool, &old, "created_at", "31 days").await;
    assert!(
        auth::find_active_session(pool, &idle, LIFETIMES)
            .await
            .expect("find")
            .is_none()
    );
    assert_eq!(
        session_count(pool).await,
        3,
        "idle session deleted on access"
    );
    assert!(
        auth::find_active_session(pool, &[9u8; 32], LIFETIMES)
            .await
            .expect("find")
            .is_none()
    );

    // The periodic prune removes the remaining expired one and nothing else.
    assert_eq!(
        auth::prune_sessions(pool, LIFETIMES).await.expect("prune"),
        1
    );
    assert_eq!(session_count(pool).await, 2);

    auth::delete_session(pool, &keep).await.expect("delete");
    assert_eq!(session_count(pool).await, 1);
    db.drop().await;
}

#[tokio::test]
async fn password_change_revokes_other_sessions() {
    let Some(db) = TestDb::create("password").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let pool = &db.pool;
    let user = auth::create_admin(pool, "admin", "old-hash")
        .await
        .expect("create")
        .expect("inserted");
    let (current, other) = ([1u8; 32], [2u8; 32]);
    for id in [&current, &other] {
        auth::create_session(pool, id, user, None, None)
            .await
            .expect("session");
    }
    let revoked = auth::change_password(pool, user, "new-hash", &current)
        .await
        .expect("change");
    assert_eq!(revoked, 1);
    assert!(
        auth::find_active_session(pool, &current, LIFETIMES)
            .await
            .expect("find")
            .is_some()
    );
    assert!(
        auth::find_active_session(pool, &other, LIFETIMES)
            .await
            .expect("find")
            .is_none()
    );
    let u = auth::find_user_by_id(pool, user)
        .await
        .expect("find")
        .expect("user");
    assert_eq!(u.password_hash, "new-hash");
    db.drop().await;
}
