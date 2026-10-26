//! Queries for bookmarks (saved frequencies). Values are validated and normalised by the API
//! (`docs/api-bookmarks.md`); the table's CHECK constraints are the last line of defence.

use sqlx::Row;

use crate::{Error, PgPool};

/// Most bookmarks that can be stored.
pub const MAX_BOOKMARKS: i64 = 10_000;

/// Parameters of a digital channel (TETRA, DMR, dPMR, P25).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digital {
    /// `tetra|dmr|dpmr|p25`.
    pub protocol: String,
    /// TETRA 1..=4, DMR 1..=2.
    pub timeslot: Option<i16>,
    /// Talkgroup / GSSI.
    pub talkgroup: Option<i64>,
    /// DMR colour code 0..=15.
    pub colour_code: Option<i16>,
    /// Mobile country code.
    pub mcc: Option<i16>,
    /// Mobile network code.
    pub mnc: Option<i16>,
    /// Plugin that should decode this channel.
    pub decoder_plugin_id: Option<String>,
}

/// The editable part of a bookmark.
#[derive(Debug, Clone, PartialEq)]
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
    /// Squelch threshold in dBFS (-120..=0).
    pub squelch_db: Option<i16>,
    /// The squelch is set automatically at tune time.
    pub squelch_auto: bool,
    /// Tuning step in Hz.
    pub step_hz: Option<i32>,
    /// `none|ctcss|dcs`.
    pub tone_mode: Option<String>,
    /// CTCSS tone in Hz.
    pub ctcss_hz: Option<f64>,
    /// DCS code (the three octal digits read as a decimal number, e.g. 023 -> 23).
    pub dcs_code: Option<i16>,
    /// DCS polarity `N|R`.
    pub dcs_polarity: Option<String>,
    /// Digital channel parameters.
    pub digital: Option<Digital>,
    /// Group (UUID text) or none.
    pub group_id: Option<String>,
    /// Marked as favourite.
    pub favourite: bool,
    /// Take part in scans.
    pub scan_include: bool,
    /// Scan priority 0..=9.
    pub scan_priority: Option<i16>,
    /// Scan dwell time in ms.
    pub scan_dwell_ms: Option<i32>,
}

/// A stored bookmark.
#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    /// UUID in canonical lower-case text form.
    pub id: String,
    /// The editable fields.
    pub data: NewBookmark,
    /// Creation time, Unix milliseconds.
    pub created_ms: i64,
    /// Last change, Unix milliseconds.
    pub updated_ms: i64,
}

/// An ordered folder of bookmarks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// UUID in canonical lower-case text form.
    pub id: String,
    /// Display name, unique ignoring case.
    pub name: String,
    /// Sort position (ascending).
    pub position: i32,
}

/// Most groups that can be stored.
pub const MAX_GROUPS: i64 = 200;

/// Why a write failed.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// Storing this would exceed [`MAX_BOOKMARKS`] (or [`MAX_GROUPS`]).
    #[error("limit reached")]
    Limit,
    /// The referenced group does not exist.
    #[error("unknown group")]
    UnknownGroup,
    /// A group with this name exists already.
    #[error("duplicate group name")]
    Duplicate,
    /// Any other database failure.
    #[error(transparent)]
    Db(#[from] Error),
}

impl From<sqlx::Error> for WriteError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &err {
            if db.is_foreign_key_violation() {
                return Self::UnknownGroup;
            }
            if db.is_unique_violation() {
                return Self::Duplicate;
            }
        }
        Self::Db(err.into())
    }
}

const COLUMNS: &str = "id::text AS id, name, freq_hz, mode, bandwidth_hz, tags, color, notes, \
    squelch_db, squelch_auto, step_hz, tone_mode, ctcss_hz, dcs_code, dcs_polarity, \
    digital_protocol, digital_timeslot, digital_talkgroup, digital_colour_code, digital_mcc, \
    digital_mnc, digital_decoder, group_id::text AS group_id, favourite, scan_include, \
    scan_priority, scan_dwell_ms, \
    floor(extract(epoch FROM created_at) * 1000)::bigint AS created_ms, \
    floor(extract(epoch FROM updated_at) * 1000)::bigint AS updated_ms";

/// Serialises writers that check the total, so concurrent inserts cannot overshoot the limit.
const WRITE_LOCK: i64 = 0x5348_424d; // "SHBM"
/// Serialises group writers (limit, positions).
const GROUP_LOCK: i64 = 0x5348_4247; // "SHBG"

