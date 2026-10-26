//! Integration test against a real PostgreSQL; runs only when `DATABASE_URL` is set.

#[tokio::test]
async fn migrations_apply_and_are_idempotent() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping database integration test");
        return;
    };

    let pool = superhet_db::connect(&url).await.expect("connect");
    superhet_db::migrate(&pool).await.expect("first migrate");
    superhet_db::migrate(&pool).await.expect("second migrate");
    superhet_db::ping(&pool).await.expect("ping");

    let version: i32 = sqlx::query_scalar("SELECT version FROM superhet.schema_version")
        .fetch_one(&pool)
        .await
        .expect("schema_version row");
    assert_eq!(version, 7);
}
