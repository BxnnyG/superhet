//! Queries for local authentication: the admin account and server-side sessions.
//!
//! Session ids are the SHA-256 of the cookie token (computed by the caller); this module never
//! sees a raw token. Lifetimes are passed in so the policy stays in one place (superhet-api).

use std::time::Duration;

use sqlx::Row;

use crate::users::Role;
use crate::{Error, PgPool};

/// A user as stored in the database.
#[derive(Debug, Clone)]
pub struct User {
    /// Primary key.
    pub id: i64,
    /// Login name as entered at setup.
    pub username: String,
    /// PHC-formatted Argon2id hash.
    pub password_hash: String,
    /// Role.
    pub role: Role,
    /// Disabled accounts cannot log in.
    pub disabled: bool,
    /// The user has to set a new password before anything else.
    pub must_change_password: bool,
}

/// A valid (unexpired) session joined with its (enabled) user.
#[derive(Debug, Clone)]
pub struct ActiveSession {
    /// Owner of the session.
    pub user_id: i64,
    /// Login name of the owner.
    pub username: String,
    /// Role of the owner right now (changes apply to the next request).
    pub role: Role,
    /// Whether the owner still has to change the password.
    pub must_change_password: bool,
    /// Whether `last_seen_at` is older than the refresh interval and should be touched.
    pub needs_touch: bool,
}

const USER_SELECT: &str = "SELECT id, username, password_hash, role, \
    disabled_at IS NOT NULL AS disabled, must_change_password FROM superhet.users";

fn user_of(r: &sqlx::postgres::PgRow) -> User {
    User {
        id: r.get("id"),
        username: r.get("username"),
        password_hash: r.get("password_hash"),
        role: Role::parse(r.get::<&str, _>("role")).unwrap_or(Role::Listener),
        disabled: r.get("disabled"),
        must_change_password: r.get("must_change_password"),
    }
}

/// Serialises first-run setups (advisory lock key, arbitrary but fixed).
const SETUP_LOCK: i64 = 0x5348_5f53_4554_5550;

/// Session lifetime policy.
#[derive(Debug, Clone, Copy)]
pub struct Lifetimes {
    /// A session unused for this long expires.
    pub idle: Duration,
    /// A session older than this expires regardless of use.
    pub absolute: Duration,
    /// `last_seen_at` is refreshed at most this often.
    pub touch_interval: Duration,
}

/// Whether any user exists (i.e. first-run setup is done).
///
/// # Errors
/// Fails if the query fails.
pub async fn has_user(pool: &PgPool) -> Result<bool, Error> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM superhet.users)")
            .fetch_one(pool)
            .await?,
    )
}

/// Create the first admin account (first-run setup). Returns `None` if any account exists
/// already, also under a race: concurrent setups are serialised by an advisory lock.
///
/// # Errors
/// Fails if a query fails.
pub async fn create_admin(
    pool: &PgPool,
    username: &str,
    password_hash: &str,
) -> Result<Option<i64>, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SETUP_LOCK)
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query(
        "INSERT INTO superhet.users (username, password_hash, role) \
         SELECT $1, $2, 'admin' WHERE NOT EXISTS (SELECT 1 FROM superhet.users) RETURNING id",
    )
    .bind(username)
    .bind(password_hash)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.map(|r| r.get("id")))
}

/// Look a user up by name (case-insensitive).
///
/// # Errors
/// Fails if the query fails.
pub async fn find_user_by_name(pool: &PgPool, username: &str) -> Result<Option<User>, Error> {
    let sql = format!("{USER_SELECT} WHERE lower(username) = lower($1)");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(username)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(user_of))
}

/// Look a user up by id.
///
/// # Errors
/// Fails if the query fails.
pub async fn find_user_by_id(pool: &PgPool, id: i64) -> Result<Option<User>, Error> {
    let sql = format!("{USER_SELECT} WHERE id = $1");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(user_of))
}

