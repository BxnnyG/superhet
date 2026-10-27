//! Queries for custom channel plans (`docs/api-channel-plans.md`). The document is validated by
//! the API; this layer stores it as JSON next to its display name.

use sqlx::Row;

use crate::{Error, PgPool};

/// Most custom plans that can be stored.
pub const MAX_PLANS: i64 = 100;

/// Serialises writers that check the total, so concurrent inserts cannot overshoot the limit.
const WRITE_LOCK: i64 = 0x5348_4350; // "SHCP"

/// A stored plan.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredPlan {
    /// UUID in canonical lower-case text form.
    pub id: String,
    /// Display name (also inside the document).
    pub name: String,
    /// The plan document.
    pub doc: serde_json::Value,
    /// Creation time, Unix milliseconds.
    pub created_ms: i64,
    /// Last change, Unix milliseconds.
    pub updated_ms: i64,
}

/// Why a write failed.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// Storing this would exceed [`MAX_PLANS`].
    #[error("limit reached")]
    Limit,
    /// Any other database failure.
    #[error(transparent)]
    Db(#[from] Error),
}

impl From<sqlx::Error> for WriteError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err.into())
    }
}

const COLUMNS: &str = "id::text AS id, name, doc::text AS doc, \
    floor(extract(epoch FROM created_at) * 1000)::bigint AS created_ms, \
    floor(extract(epoch FROM updated_at) * 1000)::bigint AS updated_ms";

fn plan(row: &sqlx::postgres::PgRow) -> Result<StoredPlan, sqlx::Error> {
    let doc: String = row.try_get("doc")?;
    Ok(StoredPlan {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        doc: serde_json::from_str(&doc).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        created_ms: row.try_get("created_ms")?,
        updated_ms: row.try_get("updated_ms")?,
    })
}

/// All plans, by name.
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool) -> Result<Vec<StoredPlan>, Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM superhet.channel_plans ORDER BY lower(name), id"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(plan).collect::<Result<_, _>>()?)
}

/// Insert a plan.
///
/// # Errors
/// [`WriteError::Limit`] when [`MAX_PLANS`] are stored already.
pub async fn create(
    pool: &PgPool,
    name: &str,
    doc: &serde_json::Value,
) -> Result<StoredPlan, WriteError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(WRITE_LOCK)
        .execute(&mut *tx)
        .await?;
    let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM superhet.channel_plans")
        .fetch_one(&mut *tx)
        .await?;
    if existing >= MAX_PLANS {
        return Err(WriteError::Limit);
    }
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO superhet.channel_plans (name, doc) VALUES ($1, $2::text::jsonb) \
         RETURNING {COLUMNS}"
    )))
    .bind(name)
    .bind(doc.to_string())
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(plan(&row)?)
}

/// Replace name and document of a plan; `None` if it does not exist.
///
/// # Errors
/// Fails if the query fails.
pub async fn update(
    pool: &PgPool,
    id: &str,
    name: &str,
    doc: &serde_json::Value,
) -> Result<Option<StoredPlan>, Error> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE superhet.channel_plans SET name = $2, doc = $3::text::jsonb, updated_at = now() \
         WHERE id = $1::uuid RETURNING {COLUMNS}"
    )))
    .bind(id)
    .bind(name)
    .bind(doc.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(plan).transpose()?)
}

/// Delete a plan; `false` if it did not exist.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete(pool: &PgPool, id: &str) -> Result<bool, Error> {
    let done = sqlx::query("DELETE FROM superhet.channel_plans WHERE id = $1::uuid")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}
