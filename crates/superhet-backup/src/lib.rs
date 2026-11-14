//! Logical backup and restore of a Superhet install (ADR-0030, `docs/backup.md`).
//!
//! A backup is one zip archive: `manifest.json` (format, app and schema version, creation time,
//! one sha256 per data file) plus `data/<table>.jsonl`, one JSON document per row. It carries
//! settings, accounts, device policies, bookmarks, profiles, channel plans, notification rules and
//! targets and plugin instances; the audit log and the decoder history are opt-in. Password
//! hashes and secrets only travel when the caller asks for credentials. The aircraft database is
//! never included (it is downloaded again). The whole file can be sealed with a passphrase
//! ([`crypto`]).
//!
//! Importing treats the archive as untrusted input: size and entry limits, only manifest-listed
//! `data/<known table>.jsonl` entries, nothing is ever extracted to disk, every file is hashed
//! before the database is touched. The import runs in one transaction; a dry run is the same
//! code rolled back, so its report is exactly what the real import would do.

pub mod archive;
pub mod crypto;
mod export;
mod import;
pub mod schedule;
mod tables;

use std::fs::{File, OpenOptions};
use std::io;

pub use archive::{FORMAT, FileEntry, Limits, Manifest, Opened};
pub use export::{ExportOptions, export, schema_version};
pub use import::{ImportOptions, LOCKED_HASH, Report, TableReport, import};
pub use tables::{AREAS, Area, default_areas};

/// Errors of backup and restore. The variants map to distinct API answers.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The archive is malformed, tampered with, or breaks a limit.
    #[error("invalid backup: {0}")]
    BadArchive(String),
    /// The archive was written by a newer Superhet (format or database schema).
    #[error("{0}")]
    Newer(String),
    /// The archive is encrypted and no passphrase was given.
    #[error("the backup is encrypted: a passphrase is required")]
    PassphraseRequired,
    /// Wrong passphrase, or the encrypted archive was modified.
    #[error("wrong passphrase, or the encrypted backup is damaged")]
    WrongPassphrase,
    /// The request itself is wrong (unknown area, weak passphrase, ...).
    #[error("{0}")]
    Invalid(String),
    /// The import would leave the install unusable or touch something that needs confirmation.
    #[error("{0}")]
    Conflict(String),
    /// The database refused the data (constraint, connectivity).
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    /// Reading or writing a file failed.
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
}

/// An anonymous read/write temporary file: created in `SUPERHET_BACKUP_TMP` (default: the system
/// temp directory) and unlinked at once, so nothing is left behind after a crash.
///
/// # Errors
/// Fails if the file cannot be created.
pub fn temp_file() -> io::Result<File> {
    let dir = std::env::var_os("SUPERHET_BACKUP_TMP")
        .filter(|d| !d.is_empty())
        .map_or_else(std::env::temp_dir, Into::into);
    let path = dir.join(format!("superhet-backup-{}.tmp", uuid::Uuid::new_v4()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    Ok(file)
}

/// Lower-case hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
