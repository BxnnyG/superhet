//! ADS-B support tables (`docs/plugins-adsb.md`, ADR-0023): the imported aircraft database and
//! the receiver range statistics.
//!
//! The aircraft database (registration and type per ICAO address, type descriptions, airline
//! names) is data the admin imported; the API replaces it in one transaction so lookups never
//! see a half-filled table. The range statistics are written by the worker from the plugin's
//! `adsb.range` events: farthest position per bearing sector and altitude band (idempotent
//! `GREATEST` upserts, so a redelivered event changes nothing) and one activity row per minute.

use sqlx::Row;

use crate::{Error, PgPool};

/// Bearing sectors of the range statistics (5 degrees each, 0 = north, clockwise).
pub const SECTORS: i16 = 72;
/// Altitude bands of the range statistics (`band_of` in the adapter; the schema allows 16).
pub const BANDS: i16 = 6;
/// Rows per insert statement while importing (keeps the statements and memory small).
const CHUNK: usize = 5_000;
/// A receiver that moved farther than this starts new statistics (kilometres).
pub const MOVE_RESET_KM: f64 = 2.0;
/// Most ICAO addresses / designators per lookup.
pub const MAX_LOOKUP: usize = 400;

/// One aircraft of the imported database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AircraftRow {
    /// 24-bit ICAO address.
    pub icao: u32,
    pub registration: String,
    /// ICAO type designator, e.g. `A320`.
    pub type_code: String,
    /// Bit 0 military, bit 1 interesting, bit 2 PIA, bit 3 LADD.
    pub flags: i16,
}

/// One ICAO aircraft type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRow {
    pub code: String,
    pub description: String,
    /// ICAO Doc 8643 class such as `L2J`.
    pub class: String,
    /// Wake turbulence category.
    pub wake: String,
}

/// One airline (ICAO designator = first three letters of its callsigns).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorRow {
    pub code: String,
    pub name: String,
    pub country: String,
    /// Radio telephony callsign.
    pub callsign: String,
}

/// A complete database to import.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dataset {
    pub aircraft: Vec<AircraftRow>,
    pub types: Vec<TypeRow>,
    pub operators: Vec<OperatorRow>,
}

/// Where a database came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub source: String,
    pub source_url: String,
    pub licence: String,
    pub licence_url: String,
    pub version: String,
    /// `download` or `file`.
    pub origin: String,
}

/// What the database currently holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseInfo {
    pub source: Source,
    pub aircraft: i64,
    pub types: i64,
    pub operators: i64,
    /// Import time, Unix milliseconds.
    pub imported_ms: i64,
}