/// Store a new session.
///
/// # Errors
/// Fails if the query fails.
pub async fn create_session(
    pool: &PgPool,
    id: &[u8],
    user_id: i64,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO superhet.sessions (id, user_id, user_agent, ip) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(user_id)
    .bind(user_agent)
    .bind(ip)
    .execute(pool)
    .await?;
    Ok(())
}

/// Resolve a session id to its user if it is within both lifetimes. An expired row is deleted
/// on the spot.
///
/// # Errors
/// Fails if a query fails.
pub async fn find_active_session(
    pool: &PgPool,
    id: &[u8],
    lifetimes: Lifetimes,
) -> Result<Option<ActiveSession>, Error> {
    let row = sqlx::query(
        "SELECT s.user_id, u.username, u.role, u.must_change_password, \
                s.last_seen_at < now() - make_interval(secs => $4) AS needs_touch \
         FROM superhet.sessions s JOIN superhet.users u ON u.id = s.user_id \
         WHERE s.id = $1 AND u.disabled_at IS NULL \
           AND s.last_seen_at > now() - make_interval(secs => $2) \
           AND s.created_at > now() - make_interval(secs => $3)",
    )
    .bind(id)
    .bind(lifetimes.idle.as_secs_f64())
    .bind(lifetimes.absolute.as_secs_f64())
    .bind(lifetimes.touch_interval.as_secs_f64())
    .fetch_optional(pool)
    .await?;
    if let Some(r) = row {
        return Ok(Some(ActiveSession {
            user_id: r.get("user_id"),
            username: r.get("username"),
            role: Role::parse(r.get::<&str, _>("role")).unwrap_or(Role::Listener),
            must_change_password: r.get("must_change_password"),
            needs_touch: r.get("needs_touch"),
        }));
    }
    // Not found or expired: drop the row if it is merely expired.
    sqlx::query(
        "DELETE FROM superhet.sessions WHERE id = $1 \
           AND (last_seen_at <= now() - make_interval(secs => $2) \
             OR created_at <= now() - make_interval(secs => $3))",
    )
    .bind(id)
    .bind(lifetimes.idle.as_secs_f64())
    .bind(lifetimes.absolute.as_secs_f64())
    .execute(pool)
    .await?;
    Ok(None)
}

/// Record activity on a session.
///
/// # Errors
/// Fails if the query fails.
pub async fn touch_session(pool: &PgPool, id: &[u8]) -> Result<(), Error> {
    sqlx::query("UPDATE superhet.sessions SET last_seen_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete one session (logout).
///
/// # Errors
/// Fails if the query fails.
pub async fn delete_session(pool: &PgPool, id: &[u8]) -> Result<(), Error> {
    sqlx::query("DELETE FROM superhet.sessions WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Replace the password hash and revoke every session of the user except `keep_session`.
/// Returns the number of revoked sessions.
///
/// # Errors
/// Fails if a query fails; nothing is changed in that case.
pub async fn change_password(
    pool: &PgPool,
    user_id: i64,
    password_hash: &str,
    keep_session: &[u8],
) -> Result<u64, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE superhet.users SET password_hash = $2, must_change_password = false WHERE id = $1",
    )
    .bind(user_id)
    .bind(password_hash)
    .execute(&mut *tx)
    .await?;
    let revoked = sqlx::query("DELETE FROM superhet.sessions WHERE user_id = $1 AND id <> $2")
        .bind(user_id)
        .bind(keep_session)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(revoked)
}

/// Delete all sessions past either lifetime. Returns how many were removed.
///
/// # Errors
/// Fails if the query fails.
pub async fn prune_sessions(pool: &PgPool, lifetimes: Lifetimes) -> Result<u64, Error> {
    Ok(sqlx::query(
        "DELETE FROM superhet.sessions \
         WHERE last_seen_at <= now() - make_interval(secs => $1) \
            OR created_at <= now() - make_interval(secs => $2)",
    )
    .bind(lifetimes.idle.as_secs_f64())
    .bind(lifetimes.absolute.as_secs_f64())
    .execute(pool)
    .await?
    .rows_affected())
}
