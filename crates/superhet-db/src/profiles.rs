//! Queries for profiles (named device presets). The document in `data` is opaque here; its
//! shape is `superhet_bus::profile::ProfileData`, validated by the API.

use sqlx::Row;

use crate::{Error, PgPool};

/// Longest accepted profile name (characters).
pub const MAX_NAME_CHARS: usize = 100;

/// A stored profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// UUID in canonical lower-case text form.
    pub id: String,
    /// Unique (case-insensitive) display name.
    pub name: String,
    /// The preset document.
    pub data: serde_json::Value,
    /// RFC 3339 UTC timestamp.
    pub created_at: String,
    /// RFC 3339 UTC timestamp.
    pub updated_at: String,
}

/// Why a write failed.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// Another profile already has this name.
    #[error("a profile with this name already exists")]
    NameTaken,
    /// Any other database failure.
    #[error(transparent)]
    Db(#[from] Error),
}

impl From<sqlx::Error> for WriteError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &err
            && db.is_unique_violation()
        {
            Self::NameTaken
        } else {
            Self::Db(err.into())
        }
    }
}

const COLUMNS: &str = "id::text AS id, name, data::text AS data, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
    to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_at";

fn profile(row: &sqlx::postgres::PgRow) -> Result<Profile, sqlx::Error> {
    let data: String = row.try_get("data")?;
    Ok(Profile {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        data: serde_json::from_str(&data).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Normalise a UUID given in a URL: canonical lower-case text, or `None` if it is not one.
#[must_use]
pub fn parse_id(raw: &str) -> Option<String> {
    let ok = raw.len() == 36
        && raw.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        });
    ok.then(|| raw.to_ascii_lowercase())
}

/// All profiles, by name.
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool) -> Result<Vec<Profile>, Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM superhet.profiles ORDER BY lower(name), id"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(profile).collect::<Result<_, _>>()?)
}

/// One profile by id (see [`parse_id`]).
///
/// # Errors
/// Fails if the query fails.
pub async fn get(pool: &PgPool, id: &str) -> Result<Option<Profile>, Error> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM superhet.profiles WHERE id = $1::uuid"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(profile).transpose()?)
}

/// Insert a profile.
///
/// # Errors
/// [`WriteError::NameTaken`] on a duplicate name.
pub async fn create(
    pool: &PgPool,
    name: &str,
    data: &serde_json::Value,
) -> Result<Profile, WriteError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO superhet.profiles (name, data) VALUES ($1, $2::text::jsonb) \
         RETURNING {COLUMNS}"
    )))
    .bind(name)
    .bind(data.to_string())
    .fetch_one(pool)
    .await?;
    Ok(profile(&row)?)
}

/// Insert a profile, or replace the one that already has this name (case-insensitive) while
/// keeping its id and `created_at`. Returns the profile and whether it was newly created.
///
/// # Errors
/// Fails if the query fails.
pub async fn upsert_by_name(
    pool: &PgPool,
    name: &str,
    data: &serde_json::Value,
) -> Result<(Profile, bool), Error> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO superhet.profiles (name, data) VALUES ($1, $2::text::jsonb) \
         ON CONFLICT (lower(name)) DO UPDATE \
         SET name = EXCLUDED.name, data = EXCLUDED.data, updated_at = now() \
         RETURNING {COLUMNS}, (xmax = 0) AS created"
    )))
    .bind(name)
    .bind(data.to_string())
    .fetch_one(pool)
    .await?;
    let created: bool = row.try_get("created")?;
    Ok((profile(&row)?, created))
}

/// Replace name and data of a profile; `None` if it does not exist.
///
/// # Errors
/// [`WriteError::NameTaken`] if another profile has the name.
pub async fn update(
    pool: &PgPool,
    id: &str,
    name: &str,
    data: &serde_json::Value,
) -> Result<Option<Profile>, WriteError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE superhet.profiles SET name = $2, data = $3::text::jsonb, updated_at = now() \
         WHERE id = $1::uuid RETURNING {COLUMNS}"
    )))
    .bind(id)
    .bind(name)
    .bind(data.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(profile).transpose()?)
}

/// Delete a profile; `false` if it did not exist.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete(pool: &PgPool, id: &str) -> Result<bool, Error> {
    let done = sqlx::query("DELETE FROM superhet.profiles WHERE id = $1::uuid")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uuids() {
        assert_eq!(
            parse_id("0F8FAD5B-D9CB-469F-A165-70867728950E").as_deref(),
            Some("0f8fad5b-d9cb-469f-a165-70867728950e")
        );
        for bad in [
            "",
            "123",
            "0f8fad5bd9cb469fa16570867728950e",
            "0f8fad5b-d9cb-469f-a165-70867728950g",
            "0f8fad5b-d9cb-469f-a165-70867728950e0",
        ] {
            assert_eq!(parse_id(bad), None, "{bad}");
        }
    }
}