/// Replace the aircraft database with `data` (one transaction) and record where it came from.
///
/// # Errors
/// Fails if a statement fails; nothing is changed then.
pub async fn replace_database(
    pool: &PgPool,
    data: &Dataset,
    source: &Source,
) -> Result<DatabaseInfo, Error> {
    let mut tx = pool.begin().await?;
    for table in ["aircraft_db", "aircraft_types", "aircraft_operators"] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM superhet.{table}")))
            .execute(&mut *tx)
            .await?;
    }
    for chunk in data.aircraft.chunks(CHUNK) {
        let icao: Vec<i32> = chunk
            .iter()
            .map(|a| i32::try_from(a.icao).unwrap_or(0))
            .collect();
        let registration: Vec<&str> = chunk.iter().map(|a| a.registration.as_str()).collect();
        let type_code: Vec<&str> = chunk.iter().map(|a| a.type_code.as_str()).collect();
        let flags: Vec<i16> = chunk.iter().map(|a| a.flags).collect();
        sqlx::query(
            "INSERT INTO superhet.aircraft_db (icao, registration, type_code, flags) \
             SELECT * FROM UNNEST($1::int4[], $2::text[], $3::text[], $4::int2[]) \
             ON CONFLICT (icao) DO NOTHING",
        )
        .bind(icao)
        .bind(registration)
        .bind(type_code)
        .bind(flags)
        .execute(&mut *tx)
        .await?;
    }
    for chunk in data.types.chunks(CHUNK) {
        let code: Vec<&str> = chunk.iter().map(|t| t.code.as_str()).collect();
        let description: Vec<&str> = chunk.iter().map(|t| t.description.as_str()).collect();
        let class: Vec<&str> = chunk.iter().map(|t| t.class.as_str()).collect();
        let wake: Vec<&str> = chunk.iter().map(|t| t.wake.as_str()).collect();
        sqlx::query(
            "INSERT INTO superhet.aircraft_types (code, description, class, wake) \
             SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[]) \
             ON CONFLICT (code) DO NOTHING",
        )
        .bind(code)
        .bind(description)
        .bind(class)
        .bind(wake)
        .execute(&mut *tx)
        .await?;
    }
    for chunk in data.operators.chunks(CHUNK) {
        let code: Vec<&str> = chunk.iter().map(|o| o.code.as_str()).collect();
        let name: Vec<&str> = chunk.iter().map(|o| o.name.as_str()).collect();
        let country: Vec<&str> = chunk.iter().map(|o| o.country.as_str()).collect();
        let callsign: Vec<&str> = chunk.iter().map(|o| o.callsign.as_str()).collect();
        sqlx::query(
            "INSERT INTO superhet.aircraft_operators (code, name, country, callsign) \
             SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[]) \
             ON CONFLICT (code) DO NOTHING",
        )
        .bind(code)
        .bind(name)
        .bind(country)
        .bind(callsign)
        .execute(&mut *tx)
        .await?;
    }
    let aircraft: i64 = sqlx::query_scalar("SELECT count(*) FROM superhet.aircraft_db")
        .fetch_one(&mut *tx)
        .await?;
    let types: i64 = sqlx::query_scalar("SELECT count(*) FROM superhet.aircraft_types")
        .fetch_one(&mut *tx)
        .await?;
    let operators: i64 = sqlx::query_scalar("SELECT count(*) FROM superhet.aircraft_operators")
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO superhet.aircraft_db_meta \
         (singleton, source, source_url, licence, licence_url, version, origin, aircraft, types, \
          operators, imported_at) \
         VALUES (true, $1, $2, $3, $4, $5, $6, $7, $8, $9, now()) \
         ON CONFLICT (singleton) DO UPDATE SET source = EXCLUDED.source, \
         source_url = EXCLUDED.source_url, licence = EXCLUDED.licence, \
         licence_url = EXCLUDED.licence_url, version = EXCLUDED.version, \
         origin = EXCLUDED.origin, aircraft = EXCLUDED.aircraft, types = EXCLUDED.types, \
         operators = EXCLUDED.operators, imported_at = EXCLUDED.imported_at",
    )
    .bind(&source.source)
    .bind(&source.source_url)
    .bind(&source.licence)
    .bind(&source.licence_url)
    .bind(&source.version)
    .bind(&source.origin)
    .bind(i32::try_from(aircraft).unwrap_or(i32::MAX))
    .bind(i32::try_from(types).unwrap_or(i32::MAX))
    .bind(i32::try_from(operators).unwrap_or(i32::MAX))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    database_info(pool)
        .await?
        .ok_or(Error::Sqlx(sqlx::Error::RowNotFound))
}

/// The imported database's description; `None` when nothing was imported.
///
/// # Errors
/// Fails if the query fails.
pub async fn database_info(pool: &PgPool) -> Result<Option<DatabaseInfo>, Error> {
    let row = sqlx::query(
        "SELECT source, source_url, licence, licence_url, version, origin, aircraft, types, \
         operators, floor(extract(epoch FROM imported_at) * 1000)::bigint AS imported_ms \
         FROM superhet.aircraft_db_meta",
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(DatabaseInfo {
        source: Source {
            source: row.try_get("source")?,
            source_url: row.try_get("source_url")?,
            licence: row.try_get("licence")?,
            licence_url: row.try_get("licence_url")?,
            version: row.try_get("version")?,
            origin: row.try_get("origin")?,
        },
        aircraft: i64::from(row.try_get::<i32, _>("aircraft")?),
        types: i64::from(row.try_get::<i32, _>("types")?),
        operators: i64::from(row.try_get::<i32, _>("operators")?),
        imported_ms: row.try_get("imported_ms")?,
    }))
}

