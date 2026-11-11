//! Storage of the notification feature (ADR-0027): rules, targets, the delivery outbox / log, the
//! server's VAPID key and the browser push subscriptions.
//!
//! Rule and target documents are validated by `superhet-notify` before they get here; this layer
//! only stores them. Secrets (webhook key, VAPID private key) are plain fields of the structs
//! that carry them but their `Debug` output hides them, and the API never serialises them.

use std::collections::HashMap;
use std::fmt;

use serde_json::Value;
use sqlx::Row;

use crate::{Error, PgPool};

/// Milliseconds since the epoch of a timestamp column.
fn ms(column: &str) -> String {
    format!("floor(extract(epoch FROM {column}) * 1000)::bigint")
}

// ---------------------------------------------------------------------------------- targets

/// A notification target.
#[derive(Clone, PartialEq)]
pub struct Target {
    /// Row id.
    pub id: i64,
    /// Display name (unique).
    pub name: String,
    /// `webhook`, `email` or `push`.
    pub kind: String,
    /// Non-secret settings.
    pub config: Value,
    /// Write-only secret (webhook HMAC key / token). Only the worker reads it.
    pub secret: Option<String>,
    /// Creation time, Unix milliseconds.
    pub created_at_ms: i64,
    /// Last change, Unix milliseconds.
    pub updated_at_ms: i64,
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Target")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

/// What to do with the secret on an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretChange {
    /// Leave it as it is.
    Keep,
    /// Remove it.
    Clear,
    /// Replace it.
    Set(String),
}

fn target_columns() -> String {
    format!(
        "id, name, kind, config::text AS config, secret, {} AS created_ms, {} AS updated_ms",
        ms("created_at"),
        ms("updated_at")
    )
}

fn target(row: &sqlx::postgres::PgRow) -> Result<Target, sqlx::Error> {
    let config: String = row.try_get("config")?;
    Ok(Target {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        kind: row.try_get("kind")?,
        config: serde_json::from_str(&config).unwrap_or(Value::Null),
        secret: row.try_get("secret")?,
        created_at_ms: row.try_get("created_ms")?,
        updated_at_ms: row.try_get("updated_ms")?,
    })
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .is_some_and(|e| e.code().as_deref() == Some("23505"))
}

/// Every target, by name.
///
/// # Errors
/// Fails if the query fails.
pub async fn list_targets(pool: &PgPool) -> Result<Vec<Target>, Error> {
    let sql = format!(
        "SELECT {} FROM superhet.notification_targets ORDER BY lower(name), id",
        target_columns()
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(target).collect::<Result<_, _>>()?)
}

/// One target.
///
/// # Errors
/// Fails if the query fails.
pub async fn get_target(pool: &PgPool, id: i64) -> Result<Option<Target>, Error> {
    let sql = format!(
        "SELECT {} FROM superhet.notification_targets WHERE id = $1",
        target_columns()
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(target).transpose()?)
}