fn bookmark(row: &sqlx::postgres::PgRow) -> Result<Bookmark, sqlx::Error> {
    let protocol: Option<String> = row.try_get("digital_protocol")?;
    let digital = match protocol {
        Some(protocol) => Some(Digital {
            protocol,
            timeslot: row.try_get("digital_timeslot")?,
            talkgroup: row.try_get("digital_talkgroup")?,
            colour_code: row.try_get("digital_colour_code")?,
            mcc: row.try_get("digital_mcc")?,
            mnc: row.try_get("digital_mnc")?,
            decoder_plugin_id: row.try_get("digital_decoder")?,
        }),
        None => None,
    };
    Ok(Bookmark {
        id: row.try_get("id")?,
        data: NewBookmark {
            name: row.try_get("name")?,
            freq_hz: row.try_get("freq_hz")?,
            mode: row.try_get("mode")?,
            bandwidth_hz: row.try_get("bandwidth_hz")?,
            tags: row.try_get("tags")?,
            color: row.try_get("color")?,
            notes: row.try_get("notes")?,
            squelch_db: row.try_get("squelch_db")?,
            squelch_auto: row.try_get("squelch_auto")?,
            step_hz: row.try_get("step_hz")?,
            tone_mode: row.try_get("tone_mode")?,
            ctcss_hz: row.try_get("ctcss_hz")?,
            dcs_code: row.try_get("dcs_code")?,
            dcs_polarity: row.try_get("dcs_polarity")?,
            digital,
            group_id: row.try_get("group_id")?,
            favourite: row.try_get("favourite")?,
            scan_include: row.try_get("scan_include")?,
            scan_priority: row.try_get("scan_priority")?,
            scan_dwell_ms: row.try_get("scan_dwell_ms")?,
        },
        created_ms: row.try_get("created_ms")?,
        updated_ms: row.try_get("updated_ms")?,
    })
}

const INSERT: &str = "INSERT INTO superhet.bookmarks \
    (name, freq_hz, mode, bandwidth_hz, tags, color, notes, squelch_db, squelch_auto, step_hz, \
     tone_mode, ctcss_hz, dcs_code, dcs_polarity, digital_protocol, digital_timeslot, \
     digital_talkgroup, digital_colour_code, digital_mcc, digital_mnc, digital_decoder, \
     group_id, favourite, scan_include, scan_priority, scan_dwell_ms) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, \
            $19, $20, $21, $22::uuid, $23, $24, $25, $26)";

const UPDATE: &str = "UPDATE superhet.bookmarks SET name = $1, freq_hz = $2, mode = $3, \
    bandwidth_hz = $4, tags = $5, color = $6, notes = $7, squelch_db = $8, squelch_auto = $9, \
    step_hz = $10, tone_mode = $11, ctcss_hz = $12, dcs_code = $13, dcs_polarity = $14, \
    digital_protocol = $15, digital_timeslot = $16, digital_talkgroup = $17, \
    digital_colour_code = $18, digital_mcc = $19, digital_mnc = $20, digital_decoder = $21, \
    group_id = $22::uuid, favourite = $23, scan_include = $24, scan_priority = $25, \
    scan_dwell_ms = $26, updated_at = now()";

type Query<'q> = sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>;

