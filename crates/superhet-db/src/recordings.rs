//! Queries for the recordings library (`docs/adr/0028-recordings-and-instant-replay.md`). The
//! files live on disk; a row is the index entry. Times are Unix milliseconds in this API.

use serde_json::Value;
use sqlx::Row;

use crate::{Error, PgPool};

/// One recording.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// Row id.
    pub id: i64,
    /// `audio` or `iq`.
    pub kind: String,
    /// `manual`, `replay` or `scanner`.
    pub source: String,
    /// `recording`, `done`, `interrupted` or `failed`.
    pub state: String,
    /// File stem (generated, `[A-Za-z0-9_-]`).
    pub stem: String,
    /// Display name.
    pub title: String,
    /// Device id.
    pub device: String,
    /// VFO the audio came from, if any.
    pub vfo: Option<i32>,
    /// Tuned frequency in Hz.
    pub freq_hz: Option<i64>,
    /// Demodulation mode.
    pub mode: Option<String>,
    /// Account that started it (display name).
    pub owner_name: Option<String>,
    /// Account id, `u:<id>`.
    pub owner_id: Option<String>,
    /// Start, Unix milliseconds.
    pub started_ms: i64,
    /// End, Unix milliseconds.
    pub stopped_ms: Option<i64>,
    /// Audio or sample duration in milliseconds.
    pub duration_ms: i64,
    /// Bytes on disk.
    pub size_bytes: i64,
    /// Exempt from quota cleanup.
    pub keep: bool,
    /// Extra metadata (segments, tags, IQ parameters).
    pub meta: Value,
}

/// A recording about to start.
#[derive(Debug, Clone, PartialEq)]
pub struct New {
    /// Pre-allocated id, see [`reserve_id`].
    pub id: i64,
    /// `audio` or `iq`.
    pub kind: String,
    /// `manual`, `replay` or `scanner`.
    pub source: String,
    /// File stem.
    pub stem: String,
    /// Display name.
    pub title: String,
    /// Device id.
    pub device: String,
    /// VFO.
    pub vfo: Option<i32>,
    /// Frequency in Hz.
    pub freq_hz: Option<i64>,
    /// Mode.
    pub mode: Option<String>,
    /// Owner display name.
    pub owner_name: Option<String>,
    /// Owner id.
    pub owner_id: Option<String>,
    /// Start, Unix milliseconds.
    pub started_ms: i64,
    /// Initial state (`recording`, or `done` for saved replays).
    pub state: String,
    /// Duration (already known for saved replays).
    pub duration_ms: i64,
    /// Size (already known for saved replays).
    pub size_bytes: i64,
    /// Metadata.
    pub meta: Value,
}

/// What a finished recording reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Finish {
    /// New state.
    pub state: String,
    /// End, Unix milliseconds.
    pub stopped_ms: i64,
    /// Duration in milliseconds.
    pub duration_ms: i64,
    /// Bytes on disk.
    pub size_bytes: i64,
    /// Final metadata.
    pub meta: Value,
}

/// List filter.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only this device.
    pub device: Option<String>,
    /// Leave these devices out (closed to the caller).
    pub exclude_devices: Vec<String>,
    /// `audio` or `iq`.
    pub kind: Option<String>,
    /// Only this state.
    pub state: Option<String>,
    /// Only kept / not kept.
    pub keep: Option<bool>,
    /// Title substring (case-insensitive).
    pub search: Option<String>,
    /// Only rows with a smaller id than this (paging, newest first).
    pub before_id: Option<i64>,
    /// Page size.
    pub limit: i64,
}

macro_rules! columns {
    () => {
        "id, kind, source, state, stem, title, device, vfo, freq_hz, mode, \
     owner_name, owner_id, (extract(epoch from started_at) * 1000)::bigint AS started_ms, \
     (extract(epoch from stopped_at) * 1000)::bigint AS stopped_ms, duration_ms, size_bytes, \
     keep, meta::text AS meta"
    };
}

fn record(row: &sqlx::postgres::PgRow) -> Result<Record, sqlx::Error> {
    let meta: String = row.try_get("meta")?;
    Ok(Record {
        id: row.try_get("id")?,
        kind: row.try_get("kind")?,
        source: row.try_get("source")?,
        state: row.try_get("state")?,
        stem: row.try_get("stem")?,
        title: row.try_get("title")?,
        device: row.try_get("device")?,
        vfo: row.try_get("vfo")?,
        freq_hz: row.try_get("freq_hz")?,
        mode: row.try_get("mode")?,
        owner_name: row.try_get("owner_name")?,
        owner_id: row.try_get("owner_id")?,
        started_ms: row.try_get("started_ms")?,
        stopped_ms: row.try_get("stopped_ms")?,
        duration_ms: row.try_get("duration_ms")?,
        size_bytes: row.try_get("size_bytes")?,
        keep: row.try_get("keep")?,
        meta: serde_json::from_str(&meta).unwrap_or(Value::Null),
    })
}

