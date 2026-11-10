//! Per-device access policies (`docs/adr/0023-device-access.md`). A device without a row uses
//! the defaults of the table, which are today's open behaviour.

use sqlx::Row;

use crate::{Error, PgPool};

/// The stored policy of one device. Roles are the database spelling (`listener`, `operator`,
/// `admin`); the API layer turns them into its own types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Device id.
    pub device: String,
    /// Lowest role that may see and listen.
    pub view_role: String,
    /// Lowest role that may change tuning, VFOs and plugins.
    pub control_role: String,
    /// Guests may see the device.
    pub guests_visible: bool,
    /// When not empty: only these account ids (and admins) may use the device.
    pub allowed_users: Vec<i64>,
}

fn record(row: &sqlx::postgres::PgRow) -> Result<Record, sqlx::Error> {
    Ok(Record {
        device: row.try_get("device")?,
        view_role: row.try_get("view_role")?,
        control_role: row.try_get("control_role")?,
        guests_visible: row.try_get("guests_visible")?,
        allowed_users: row.try_get("allowed_users")?,
    })
}

/// Every stored policy, by device id.
///
/// # Errors
/// Fails if the query fails.
pub async fn list(pool: &PgPool) -> Result<Vec<Record>, Error> {
    let rows = sqlx::query(
        "SELECT device, view_role, control_role, guests_visible, allowed_users \
         FROM superhet.device_policies ORDER BY device",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(record).collect::<Result<_, _>>()?)
}

/// Create or replace the policy of a device.
///
/// # Errors
/// Fails if the query fails (for example a role outside the allowed three).
pub async fn put(pool: &PgPool, policy: &Record, updated_by: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO superhet.device_policies \
            (device, view_role, control_role, guests_visible, allowed_users, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (device) DO UPDATE SET view_role = EXCLUDED.view_role, \
            control_role = EXCLUDED.control_role, guests_visible = EXCLUDED.guests_visible, \
            allowed_users = EXCLUDED.allowed_users, updated_at = now(), \
            updated_by = EXCLUDED.updated_by",
    )
    .bind(&policy.device)
    .bind(&policy.view_role)
    .bind(&policy.control_role)
    .bind(policy.guests_visible)
    .bind(&policy.allowed_users)
    .bind(updated_by)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove the policy of a device (back to the defaults). Returns whether a row existed.
///
/// # Errors
/// Fails if the query fails.
pub async fn delete(pool: &PgPool, device: &str) -> Result<bool, Error> {
    let done = sqlx::query("DELETE FROM superhet.device_policies WHERE device = $1")
        .bind(device)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}
