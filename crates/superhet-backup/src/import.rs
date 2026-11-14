//! Applying a backup: one transaction, optionally rolled back (dry run).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, PgConnection, PgPool, Row};

use crate::Error;
use crate::archive::{Manifest, Opened};
use crate::export::{resolve_areas, schema_version};
use crate::tables::{self, Kind, Table};

const BATCH_ROWS: usize = 500;
/// Stored for accounts imported without a password hash: not a valid PHC string, so no password
/// ever verifies against it. The admin sets a password or sends an invite.
pub const LOCKED_HASH: &str = "!restored-without-credentials";

/// What to import and how.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Areas to apply; empty means everything the archive contains.
    pub areas: Vec<String>,
    /// Let the archive's version of the importing admin's own account replace the live one.
    pub overwrite_self: bool,
    /// The importing admin (kept through an account import; required for the accounts area).
    pub self_user_id: Option<i64>,
    /// Compute and report everything, then roll back.
    pub dry_run: bool,
    /// The recordings library: where recording files of the archive are restored to (without it
    /// they are reported and skipped).
    pub recordings_dir: Option<PathBuf>,
}

/// Effect on one table.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TableReport {
    /// Table name.
    pub table: String,
    /// Area it belongs to.
    pub area: String,
    /// Rows in the archive.
    pub in_archive: u64,
    /// Rows present before the import.
    pub existing: u64,
    /// Rows removed (replace semantics).
    pub deleted: u64,
    /// Rows written.
    pub inserted: u64,
    /// Rows of the live install that were updated in place (the admin's own account).
    pub updated: u64,
    /// Rows not written (duplicates, unknown account).
    pub skipped: u64,
}

/// Result of an import or dry run.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// `true` if nothing was changed.
    pub dry_run: bool,
    /// Header of the archive (without the file list).
    pub backup: Value,
    /// Schema version of this install.
    pub current_schema_version: i64,
    /// Areas that were (or would be) applied.
    pub areas: Vec<String>,
    /// Per table.
    pub tables: Vec<TableReport>,
    /// Recording files in the archive.
    pub files_in_archive: u64,
    /// Recording files written to the library (always 0 for a dry run).
    pub files_restored: u64,
    /// Things that need the admin's attention but did not stop the import.
    pub warnings: Vec<String>,
    /// Differences that were left alone, or need `overwrite_self`.
    pub conflicts: Vec<String>,
}

fn summary(m: &Manifest) -> Value {
    json!({
        "format": m.format,
        "app_version": m.app_version,
        "schema_version": m.schema_version,
        "created_at": m.created_at,
        "created_by": m.created_by,
        "includes_credentials": m.includes_credentials,
        "areas": m.areas,
        "files": m.files.iter().map(|f| json!({"table": f.table, "rows": f.rows})).collect::<Vec<_>>(),
    })
}

async fn count(conn: &mut PgConnection, table: &str) -> Result<u64, Error> {
    let sql = format!("SELECT count(*)::bigint AS n FROM superhet.{table}");
    let row = sqlx::query(AssertSqlSafe(sql)).fetch_one(conn).await?;
    Ok(u64::try_from(row.get::<i64, _>("n")).unwrap_or(0))
}

async fn catalogue(conn: &mut PgConnection, table: &str) -> Result<Vec<String>, Error> {
    let rows = sqlx::query(
        "SELECT column_name::text AS c FROM information_schema.columns \
         WHERE table_schema = 'superhet' AND table_name = $1",
    )
    .bind(table)
    .fetch_all(conn)
    .await?;
    Ok(rows.iter().map(|r| r.get("c")).collect())
}

