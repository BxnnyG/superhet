//! The append-only audit log (docs/adr/0012, docs/api-audit.md).
//!
//! `audit_events` rejects UPDATE/DELETE/TRUNCATE unless the transaction sets
//! `superhet.audit_maintenance`; only [`prune`] (and the pseudonymisation in
//! [`crate::users::delete`]) do.

use serde_json::{Value, json};
use sqlx::{Postgres, QueryBuilder, Row};

use crate::{Error, PgPool};

/// Most rows one [`query`] returns.
pub const MAX_PAGE: i64 = 1000;

/// An event to store. `ts_ms: None` stamps it with the database clock.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    /// Event time, Unix milliseconds.
    pub ts_ms: Option<i64>,
    /// `user`, `guest` or `system`.
    pub actor_kind: String,
    /// User id or guest session id.
    pub actor_id: Option<String>,
    /// Denormalised display name (survives user deletion).
    pub actor_name: Option<String>,
    /// Role at the time (`admin`, `operator`, `listener`).
    pub role: Option<String>,
    /// Client address, already truncated per the IP mode.
    pub ip: Option<String>,
    /// Truncated user agent.
    pub user_agent: Option<String>,
    /// Dotted action name.
    pub action: String,
    /// What was acted on.
    pub target: Option<String>,
    /// `ok`, `denied` or `error`.
    pub outcome: String,
    /// Small extra document.
    pub details: Value,
}

/// A stored event.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Monotonic row id.
    pub id: i64,
    /// Event time, Unix milliseconds.
    pub ts_ms: i64,
    /// The stored fields (`ts_ms` repeated).
    pub data: NewEvent,
}

impl Event {
    /// JSON form used by the API and the JSON export.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let d = &self.data;
        json!({
            "id": self.id,
            "ts_ms": self.ts_ms,
            "actor_kind": d.actor_kind,
            "actor_id": d.actor_id,
            "actor_name": d.actor_name,
            "role": d.role,
            "ip": d.ip,
            "user_agent": d.user_agent,
            "action": d.action,
            "target": d.target,
            "outcome": d.outcome,
            "details": d.details,
        })
    }
}

