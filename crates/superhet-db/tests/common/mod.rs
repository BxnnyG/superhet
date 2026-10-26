//! Throwaway databases for the multi-user tests (`DATABASE_URL` must point at a PostgreSQL where
//! the user may create databases and roles; tests are skipped otherwise).
#![allow(dead_code)]

use superhet_db::PgPool;

pub struct TestDb {
    admin: PgPool,
    pub name: String,
    pub url: String,
    pub pool: PgPool,
}

impl TestDb {
    /// A fresh database; `migrate` applies all migrations.
    pub async fn create(tag: &str, migrate: bool) -> Option<Self> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let admin = superhet_db::connect(&url).await.expect("connect admin");
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let name = format!(
            "sh_mu_{tag}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("create db");
        let (base, _) = url.rsplit_once('/').expect("url with database");
        let db_url = format!("{base}/{name}");
        let pool = superhet_db::connect(&db_url)
            .await
            .expect("connect test db");
        if migrate {
            superhet_db::migrate(&pool).await.expect("migrate");
        }
        Some(Self {
            admin,
            name,
            url: db_url,
            pool,
        })
    }

    pub async fn drop(self) {
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
