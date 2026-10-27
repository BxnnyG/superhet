//! Accounts with roles, invites and session listings (docs/adr/0011).
//!
//! The rule "at least one active admin remains" lives here: every query that could remove an
//! admin (demote, disable, delete) locks the active admin rows first, so two concurrent requests
//! cannot both succeed.

use std::time::Duration;

use sqlx::Row;

use crate::{Error, PgPool};

/// The three roles of ADR-0011, ordered by privilege.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Own VFOs, own audio, spectrum.
    Listener,
    /// Listener plus tuning, profiles, bookmarks.
    Operator,
    /// Everything, including users and the audit log.
    Admin,
}

impl Role {
    /// The value stored in the database and shown in the API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Listener => "listener",
            Self::Operator => "operator",
            Self::Admin => "admin",
        }
    }

    /// Parse the database / API spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "listener" => Some(Self::Listener),
            "operator" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

fn role_of(row: &sqlx::postgres::PgRow) -> Role {
    // The CHECK constraint guarantees a known value; the fallback is the least privilege.
    Role::parse(row.get::<&str, _>("role")).unwrap_or(Role::Listener)
}

/// An account as shown on the admin page (no password hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    /// Primary key.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Role.
    pub role: Role,
    /// Whether the account is disabled (cannot log in, sessions revoked).
    pub disabled: bool,
    /// Whether the user has to choose a new password before doing anything else.
    pub must_change_password: bool,
    /// Creation time, Unix milliseconds.
    pub created_at_ms: i64,
    /// Last successful login, Unix milliseconds.
    pub last_login_ms: Option<i64>,
}

const USER_COLUMNS: &str = "id, username, role, disabled_at IS NOT NULL AS disabled, \
    must_change_password, (extract(epoch FROM created_at) * 1000)::bigint AS created_ms, \
    (extract(epoch FROM last_login_at) * 1000)::bigint AS login_ms";

fn user_info(row: &sqlx::postgres::PgRow) -> UserInfo {
    UserInfo {
        id: row.get("id"),
        username: row.get("username"),
        role: role_of(row),
        disabled: row.get("disabled"),
        must_change_password: row.get("must_change_password"),
        created_at_ms: row.get("created_ms"),
        last_login_ms: row.get("login_ms"),
    }
}

/// Result of an operation guarded by the last-admin rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guarded {
    /// Done.
    Done,
    /// No such user.
    NotFound,
    /// Refused: it would leave no active admin.
    LastAdmin,
}

/// Result of creating an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Created {
    /// The new user id.
    Id(i64),
    /// The (case-insensitive) username exists already.
    UsernameTaken,
}

/// All accounts, oldest first.
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool) -> Result<Vec<UserInfo>, Error> {
    let sql = format!("SELECT {USER_COLUMNS} FROM superhet.users ORDER BY id");
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(user_info).collect())
}

/// One account.
///
/// # Errors
/// Fails if the query fails.
pub async fn get(pool: &PgPool, id: i64) -> Result<Option<UserInfo>, Error> {
    let sql = format!("SELECT {USER_COLUMNS} FROM superhet.users WHERE id = $1");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(user_info))
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .is_some_and(|e| e.is_unique_violation())
}