/// Create a target; `None` if the name is taken.
///
/// # Errors
/// Fails if the query fails or a CHECK constraint rejects the values.
pub async fn insert_target(
    pool: &PgPool,
    name: &str,
    kind: &str,
    config: &Value,
    secret: Option<&str>,
) -> Result<Option<Target>, Error> {
    let sql = format!(
        "INSERT INTO superhet.notification_targets (name, kind, config, secret) \
         VALUES ($1, $2, $3::text::jsonb, $4) RETURNING {}",
        target_columns()
    );
    match sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(name)
        .bind(kind)
        .bind(config.to_string())
        .bind(secret)
        .fetch_one(pool)
        .await
    {
        Ok(row) => Ok(Some(target(&row)?)),
        Err(err) if is_unique_violation(&err) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Outcome of an update.
#[derive(Debug, Clone, PartialEq)]
pub enum Updated<T> {
    /// Stored; the new row.
    Ok(T),
    /// No such row.
    Missing,
    /// The name belongs to another row.
    NameTaken,
}

/// Change a target (the kind is fixed).
///
/// # Errors
/// Fails if the query fails or a CHECK constraint rejects the values.
pub async fn update_target(
    pool: &PgPool,
    id: i64,
    name: &str,
    config: &Value,
    secret: &SecretChange,
) -> Result<Updated<Target>, Error> {
    let (set, value) = match secret {
        SecretChange::Keep => (false, None),
        SecretChange::Clear => (true, None),
        SecretChange::Set(s) => (true, Some(s.as_str())),
    };
    let sql = format!(
        "UPDATE superhet.notification_targets SET name = $2, config = $3::text::jsonb, \
            secret = CASE WHEN $4 THEN $5 ELSE secret END, updated_at = now() \
         WHERE id = $1 RETURNING {}",
        target_columns()
    );
    match sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(name)
        .bind(config.to_string())
        .bind(set)
        .bind(value)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(row)) => Ok(Updated::Ok(target(&row)?)),
        Ok(None) => Ok(Updated::Missing),
        Err(err) if is_unique_violation(&err) => Ok(Updated::NameTaken),
        Err(err) => Err(err.into()),
    }
}

/// Delete a target (rules lose it; delivery log rows keep its name).
///
/// # Errors
/// Fails if the query fails.
pub async fn delete_target(pool: &PgPool, id: i64) -> Result<Option<String>, Error> {
    let row = sqlx::query("DELETE FROM superhet.notification_targets WHERE id = $1 RETURNING name")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("name")))
}

// ------------------------------------------------------------------------------------ rules

/// Quiet hours: no notifications between `start_min` and `end_min` (minutes after local
/// midnight in `tz`; the window may wrap past midnight).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quiet {
    /// Start, minutes after local midnight.
    pub start_min: i32,
    /// End (exclusive), minutes after local midnight.
    pub end_min: i32,
    /// IANA zone name.
    pub tz: String,
}

/// The editable part of a rule.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleInput {
    /// Display name (unique).
    pub name: String,
    /// Switched on.
    pub enabled: bool,
    /// Plugin id or `core`.
    pub source: Option<String>,
    /// Event type.
    pub kind: Option<String>,
    /// `[{field, op, value}]`.
    pub conditions: Value,
    /// Minimum seconds between two notifications of the rule.
    pub cooldown_secs: i32,
    /// At most this many notifications per hour.
    pub max_per_hour: Option<i32>,
    /// Quiet hours.
    pub quiet: Option<Quiet>,
    /// Title template.
    pub title_template: String,
    /// Body template.
    pub body_template: String,
    /// Targets that receive it.
    pub target_ids: Vec<i64>,
}

/// A stored rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// Row id.
    pub id: i64,
    /// The stored values.
    pub data: RuleInput,
    /// Creation time, Unix milliseconds.
    pub created_at_ms: i64,
    /// Last change, Unix milliseconds.
    pub updated_at_ms: i64,
}

fn rule_select() -> String {
    format!(
        "SELECT r.id, r.name, r.enabled, r.source, r.kind, r.conditions::text AS conditions, \
            r.cooldown_secs, r.max_per_hour, r.quiet_start, r.quiet_end, r.quiet_tz, \
            r.title_template, r.body_template, {} AS created_ms, {} AS updated_ms, \
            COALESCE((SELECT array_agg(t.target_id ORDER BY t.target_id) \
                      FROM superhet.notification_rule_targets t WHERE t.rule_id = r.id), \
                     '{{}}'::bigint[]) AS target_ids \
         FROM superhet.notification_rules r",
        ms("r.created_at"),
        ms("r.updated_at")
    )
}

