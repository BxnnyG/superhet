//! Address bans: hosts or networks that may not reach the API, optionally until a deadline.

use sqlx::{PgPool, Row, postgres::PgRow};

use crate::Error;

/// A ban as stored.
#[derive(Debug, Clone)]
pub struct Ban {
    /// Row id.
    pub id: i64,
    /// Banned network in CIDR notation (`203.0.113.5/32`).
    pub network: String,
    /// Admin note.
    pub reason: Option<String>,
    /// Creation, Unix milliseconds.
    pub created_at_ms: i64,
    /// Name of the admin who banned.
    pub created_by: String,
    /// Deadline, Unix milliseconds; `None` = permanent.
    pub expires_at_ms: Option<i64>,
    /// When it was lifted by hand, Unix milliseconds.
    pub lifted_at_ms: Option<i64>,
    /// Who lifted it.
    pub lifted_by: Option<String>,
    /// Still in force (not lifted, not past its deadline) at query time.
    pub active: bool,
}

macro_rules! columns {
    () => {
        "id, network::text AS network, reason, created_by_name, \
    (extract(epoch FROM created_at) * 1000)::bigint AS created_ms, \
    (extract(epoch FROM expires_at) * 1000)::bigint AS expires_ms, \
    (extract(epoch FROM lifted_at) * 1000)::bigint AS lifted_ms, lifted_by_name, \
    (lifted_at IS NULL AND (expires_at IS NULL OR expires_at > now())) AS active"
    };
}

fn ban_of(r: &PgRow) -> Ban {
    Ban {
        id: r.get("id"),
        network: r.get("network"),
        reason: r.get("reason"),
        created_at_ms: r.get("created_ms"),
        created_by: r.get("created_by_name"),
        expires_at_ms: r.get("expires_ms"),
        lifted_at_ms: r.get("lifted_ms"),
        lifted_by: r.get("lifted_by_name"),
        active: r.get("active"),
    }
}

/// Add a ban for `network` (CIDR text, host bits already cleared). Returns `None` when an
/// active ban for exactly this network exists.
///
/// # Errors
/// Fails if the query fails (including an unparsable network).
pub async fn insert(
    pool: &PgPool,
    network: &str,
    reason: Option<&str>,
    expires_at_ms: Option<i64>,
    created_by: i64,
    created_by_name: &str,
) -> Result<Option<Ban>, Error> {
    let row = sqlx::query(concat!(
        "INSERT INTO superhet.ip_bans (network, reason, expires_at, created_by, created_by_name) \
         SELECT $1::text::cidr, $2, to_timestamp($3::bigint / 1000.0), $4, $5 \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM superhet.ip_bans \
             WHERE network = $1::text::cidr AND lifted_at IS NULL \
               AND (expires_at IS NULL OR expires_at > now())) \
         RETURNING ",
        columns!()
    ))
    .bind(network)
    .bind(reason)
    .bind(expires_at_ms)
    .bind(created_by)
    .bind(created_by_name)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(ban_of))
}

/// Bans, newest first: only the ones in force, or all (history included, newest 500).
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool, only_active: bool) -> Result<Vec<Ban>, Error> {
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM superhet.ip_bans \
         WHERE (NOT $1 OR (lifted_at IS NULL AND (expires_at IS NULL OR expires_at > now()))) \
         ORDER BY created_at DESC, id DESC LIMIT 500"
    ))
    .bind(only_active)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(ban_of).collect())
}

/// Lift an active ban. Returns the ban as it was, or `None` when it is not in force.
///
/// # Errors
/// Fails if the query fails.
pub async fn lift(pool: &PgPool, id: i64, by: &str) -> Result<Option<Ban>, Error> {
    let row = sqlx::query(concat!(
        "UPDATE superhet.ip_bans SET lifted_at = now(), lifted_by_name = $2 \
         WHERE id = $1 AND lifted_at IS NULL AND (expires_at IS NULL OR expires_at > now()) \
         RETURNING ",
        columns!()
    ))
    .bind(id)
    .bind(by)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(ban_of))
}

/// Delete lifted and expired bans that ended more than `keep_days` days ago. Returns the number
/// of rows removed.
///
/// # Errors
/// Fails if the query fails.
pub async fn prune(pool: &PgPool, keep_days: i32) -> Result<u64, Error> {
    Ok(sqlx::query(
        "DELETE FROM superhet.ip_bans \
         WHERE COALESCE(lifted_at, expires_at) < now() - make_interval(days => $1)",
    )
    .bind(keep_days)
    .execute(pool)
    .await?
    .rows_affected())
}
