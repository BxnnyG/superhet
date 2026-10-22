//! Profile queries against a real PostgreSQL; runs only when `DATABASE_URL` is set, in a
//! throwaway database.

use serde_json::json;
use superhet_db::{PgPool, profiles};

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

#[tokio::test]
async fn profiles_crud_and_unique_names() {
    let Some(db) = TestDb::create("profiles").await else {
        eprintln!("DATABASE_URL not set; skipping database integration test");
        return;
    };
    let pool = &db.pool;
    assert!(profiles::list(pool).await.expect("list").is_empty());

    let data = json!({"v": 1, "center_hz": 433_920_000u64, "sample_rate": 2_048_000, "vfos": []});
    let made = profiles::create(pool, "Beta", &data).await.expect("create");
    assert_eq!(
        profiles::parse_id(&made.id).as_deref(),
        Some(made.id.as_str())
    );
    assert_eq!(made.data, data);
    assert!(made.created_at.ends_with('Z') && made.created_at.contains('T'));
    profiles::create(pool, "alpha", &data)
        .await
        .expect("create");

    // Names are unique regardless of case, on create and on rename.
    assert!(matches!(
        profiles::create(pool, "BETA", &data).await,
        Err(profiles::WriteError::NameTaken)
    ));
    assert!(matches!(
        profiles::update(pool, &made.id, "Alpha", &data).await,
        Err(profiles::WriteError::NameTaken)
    ));

    let names: Vec<String> = profiles::list(pool)
        .await
        .expect("list")
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["alpha", "Beta"]);

    let new_data = json!({"v": 1, "center_hz": 1, "sample_rate": 2});
    let updated = profiles::update(pool, &made.id, "Beta 2", &new_data)
        .await
        .expect("update")
        .expect("exists");
    assert_eq!(
        (updated.name.as_str(), &updated.data),
        ("Beta 2", &new_data)
    );
    assert_eq!(updated.created_at, made.created_at);
    assert!(updated.updated_at >= made.updated_at);
    // Renaming to the own name is fine.
    profiles::update(pool, &made.id, "Beta 2", &new_data)
        .await
        .expect("same name");

    let got = profiles::get(pool, &made.id).await.expect("get");
    assert_eq!(got.as_ref().map(|p| p.name.as_str()), Some("Beta 2"));
    let missing = "00000000-0000-0000-0000-000000000000";
    assert!(profiles::get(pool, missing).await.expect("get").is_none());
    assert!(
        profiles::update(pool, missing, "x", &new_data)
            .await
            .expect("update")
            .is_none()
    );

    assert!(profiles::delete(pool, &made.id).await.expect("delete"));
    assert!(
        !profiles::delete(pool, &made.id)
            .await
            .expect("delete again")
    );
    assert_eq!(profiles::list(pool).await.expect("list").len(), 1);

    // Upsert by name: creates, then replaces in place (case-insensitive), keeping id and created_at.
    let (fresh, created) = profiles::upsert_by_name(pool, "Gamma", &data)
        .await
        .expect("upsert new");
    assert!(created);
    let (again, created) = profiles::upsert_by_name(pool, "gAMMA", &new_data)
        .await
        .expect("upsert existing");
    assert!(!created);
    assert_eq!(
        (again.id.as_str(), again.name.as_str(), &again.data),
        (fresh.id.as_str(), "gAMMA", &new_data)
    );
    assert_eq!(again.created_at, fresh.created_at);
    assert!(again.updated_at >= fresh.updated_at);
    assert_eq!(profiles::list(pool).await.expect("list").len(), 2);
    db.drop().await;
}