/// Reserve the id of a recording before its file name is made.
///
/// # Errors
/// Fails if the query fails.
pub async fn reserve_id(pool: &PgPool) -> Result<i64, Error> {
    let row = sqlx::query("SELECT nextval(pg_get_serial_sequence('superhet.recordings', 'id'))")
        .fetch_one(pool)
        .await?;
    Ok(row.try_get(0)?)
}

/// Insert a recording.
///
/// # Errors
/// Fails if the query fails.
pub async fn insert(pool: &PgPool, new: &New) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO superhet.recordings \
            (id, kind, source, state, stem, title, device, vfo, freq_hz, mode, owner_name, \
             owner_id, started_at, stopped_at, duration_ms, size_bytes, meta) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, \
             to_timestamp($13::double precision / 1000.0), \
             CASE WHEN $4 = 'recording' THEN NULL \
                  ELSE to_timestamp(($13 + $14)::double precision / 1000.0) END, \
             $14, $15, $16::text::jsonb)",
    )
    .bind(new.id)
    .bind(&new.kind)
    .bind(&new.source)
    .bind(&new.state)
    .bind(&new.stem)
    .bind(&new.title)
    .bind(&new.device)
    .bind(new.vfo)
    .bind(new.freq_hz)
    .bind(&new.mode)
    .bind(&new.owner_name)
    .bind(&new.owner_id)
    .bind(new.started_ms)
    .bind(new.duration_ms)
    .bind(new.size_bytes)
    .bind(new.meta.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// Record how a recording ended.
///
/// # Errors
/// Fails if the query fails.
pub async fn finish(pool: &PgPool, id: i64, done: &Finish) -> Result<(), Error> {
    sqlx::query(
        "UPDATE superhet.recordings SET state = $2, \
            stopped_at = to_timestamp($3::double precision / 1000.0), duration_ms = $4, \
            size_bytes = $5, meta = $6::text::jsonb WHERE id = $1",
    )
    .bind(id)
    .bind(&done.state)
    .bind(done.stopped_ms)
    .bind(done.duration_ms)
    .bind(done.size_bytes)
    .bind(done.meta.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// One recording by id.
///
/// # Errors
/// Fails if the query fails.
pub async fn get(pool: &PgPool, id: i64) -> Result<Option<Record>, Error> {
    let row = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM superhet.recordings WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(record).transpose()?)
}

/// Recordings, newest first.
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool, filter: &Filter) -> Result<Vec<Record>, Error> {
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM superhet.recordings \
         WHERE ($1::text IS NULL OR device = $1) \
           AND NOT (device = ANY($2::text[])) \
           AND ($3::text IS NULL OR kind = $3) \
           AND ($4::text IS NULL OR state = $4) \
           AND ($5::boolean IS NULL OR keep = $5) \
           AND ($6::text IS NULL OR title ILIKE '%' || $6 || '%') \
           AND ($7::bigint IS NULL OR id < $7) \
         ORDER BY id DESC LIMIT $8"
    ))
    .bind(&filter.device)
    .bind(&filter.exclude_devices)
    .bind(&filter.kind)
    .bind(&filter.state)
    .bind(filter.keep)
    .bind(filter.search.as_deref().map(escape_like))
    .bind(filter.before_id)
    .bind(filter.limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(record).collect::<Result<_, _>>()?)
}