/// Insert `rows` into `table`. Columns are the keys of the first row that exist in the live table
/// (so an archive from an older schema simply leaves new columns at their defaults).
async fn insert_rows(
    conn: &mut PgConnection,
    table: &Table,
    live_columns: &[String],
    rows: &[Value],
    drop: &[&str],
) -> Result<u64, Error> {
    let Some(first) = rows.first().and_then(Value::as_object) else {
        return Ok(0);
    };
    let cols: Vec<&String> = live_columns
        .iter()
        .filter(|c| first.contains_key(c.as_str()) && !drop.contains(&c.as_str()))
        .collect();
    if cols.is_empty() {
        return Err(Error::BadArchive(format!(
            "{}: no known columns",
            table.name
        )));
    }
    let list = cols
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let select = cols
        .iter()
        .map(|c| format!("r.\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let (overriding, filter) = match table.kind {
        Kind::Plain { identity: true } => (" OVERRIDING SYSTEM VALUE", String::new()),
        Kind::Append { dedupe } => (
            "",
            format!(
                " WHERE NOT EXISTS (SELECT 1 FROM superhet.{} e WHERE {dedupe})",
                table.name
            ),
        ),
        _ => ("", String::new()),
    };
    let sql = format!(
        "INSERT INTO superhet.{name} ({list}){overriding} \
         SELECT {select} FROM jsonb_populate_recordset(NULL::superhet.{name}, $1::text::jsonb) r{filter}",
        name = table.name
    );
    let payload = serde_json::to_string(rows).map_err(|e| Error::BadArchive(e.to_string()))?;
    let done = sqlx::query(AssertSqlSafe(sql))
        .bind(payload)
        .execute(conn)
        .await?;
    Ok(done.rows_affected())
}

async fn usernames(conn: &mut PgConnection) -> Result<HashMap<String, i64>, Error> {
    let rows = sqlx::query("SELECT lower(username) AS u, id FROM superhet.users")
        .fetch_all(conn)
        .await?;
    Ok(rows.iter().map(|r| (r.get("u"), r.get("id"))).collect())
}

fn valid_username(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

struct Ctx<'a> {
    opts: &'a ImportOptions,
    self_user: Option<(i64, String)>,
    warnings: Vec<String>,
    conflicts: Vec<String>,
}

/// Apply one table.
async fn apply_table(
    conn: &mut PgConnection,
    opened: &mut Opened,
    table: &'static Table,
    in_archive: u64,
    ctx: &mut Ctx<'_>,
) -> Result<TableReport, Error> {
    let live = catalogue(conn, table.name).await?;
    let mut report = TableReport {
        table: table.name.to_owned(),
        area: table.area.to_owned(),
        in_archive,
        existing: count(conn, table.name).await?,
        deleted: 0,
        inserted: 0,
        updated: 0,
        skipped: 0,
    };
    // Users stay in the database while their dependants are re-created, so replacing by account
    // name never depends on ids from another install.
    let mut users_by_name = HashMap::new();
    match table.kind {
        Kind::Users => {
            let keep = ctx.self_user.as_ref().map(|(id, _)| *id);
            let done =
                sqlx::query("DELETE FROM superhet.users WHERE $1::bigint IS NULL OR id <> $1")
                    .bind(keep)
                    .execute(&mut *conn)
                    .await?;
            report.deleted = done.rows_affected();
        }
        Kind::Plain { .. } | Kind::ByUser | Kind::DevicePolicies => {
            let sql = format!("DELETE FROM superhet.{}", table.name);
            report.deleted = sqlx::query(AssertSqlSafe(sql))
                .execute(&mut *conn)
                .await?
                .rows_affected();
            if matches!(table.kind, Kind::ByUser | Kind::DevicePolicies) {
                users_by_name = usernames(conn).await?;
            }
        }
        Kind::Append { .. } => {}
    }

    let mut no_credentials = 0u64;
    let mut batches = opened.batches(table.name)?;
    while let Some(mut rows) = batches.next_batch(BATCH_ROWS)? {
        let mut drop: Vec<&str> = Vec::new();
        match table.kind {
            Kind::Users => {
                let mut keep = Vec::with_capacity(rows.len());
                for mut row in rows {
                    let name = row
                        .get("username")
                        .and_then(Value::as_str)
                        .filter(|n| valid_username(n))
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            Error::BadArchive("an account has no valid username".into())
                        })?;
                    if let Some((id, own)) = &ctx.self_user
                        && own.eq_ignore_ascii_case(&name)
                    {
                        self_account(
                            conn,
                            *id,
                            &name,
                            &row,
                            ctx.opts.overwrite_self,
                            &mut report,
                            &mut ctx.conflicts,
                        )
                        .await?;
                        continue;
                    }
                    if row.get("password_hash").is_none_or(Value::is_null) {
                        row["password_hash"] = json!(LOCKED_HASH);
                        no_credentials += 1;
                    }
                    keep.push(row);
                }
                rows = keep;
                drop.push("id");
            }
            Kind::ByUser => {
                let mut keep = Vec::with_capacity(rows.len());
                for mut row in rows {
                    let uid = row
                        .get("username")
                        .and_then(Value::as_str)
                        .and_then(|n| users_by_name.get(&n.to_ascii_lowercase()))
                        .copied();
                    match uid {
                        Some(id) => {
                            if let Some(obj) = row.as_object_mut() {
                                obj.remove("username");
                                obj.insert("user_id".into(), json!(id));
                            }
                            keep.push(row);
                        }
                        None => report.skipped += 1,
                    }
                }
                rows = keep;
                drop.push("id");
            }
            Kind::DevicePolicies => {
                for row in &mut rows {
                    let names = row
                        .get("allowed_usernames")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let ids: Vec<i64> = names
                        .iter()
                        .filter_map(Value::as_str)
                        .filter_map(|n| users_by_name.get(&n.to_ascii_lowercase()).copied())
                        .collect();
                    if ids.len() < names.len() {
                        report.skipped += (names.len() - ids.len()) as u64;
                    }
                    if let Some(obj) = row.as_object_mut() {
                        obj.remove("allowed_usernames");
                        obj.insert("allowed_users".into(), json!(ids));
                    }
                }
            }
            Kind::Append { .. } => drop.push("id"),
            Kind::Plain { .. } => {}
        }
        let wrote = insert_rows(conn, table, &live, &rows, &drop).await?;
        report.inserted += wrote;
        if matches!(table.kind, Kind::Append { .. }) {
            report.skipped += rows.len() as u64 - wrote;
        }
    }
    if no_credentials > 0 {
        ctx.warnings.push(format!(
            "{no_credentials} account(s) come without a password hash: they cannot sign in until you reset their password or send an invite"
        ));
    }
    if matches!(table.kind, Kind::ByUser | Kind::DevicePolicies) && report.skipped > 0 {
        ctx.warnings.push(format!(
            "{}: {} entr(ies) refer to accounts that do not exist here and were skipped",
            table.name, report.skipped
        ));
    }
    if matches!(table.kind, Kind::Plain { identity: true }) && !ctx.opts.dry_run {
        let sql = format!(
            "SELECT setval(pg_get_serial_sequence('superhet.{n}', 'id'), \
                    GREATEST((SELECT COALESCE(max(id), 1) FROM superhet.{n}), 1))",
            n = table.name
        );
        sqlx::query(AssertSqlSafe(sql)).execute(&mut *conn).await?;
    }
    Ok(report)
}