/// Create an account.
///
/// # Errors
/// Fails if the query fails for any reason other than a duplicate username.
pub async fn create(
    pool: &PgPool,
    username: &str,
    password_hash: &str,
    role: Role,
    must_change_password: bool,
) -> Result<Created, Error> {
    let result = sqlx::query(
        "INSERT INTO superhet.users (username, password_hash, role, must_change_password) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(username)
    .bind(password_hash)
    .bind(role.as_str())
    .bind(must_change_password)
    .fetch_one(pool)
    .await;
    match result {
        Ok(row) => Ok(Created::Id(row.get("id"))),
        Err(err) if is_unique_violation(&err) => Ok(Created::UsernameTaken),
        Err(err) => Err(err.into()),
    }
}

/// Lock the active admins (serialises concurrent last-admin checks) and report whether
/// `except` is the only one.
async fn is_last_active_admin(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: i64,
) -> Result<bool, Error> {
    let admins: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM superhet.users WHERE role = 'admin' AND disabled_at IS NULL \
         ORDER BY id FOR UPDATE",
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(admins == [user_id])
}

/// Change a role. Sessions stay valid; the new role applies to the next request.
///
/// # Errors
/// Fails if a query fails.
pub async fn set_role(pool: &PgPool, id: i64, role: Role) -> Result<Guarded, Error> {
    let mut tx = pool.begin().await?;
    if role != Role::Admin && is_last_active_admin(&mut tx, id).await? {
        return Ok(Guarded::LastAdmin);
    }
    let changed = sqlx::query("UPDATE superhet.users SET role = $2 WHERE id = $1")
        .bind(id)
        .bind(role.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(if changed == 0 {
        Guarded::NotFound
    } else {
        Guarded::Done
    })
}

/// Disable or re-enable an account. Disabling revokes all its sessions.
///
/// # Errors
/// Fails if a query fails.
pub async fn set_disabled(pool: &PgPool, id: i64, disabled: bool) -> Result<Guarded, Error> {
    let mut tx = pool.begin().await?;
    if disabled && is_last_active_admin(&mut tx, id).await? {
        return Ok(Guarded::LastAdmin);
    }
    let changed = sqlx::query(
        "UPDATE superhet.users SET disabled_at = CASE WHEN $2 THEN coalesce(disabled_at, now()) \
         ELSE NULL END WHERE id = $1",
    )
    .bind(id)
    .bind(disabled)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed > 0 && disabled {
        sqlx::query("DELETE FROM superhet.sessions WHERE user_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(if changed == 0 {
        Guarded::NotFound
    } else {
        Guarded::Done
    })
}

/// Delete an account and its sessions. Audit rows of the user stay; their `actor_name` is
/// replaced by `deleted-user-<id>` (pseudonymised).
///
/// # Errors
/// Fails if a query fails.
pub async fn delete(pool: &PgPool, id: i64) -> Result<Guarded, Error> {
    let mut tx = pool.begin().await?;
    if is_last_active_admin(&mut tx, id).await? {
        return Ok(Guarded::LastAdmin);
    }
    sqlx::query("SELECT set_config('superhet.audit_maintenance', 'on', true)")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE superhet.audit_events SET actor_name = 'deleted-user-' || actor_id \
         WHERE actor_kind = 'user' AND actor_id = $1::text",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let deleted = sqlx::query("DELETE FROM superhet.users WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(if deleted == 0 {
        Guarded::NotFound
    } else {
        Guarded::Done
    })
}

/// Replace a password on behalf of an admin: every session of the user is revoked. With
/// `must_change` the user has to pick their own password at the next login.
///
/// # Errors
/// Fails if a query fails.
pub async fn reset_password(
    pool: &PgPool,
    id: i64,
    password_hash: &str,
    must_change: bool,
) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE superhet.users SET password_hash = $2, must_change_password = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(password_hash)
    .bind(must_change)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed > 0 {
        sqlx::query("DELETE FROM superhet.sessions WHERE user_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(changed > 0)
}

/// Record a successful login.
///
/// # Errors
/// Fails if the query fails.
pub async fn mark_login(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("UPDATE superhet.users SET last_login_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// A live session row for the session lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// Session key (SHA-256 of the cookie token).
    pub id: Vec<u8>,
    /// Owner.
    pub user_id: i64,
    /// Owner's login name.
    pub username: String,
    /// Owner's role.
    pub role: Role,
    /// Login time, Unix milliseconds.
    pub created_at_ms: i64,
    /// Last activity, Unix milliseconds.
    pub last_seen_ms: i64,
    /// Stored user agent.
    pub user_agent: Option<String>,
    /// Stored client address (as the API resolved it at login).
    pub ip: Option<String>,
    /// Stored TCP peer address at login.
    pub peer_ip: Option<String>,
}

/// Sessions of one user, or of everybody (`None`), newest activity first.
///
/// # Errors
/// Fails if the query fails.
pub async fn list_sessions(pool: &PgPool, user_id: Option<i64>) -> Result<Vec<SessionInfo>, Error> {
    let rows = sqlx::query(
        "SELECT s.id, s.user_id, u.username, u.role, \
                (extract(epoch FROM s.created_at) * 1000)::bigint AS created_ms, \
                (extract(epoch FROM s.last_seen_at) * 1000)::bigint AS seen_ms, \
                s.user_agent, s.ip, s.peer_ip \
         FROM superhet.sessions s JOIN superhet.users u ON u.id = s.user_id \
         WHERE ($1::bigint IS NULL OR s.user_id = $1) \
         ORDER BY s.last_seen_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| SessionInfo {
            id: r.get("id"),
            user_id: r.get("user_id"),
            username: r.get("username"),
            role: role_of(r),
            created_at_ms: r.get("created_ms"),
            last_seen_ms: r.get("seen_ms"),
            user_agent: r.get("user_agent"),
            ip: r.get("ip"),
            peer_ip: r.get("peer_ip"),
        })
        .collect())
}

/// Delete one session by key; with `owner` only if it belongs to that user. Returns whether a
/// row went away.
///
/// # Errors
/// Fails if the query fails.
pub async fn revoke_session(pool: &PgPool, id: &[u8], owner: Option<i64>) -> Result<bool, Error> {
    Ok(sqlx::query(
        "DELETE FROM superhet.sessions WHERE id = $1 AND ($2::bigint IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// Where an invite stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteState {
    /// Can be used.
    Open,
    /// Past its expiry.
    Expired,
    /// Used to create an account.
    Used,
    /// Revoked by an admin.
    Revoked,
}

impl InviteState {
    /// API spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Expired => "expired",
            Self::Used => "used",
            Self::Revoked => "revoked",
        }
    }
}

/// An invite as listed for admins (never the token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteInfo {
    /// Primary key.
    pub id: i64,
    /// Role the new account gets.
    pub role: Role,
    /// Free-text note of the admin.
    pub note: Option<String>,
    /// Name of the admin who created it.
    pub created_by_name: String,
    /// Creation time, Unix milliseconds.
    pub created_at_ms: i64,
    /// Expiry, Unix milliseconds.
    pub expires_at_ms: i64,
    /// Current state.
    pub state: InviteState,
    /// Login name of the account created from it.
    pub used_by_name: Option<String>,
}

const INVITE_COLUMNS: &str = "i.id, i.role, i.note, i.created_by_name, \
    (extract(epoch FROM i.created_at) * 1000)::bigint AS created_ms, \
    (extract(epoch FROM i.expires_at) * 1000)::bigint AS expires_ms, \
    CASE WHEN i.revoked_at IS NOT NULL THEN 'revoked' WHEN i.used_at IS NOT NULL THEN 'used' \
         WHEN i.expires_at <= now() THEN 'expired' ELSE 'open' END AS state, \
    u.username AS used_by_name";

fn invite_info(row: &sqlx::postgres::PgRow) -> InviteInfo {
    InviteInfo {
        id: row.get("id"),
        role: role_of(row),
        note: row.get("note"),
        created_by_name: row.get("created_by_name"),
        created_at_ms: row.get("created_ms"),
        expires_at_ms: row.get("expires_ms"),
        state: match row.get::<&str, _>("state") {
            "revoked" => InviteState::Revoked,
            "used" => InviteState::Used,
            "expired" => InviteState::Expired,
            _ => InviteState::Open,
        },
        used_by_name: row.get("used_by_name"),
    }
}

/// Store a new invite (only the token hash).
///
/// # Errors
/// Fails if the query fails.
pub async fn create_invite(
    pool: &PgPool,
    token_hash: &[u8],
    role: Role,
    note: Option<&str>,
    created_by: i64,
    created_by_name: &str,
    lifetime: Duration,
) -> Result<InviteInfo, Error> {
    let sql = format!(
        "WITH i AS (INSERT INTO superhet.invites \
            (token_hash, role, note, created_by, created_by_name, expires_at) \
          VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6)) RETURNING *) \
         SELECT {INVITE_COLUMNS} FROM i LEFT JOIN superhet.users u ON u.id = i.used_by"
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(token_hash)
        .bind(role.as_str())
        .bind(note)
        .bind(created_by)
        .bind(created_by_name)
        .bind(lifetime.as_secs_f64())
        .fetch_one(pool)
        .await?;
    Ok(invite_info(&row))
}

/// All invites, newest first.
///
/// # Errors
/// Fails if the query fails.
pub async fn list_invites(pool: &PgPool) -> Result<Vec<InviteInfo>, Error> {
    let sql = format!(
        "SELECT {INVITE_COLUMNS} FROM superhet.invites i \
         LEFT JOIN superhet.users u ON u.id = i.used_by ORDER BY i.id DESC"
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(invite_info).collect())
}

/// Look an invite up by the hash of its token.
///
/// # Errors
/// Fails if the query fails.
pub async fn find_invite(pool: &PgPool, token_hash: &[u8]) -> Result<Option<InviteInfo>, Error> {
    let sql = format!(
        "SELECT {INVITE_COLUMNS} FROM superhet.invites i \
         LEFT JOIN superhet.users u ON u.id = i.used_by WHERE i.token_hash = $1"
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(token_hash)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(invite_info))
}

/// Revoke an open invite. Returns whether one was open.
///
/// # Errors
/// Fails if the query fails.
pub async fn revoke_invite(pool: &PgPool, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE superhet.invites SET revoked_at = now() \
         WHERE id = $1 AND revoked_at IS NULL AND used_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// Result of redeeming an invite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    /// The account was created with the invite's role.
    Created {
        /// New user id.
        user_id: i64,
        /// Role fixed at invite time.
        role: Role,
    },
    /// Unknown, expired, used or revoked.
    InvalidToken,
    /// The username exists already; the invite stays usable.
    UsernameTaken,
}

/// Redeem an invite: mark it used and create the account in one transaction.
///
/// # Errors
/// Fails if a query fails.
pub async fn accept_invite(
    pool: &PgPool,
    token_hash: &[u8],
    username: &str,
    password_hash: &str,
) -> Result<Accepted, Error> {
    let mut tx = pool.begin().await?;
    let invite = sqlx::query(
        "UPDATE superhet.invites SET used_at = now() \
         WHERE token_hash = $1 AND used_at IS NULL AND revoked_at IS NULL AND expires_at > now() \
         RETURNING id, role",
    )
    .bind(token_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(invite) = invite else {
        return Ok(Accepted::InvalidToken);
    };
    let role = role_of(&invite);
    let invite_id: i64 = invite.get("id");
    let inserted = sqlx::query(
        "INSERT INTO superhet.users (username, password_hash, role) VALUES ($1, $2, $3) \
         RETURNING id",
    )
    .bind(username)
    .bind(password_hash)
    .bind(role.as_str())
    .fetch_one(&mut *tx)
    .await;
    let user_id: i64 = match inserted {
        Ok(row) => row.get("id"),
        // Dropping the transaction rolls the "used" mark back.
        Err(err) if is_unique_violation(&err) => return Ok(Accepted::UsernameTaken),
        Err(err) => return Err(err.into()),
    };
    sqlx::query("UPDATE superhet.invites SET used_by = $2 WHERE id = $1")
        .bind(invite_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Accepted::Created { user_id, role })
}