/// Delete the imported database (the tables and the description).
///
/// # Errors
/// Fails if a statement fails; nothing is changed then.
pub async fn clear_database(pool: &PgPool) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for table in [
        "aircraft_db",
        "aircraft_types",
        "aircraft_operators",
        "aircraft_db_meta",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM superhet.{table}")))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// What the database knows about one aircraft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub icao: u32,
    pub registration: String,
    pub type_code: String,
    /// Description of the type, empty if the type is unknown.
    pub type_description: String,
    pub class: String,
    pub wake: String,
    pub flags: i16,
}

/// Look up aircraft by ICAO address (at most [`MAX_LOOKUP`]; unknown addresses are absent).
///
/// # Errors
/// Fails if the query fails.
pub async fn lookup_aircraft(pool: &PgPool, icao: &[u32]) -> Result<Vec<Found>, Error> {
    let ids: Vec<i32> = icao
        .iter()
        .take(MAX_LOOKUP)
        .filter_map(|i| i32::try_from(*i).ok())
        .collect();
    let rows = sqlx::query(
        "SELECT a.icao, a.registration, a.type_code, a.flags, \
         coalesce(t.description, '') AS description, coalesce(t.class, '') AS class, \
         coalesce(t.wake, '') AS wake \
         FROM superhet.aircraft_db a \
         LEFT JOIN superhet.aircraft_types t ON t.code = a.type_code \
         WHERE a.icao = ANY($1::int4[]) ORDER BY a.icao",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(Found {
                icao: u32::try_from(r.try_get::<i32, _>("icao")?).unwrap_or(0),
                registration: r.try_get("registration")?,
                type_code: r.try_get("type_code")?,
                type_description: r.try_get("description")?,
                class: r.try_get("class")?,
                wake: r.try_get("wake")?,
                flags: r.try_get("flags")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()
        .map_err(Error::from)
}

/// Look up airlines by ICAO designator (at most [`MAX_LOOKUP`]; unknown ones are absent).
///
/// # Errors
/// Fails if the query fails.
pub async fn lookup_operators(pool: &PgPool, codes: &[String]) -> Result<Vec<OperatorRow>, Error> {
    let codes: Vec<&str> = codes.iter().take(MAX_LOOKUP).map(String::as_str).collect();
    let rows = sqlx::query(
        "SELECT code, name, country, callsign FROM superhet.aircraft_operators \
         WHERE code = ANY($1::text[]) ORDER BY code",
    )
    .bind(codes)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(OperatorRow {
                code: r.try_get("code")?,
                name: r.try_get("name")?,
                country: r.try_get("country")?,
                callsign: r.try_get("callsign")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()
        .map_err(Error::from)
}

/// Farthest position heard in one sector and altitude band.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeCell {
    pub sector: i16,
    pub band: i16,
    pub max_km: f32,
    /// ICAO address of the aircraft that set the record (may be empty).
    pub icao: String,
}

/// Aircraft counts of one minute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Activity {
    pub msgs_per_s: f32,
    pub aircraft: i16,
    pub with_position: i16,
}

/// What one `adsb.range` event carries.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeUpdate {
    /// Start of the minute the numbers describe, Unix milliseconds.
    pub ts_ms: i64,
    /// Receiver position the cells were measured from, if configured.
    pub receiver: Option<(f64, f64)>,
    /// New maxima since the previous event.
    pub cells: Vec<RangeCell>,
    pub activity: Activity,
}

/// What [`record_range`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recorded {
    /// The receiver moved, so the old cells were dropped first.
    pub restarted: bool,
}

/// Great-circle distance between two positions, kilometres.
#[must_use]
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lat2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    6371.0 * 2.0 * h.sqrt().min(1.0).asin()
}