/// The importing admin's own row: untouched unless the admin confirmed.
async fn self_account(
    conn: &mut PgConnection,
    id: i64,
    name: &str,
    row: &Value,
    overwrite: bool,
    report: &mut TableReport,
    conflicts: &mut Vec<String>,
) -> Result<(), Error> {
    let live = sqlx::query("SELECT role, email, password_hash FROM superhet.users WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let role: String = live.get("role");
    let email: Option<String> = live.get("email");
    let hash: String = live.get("password_hash");
    let new_role = row
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or(&role)
        .to_owned();
    let new_email = row.get("email").and_then(Value::as_str).map(str::to_owned);
    let new_hash = row
        .get("password_hash")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut diff = Vec::new();
    if new_role != role {
        diff.push("role");
    }
    if new_email != email {
        diff.push("e-mail");
    }
    if new_hash.as_deref().is_some_and(|h| h != hash) {
        diff.push("password");
    }
    if diff.is_empty() {
        report.skipped += 1;
        return Ok(());
    }
    if !overwrite {
        conflicts.push(format!(
            "your own account \"{name}\" differs in {}: kept as it is (confirm to overwrite it)",
            diff.join(", ")
        ));
        report.skipped += 1;
        return Ok(());
    }
    sqlx::query(
        "UPDATE superhet.users SET role = $2, email = $3, password_hash = COALESCE($4, password_hash), \
         disabled_at = NULL WHERE id = $1",
    )
    .bind(id)
    .bind(new_role)
    .bind(new_email)
    .bind(new_hash)
    .execute(&mut *conn)
    .await?;
    report.updated += 1;
    conflicts.push(format!(
        "your own account \"{name}\" was overwritten with the backup's version ({})",
        diff.join(", ")
    ));
    Ok(())
}

/// Import (or, with `dry_run`, rehearse the import of) an opened archive.
///
/// # Errors
/// [`Error::Newer`] if the backup's schema is newer than this install; [`Error::Invalid`] for
/// areas the archive does not hold; [`Error::Conflict`] if no active admin would be left;
/// database errors (the transaction is rolled back, nothing changes).
pub async fn import(
    pool: &PgPool,
    opened: &mut Opened,
    opts: &ImportOptions,
) -> Result<Report, Error> {
    let current = schema_version(pool).await?;
    let manifest = opened.manifest.clone();
    if manifest.schema_version > current {
        return Err(Error::Newer(format!(
            "this backup was made with database schema {} but this install is at {current}: update Superhet before restoring it",
            manifest.schema_version
        )));
    }
    let mut ctx = Ctx {
        opts,
        self_user: None,
        warnings: Vec::new(),
        conflicts: Vec::new(),
    };
    let areas = if opts.areas.is_empty() {
        manifest.areas.clone()
    } else {
        resolve_areas(&opts.areas)?
    };
    if let Some(missing) = areas.iter().find(|a| !manifest.areas.contains(a)) {
        return Err(Error::Invalid(format!(
            "the backup does not contain the area {missing:?}"
        )));
    }
    let touches_users = areas.iter().any(|a| a == "users");
    let mut tx = pool.begin().await?;
    if touches_users {
        let id = opts.self_user_id.ok_or_else(|| {
            Error::Invalid("restoring accounts needs the signed-in admin to be known".into())
        })?;
        let row = sqlx::query("SELECT username FROM superhet.users WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| Error::Conflict("your account no longer exists".into()))?;
        ctx.self_user = Some((id, row.get("username")));
    }
    if manifest.schema_version < current {
        ctx.warnings.push(format!(
            "the backup is from an older schema ({} < {current}): columns it does not know keep their defaults",
            manifest.schema_version
        ));
    }
    let mut reports = Vec::new();
    for table in tables::tables_of(&areas) {
        let Some(file) = manifest.files.iter().find(|f| f.table == table.name) else {
            if !table.creds_only {
                ctx.warnings.push(format!(
                    "{} is not in the backup and stays as it is",
                    table.name
                ));
            }
            continue;
        };
        reports.push(apply_table(&mut tx, opened, table, file.rows, &mut ctx).await?);
    }
    if touches_users {
        let admins: i64 = sqlx::query(
            "SELECT count(*)::bigint AS n FROM superhet.users WHERE role = 'admin' AND disabled_at IS NULL",
        )
        .fetch_one(&mut *tx)
        .await?
        .get("n");
        if admins == 0 {
            return Err(Error::Conflict(
                "the restore would leave no active admin account; nothing was changed".into(),
            ));
        }
    }
    let files_in_archive = manifest.blobs.len() as u64;
    let mut files_restored = 0;
    if !manifest.blobs.is_empty() && areas.iter().any(|a| a == "recording_files") {
        match (&opts.recordings_dir, opts.dry_run) {
            (None, _) => ctx.warnings.push(
                "recording files were skipped: this service has no recordings directory".into(),
            ),
            (Some(dir), false) => files_restored = restore_blobs(opened, dir)?,
            (Some(_), true) => {}
        }
    }
    if opts.dry_run {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(Report {
        dry_run: opts.dry_run,
        backup: summary(&manifest),
        current_schema_version: current,
        areas,
        tables: reports,
        files_in_archive,
        files_restored,
        warnings: ctx.warnings,
        conflicts: ctx.conflicts,
    })
}

/// Write the recording files of the archive into the library. The path is rebuilt from the
/// validated kind and stem (never from the entry name), written as `.partial` and renamed.
fn restore_blobs(opened: &mut Opened, dir: &std::path::Path) -> Result<u64, Error> {
    use std::io::Write;
    use superhet_recording::files::{Kind, Part, resolve};
    let blobs = opened.manifest.blobs.clone();
    let mut restored = 0;
    for blob in blobs {
        let part = if blob.part == "meta" {
            Part::Meta
        } else {
            Part::Main
        };
        let target = Kind::parse(&blob.kind)
            .and_then(|kind| resolve(dir, kind, &blob.stem, part))
            .ok_or_else(|| Error::BadArchive(format!("{} is not a recording file", blob.path)))?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let partial = target.with_extension("partial");
        let mut out = std::fs::File::create(&partial)?;
        let copied = std::io::copy(&mut opened.blob_reader(&blob.path)?, &mut out)?;
        out.flush()?;
        if copied != blob.bytes {
            let _ = std::fs::remove_file(&partial);
            return Err(Error::BadArchive(format!(
                "{} is shorter than declared",
                blob.path
            )));
        }
        std::fs::rename(&partial, &target)?;
        restored += 1;
    }
    Ok(restored)
}