/// Store an event.
///
/// # Errors
/// Fails if the insert violates a constraint or the query fails.
pub async fn insert(pool: &PgPool, event: &NewEvent) -> Result<i64, Error> {
    let row = sqlx::query(
        "INSERT INTO superhet.audit_events \
            (ts, actor_kind, actor_id, actor_name, role, ip, user_agent, action, target, \
             outcome, details) \
         VALUES (coalesce(to_timestamp($1::double precision / 1000.0), now()), $2, $3, $4, $5, \
                 $6::text::inet, $7, $8, $9, $10, $11) \
         RETURNING id",
    )
    .bind(event.ts_ms)
    .bind(&event.actor_kind)
    .bind(&event.actor_id)
    .bind(&event.actor_name)
    .bind(&event.role)
    .bind(&event.ip)
    .bind(&event.user_agent)
    .bind(&event.action)
    .bind(&event.target)
    .bind(&event.outcome)
    .bind(&event.details)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

/// Filters of [`query`]; `None` fields do not filter.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// Events at or after this time, Unix milliseconds.
    pub from_ms: Option<i64>,
    /// Events before this time, Unix milliseconds.
    pub to_ms: Option<i64>,
    /// `user`, `guest` or `system`.
    pub actor_kind: Option<String>,
    /// Exact actor id, or a case-insensitive part of the actor name.
    pub actor: Option<String>,
    /// Role.
    pub role: Option<String>,
    /// Exact action, or a prefix when it ends with `*` (`user.*`).
    pub action: Option<String>,
    /// `ok`, `denied` or `error`.
    pub outcome: Option<String>,
    /// Address: exact host, or `a.b.c.0/24` style network containing the stored address.
    pub ip: Option<String>,
    /// Case-insensitive part of the target.
    pub target: Option<String>,
    /// Only events with an id below this (paging cursor).
    pub before_id: Option<i64>,
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn apply(builder: &mut QueryBuilder<Postgres>, filter: &Filter) {
    if let Some(v) = filter.from_ms {
        builder
            .push(" AND ts >= to_timestamp(")
            .push_bind(v)
            .push("::double precision / 1000.0)");
    }
    if let Some(v) = filter.to_ms {
        builder
            .push(" AND ts < to_timestamp(")
            .push_bind(v)
            .push("::double precision / 1000.0)");
    }
    if let Some(v) = &filter.actor_kind {
        builder.push(" AND actor_kind = ").push_bind(v.clone());
    }
    if let Some(v) = &filter.actor {
        builder
            .push(" AND (actor_id = ")
            .push_bind(v.clone())
            .push(" OR actor_name ILIKE ")
            .push_bind(format!("%{}%", escape_like(v)))
            .push(")");
    }
    if let Some(v) = &filter.role {
        builder.push(" AND role = ").push_bind(v.clone());
    }
    if let Some(v) = &filter.action {
        if let Some(prefix) = v.strip_suffix('*') {
            builder
                .push(" AND action LIKE ")
                .push_bind(format!("{}%", escape_like(prefix)));
        } else {
            builder.push(" AND action = ").push_bind(v.clone());
        }
    }
    if let Some(v) = &filter.outcome {
        builder.push(" AND outcome = ").push_bind(v.clone());
    }
    if let Some(v) = &filter.ip {
        // `inet <<= inet` matches both a host and a containing network.
        builder
            .push(" AND ip <<= ")
            .push_bind(v.clone())
            .push("::text::inet");
    }
    if let Some(v) = &filter.target {
        builder
            .push(" AND target ILIKE ")
            .push_bind(format!("%{}%", escape_like(v)));
    }
    if let Some(v) = filter.before_id {
        builder.push(" AND id < ").push_bind(v);
    }
}

fn event_of(row: &sqlx::postgres::PgRow) -> Event {
    let ts_ms: i64 = row.get("ts_ms");
    Event {
        id: row.get("id"),
        ts_ms,
        data: NewEvent {
            ts_ms: Some(ts_ms),
            actor_kind: row.get("actor_kind"),
            actor_id: row.get("actor_id"),
            actor_name: row.get("actor_name"),
            role: row.get("role"),
            ip: row.get("ip"),
            user_agent: row.get("user_agent"),
            action: row.get("action"),
            target: row.get("target"),
            outcome: row.get("outcome"),
            details: row.get("details"),
        },
    }
}

const SELECT: &str = "SELECT id, (extract(epoch FROM ts) * 1000)::bigint AS ts_ms, actor_kind, \
    actor_id, actor_name, role, host(ip) AS ip, user_agent, action, target, outcome, details \
    FROM superhet.audit_events WHERE true";

/// Matching events, newest first. `limit` is clamped to 1..=[`MAX_PAGE`].
///
/// # Errors
/// Fails if the query fails (including an unparsable `ip` filter).
pub async fn query(pool: &PgPool, filter: &Filter, limit: i64) -> Result<Vec<Event>, Error> {
    let mut builder = QueryBuilder::new(SELECT);
    apply(&mut builder, filter);
    builder
        .push(" ORDER BY id DESC LIMIT ")
        .push_bind(limit.clamp(1, MAX_PAGE));
    let rows = builder.build().fetch_all(pool).await?;
    Ok(rows.iter().map(event_of).collect())
}

/// Matching events oldest first for exports: ids above `after_id`, at most `limit` rows.
///
/// # Errors
/// Fails if the query fails.
pub async fn export_page(
    pool: &PgPool,
    filter: &Filter,
    after_id: i64,
    limit: i64,
) -> Result<Vec<Event>, Error> {
    let mut builder = QueryBuilder::new(SELECT);
    // Exports walk forward by id; `before_id` still applies as an upper bound.
    apply(&mut builder, filter);
    builder
        .push(" AND id > ")
        .push_bind(after_id)
        .push(" ORDER BY id ASC LIMIT ")
        .push_bind(limit.clamp(1, MAX_PAGE));
    let rows = builder.build().fetch_all(pool).await?;
    Ok(rows.iter().map(event_of).collect())
}

/// Row count, oldest event and table size for the admin page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Rows in the table.
    pub count: i64,
    /// Oldest event, Unix milliseconds.
    pub oldest_ms: Option<i64>,
    /// Table plus indexes, bytes.
    pub bytes: i64,
}

/// Table statistics.
///
/// # Errors
/// Fails if the query fails.
pub async fn stats(pool: &PgPool) -> Result<Stats, Error> {
    let row = sqlx::query(
        "SELECT count(*) AS n, (extract(epoch FROM min(ts)) * 1000)::bigint AS oldest, \
                pg_total_relation_size('superhet.audit_events') AS bytes \
         FROM superhet.audit_events",
    )
    .fetch_one(pool)
    .await?;
    Ok(Stats {
        count: row.get("n"),
        oldest_ms: row.get("oldest"),
        bytes: row.get("bytes"),
    })
}

/// Delete events older than the retention: `user_days` for accounts and system events,
/// `guest_days` for guests; `0` keeps that class forever. Returns the number of rows removed.
///
/// # Errors
/// Fails if the query fails.
pub async fn prune(pool: &PgPool, user_days: u32, guest_days: u32) -> Result<u64, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('superhet.audit_maintenance', 'on', true)")
        .execute(&mut *tx)
        .await?;
    let removed = sqlx::query(
        "DELETE FROM superhet.audit_events WHERE \
            (actor_kind = 'guest' AND $2 > 0 AND ts < now() - make_interval(days => $2)) \
         OR (actor_kind <> 'guest' AND $1 > 0 AND ts < now() - make_interval(days => $1))",
    )
    .bind(i32::try_from(user_days).unwrap_or(i32::MAX))
    .bind(i32::try_from(guest_days).unwrap_or(i32::MAX))
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(removed)
}
