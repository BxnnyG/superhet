//! Queries for the decoder event history (DTMF bursts, tone changes, RDS changes and the events
//! of decoder plugins; `docs/api-decoder-events.md`). The worker inserts, the API lists and
//! clears.

use serde_json::{Value, json};
use sqlx::Row;

use crate::{Error, PgPool};

/// Event kinds of the built-in detectors (`source` [`CORE`]); plugins use their own event types.
pub const KINDS: [&str; 6] = ["dtmf", "ctcss", "dcs", "rds_ps", "rds_rt", "rds_pi"];

/// `source` of the built-in detectors; plugin events carry the plugin id.
pub const CORE: &str = "core";

/// Whether `kind` is a plugin event type as the table accepts it: 1-8 dotted segments of
/// `[a-z][a-z0-9-]*`, at most 64 characters (the plugin contract's event type rule).
#[must_use]
pub fn is_plugin_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= 64
        && kind.split('.').count() <= 8
        && kind.split('.').all(|segment| {
            segment.starts_with(|c: char| c.is_ascii_lowercase())
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// Most rows kept by [`prune`]; the oldest go first. A safety net against a runaway producer
/// filling the disk within the retention window.
pub const MAX_ROWS: i64 = 2_000_000;

/// An event to store.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    /// Event time, Unix milliseconds.
    pub ts_ms: i64,
    /// Device id (`-` for a plugin instance that is not bound to a device).
    pub device: String,
    /// VFO id on the device; absent for plugin instances without a VFO.
    pub vfo: Option<i32>,
    /// Tuned frequency of the VFO at that time, Hz.
    pub freq_hz: Option<i64>,
    /// Demodulation mode of the VFO.
    pub mode: Option<String>,
    /// One of [`KINDS`] for source [`CORE`], a plugin event type otherwise.
    pub kind: String,
    /// [`CORE`] or the id of the plugin that produced the event.
    pub source: String,
    /// Human-readable value (digits, tone, station name, text).
    pub value: String,
    /// Kind-specific extras (JSON object).
    pub meta: Value,
}

/// A stored event.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Monotonic row id.
    pub id: i64,
    /// The stored fields.
    pub data: NewEvent,
}

impl Event {
    /// JSON form used by REST, CSV-free clients and the live feed.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let d = &self.data;
        json!({
            "id": self.id,
            "ts_ms": d.ts_ms,
            "device": d.device,
            "vfo": d.vfo,
            "freq_hz": d.freq_hz,
            "mode": d.mode,
            "kind": d.kind,
            "source": d.source,
            "value": d.value,
            "meta": d.meta,
        })
    }
}

/// A page request. Results are ordered newest first by `(ts, id)`.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// Only these kinds (empty = all).
    pub kinds: Vec<String>,
    /// Only this device.
    pub device: Option<String>,
    /// Only this VFO.
    pub vfo: Option<i32>,
    /// Only this source ([`CORE`] or a plugin id).
    pub source: Option<String>,
    /// Only events at or after this time, Unix milliseconds.
    pub from_ms: Option<i64>,
    /// Only events before this time, Unix milliseconds.
    pub to_ms: Option<i64>,
    /// Case-insensitive substring of the value.
    pub search: Option<String>,
    /// Page size.
    pub limit: i64,
    /// Continue after this `(ts_ms, id)` (the last row of the previous page).
    pub after: Option<(i64, i64)>,
}

/// `timestamptz` from a Unix-millisecond bind parameter, exact to the microsecond.
fn ts_of(param: &str) -> String {
    format!("(timestamptz 'epoch' + {param}::double precision * interval '1 millisecond')")
}

const COLUMNS: &str = "id, floor(extract(epoch FROM ts) * 1000)::bigint AS ts_ms, device, vfo, \
    freq_hz, mode, kind, source, value, meta::text AS meta";

fn event(row: &sqlx::postgres::PgRow) -> Result<Event, sqlx::Error> {
    let meta: String = row.try_get("meta")?;
    Ok(Event {
        id: row.try_get("id")?,
        data: NewEvent {
            ts_ms: row.try_get("ts_ms")?,
            device: row.try_get("device")?,
            vfo: row.try_get("vfo")?,
            freq_hz: row.try_get("freq_hz")?,
            mode: row.try_get("mode")?,
            kind: row.try_get("kind")?,
            source: row.try_get("source")?,
            value: row.try_get("value")?,
            meta: serde_json::from_str(&meta).unwrap_or(Value::Null),
        },
    })
}