fn bind_new<'q>(query: Query<'q>, new: &'q NewBookmark) -> Query<'q> {
    let d = new.digital.as_ref();
    query
        .bind(&new.name)
        .bind(new.freq_hz)
        .bind(new.mode.as_deref())
        .bind(new.bandwidth_hz)
        .bind(&new.tags)
        .bind(new.color.as_deref())
        .bind(new.notes.as_deref())
        .bind(new.squelch_db)
        .bind(new.squelch_auto)
        .bind(new.step_hz)
        .bind(new.tone_mode.as_deref())
        .bind(new.ctcss_hz)
        .bind(new.dcs_code)
        .bind(new.dcs_polarity.as_deref())
        .bind(d.map(|d| d.protocol.as_str()))
        .bind(d.and_then(|d| d.timeslot))
        .bind(d.and_then(|d| d.talkgroup))
        .bind(d.and_then(|d| d.colour_code))
        .bind(d.and_then(|d| d.mcc))
        .bind(d.and_then(|d| d.mnc))
        .bind(d.and_then(|d| d.decoder_plugin_id.as_deref()))
        .bind(new.group_id.as_deref())
        .bind(new.favourite)
        .bind(new.scan_include)
        .bind(new.scan_priority)
        .bind(new.scan_dwell_ms)
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
/// [`WriteError::Limit`] when [`MAX_BOOKMARKS`] are stored already, [`WriteError::UnknownGroup`]
/// when `group_id` does not exist.
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
/// [`WriteError::Limit`] if the result would exceed [`MAX_BOOKMARKS`], [`WriteError::UnknownGroup`]
/// for a missing group; nothing is changed then.
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
/// [`WriteError::UnknownGroup`] when `group_id` does not exist.
pub async fn update(
    pool: &PgPool,
    id: &str,
    new: &NewBookmark,
) -> Result<Option<Bookmark>, WriteError> {
    let row = bind_new(
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{UPDATE} WHERE id = $27::uuid RETURNING {COLUMNS}"
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

// ---------- groups ----------

fn group(row: &sqlx::postgres::PgRow) -> Result<Group, sqlx::Error> {
    Ok(Group {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        position: row.try_get("position")?,
    })
}

const GROUP_COLUMNS: &str = "id::text AS id, name, position";

/// All groups, in display order.
///
/// # Errors
/// Fails if the query fails.
pub async fn list_groups(pool: &PgPool) -> Result<Vec<Group>, Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {GROUP_COLUMNS} FROM superhet.bookmark_groups ORDER BY position, lower(name), id"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(group).collect::<Result<_, _>>()?)
}

/// Create a group at the end of the order.
///
/// # Errors
/// [`WriteError::Limit`] at [`MAX_GROUPS`], [`WriteError::Duplicate`] for a taken name.
pub async fn create_group(pool: &PgPool, name: &str) -> Result<Group, WriteError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(GROUP_LOCK)
        .execute(&mut *tx)
        .await?;
    let (count, next): (i64, i32) = sqlx::query_as(
        "SELECT count(*), COALESCE(max(position) + 1, 0)::int FROM superhet.bookmark_groups",
    )
    .fetch_one(&mut *tx)
    .await?;
    if count >= MAX_GROUPS {
        return Err(WriteError::Limit);
    }
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO superhet.bookmark_groups (name, position) VALUES ($1, $2) \
         RETURNING {GROUP_COLUMNS}"
    )))
    .bind(name)
    .bind(next)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(group(&row)?)
}

/// Rename a group; `None` if it does not exist.
///
/// # Errors
/// [`WriteError::Duplicate`] for a taken name.
pub async fn rename_group(
    pool: &PgPool,
    id: &str,
    name: &str,
) -> Result<Option<Group>, WriteError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE superhet.bookmark_groups SET name = $1, updated_at = now() \
         WHERE id = $2::uuid RETURNING {GROUP_COLUMNS}"
    )))
    .bind(name)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(group).transpose()?)
}

/// Delete a group; its bookmarks stay and become ungrouped. `false` if it did not exist.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete_group(pool: &PgPool, id: &str) -> Result<bool, Error> {
    let done = sqlx::query("DELETE FROM superhet.bookmark_groups WHERE id = $1::uuid")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Set the display order: the listed groups come first in that order, groups not listed follow
/// in their previous order. Returns `false` (and changes nothing) if an id is unknown.
///
/// # Errors
/// Fails if a query fails.
pub async fn reorder_groups(pool: &PgPool, ids: &[String]) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(GROUP_LOCK)
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {GROUP_COLUMNS} FROM superhet.bookmark_groups ORDER BY position, lower(name), id"
    )))
    .fetch_all(&mut *tx)
    .await?;
    let existing: Vec<Group> = rows.iter().map(group).collect::<Result<_, _>>()?;
    let mut order: Vec<String> = Vec::with_capacity(existing.len());
    for id in ids {
        if !existing.iter().any(|g| &g.id == id) {
            return Ok(false);
        }
        if !order.contains(id) {
            order.push(id.clone());
        }
    }
    for g in &existing {
        if !order.contains(&g.id) {
            order.push(g.id.clone());
        }
    }
    for (position, id) in order.iter().enumerate() {
        sqlx::query("UPDATE superhet.bookmark_groups SET position = $1 WHERE id = $2::uuid")
            .bind(i32::try_from(position).unwrap_or(i32::MAX))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(true)
}
