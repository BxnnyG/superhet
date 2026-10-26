//! Small JSON settings documents (`superhet.settings`): guest policy, audit settings, limits.

use serde_json::Value;
use sqlx::Row;

use crate::{Error, PgPool};

/// Read a document.
///
/// # Errors
/// Fails if the query fails.
pub async fn get(pool: &PgPool, key: &str) -> Result<Option<Value>, Error> {
    let row = sqlx::query("SELECT value FROM superhet.settings WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("value")))
}

/// Create or replace a document.
///
/// # Errors
/// Fails if the query fails.
pub async fn put(pool: &PgPool, key: &str, value: &Value, updated_by: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO superhet.settings (key, value, updated_by) VALUES ($1, $2, $3) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now(), \
            updated_by = EXCLUDED.updated_by",
    )
    .bind(key)
    .bind(value)
    .bind(updated_by)
    .execute(pool)
    .await?;
    Ok(())
}