/// Store the numbers of one event for `device`. A receiver more than [`MOVE_RESET_KM`] away from
/// the one the statistics belong to clears the old cells first. Cells keep the larger value, so
/// storing an event twice changes nothing.
///
/// # Errors
/// Fails if a statement fails; nothing is changed then.
pub async fn record_range(
    pool: &PgPool,
    device: &str,
    update: &RangeUpdate,
) -> Result<Recorded, Error> {
    let mut tx = pool.begin().await?;
    let mut restarted = false;
    if let Some((lat, lon)) = update.receiver {
        let known = sqlx::query(
            "SELECT lat, lon FROM superhet.adsb_range_meta WHERE device = $1 FOR UPDATE",
        )
        .bind(device)
        .fetch_optional(&mut *tx)
        .await?;
        let moved = match &known {
            Some(row) => {
                let old = (row.try_get::<f64, _>("lat")?, row.try_get::<f64, _>("lon")?);
                distance_km(old, (lat, lon)) > MOVE_RESET_KM
            }
            None => false,
        };
        if moved {
            sqlx::query("DELETE FROM superhet.adsb_range_cells WHERE device = $1")
                .bind(device)
                .execute(&mut *tx)
                .await?;
            restarted = true;
        }
        if known.is_none() || moved {
            sqlx::query(
                "INSERT INTO superhet.adsb_range_meta (device, lat, lon, since) \
                 VALUES ($1, $2, $3, now()) \
                 ON CONFLICT (device) DO UPDATE SET lat = EXCLUDED.lat, lon = EXCLUDED.lon, \
                 since = EXCLUDED.since",
            )
            .bind(device)
            .bind(lat)
            .bind(lon)
            .execute(&mut *tx)
            .await?;
        }
    }
    if !update.cells.is_empty() {
        let sector: Vec<i16> = update.cells.iter().map(|c| c.sector).collect();
        let band: Vec<i16> = update.cells.iter().map(|c| c.band).collect();
        let km: Vec<f32> = update.cells.iter().map(|c| c.max_km).collect();
        let icao: Vec<&str> = update.cells.iter().map(|c| c.icao.as_str()).collect();
        sqlx::query(
            "INSERT INTO superhet.adsb_range_cells (device, sector, band, max_km, icao, updated_at) \
             SELECT $1, s, b, k, i, now() \
             FROM UNNEST($2::int2[], $3::int2[], $4::float4[], $5::text[]) AS u(s, b, k, i) \
             ON CONFLICT (device, sector, band) DO UPDATE SET \
             max_km = EXCLUDED.max_km, icao = EXCLUDED.icao, updated_at = now() \
             WHERE EXCLUDED.max_km > superhet.adsb_range_cells.max_km",
        )
        .bind(device)
        .bind(sector)
        .bind(band)
        .bind(km)
        .bind(icao)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO superhet.adsb_activity (device, ts_ms, msgs_per_s, aircraft, with_position) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (device, ts_ms) DO UPDATE SET msgs_per_s = EXCLUDED.msgs_per_s, \
         aircraft = EXCLUDED.aircraft, with_position = EXCLUDED.with_position",
    )
    .bind(device)
    .bind(update.ts_ms)
    .bind(update.activity.msgs_per_s)
    .bind(update.activity.aircraft)
    .bind(update.activity.with_position)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Recorded { restarted })
}

/// The stored range statistics of a device.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeSummary {
    /// Receiver position the cells belong to.
    pub receiver: Option<(f64, f64)>,
    /// Since when (Unix milliseconds) the cells have been collected.
    pub since_ms: Option<i64>,
    pub cells: Vec<RangeCell>,
}

