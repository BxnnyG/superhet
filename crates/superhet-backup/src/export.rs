//! Writing a backup archive from the database.

use std::fs::File;
use std::path::PathBuf;

use futures_util::TryStreamExt;
use sqlx::{PgPool, Row};

use crate::archive::{FORMAT, Manifest, Writer};
use crate::{Error, tables, temp_file};

/// What to put into a backup.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Area ids (see [`crate::AREAS`]); empty means the default set.
    pub areas: Vec<String>,
    /// Include password hashes, invites, target secrets and push keys.
    pub include_credentials: bool,
    /// Who makes the backup (shown in the manifest).
    pub created_by: String,
    /// The recordings library: where the `recording_files` area reads from. Without it that
    /// area is refused.
    pub recordings_dir: Option<PathBuf>,
}

/// Newest applied database migration: the schema version of this install.
///
/// # Errors
/// Fails if the database cannot be read.
pub async fn schema_version(pool: &PgPool) -> Result<i64, Error> {
    let row = sqlx::query(
        "SELECT COALESCE(max(version), 0)::bigint AS v FROM superhet._sqlx_migrations WHERE success",
    )
    .fetch_one(pool)
    .await?;
    Ok(row.get("v"))
}

/// Normalise a requested area list: known ids only, no duplicates, default set when empty.
pub(crate) fn resolve_areas(requested: &[String]) -> Result<Vec<String>, Error> {
    if requested.is_empty() {
        return Ok(tables::default_areas()
            .into_iter()
            .map(str::to_owned)
            .collect());
    }
    let mut out: Vec<String> = Vec::new();
    for id in requested {
        if tables::area(id).is_none() {
            return Err(Error::Invalid(format!("unknown area {id:?}")));
        }
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    if out.iter().any(|a| a == "recording_files") && !out.iter().any(|a| a == "recordings") {
        return Err(Error::Invalid(
            "recording files need the recordings metadata as well".into(),
        ));
    }
    // Keep the registry order, so archives look alike.
    out.sort_by_key(|id| tables::AREAS.iter().position(|a| a.id == id));
    Ok(out)
}

/// Write a backup into an anonymous temporary file and return it rewound, with its manifest.
///
/// All tables are read in one repeatable-read transaction, so the archive is a consistent
/// snapshot even while the install is in use.
///
/// # Errors
/// [`Error::Invalid`] for an unknown area; database and I/O errors.
pub async fn export(pool: &PgPool, options: &ExportOptions) -> Result<(File, Manifest), Error> {
    let areas = resolve_areas(&options.areas)?;
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let stamp = sqlx::query(
        "SELECT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS t, \
                (SELECT COALESCE(max(version), 0)::bigint FROM superhet._sqlx_migrations WHERE success) AS v",
    )
    .fetch_one(&mut *tx)
    .await?;
    let created_at: String = stamp.get("t");
    let schema: i64 = stamp.get("v");

    let mut writer = Writer::new(temp_file()?);
    for table in tables::tables_of(&areas) {
        if table.creds_only && !options.include_credentials {
            continue;
        }
        writer.begin(table.name, table.area)?;
        let mut rows = sqlx::query(table.export)
            .bind(options.include_credentials)
            .fetch(&mut *tx);
        while let Some(row) = rows.try_next().await? {
            let json: String = row.try_get(0)?;
            writer.row(&json)?;
        }
        drop(rows);
        writer.end()?;
    }
    if areas.iter().any(|a| a == "recording_files") {
        let dir = options
            .recordings_dir
            .as_ref()
            .ok_or_else(|| Error::Invalid("this service has no recordings directory".into()))?;
        add_recording_files(&mut tx, &mut writer, dir).await?;
    }
    tx.rollback().await?;
    writer.finish(Manifest {
        format: FORMAT,
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        schema_version: schema,
        created_at,
        created_by: options.created_by.clone(),
        includes_credentials: options.include_credentials,
        areas,
        files: Vec::new(),
        blobs: Vec::new(),
    })
}

/// Stream the files of every finished recording into the archive (missing files are skipped).
async fn add_recording_files(
    tx: &mut sqlx::PgConnection,
    writer: &mut Writer,
    dir: &std::path::Path,
) -> Result<(), Error> {
    use superhet_recording::files::{Kind, Part, resolve};
    let rows = sqlx::query(
        "SELECT kind, stem FROM superhet.recordings WHERE state <> 'recording' ORDER BY id",
    )
    .fetch_all(tx)
    .await?;
    for row in rows {
        let kind: String = row.get("kind");
        let stem: String = row.get("stem");
        let Some(kind) = Kind::parse(&kind) else {
            continue;
        };
        for (part, label) in [(Part::Main, "main"), (Part::Meta, "meta")] {
            let (Some(path), Some(ext)) = (resolve(dir, kind, &stem, part), kind.extension(part))
            else {
                continue;
            };
            let Ok(mut file) = File::open(&path) else {
                continue;
            };
            if !file.metadata().is_ok_and(|m| m.is_file()) {
                continue;
            }
            writer.blob(kind.as_str(), &stem, label, ext, &mut file)?;
        }
    }
    Ok(())
}