/// Escape LIKE wildcards so a search text is taken literally.
fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Rename and/or change the keep flag. Returns whether the row exists.
///
/// # Errors
/// Fails if the query fails.
pub async fn update(
    pool: &PgPool,
    id: i64,
    title: Option<&str>,
    keep: Option<bool>,
) -> Result<bool, Error> {
    let done = sqlx::query(
        "UPDATE superhet.recordings SET title = COALESCE($2, title), keep = COALESCE($3, keep) \
         WHERE id = $1",
    )
    .bind(id)
    .bind(title)
    .bind(keep)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Delete the row and return it (so the caller can remove the files).
///
/// # Errors
/// Fails if the query fails.
pub async fn delete(pool: &PgPool, id: i64) -> Result<Option<Record>, Error> {
    let row = sqlx::query(concat!(
        "DELETE FROM superhet.recordings WHERE id = $1 RETURNING ",
        columns!()
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(record).transpose()?)
}

/// Every finished recording as `(id, size_bytes, started_ms, keep)` for quota planning.
///
/// # Errors
/// Fails if the query fails.
pub async fn finished_sizes(pool: &PgPool) -> Result<Vec<(i64, i64, i64, bool)>, Error> {
    let rows = sqlx::query(
        "SELECT id, size_bytes, (extract(epoch from started_at) * 1000)::bigint AS started_ms, keep \
         FROM superhet.recordings WHERE state <> 'recording'",
    )
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            Ok((
                r.try_get("id")?,
                r.try_get("size_bytes")?,
                r.try_get("started_ms")?,
                r.try_get("keep")?,
            ))
        })
        .collect::<Result<_, sqlx::Error>>()
        .map_err(Into::into)
}

/// Rows still marked `recording` (left over from a previous run) become `interrupted`; returns
/// them so the caller can fill in the real file size.
///
/// # Errors
/// Fails if the query fails.
pub async fn mark_interrupted(pool: &PgPool) -> Result<Vec<Record>, Error> {
    let rows = sqlx::query(concat!(
        "UPDATE superhet.recordings SET state = 'interrupted', stopped_at = now() \
         WHERE state = 'recording' RETURNING ",
        columns!()
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(record).collect::<Result<_, _>>()?)
}

/// Set the size of a row (after the real file was measured).
///
/// # Errors
/// Fails if the query fails.
pub async fn set_size(pool: &PgPool, id: i64, size_bytes: i64) -> Result<(), Error> {
    sqlx::query("UPDATE superhet.recordings SET size_bytes = $2 WHERE id = $1")
        .bind(id)
        .bind(size_bytes)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_wildcards_are_escaped() {
        assert_eq!(escape_like("50%_a\\b"), "50\\%\\_a\\\\b");
    }

    /// Needs a database: `DATABASE_URL=postgres://... cargo test -p superhet-db`.
    #[tokio::test]
    async fn rows_round_trip_and_cleanup_queries_work() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let pool = crate::connect(&url).await.expect("connect");
        crate::migrate(&pool).await.expect("migrate");
        let id = reserve_id(&pool).await.expect("id");
        let new = New {
            id,
            kind: "audio".into(),
            source: "manual".into(),
            stem: format!("rec{id}"),
            title: "Tower 118.3".into(),
            device: "dbtest".into(),
            vfo: Some(1),
            freq_hz: Some(118_300_000),
            mode: Some("am".into()),
            owner_name: Some("op".into()),
            owner_id: Some("u:1".into()),
            started_ms: 1_700_000_000_000,
            state: "recording".into(),
            duration_ms: 0,
            size_bytes: 0,
            meta: serde_json::json!({"segments": []}),
        };
        insert(&pool, &new).await.expect("insert");
        let row = get(&pool, id).await.expect("get").expect("row");
        assert_eq!(row.state, "recording");
        assert_eq!(row.started_ms, 1_700_000_000_000);
        assert!(row.stopped_ms.is_none());
        finish(
            &pool,
            id,
            &Finish {
                state: "done".into(),
                stopped_ms: 1_700_000_060_000,
                duration_ms: 60_000,
                size_bytes: 480_000,
                meta: serde_json::json!({"ps": "ABC"}),
            },
        )
        .await
        .expect("finish");
        assert!(
            update(&pool, id, Some("Renamed"), Some(true))
                .await
                .expect("update")
        );
        let listed = list(
            &pool,
            &Filter {
                device: Some("dbtest".into()),
                search: Some("rena".into()),
                limit: 10,
                ..Filter::default()
            },
        )
        .await
        .expect("list");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].keep && listed[0].meta["ps"] == "ABC" && listed[0].size_bytes == 480_000);
        let hidden = list(
            &pool,
            &Filter {
                exclude_devices: vec!["dbtest".into()],
                search: Some("rena".into()),
                limit: 10,
                ..Filter::default()
            },
        )
        .await
        .expect("list");
        assert!(hidden.is_empty());
        assert!(
            finished_sizes(&pool)
                .await
                .expect("sizes")
                .iter()
                .any(|r| r.0 == id && r.3)
        );
        assert!(delete(&pool, id).await.expect("delete").is_some());
        assert!(get(&pool, id).await.expect("get").is_none());
    }
}