fn rule(row: &sqlx::postgres::PgRow) -> Result<Rule, sqlx::Error> {
    let conditions: String = row.try_get("conditions")?;
    let quiet = match (
        row.try_get::<Option<i32>, _>("quiet_start")?,
        row.try_get::<Option<i32>, _>("quiet_end")?,
        row.try_get::<Option<String>, _>("quiet_tz")?,
    ) {
        (Some(start_min), Some(end_min), Some(tz)) => Some(Quiet {
            start_min,
            end_min,
            tz,
        }),
        _ => None,
    };
    Ok(Rule {
        id: row.try_get("id")?,
        created_at_ms: row.try_get("created_ms")?,
        updated_at_ms: row.try_get("updated_ms")?,
        data: RuleInput {
            name: row.try_get("name")?,
            enabled: row.try_get("enabled")?,
            source: row.try_get("source")?,
            kind: row.try_get("kind")?,
            conditions: serde_json::from_str(&conditions).unwrap_or(Value::Null),
            cooldown_secs: row.try_get("cooldown_secs")?,
            max_per_hour: row.try_get("max_per_hour")?,
            quiet,
            title_template: row.try_get("title_template")?,
            body_template: row.try_get("body_template")?,
            target_ids: row.try_get("target_ids")?,
        },
    })
}

/// Every rule, by name; `only_enabled` for the matcher.
///
/// # Errors
/// Fails if the query fails.
pub async fn list_rules(pool: &PgPool, only_enabled: bool) -> Result<Vec<Rule>, Error> {
    let sql = format!(
        "{} WHERE ($1 = false OR r.enabled) ORDER BY lower(r.name), r.id",
        rule_select()
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(only_enabled)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(rule).collect::<Result<_, _>>()?)
}

/// One rule.
///
/// # Errors
/// Fails if the query fails.
pub async fn get_rule(pool: &PgPool, id: i64) -> Result<Option<Rule>, Error> {
    let sql = format!("{} WHERE r.id = $1", rule_select());
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(rule).transpose()?)
}

async fn set_rule_targets(
    tx: &mut sqlx::PgConnection,
    rule_id: i64,
    target_ids: &[i64],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM superhet.notification_rule_targets WHERE rule_id = $1")
        .bind(rule_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO superhet.notification_rule_targets (rule_id, target_id) \
         SELECT $1, t.id FROM superhet.notification_targets t WHERE t.id = ANY($2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(rule_id)
    .bind(target_ids)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Create a rule; `None` if the name is taken. Unknown target ids are ignored.
///
/// # Errors
/// Fails if the query fails or a CHECK constraint rejects the values.
pub async fn insert_rule(pool: &PgPool, new: &RuleInput) -> Result<Option<Rule>, Error> {
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO superhet.notification_rules (name, enabled, source, kind, conditions, \
            cooldown_secs, max_per_hour, quiet_start, quiet_end, quiet_tz, title_template, \
            body_template) \
         VALUES ($1, $2, $3, $4, $5::text::jsonb, $6, $7, $8, $9, $10, $11, $12) RETURNING id",
    )
    .bind(&new.name)
    .bind(new.enabled)
    .bind(&new.source)
    .bind(&new.kind)
    .bind(new.conditions.to_string())
    .bind(new.cooldown_secs)
    .bind(new.max_per_hour)
    .bind(new.quiet.as_ref().map(|q| q.start_min))
    .bind(new.quiet.as_ref().map(|q| q.end_min))
    .bind(new.quiet.as_ref().map(|q| q.tz.as_str()))
    .bind(&new.title_template)
    .bind(&new.body_template)
    .fetch_one(&mut *tx)
    .await;
    let id: i64 = match inserted {
        Ok(row) => row.get("id"),
        Err(err) if is_unique_violation(&err) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    set_rule_targets(&mut tx, id, &new.target_ids).await?;
    tx.commit().await?;
    get_rule(pool, id).await
}

/// Replace the values of a rule.
///
/// # Errors
/// Fails if the query fails or a CHECK constraint rejects the values.
pub async fn update_rule(pool: &PgPool, id: i64, new: &RuleInput) -> Result<Updated<Rule>, Error> {
    let mut tx = pool.begin().await?;
    let done = sqlx::query(
        "UPDATE superhet.notification_rules SET name = $2, enabled = $3, source = $4, \
            kind = $5, conditions = $6::text::jsonb, cooldown_secs = $7, max_per_hour = $8, \
            quiet_start = $9, quiet_end = $10, quiet_tz = $11, title_template = $12, \
            body_template = $13, updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(&new.name)
    .bind(new.enabled)
    .bind(&new.source)
    .bind(&new.kind)
    .bind(new.conditions.to_string())
    .bind(new.cooldown_secs)
    .bind(new.max_per_hour)
    .bind(new.quiet.as_ref().map(|q| q.start_min))
    .bind(new.quiet.as_ref().map(|q| q.end_min))
    .bind(new.quiet.as_ref().map(|q| q.tz.as_str()))
    .bind(&new.title_template)
    .bind(&new.body_template)
    .execute(&mut *tx)
    .await;
    match done {
        Ok(r) if r.rows_affected() == 0 => return Ok(Updated::Missing),
        Ok(_) => {}
        Err(err) if is_unique_violation(&err) => return Ok(Updated::NameTaken),
        Err(err) => return Err(err.into()),
    }
    set_rule_targets(&mut tx, id, &new.target_ids).await?;
    tx.commit().await?;
    Ok(get_rule(pool, id)
        .await?
        .map_or(Updated::Missing, Updated::Ok))
}

/// Delete a rule; returns its name.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete_rule(pool: &PgPool, id: i64) -> Result<Option<String>, Error> {
    let row = sqlx::query("DELETE FROM superhet.notification_rules WHERE id = $1 RETURNING name")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("name")))
}

/// The current UTC offset in seconds of each named IANA zone; unknown names are absent.
///
/// # Errors
/// Fails if the query fails.
pub async fn zone_offsets(pool: &PgPool, names: &[String]) -> Result<HashMap<String, i32>, Error> {
    let rows = sqlx::query(
        "SELECT name, extract(epoch FROM utc_offset)::int AS offset_secs \
         FROM pg_timezone_names WHERE name = ANY($1)",
    )
    .bind(names)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| Ok((r.try_get("name")?, r.try_get("offset_secs")?)))
        .collect::<Result<_, sqlx::Error>>()
        .map_err(Into::into)
}