/// Store an event.
///
/// # Errors
/// Fails if the query fails or a CHECK constraint rejects the values.
pub async fn insert(pool: &PgPool, new: &NewEvent) -> Result<Event, Error> {
    let sql = format!(
        "INSERT INTO superhet.decoder_events (ts, device, vfo, freq_hz, mode, kind, source, value, meta) \
         VALUES ({}, $2, $3, $4, $5, $6, $7, $8, $9::text::jsonb) RETURNING {COLUMNS}",
        ts_of("$1")
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(new.ts_ms)
        .bind(&new.device)
        .bind(new.vfo)
        .bind(new.freq_hz)
        .bind(&new.mode)
        .bind(&new.kind)
        .bind(&new.source)
        .bind(&new.value)
        .bind(new.meta.to_string())
        .fetch_one(pool)
        .await?;
    Ok(event(&row)?)
}

/// One page of events, newest first.
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool, q: &Query) -> Result<Vec<Event>, Error> {
    let sql = format!(
        "SELECT {COLUMNS} FROM superhet.decoder_events \
         WHERE (cardinality($1::text[]) = 0 OR kind = ANY($1)) \
           AND ($2::text IS NULL OR device = $2) \
           AND ($3::integer IS NULL OR vfo = $3) \
           AND ($4::bigint IS NULL OR ts >= {from}) \
           AND ($5::bigint IS NULL OR ts < {to}) \
           AND ($6::text IS NULL OR strpos(lower(value), lower($6)) > 0) \
           AND ($7::bigint IS NULL OR (ts, id) < ({after}, $8::bigint)) \
           AND ($10::text IS NULL OR source = $10) \
         ORDER BY ts DESC, id DESC LIMIT $9",
        from = ts_of("$4"),
        to = ts_of("$5"),
        after = ts_of("$7"),
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(&q.kinds)
        .bind(&q.device)
        .bind(q.vfo)
        .bind(q.from_ms)
        .bind(q.to_ms)
        .bind(&q.search)
        .bind(q.after.map(|(ts, _)| ts))
        .bind(q.after.map_or(0, |(_, id)| id))
        .bind(q.limit.clamp(1, 10_000))
        .bind(&q.source)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(event).collect::<Result<_, _>>()?)
}

/// Delete every event. Returns how many rows went away.
///
/// # Errors
/// Fails if the query fails.
pub async fn clear(pool: &PgPool) -> Result<u64, Error> {
    let done = sqlx::query("DELETE FROM superhet.decoder_events")
        .execute(pool)
        .await?;
    Ok(done.rows_affected())
}

/// Delete events older than `cutoff_ms` and everything beyond the newest [`MAX_ROWS`] rows.
/// Returns how many rows went away.
///
/// # Errors
/// Fails if a query fails.
pub async fn prune(pool: &PgPool, cutoff_ms: i64) -> Result<u64, Error> {
    let sql = format!(
        "DELETE FROM superhet.decoder_events WHERE ts < {} \
         OR id <= COALESCE((SELECT id FROM superhet.decoder_events \
            ORDER BY id DESC OFFSET $2 LIMIT 1), 0)",
        ts_of("$1")
    );
    let done = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(cutoff_ms)
        .bind(MAX_ROWS)
        .execute(pool)
        .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape_is_stable() {
        let e = Event {
            id: 7,
            data: NewEvent {
                ts_ms: 1_700_000_000_123,
                device: "rtl0".into(),
                vfo: Some(2),
                freq_hz: Some(433_500_000),
                mode: Some("nfm".into()),
                kind: "dtmf".into(),
                source: CORE.into(),
                value: "123#".into(),
                meta: json!({"digits": 4}),
            },
        };
        let v = e.to_json();
        assert_eq!(v["id"], 7);
        assert_eq!(v["ts_ms"], 1_700_000_000_123_i64);
        assert_eq!(v["freq_hz"], 433_500_000);
        assert_eq!(v["meta"]["digits"], 4);
        assert_eq!(v["source"], "core");
    }

    #[test]
    fn plugin_kinds_follow_the_event_type_rule() {
        for ok in ["pocsag.message", "a", "x-1.y2", "a.b.c.d.e.f.g.h"] {
            assert!(is_plugin_kind(ok), "{ok}");
        }
        for bad in [
            "",
            "A",
            "1a",
            "a..b",
            "a.b.c.d.e.f.g.h.i",
            "a_b",
            &"a".repeat(65),
        ] {
            assert!(!is_plugin_kind(bad), "{bad}");
        }
    }

    #[test]
    fn kinds_match_the_migration() {
        let sql = include_str!("../migrations/0006_decoder_events.sql");
        for kind in KINDS {
            assert!(sql.contains(&format!("'{kind}'")), "{kind}");
        }
    }
}
