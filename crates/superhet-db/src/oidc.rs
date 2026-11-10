//! OpenID Connect identities: which provider identity belongs to which account
//! (docs/adr/0019-oidc-login.md).
//!
//! The decisions (link, provision, deny) live in superhet-api; this module only stores and looks
//! things up. Accounts created through OIDC get [`LOCKED_PASSWORD_HASH`], which no password
//! verifies against, so they can sign in through the provider only (an admin can still set a
//! local password for them).

use sqlx::Row;

use crate::users::Role;
use crate::{Error, PgPool};

/// Stored instead of a PHC string for accounts without a local password. It is not a valid PHC
/// string, so Argon2 verification always fails.
pub const LOCKED_PASSWORD_HASH: &str = "!";

/// The account an identity or email address resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedUser {
    /// Primary key.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Current role.
    pub role: Role,
    /// Disabled accounts cannot log in.
    pub disabled: bool,
}

fn linked_user(row: &sqlx::postgres::PgRow) -> LinkedUser {
    LinkedUser {
        id: row.get("id"),
        username: row.get("username"),
        role: Role::parse(row.get::<&str, _>("role")).unwrap_or(Role::Listener),
        disabled: row.get("disabled"),
    }
}

const USER_COLUMNS: &str = "u.id, u.username, u.role, u.disabled_at IS NOT NULL AS disabled";

/// The account linked to `(issuer, subject)`.
///
/// # Errors
/// Fails if the query fails.
pub async fn find_by_identity(
    pool: &PgPool,
    issuer: &str,
    subject: &str,
) -> Result<Option<LinkedUser>, Error> {
    let sql = format!(
        "SELECT {USER_COLUMNS} FROM superhet.oidc_identities i \
         JOIN superhet.users u ON u.id = i.user_id WHERE i.issuer = $1 AND i.subject = $2"
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(issuer)
        .bind(subject)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(linked_user))
}

/// The account with this email address (case-insensitive).
///
/// # Errors
/// Fails if the query fails.
pub async fn find_by_email(pool: &PgPool, email: &str) -> Result<Option<LinkedUser>, Error> {
    let sql =
        format!("SELECT {USER_COLUMNS} FROM superhet.users u WHERE lower(u.email) = lower($1)");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(email)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(linked_user))
}

/// Whether the account already has an identity at `issuer`.
///
/// # Errors
/// Fails if the query fails.
pub async fn has_identity_at(pool: &PgPool, user_id: i64, issuer: &str) -> Result<bool, Error> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM superhet.oidc_identities WHERE user_id = $1 AND issuer = $2)",
    )
    .bind(user_id)
    .bind(issuer)
    .fetch_one(pool)
    .await?)
}

/// Outcome of storing an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linked {
    /// Stored.
    Done,
    /// The identity or the account's identity at that issuer exists already.
    Conflict,
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .is_some_and(|e| e.is_unique_violation())
}

/// Attach an identity to an existing account.
///
/// # Errors
/// Fails if the query fails for any reason other than a uniqueness conflict.
pub async fn link(
    pool: &PgPool,
    user_id: i64,
    issuer: &str,
    subject: &str,
    email: Option<&str>,
) -> Result<Linked, Error> {
    let result = sqlx::query(
        "INSERT INTO superhet.oidc_identities (user_id, issuer, subject, email) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(issuer)
    .bind(subject)
    .bind(email)
    .execute(pool)
    .await;
    match result {
        Ok(_) => Ok(Linked::Done),
        Err(err) if is_unique_violation(&err) => Ok(Linked::Conflict),
        Err(err) => Err(err.into()),
    }
}

/// A freshly provisioned account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioned {
    /// New user id.
    pub id: i64,
    /// The login name that was free.
    pub username: String,
}

/// How many `-2`, `-3`, ... suffixes are tried before falling back to a random one.
const SUFFIX_TRIES: u32 = 20;

