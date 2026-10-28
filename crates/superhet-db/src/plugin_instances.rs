//! Persistent instances of the plugin host: the desired state it restores after a restart
//! (`docs/plugin-contract.md`).

use serde_json::Value;
use sqlx::Row;

use crate::{Error, PgPool};

/// One instance as the host wants it to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// Instance id.
    pub id: String,
    /// Plugin id.
    pub plugin_id: String,
    /// Device the instance is bound to.
    pub device: Option<String>,
    /// VFO the instance is fed from.
    pub vfo: Option<i32>,
    /// Frequency (Hz) the instance is bound to.
    pub freq_hz: Option<i64>,
    /// Plugin configuration.
    pub config: Value,
}

fn record(row: &sqlx::postgres::PgRow) -> Result<Record, sqlx::Error> {
    let config: String = row.try_get("config")?;
    Ok(Record {
        id: row.try_get("id")?,
        plugin_id: row.try_get("plugin_id")?,
        device: row.try_get("device")?,
        vfo: row.try_get("vfo")?,
        freq_hz: row.try_get("freq_hz")?,
        config: serde_json::from_str(&config).unwrap_or(Value::Null),
    })
}

/// Every stored instance, oldest first.
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool) -> Result<Vec<Record>, Error> {
    let rows = sqlx::query(
        "SELECT id, plugin_id, device, vfo, freq_hz, config::text AS config \
         FROM superhet.plugin_instances ORDER BY created_at, id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(record).collect::<Result<_, _>>()?)
}

/// Store an instance (replacing a row with the same id).
///
/// # Errors
/// Fails if the query fails or a CHECK constraint rejects the values.
pub async fn upsert(pool: &PgPool, new: &Record) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO superhet.plugin_instances (id, plugin_id, device, vfo, freq_hz, config) \
         VALUES ($1, $2, $3, $4, $5, $6::text::jsonb) \
         ON CONFLICT (id) DO UPDATE SET plugin_id = EXCLUDED.plugin_id, device = EXCLUDED.device, \
            vfo = EXCLUDED.vfo, freq_hz = EXCLUDED.freq_hz, config = EXCLUDED.config",
    )
    .bind(&new.id)
    .bind(&new.plugin_id)
    .bind(&new.device)
    .bind(new.vfo)
    .bind(new.freq_hz)
    .bind(new.config.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// Change the stored frequency of an instance (a plugin moved its claimed window). Does nothing
/// for an unknown id, so a late update cannot bring back an instance that was stopped.
///
/// # Errors
/// Fails if the query fails.
pub async fn set_freq(pool: &PgPool, id: &str, freq_hz: i64) -> Result<bool, Error> {
    let done = sqlx::query("UPDATE superhet.plugin_instances SET freq_hz = $2 WHERE id = $1")
        .bind(id)
        .bind(freq_hz)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Forget an instance. Returns whether a row existed.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete(pool: &PgPool, id: &str) -> Result<bool, Error> {
    let done = sqlx::query("DELETE FROM superhet.plugin_instances WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}
