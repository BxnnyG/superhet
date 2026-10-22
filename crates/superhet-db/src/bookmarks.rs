//! Queries for bookmarks (saved frequencies). Values are validated and normalised by the API
//! (`docs/api-bookmarks.md`); the table's CHECK constraints are the last line of defence.

use sqlx::Row;

use crate::{Error, PgPool};

/// Most bookmarks that can be stored.
pub const MAX_BOOKMARKS: i64 = 10_000;

/// A stored bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    /// UUID in canonical lower-case text form.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Frequency in Hz.
    pub freq_hz: i64,
    /// Demodulation mode (`wfm|nfm|am|usb|lsb|cw`).
    pub mode: Option<String>,
    /// Channel bandwidth in Hz.
    pub bandwidth_hz: Option<i32>,
    /// Free-form labels.
    pub tags: Vec<String>,
    /// `#rrggbb`.
    pub color: Option<String>,
    /// Free text.
    pub notes: Option<String>,
    /// Creation time, Unix milliseconds.
    pub created_ms: i64,
    /// Last change, Unix milliseconds.
    pub updated_ms: i64,
}

/// The editable part of a bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBookmark {
    /// Display name.
    pub name: String,
    /// Frequency in Hz.
    pub freq_hz: i64,
    /// Demodulation mode.
    pub mode: Option<String>,
    /// Channel bandwidth in Hz.
    pub bandwidth_hz: Option<i32>,
    /// Free-form labels.
    pub tags: Vec<String>,
    /// `#rrggbb`.
    pub color: Option<String>,
    /// Free text.
    pub notes: Option<String>,
}

/// Why a write failed.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// Storing this would exceed [`MAX_BOOKMARKS`].
    #[error("bookmark limit reached")]
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

const COLUMNS: &str = "id::text AS id, name, freq_hz, mode, bandwidth_hz, tags, color, notes, \
    floor(extract(epoch FROM created_at) * 1000)::bigint AS created_ms, \
    floor(extract(epoch FROM updated_at) * 1000)::bigint AS updated_ms";

/// Serialises writers that check the total, so concurrent inserts cannot overshoot the limit.
const WRITE_LOCK: i64 = 0x5348_424d; // "SHBM"

fn bookmark(row: &sqlx::postgres::PgRow) -> Result<Bookmark, sqlx::Error> {
    Ok(Bookmark {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        freq_hz: row.try_get("freq_hz")?,
        mode: row.try_get("mode")?,
        bandwidth_hz: row.try_get("bandwidth_hz")?,
        tags: row.try_get("tags")?,
        color: row.try_get("color")?,
        notes: row.try_get("notes")?,
        created_ms: row.try_get("created_ms")?,
        updated_ms: row.try_get("updated_ms")?,
    })
}

const INSERT: &str = "INSERT INTO superhet.bookmarks \
    (name, freq_hz, mode, bandwidth_hz, tags, color, notes) VALUES ($1, $2, $3, $4, $5, $6, $7)";

type Query<'q> = sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>;

fn bind_new<'q>(query: Query<'q>, new: &'q NewBookmark) -> Query<'q> {
    query
        .bind(&new.name)
        .bind(new.freq_hz)
        .bind(new.mode.as_deref())
        .bind(new.bandwidth_hz)
        .bind(&new.tags)
        .bind(new.color.as_deref())
        .bind(new.notes.as_deref())
}

/// All bookmarks, by frequency.
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool) -> Result<Vec<Bookmark>, Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM superhet.bookmarks ORDER BY freq_hz, lower(name), id"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(bookmark).collect::<Result<_, _>>()?)
}

/// One bookmark by id (see [`crate::profiles::parse_id`]).
///
/// # Errors
/// Fails if the query fails.
pub async fn get(pool: &PgPool, id: &str) -> Result<Option<Bookmark>, Error> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM superhet.bookmarks WHERE id = $1::uuid"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(bookmark).transpose()?)
}

/// Insert a bookmark.
///
/// # Errors
/// [`WriteError::Limit`] when [`MAX_BOOKMARKS`] are stored already.
pub async fn create(pool: &PgPool, new: &NewBookmark) -> Result<Bookmark, WriteError> {
    let mut tx = pool.begin().await?;
    lock_and_check(&mut tx, 1, false).await?;
    let row = bind_new(
        sqlx::query(sqlx::AssertSqlSafe(format!("{INSERT} RETURNING {COLUMNS}"))),
        new,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(bookmark(&row)?)
}

/// Insert many bookmarks in one transaction, optionally deleting all existing ones first.
/// Returns the number inserted.
///
/// # Errors
/// [`WriteError::Limit`] if the result would exceed [`MAX_BOOKMARKS`]; nothing is changed then.
pub async fn create_many(
    pool: &PgPool,
    items: &[NewBookmark],
    replace: bool,
) -> Result<usize, WriteError> {
    let mut tx = pool.begin().await?;
    lock_and_check(&mut tx, items.len(), replace).await?;
    if replace {
        sqlx::query("DELETE FROM superhet.bookmarks")
            .execute(&mut *tx)
            .await?;
    }
    for item in items {
        bind_new(sqlx::query(INSERT), item)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(items.len())
}

async fn lock_and_check(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    adding: usize,
    replace: bool,
) -> Result<(), WriteError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(WRITE_LOCK)
        .execute(&mut **tx)
        .await?;
    let existing: i64 = if replace {
        0
    } else {
        sqlx::query_scalar("SELECT count(*) FROM superhet.bookmarks")
            .fetch_one(&mut **tx)
            .await?
    };
    let adding = i64::try_from(adding).unwrap_or(i64::MAX);
    if existing.saturating_add(adding) > MAX_BOOKMARKS {
        return Err(WriteError::Limit);
    }
    Ok(())
}

/// Replace the editable fields of a bookmark; `None` if it does not exist.
///
/// # Errors
/// Fails if the query fails.
pub async fn update(pool: &PgPool, id: &str, new: &NewBookmark) -> Result<Option<Bookmark>, Error> {
    let row = bind_new(
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE superhet.bookmarks SET name = $1, freq_hz = $2, mode = $3, \
             bandwidth_hz = $4, tags = $5, color = $6, notes = $7, updated_at = now() \
             WHERE id = $8::uuid RETURNING {COLUMNS}"
        ))),
        new,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(bookmark).transpose()?)
}

/// Delete a bookmark; `false` if it did not exist.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete(pool: &PgPool, id: &str) -> Result<bool, Error> {
    let done = sqlx::query("DELETE FROM superhet.bookmarks WHERE id = $1::uuid")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}