/// The range statistics of `device`.
///
/// # Errors
/// Fails if a query fails.
pub async fn range_summary(pool: &PgPool, device: &str) -> Result<RangeSummary, Error> {
    let meta = sqlx::query(
        "SELECT lat, lon, floor(extract(epoch FROM since) * 1000)::bigint AS since_ms \
         FROM superhet.adsb_range_meta WHERE device = $1",
    )
    .bind(device)
    .fetch_optional(pool)
    .await?;
    let rows = sqlx::query(
        "SELECT sector, band, max_km, icao FROM superhet.adsb_range_cells \
         WHERE device = $1 ORDER BY sector, band",
    )
    .bind(device)
    .fetch_all(pool)
    .await?;
    let cells = rows
        .iter()
        .map(|r| {
            Ok(RangeCell {
                sector: r.try_get("sector")?,
                band: r.try_get("band")?,
                max_km: r.try_get("max_km")?,
                icao: r.try_get("icao")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()?;
    let (receiver, since_ms) = match meta {
        Some(row) => (
            Some((row.try_get("lat")?, row.try_get("lon")?)),
            Some(row.try_get("since_ms")?),
        ),
        None => (None, None),
    };
    Ok(RangeSummary {
        receiver,
        since_ms,
        cells,
    })
}

/// One point of the activity history (an average over `bucket_ms`).
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityPoint {
    /// Start of the bucket, Unix milliseconds.
    pub ts_ms: i64,
    pub msgs_per_s: f32,
    pub aircraft: i16,
    pub with_position: i16,
}

/// Message rate and aircraft counts since `since_ms`, averaged (rate) or maximised (counts) into
/// buckets of `bucket_ms`, oldest first.
///
/// # Errors
/// Fails if the query fails.
pub async fn activity(
    pool: &PgPool,
    device: &str,
    since_ms: i64,
    bucket_ms: i64,
) -> Result<Vec<ActivityPoint>, Error> {
    let bucket_ms = bucket_ms.max(60_000);
    let rows = sqlx::query(
        "SELECT (ts_ms / $3) * $3 AS bucket, avg(msgs_per_s)::float4 AS msgs, \
         max(aircraft) AS aircraft, max(with_position) AS with_position \
         FROM superhet.adsb_activity WHERE device = $1 AND ts_ms >= $2 \
         GROUP BY bucket ORDER BY bucket",
    )
    .bind(device)
    .bind(since_ms)
    .bind(bucket_ms)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(ActivityPoint {
                ts_ms: r.try_get("bucket")?,
                msgs_per_s: r.try_get("msgs")?,
                aircraft: r.try_get("aircraft")?,
                with_position: r.try_get("with_position")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()
        .map_err(Error::from)
}

/// Forget the range statistics of `device` (cells, receiver position and activity history).
/// Returns the number of removed cells.
///
/// # Errors
/// Fails if a statement fails; nothing is changed then.
pub async fn reset_range(pool: &PgPool, device: &str) -> Result<u64, Error> {
    let mut tx = pool.begin().await?;
    let removed = sqlx::query("DELETE FROM superhet.adsb_range_cells WHERE device = $1")
        .bind(device)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM superhet.adsb_range_meta WHERE device = $1")
        .bind(device)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM superhet.adsb_activity WHERE device = $1")
        .bind(device)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(removed)
}

/// Delete activity rows older than `before_ms`. Returns the number removed.
///
/// # Errors
/// Fails if the statement fails.
pub async fn prune_activity(pool: &PgPool, before_ms: i64) -> Result<u64, Error> {
    Ok(
        sqlx::query("DELETE FROM superhet.adsb_activity WHERE ts_ms < $1")
            .bind(before_ms)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_between_known_points() {
        // Cologne to Berlin is about 477 km.
        let d = distance_km((50.9375, 6.9603), (52.52, 13.405));
        assert!((d - 477.0).abs() < 5.0, "{d}");
        assert!(distance_km((51.0, 7.0), (51.0, 7.0)) < 1e-9);
    }
}