// -------------------------------------------------------------------------------- deliveries

/// A delivery to enqueue.
#[derive(Debug, Clone, PartialEq)]
pub struct NewDelivery {
    /// The rule (kept for the log even if it is deleted later).
    pub rule_id: Option<i64>,
    /// Its name at that time.
    pub rule_name: String,
    /// The target.
    pub target_id: i64,
    /// Its name at that time.
    pub target_name: String,
    /// The decoder event row; `None` for a test send.
    pub event_id: Option<i64>,
    /// Rendered `{title, body, event}`.
    pub message: Value,
}

/// A queued or finished delivery.
#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    /// Row id.
    pub id: i64,
    /// The rule, if it still exists.
    pub rule_id: Option<i64>,
    /// Rule name at that time.
    pub rule_name: String,
    /// The target, if it still exists.
    pub target_id: Option<i64>,
    /// Target name at that time.
    pub target_name: String,
    /// The decoder event row.
    pub event_id: Option<i64>,
    /// `pending`, `sent` or `failed`.
    pub status: String,
    /// Send attempts so far.
    pub attempts: i32,
    /// The last error text.
    pub last_error: Option<String>,
    /// The rendered message.
    pub message: Value,
    /// Enqueue time, Unix milliseconds.
    pub created_at_ms: i64,
    /// Time it was sent or given up, Unix milliseconds.
    pub finished_at_ms: Option<i64>,
}