/// Create an account for a new identity: the first free name out of `base`, `base-2`, ...; no
/// local password; `email` is stored only while no other account has it. Returns `None` if the
/// identity was created concurrently (the caller looks it up again).
///
/// # Errors
/// Fails if a query fails.
pub async fn provision(
    pool: &PgPool,
    base: &str,
    role: Role,
    issuer: &str,
    subject: &str,
    email: Option<&str>,
) -> Result<Option<Provisioned>, Error> {
    let mut tx = pool.begin().await?;
    let mut username = base.to_owned();
    let mut id = None;
    for attempt in 1..=SUFFIX_TRIES + 1 {
        let row = sqlx::query(
            "INSERT INTO superhet.users (username, password_hash, role, email) \
             VALUES ($1, $2, $3, (SELECT $4::text WHERE NOT EXISTS \
                 (SELECT 1 FROM superhet.users WHERE lower(email) = lower($4::text)))) \
             ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(&username)
        .bind(LOCKED_PASSWORD_HASH)
        .bind(role.as_str())
        .bind(email)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = row {
            id = Some(row.get::<i64, _>("id"));
            break;
        }
        username = if attempt <= SUFFIX_TRIES {
            suffixed(base, attempt + 1)
        } else {
            suffixed(base, 1000 + attempt)
        };
    }
    let Some(id) = id else {
        return Err(sqlx::Error::Protocol("no free username".to_owned()).into());
    };
    let linked = sqlx::query(
        "INSERT INTO superhet.oidc_identities (user_id, issuer, subject, email) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(issuer)
    .bind(subject)
    .bind(email)
    .execute(&mut *tx)
    .await;
    match linked {
        Ok(_) => {}
        Err(err) if is_unique_violation(&err) => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    tx.commit().await?;
    Ok(Some(Provisioned { id, username }))
}

/// `base-n`, with `base` shortened so that the result stays within 64 characters.
fn suffixed(base: &str, n: u32) -> String {
    let suffix = format!("-{n}");
    let keep = 64 - suffix.len();
    let head: String = base.chars().take(keep).collect();
    format!("{head}{suffix}")
}

/// Record a login through the identity and refresh the stored email.
///
/// # Errors
/// Fails if the query fails.
pub async fn touch_login(
    pool: &PgPool,
    issuer: &str,
    subject: &str,
    email: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "UPDATE superhet.oidc_identities SET last_login_at = now(), email = $3 \
         WHERE issuer = $1 AND subject = $2",
    )
    .bind(issuer)
    .bind(subject)
    .bind(email)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether an enabled admin account is linked to an OIDC identity (the precondition for
/// switching local logins off).
///
/// # Errors
/// Fails if the query fails.
pub async fn has_active_oidc_admin(pool: &PgPool) -> Result<bool, Error> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM superhet.users u WHERE u.role = 'admin' \
         AND u.disabled_at IS NULL \
         AND EXISTS (SELECT 1 FROM superhet.oidc_identities i WHERE i.user_id = u.id))",
    )
    .fetch_one(pool)
    .await?)
}

/// What a session remembers about the OIDC login it came from (docs/adr/0024-oidc-logout.md).
#[derive(Debug, Clone, Copy)]
pub struct SessionOidc<'a> {
    /// Issuer that authenticated the user.
    pub issuer: &'a str,
    /// The provider's session id (`sid` claim), if the ID token had one.
    pub sid: Option<&'a str>,
    /// The ID token, kept as `id_token_hint` for the provider logout.
    pub id_token: Option<&'a str>,
}

/// Store a new session that came from an OIDC login. `client` is user agent, client address and
/// TCP peer address.
///
/// # Errors
/// Fails if the insert fails.
pub async fn create_session(
    pool: &PgPool,
    id: &[u8],
    user_id: i64,
    client: (Option<&str>, Option<&str>, Option<&str>),
    oidc: &SessionOidc<'_>,
) -> Result<(), Error> {
    let (user_agent, ip, peer) = client;
    sqlx::query(
        "INSERT INTO superhet.sessions \
         (id, user_id, user_agent, ip, peer_ip, oidc_issuer, oidc_sid, oidc_id_token) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(user_id)
    .bind(user_agent)
    .bind(ip)
    .bind(peer)
    .bind(oidc.issuer)
    .bind(oidc.sid)
    .bind(oidc.id_token)
    .execute(pool)
    .await?;
    Ok(())
}

/// The OIDC part of a session that was just ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndedSession {
    /// Issuer of the login, `None` for a password session.
    pub issuer: Option<String>,
    /// The stored ID token (the `id_token_hint`).
    pub id_token: Option<String>,
}

/// Delete one session (logout) and report what it knew about its OIDC login.
///
/// # Errors
/// Fails if the query fails.
pub async fn end_session(pool: &PgPool, id: &[u8]) -> Result<Option<EndedSession>, Error> {
    let row = sqlx::query(
        "DELETE FROM superhet.sessions WHERE id = $1 RETURNING oidc_issuer, oidc_id_token",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| EndedSession {
        issuer: r.get("oidc_issuer"),
        id_token: r.get("oidc_id_token"),
    }))
}

/// End the OIDC sessions a back-channel logout names: those of `issuer` with provider session
/// `sid` and/or belonging to the account linked to `subject`. With both, a session must match
/// both. Without either nothing is deleted. Returns the ids of the sessions that ended (so open
/// connections of them can be closed).
///
/// # Errors
/// Fails if the query fails.
pub async fn end_sessions_for_logout(
    pool: &PgPool,
    issuer: &str,
    sid: Option<&str>,
    subject: Option<&str>,
) -> Result<Vec<Vec<u8>>, Error> {
    if sid.is_none() && subject.is_none() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "DELETE FROM superhet.sessions s WHERE s.oidc_issuer = $1 \
           AND ($2::text IS NULL OR s.oidc_sid = $2) \
           AND ($3::text IS NULL OR s.user_id IN \
                (SELECT i.user_id FROM superhet.oidc_identities i \
                 WHERE i.issuer = $1 AND i.subject = $3)) \
         RETURNING s.id",
    )
    .bind(issuer)
    .bind(sid)
    .bind(subject)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|r| r.get("id")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixed_names_stay_within_the_limit() {
        assert_eq!(suffixed("anna", 2), "anna-2");
        let long = "a".repeat(64);
        let s = suffixed(&long, 12);
        assert_eq!(s.len(), 64);
        assert!(s.ends_with("-12"));
    }
}
