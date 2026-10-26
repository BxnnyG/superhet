//! PostgreSQL access layer shared by Superhet services.

pub mod auth;
pub mod bookmarks;
pub mod decoder_events;
pub mod profiles;

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;

pub use sqlx::PgPool;

/// Environment variable holding the PostgreSQL connection URL.
pub const ENV_URL: &str = "SUPERHET_DATABASE_URL";

/// Errors of the database layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Connecting or querying failed.
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Applying migrations failed.
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// Open a connection pool for `url` and verify that the server is reachable.
///
/// # Errors
/// Fails if the server cannot be reached within the acquire timeout.
pub async fn connect(url: &str) -> Result<PgPool, Error> {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await?;
    Ok(pool)
}

/// Connect using `SUPERHET_DATABASE_URL`; returns `None` when it is unset or empty.
///
/// # Errors
/// Fails if the server cannot be reached.
pub async fn connect_from_env() -> Result<Option<PgPool>, Error> {
    match std::env::var(ENV_URL) {
        Ok(url) if !url.is_empty() => Ok(Some(connect(&url).await?)),
        _ => Ok(None),
    }
}

/// Apply all embedded migrations that have not run yet.
///
/// # Errors
/// Fails if a migration cannot be applied or the history diverged.
pub async fn migrate(pool: &PgPool) -> Result<(), Error> {
    // Pin the bookkeeping table to our schema. Unqualified, it would land in `public`
    // on the first run and in `superhet` (via the `$user` search_path entry) afterwards.
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.create_schema("superhet");
    migrator.dangerous_set_table_name("superhet._sqlx_migrations");
    migrator.run(pool).await?;
    Ok(())
}

/// Cheap liveness probe for readiness checks.
///
/// # Errors
/// Fails if the database does not answer.
pub async fn ping(pool: &PgPool) -> Result<(), Error> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}