fn delivery(row: &sqlx::postgres::PgRow) -> Result<Delivery, sqlx::Error> {
    let message: String = row.try_get("message")?;
    Ok(Delivery {
        id: row.try_get("id")?,
        rule_id: row.try_get("rule_id")?,
        rule_name: row.try_get("rule_name")?,
        target_id: row.try_get("target_id")?,
        target_name: row.try_get("target_name")?,
        event_id: row.try_get("event_id")?,
        status: row.try_get("status")?,
        attempts: row.try_get("attempts")?,
        last_error: row.try_get("last_error")?,
        message: serde_json::from_str(&message).unwrap_or(Value::Null),
        created_at_ms: row.try_get("created_ms")?,
        finished_at_ms: row.try_get("finished_ms")?,
    })
}

fn delivery_columns() -> String {
    format!(
        "id, rule_id, rule_name, target_id, target_name, event_id, status, attempts, last_error, \
         message::text AS message, {} AS created_ms, \
         CASE WHEN finished_at IS NULL THEN NULL ELSE {} END AS finished_ms",
        ms("created_at"),
        ms("finished_at")
    )
}

/// Queue a delivery. Returns `None` when the same (rule, target, event) is already queued, which
/// is how a redelivered event is dropped.
///
/// # Errors
/// Fails if the query fails.
pub async fn enqueue(pool: &PgPool, new: &NewDelivery) -> Result<Option<i64>, Error> {
    let row = sqlx::query(
        "INSERT INTO superhet.notification_deliveries \
            (rule_id, rule_name, target_id, target_name, event_id, message) \
         VALUES ($1, $2, $3, $4, $5, $6::text::jsonb) \
         ON CONFLICT (rule_id, target_id, event_id) WHERE event_id IS NOT NULL DO NOTHING \
         RETURNING id",
    )
    .bind(new.rule_id)
    .bind(&new.rule_name)
    .bind(new.target_id)
    .bind(&new.target_name)
    .bind(new.event_id)
    .bind(new.message.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get("id")))
}

/// Take up to `limit` due deliveries for sending. Each is leased for `lease_secs` (its
/// `next_attempt_at` moves forward), so a crash during the send makes it due again later.
///
/// # Errors
/// Fails if the query fails.
pub async fn claim_due(pool: &PgPool, limit: i64, lease_secs: i32) -> Result<Vec<Delivery>, Error> {
    let sql = format!(
        "UPDATE superhet.notification_deliveries d SET next_attempt_at = now() + make_interval(secs => $2) \
         WHERE d.id IN (SELECT id FROM superhet.notification_deliveries \
                        WHERE status = 'pending' AND next_attempt_at <= now() \
                        ORDER BY next_attempt_at, id LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING {}",
        delivery_columns()
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(limit)
        .bind(f64::from(lease_secs))
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(delivery).collect::<Result<_, _>>()?)
}

