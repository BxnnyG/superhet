//! Public receiver share links (`docs/adr/0029-share-links.md`). The token itself is never
//! stored, only its SHA-256.

use sqlx::{PgPool, Row, postgres::PgRow};

use crate::Error;

/// One link as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Row id (`0` before insertion).
    pub id: i64,
    /// SHA-256 of the token.
    pub token_hash: [u8; 32],
    /// Last characters of the token, to tell links apart in the admin list.
    pub token_hint: String,
    /// Admin-only name.
    pub label: String,
    /// Station name shown on the page.
    pub title: String,
    /// Free text shown on the page.
    pub description: String,
    /// Free location text shown on the page.
    pub location: String,
    /// Device id.
    pub device: String,
    /// Listeners follow the operator instead of getting a VFO of their own.
    pub follow: bool,
    /// Fixed links: start frequency.
    pub freq_hz: Option<i64>,
    /// Fixed links: demodulation mode.
    pub mode: Option<String>,
    /// Fixed links: lowest frequency a listener may retune to.
    pub tune_min_hz: Option<i64>,
    /// Fixed links: highest frequency a listener may retune to.
    pub tune_max_hz: Option<i64>,
    /// Allowed features.
    pub features: Vec<String>,
    /// Concurrent connections per kind.
    pub max_listeners: i32,
    /// Origins that may frame the embed page.
    pub embed_origins: Vec<String>,
    /// Expiry, Unix milliseconds.
    pub expires_at_ms: Option<i64>,
    /// Revocation, Unix milliseconds.
    pub revoked_at_ms: Option<i64>,
    /// Creation, Unix milliseconds.
    pub created_at_ms: i64,
    /// Name of the admin who made it.
    pub created_by: Option<String>,
}

macro_rules! columns {
    () => {
        "id, token_hash, token_hint, label, title, description, location, device, \
    follow, freq_hz, mode, tune_min_hz, tune_max_hz, features, max_listeners, embed_origins, \
    (extract(epoch FROM expires_at) * 1000)::bigint AS expires_ms, \
    (extract(epoch FROM revoked_at) * 1000)::bigint AS revoked_ms, \
    (extract(epoch FROM created_at) * 1000)::bigint AS created_ms, created_by"
    };
}

fn record(r: &PgRow) -> Result<Record, sqlx::Error> {
    let hash: Vec<u8> = r.try_get("token_hash")?;
    Ok(Record {
        id: r.try_get("id")?,
        token_hash: hash
            .try_into()
            .map_err(|_| sqlx::Error::Decode("share link token hash is not 32 bytes".into()))?,
        token_hint: r.try_get("token_hint")?,
        label: r.try_get("label")?,
        title: r.try_get("title")?,
        description: r.try_get("description")?,
        location: r.try_get("location")?,
        device: r.try_get("device")?,
        follow: r.try_get("follow")?,
        freq_hz: r.try_get("freq_hz")?,
        mode: r.try_get("mode")?,
        tune_min_hz: r.try_get("tune_min_hz")?,
        tune_max_hz: r.try_get("tune_max_hz")?,
        features: r.try_get("features")?,
        max_listeners: r.try_get("max_listeners")?,
        embed_origins: r.try_get("embed_origins")?,
        expires_at_ms: r.try_get("expires_ms")?,
        revoked_at_ms: r.try_get("revoked_ms")?,
        created_at_ms: r.try_get("created_ms")?,
        created_by: r.try_get("created_by")?,
    })
}

/// Milliseconds as the double `to_timestamp` wants (a `NULL` stays `NULL`).
fn ms_arg(ms: Option<i64>) -> Option<f64> {
    // Dates stay far below 2^53 ms, so the conversion is exact.
    #[allow(clippy::cast_precision_loss)]
    ms.map(|ms| ms as f64)
}

/// Every link, newest first.
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool) -> Result<Vec<Record>, Error> {
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM superhet.share_links ORDER BY id DESC"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(record).collect::<Result<_, _>>()?)
}

/// Insert a link; returns its id.
///
/// # Errors
/// Fails if the query or one of the table's checks fails.
pub async fn insert(pool: &PgPool, r: &Record) -> Result<i64, Error> {
    let row = sqlx::query(
        "INSERT INTO superhet.share_links (token_hash, token_hint, label, title, description, \
            location, device, follow, freq_hz, mode, tune_min_hz, tune_max_hz, features, \
            max_listeners, embed_origins, expires_at, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, \
            to_timestamp($16::double precision / 1000), $17) RETURNING id",
    )
    .bind(r.token_hash.as_slice())
    .bind(&r.token_hint)
    .bind(&r.label)
    .bind(&r.title)
    .bind(&r.description)
    .bind(&r.location)
    .bind(&r.device)
    .bind(r.follow)
    .bind(r.freq_hz)
    .bind(&r.mode)
    .bind(r.tune_min_hz)
    .bind(r.tune_max_hz)
    .bind(&r.features)
    .bind(r.max_listeners)
    .bind(&r.embed_origins)
    .bind(ms_arg(r.expires_at_ms))
    .bind(&r.created_by)
    .fetch_one(pool)
    .await?;
    Ok(row.try_get("id")?)
}

/// Replace everything an admin may edit (not the token, creation data or revocation). Returns
/// whether the link exists.
///
/// # Errors
/// Fails if the query or one of the table's checks fails.
pub async fn update(pool: &PgPool, r: &Record) -> Result<bool, Error> {
    let done = sqlx::query(
        "UPDATE superhet.share_links SET label = $2, title = $3, description = $4, \
            location = $5, device = $6, follow = $7, freq_hz = $8, mode = $9, \
            tune_min_hz = $10, tune_max_hz = $11, features = $12, max_listeners = $13, \
            embed_origins = $14, expires_at = to_timestamp($15::double precision / 1000) \
         WHERE id = $1",
    )
    .bind(r.id)
    .bind(&r.label)
    .bind(&r.title)
    .bind(&r.description)
    .bind(&r.location)
    .bind(&r.device)
    .bind(r.follow)
    .bind(r.freq_hz)
    .bind(&r.mode)
    .bind(r.tune_min_hz)
    .bind(r.tune_max_hz)
    .bind(&r.features)
    .bind(r.max_listeners)
    .bind(&r.embed_origins)
    .bind(ms_arg(r.expires_at_ms))
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Give a link a new token (the old one stops working) and make it usable again.
///
/// # Errors
/// Fails if the query fails.
pub async fn set_token(
    pool: &PgPool,
    id: i64,
    token_hash: &[u8; 32],
    hint: &str,
) -> Result<bool, Error> {
    let done = sqlx::query(
        "UPDATE superhet.share_links SET token_hash = $2, token_hint = $3, revoked_at = NULL \
         WHERE id = $1",
    )
    .bind(id)
    .bind(token_hash.as_slice())
    .bind(hint)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Revoke a link (idempotent). Returns whether it exists.
///
/// # Errors
/// Fails if the query fails.
pub async fn revoke(pool: &PgPool, id: i64) -> Result<bool, Error> {
    let done = sqlx::query(
        "UPDATE superhet.share_links SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Delete a link for good. Returns whether it existed.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete(pool: &PgPool, id: i64) -> Result<bool, Error> {
    let done = sqlx::query("DELETE FROM superhet.share_links WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}