/// Record a successful send.
///
/// # Errors
/// Fails if the query fails.
pub async fn mark_sent(pool: &PgPool, id: i64, note: Option<&str>) -> Result<(), Error> {
    sqlx::query(
        "UPDATE superhet.notification_deliveries SET status = 'sent', attempts = attempts + 1, \
            last_error = left($2, 500), finished_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(note)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed attempt and schedule the next one in `retry_in_secs`, or give up when `None`.
///
/// # Errors
/// Fails if the query fails.
pub async fn mark_failed(
    pool: &PgPool,
    id: i64,
    error: &str,
    retry_in_secs: Option<i32>,
) -> Result<(), Error> {
    match retry_in_secs {
        Some(secs) => {
            sqlx::query(
                "UPDATE superhet.notification_deliveries SET attempts = attempts + 1, \
                    last_error = left($2, 500), \
                    next_attempt_at = now() + make_interval(secs => $3) WHERE id = $1",
            )
            .bind(id)
            .bind(error)
            .bind(f64::from(secs))
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query(
                "UPDATE superhet.notification_deliveries SET status = 'failed', \
                    attempts = attempts + 1, last_error = left($2, 500), finished_at = now() \
                 WHERE id = $1",
            )
            .bind(id)
            .bind(error)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// A page of the delivery log, newest first; `before` continues after a row id.
///
/// # Errors
/// Fails if the query fails.
pub async fn list_deliveries(
    pool: &PgPool,
    limit: i64,
    before: Option<i64>,
    status: Option<&str>,
    rule_id: Option<i64>,
) -> Result<Vec<Delivery>, Error> {
    let sql = format!(
        "SELECT {} FROM superhet.notification_deliveries \
         WHERE ($2::bigint IS NULL OR id < $2) AND ($3::text IS NULL OR status = $3) \
            AND ($4::bigint IS NULL OR rule_id = $4) \
         ORDER BY id DESC LIMIT $1",
        delivery_columns()
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(limit)
        .bind(before)
        .bind(status)
        .bind(rule_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(delivery).collect::<Result<_, _>>()?)
}

/// One delivery.
///
/// # Errors
/// Fails if the query fails.
pub async fn get_delivery(pool: &PgPool, id: i64) -> Result<Option<Delivery>, Error> {
    let sql = format!(
        "SELECT {} FROM superhet.notification_deliveries WHERE id = $1",
        delivery_columns()
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(delivery).transpose()?)
}

/// Delete finished deliveries older than `days`; returns how many.
///
/// # Errors
/// Fails if the query fails.
pub async fn prune_deliveries(pool: &PgPool, days: u32) -> Result<u64, Error> {
    let done = sqlx::query(
        "DELETE FROM superhet.notification_deliveries \
         WHERE status <> 'pending' AND created_at < now() - make_interval(days => $1)",
    )
    .bind(i32::try_from(days).unwrap_or(i32::MAX))
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

// ------------------------------------------------------------------------------------ VAPID

/// The server's VAPID key pair.
#[derive(Clone, PartialEq, Eq)]
pub struct Vapid {
    /// PKCS#8 document of the ECDSA P-256 private key, base64url. Secret.
    pub private_pkcs8: String,
    /// Uncompressed public key point, base64url (the `applicationServerKey`).
    pub public_key: String,
}

impl fmt::Debug for Vapid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vapid")
            .field("public_key", &self.public_key)
            .field("private_pkcs8", &"<redacted>")
            .finish()
    }
}

/// The stored key pair, if any.
///
/// # Errors
/// Fails if the query fails.
pub async fn get_vapid(pool: &PgPool) -> Result<Option<Vapid>, Error> {
    let row = sqlx::query("SELECT private_pkcs8, public_key FROM superhet.notification_vapid")
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| Vapid {
        private_pkcs8: r.get("private_pkcs8"),
        public_key: r.get("public_key"),
    }))
}

/// Store `vapid` unless a key pair exists already (two services may race to create it), and
/// return whichever one is stored.
///
/// # Errors
/// Fails if the query fails.
pub async fn ensure_vapid(pool: &PgPool, vapid: &Vapid) -> Result<Vapid, Error> {
    sqlx::query(
        "INSERT INTO superhet.notification_vapid (id, private_pkcs8, public_key) \
         VALUES (1, $1, $2) ON CONFLICT (id) DO NOTHING",
    )
    .bind(&vapid.private_pkcs8)
    .bind(&vapid.public_key)
    .execute(pool)
    .await?;
    get_vapid(pool)
        .await?
        .ok_or_else(|| Error::Sqlx(sqlx::Error::RowNotFound))
}

// --------------------------------------------------------------------- push subscriptions

/// A browser's push subscription.
#[derive(Clone, PartialEq, Eq)]
pub struct Subscription {
    /// Row id.
    pub id: i64,
    /// Owner.
    pub user_id: i64,
    /// Push service URL; unguessable, treated as a secret in logs.
    pub endpoint: String,
    /// Browser public key, base64url.
    pub p256dh: String,
    /// Browser auth secret, base64url.
    pub auth: String,
    /// Label the user gave (browser / device).
    pub label: Option<String>,
    /// Creation time, Unix milliseconds.
    pub created_at_ms: i64,
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

fn subscription(row: &sqlx::postgres::PgRow) -> Result<Subscription, sqlx::Error> {
    Ok(Subscription {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        endpoint: row.try_get("endpoint")?,
        p256dh: row.try_get("p256dh")?,
        auth: row.try_get("auth")?,
        label: row.try_get("label")?,
        created_at_ms: row.try_get("created_ms")?,
    })
}

fn subscription_columns() -> String {
    format!(
        "id, user_id, endpoint, p256dh, auth, label, {} AS created_ms",
        ms("created_at")
    )
}

/// Most subscriptions one account may hold.
pub const MAX_SUBSCRIPTIONS_PER_USER: i64 = 10;

/// Outcome of [`upsert_subscription`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subscribed {
    /// Stored (or refreshed); the row.
    Ok(Subscription),
    /// The account has too many subscriptions.
    TooMany,
    /// The endpoint belongs to another account.
    OwnedByOther,
}

/// Store the subscription of `user_id`, replacing the keys of the same endpoint.
///
/// # Errors
/// Fails if the query fails or a CHECK constraint rejects the values.
pub async fn upsert_subscription(
    pool: &PgPool,
    user_id: i64,
    endpoint: &str,
    p256dh: &str,
    auth: &str,
    label: Option<&str>,
) -> Result<Subscribed, Error> {
    let mut tx = pool.begin().await?;
    let owner: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM superhet.push_subscriptions WHERE endpoint = $1")
            .bind(endpoint)
            .fetch_optional(&mut *tx)
            .await?;
    match owner {
        Some(other) if other != user_id => return Ok(Subscribed::OwnedByOther),
        Some(_) => {}
        None => {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM superhet.push_subscriptions WHERE user_id = $1",
            )
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
            if count >= MAX_SUBSCRIPTIONS_PER_USER {
                return Ok(Subscribed::TooMany);
            }
        }
    }
    let sql = format!(
        "INSERT INTO superhet.push_subscriptions (user_id, endpoint, p256dh, auth, label) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (endpoint) DO UPDATE SET p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth, \
            label = COALESCE(EXCLUDED.label, superhet.push_subscriptions.label) \
         RETURNING {}",
        subscription_columns()
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .bind(endpoint)
        .bind(p256dh)
        .bind(auth)
        .bind(label)
        .fetch_one(&mut *tx)
        .await?;
    let stored = subscription(&row)?;
    tx.commit().await?;
    Ok(Subscribed::Ok(stored))
}

/// The subscriptions of one account, oldest first.
///
/// # Errors
/// Fails if the query fails.
pub async fn list_subscriptions_of(
    pool: &PgPool,
    user_id: i64,
) -> Result<Vec<Subscription>, Error> {
    let sql = format!(
        "SELECT {} FROM superhet.push_subscriptions WHERE user_id = $1 ORDER BY id",
        subscription_columns()
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(subscription).collect::<Result<_, _>>()?)
}

/// The subscriptions of the given accounts (all accounts if `user_ids` is empty) whose account is
/// not disabled.
///
/// # Errors
/// Fails if the query fails.
pub async fn list_subscriptions_for(
    pool: &PgPool,
    user_ids: &[i64],
) -> Result<Vec<Subscription>, Error> {
    let sql = format!(
        "SELECT {} FROM superhet.push_subscriptions \
         WHERE (cardinality($1::bigint[]) = 0 OR user_id = ANY($1)) \
            AND NOT EXISTS (SELECT 1 FROM superhet.users u \
                            WHERE u.id = push_subscriptions.user_id AND u.disabled_at IS NOT NULL) \
         ORDER BY id",
        subscription_columns()
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(user_ids)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(subscription).collect::<Result<_, _>>()?)
}

/// Remove a subscription of `user_id`.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete_subscription(pool: &PgPool, user_id: i64, id: i64) -> Result<bool, Error> {
    let done =
        sqlx::query("DELETE FROM superhet.push_subscriptions WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(pool)
            .await?;
    Ok(done.rows_affected() > 0)
}

/// Remove a subscription the push service reported as gone.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete_subscription_by_id(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM superhet.push_subscriptions WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
