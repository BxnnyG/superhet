//! The archive container: writing, and opening an untrusted file.
//!
//! Why zip: one dependency the API already has (aircraft database import), a central directory so
//! the manifest can be read first and every entry can be checked against its declared size before
//! a byte is inflated, and the result opens with any unzip tool. Everything here is defensive:
//! [`open`] never extracts to disk, accepts only `manifest.json` and the `data/<table>.jsonl`
//! entries the manifest lists for tables this version knows, caps entries, sizes and line length,
//! and verifies the sha256 of every file before the caller sees a row.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{Error, hex, tables};

/// Archive format version written (and the newest one read).
pub const FORMAT: u32 = 1;
/// Name of the manifest entry.
pub const MANIFEST: &str = "manifest.json";

/// One data file listed in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    /// Entry name, `data/<table>.jsonl`.
    pub path: String,
    /// Table the rows belong to.
    pub table: String,
    /// Area of the table.
    pub area: String,
    /// Number of rows (lines).
    pub rows: u64,
    /// Uncompressed size in bytes.
    pub bytes: u64,
    /// Lower-case hex sha256 of the uncompressed content.
    pub sha256: String,
}

/// A recording file travelling in the archive, `recordings/<kind>/<stem>.<ext>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Blob {
    /// Entry name.
    pub path: String,
    /// `audio` or `iq`.
    pub kind: String,
    /// Stem of the recording (the library's generated file stem).
    pub stem: String,
    /// `main` or `meta`.
    pub part: String,
    /// Size in bytes.
    pub bytes: u64,
    /// Lower-case hex sha256.
    pub sha256: String,
}

/// The manifest: what the archive is and what it holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// Archive format version.
    pub format: u32,
    /// Superhet version that wrote the archive.
    pub app_version: String,
    /// Database schema version (newest applied migration) at the time.
    pub schema_version: i64,
    /// Creation time, UTC, RFC 3339.
    pub created_at: String,
    /// Who made it (account name, or `scheduled`).
    pub created_by: String,
    /// Whether password hashes and secrets are included.
    pub includes_credentials: bool,
    /// Areas contained.
    pub areas: Vec<String>,
    /// The data files with their checksums.
    pub files: Vec<FileEntry>,
    /// Recording files (empty unless the `recording_files` area was selected).
    #[serde(default)]
    pub blobs: Vec<Blob>,
}

/// Bounds applied to an archive from outside.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Most entries (manifest included).
    pub max_entries: usize,
    /// Largest uncompressed data file.
    pub max_entry_bytes: u64,
    /// Largest sum of all uncompressed files.
    pub max_total_bytes: u64,
    /// Longest accepted row (bytes).
    pub max_line_bytes: usize,
    /// Largest manifest.
    pub max_manifest_bytes: u64,
    /// Largest single recording file (IQ recordings are capped at 16 GiB by the library).
    pub max_blob_bytes: u64,
    /// Largest sum of all recording files.
    pub max_blob_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 4096,
            max_entry_bytes: 2 << 30,
            max_total_bytes: 4 << 30,
            max_line_bytes: 1 << 20,
            max_manifest_bytes: 1 << 20,
            max_blob_bytes: 16 << 30,
            max_blob_total_bytes: 64 << 30,
        }
    }
}

fn bad(msg: impl Into<String>) -> Error {
    Error::BadArchive(msg.into())
}

/// Builds an archive in a seekable file, one table after the other.
pub struct Writer {
    zip: ZipWriter<File>,
    files: Vec<FileEntry>,
    blobs: Vec<Blob>,
    current: Option<(FileEntry, Sha256)>,
}

impl Writer {
    /// Start an archive in `file` (empty, read/write).
    #[must_use]
    pub fn new(file: File) -> Self {
        Self {
            zip: ZipWriter::new(file),
            files: Vec::new(),
            blobs: Vec::new(),
            current: None,
        }
    }

    /// Begin the data file of `table`.
    ///
    /// # Errors
    /// Fails on I/O errors or if a table is already open.
    pub fn begin(&mut self, table: &str, area: &str) -> Result<(), Error> {
        if self.current.is_some() {
            return Err(bad("a table is still open"));
        }
        let path = format!("data/{table}.jsonl");
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        self.zip
            .start_file(path.clone(), options)
            .map_err(zip_err)?;
        self.current = Some((
            FileEntry {
                path,
                table: table.to_owned(),
                area: area.to_owned(),
                rows: 0,
                bytes: 0,
                sha256: String::new(),
            },
            Sha256::new(),
        ));
        Ok(())
    }

    /// Append one row (a JSON document without newline).
    ///
    /// # Errors
    /// Fails on I/O errors or if no table is open.
    pub fn row(&mut self, json: &str) -> Result<(), Error> {
        let (entry, hash) = self.current.as_mut().ok_or_else(|| bad("no table open"))?;
        self.zip.write_all(json.as_bytes())?;
        self.zip.write_all(b"\n")?;
        hash.update(json.as_bytes());
        hash.update(b"\n");
        entry.rows += 1;
        entry.bytes += json.len() as u64 + 1;
        Ok(())
    }

    /// Close the open table.
    ///
    /// # Errors
    /// Fails if no table is open.
    pub fn end(&mut self) -> Result<(), Error> {
        let (mut entry, hash) = self.current.take().ok_or_else(|| bad("no table open"))?;
        entry.sha256 = hex(&hash.finalize());
        self.files.push(entry);
        Ok(())
    }

    /// Add a recording file, streamed from `source`, stored uncompressed (Ogg and IQ do not
    /// shrink).
    ///
    /// # Errors
    /// Fails on I/O errors or if a table is open.
    pub fn blob(
        &mut self,
        kind: &str,
        stem: &str,
        part: &str,
        ext: &str,
        source: &mut impl Read,
    ) -> Result<(), Error> {
        if self.current.is_some() {
            return Err(bad("a table is still open"));
        }
        let path = format!("recordings/{kind}/{stem}.{ext}");
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(true);
        self.zip
            .start_file(path.clone(), options)
            .map_err(zip_err)?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut bytes = 0u64;
        loop {
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
            self.zip.write_all(&buf[..n])?;
            bytes += n as u64;
        }
        self.blobs.push(Blob {
            path,
            kind: kind.to_owned(),
            stem: stem.to_owned(),
            part: part.to_owned(),
            bytes,
            sha256: hex(&hash.finalize()),
        });
        Ok(())
    }

    /// Write the manifest (the data files collected so far go into it) and return the file,
    /// rewound to the start.
    ///
    /// # Errors
    /// Fails on I/O errors.
    pub fn finish(mut self, mut manifest: Manifest) -> Result<(File, Manifest), Error> {
        if self.current.is_some() {
            return Err(bad("a table is still open"));
        }
        manifest.files = std::mem::take(&mut self.files);
        manifest.blobs = std::mem::take(&mut self.blobs);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        self.zip.start_file(MANIFEST, options).map_err(zip_err)?;
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| bad(e.to_string()))?;
        self.zip.write_all(&json)?;
        let mut file = self.zip.finish().map_err(zip_err)?;
        file.flush()?;
        file.rewind()?;
        Ok((file, manifest))
    }
}

fn zip_err(e: zip::result::ZipError) -> Error {
    match e {
        zip::result::ZipError::Io(io) => Error::Io(io),
        other => bad(other.to_string()),
    }
}

/// An archive that passed every check.
pub struct Opened {
    archive: ZipArchive<File>,
    /// The verified manifest.
    pub manifest: Manifest,
    limits: Limits,
}

/// Entry names are `manifest.json` or `data/<snake_case>.jsonl`: no separators beyond the one
/// slash, no dots but the extension, nothing that could mean a path outside the archive.
fn plain_name(name: &str) -> bool {
    if name == MANIFEST {
        return true;
    }
    if name.starts_with("recordings/") {
        return blob_parts(name).is_some();
    }
    let Some(file) = name.strip_prefix("data/") else {
        return false;
    };
    let Some(stem) = file.strip_suffix(".jsonl") else {
        return false;
    };
    !stem.is_empty()
        && stem.len() <= 64
        && stem
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// `recordings/<audio|iq>/<safe stem>.<ogg|sigmf-data|sigmf-meta>` split into kind, stem, part.
fn blob_parts(name: &str) -> Option<(&str, &str, &str)> {
    use superhet_recording::files::{Kind, Part, is_safe_stem};
    let rest = name.strip_prefix("recordings/")?;
    let (kind_dir, file) = rest.split_once('/')?;
    let kind = Kind::parse(kind_dir)?;
    let (stem, ext) = file.rsplit_once('.')?;
    if !is_safe_stem(stem) {
        return None;
    }
    let part = [Part::Main, Part::Meta]
        .into_iter()
        .find(|p| kind.extension(*p) == Some(ext))?;
    Some((
        kind.as_str(),
        stem,
        if part == Part::Main { "main" } else { "meta" },
    ))
}

/// Hash `reader` up to `declared` bytes; any difference from the declared length is an error.
fn digest(reader: impl Read, declared: u64) -> Result<(u64, String), Error> {
    let mut limited = reader.take(declared.saturating_add(1));
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = limited
            .read(&mut buf)
            .map_err(|e| bad(format!("unreadable entry: {e}")))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        total += n as u64;
    }
    Ok((total, hex(&hash.finalize())))
}

/// Open and verify an archive.
///
/// # Errors
/// [`Error::BadArchive`] for anything malformed, over a limit, unexpected or tampered with;
/// [`Error::Newer`] for a format this version does not know.
pub fn open(file: File, limits: Limits) -> Result<Opened, Error> {
    let mut archive = ZipArchive::new(file).map_err(|e| bad(format!("not a zip archive ({e})")))?;
    if archive.len() > limits.max_entries {
        return Err(bad(format!("more than {} entries", limits.max_entries)));
    }
    let mut names = HashSet::new();
    let mut sizes = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).map_err(zip_err)?;
        let name = entry.name().map_err(zip_err)?.into_owned();
        if entry.is_dir() || entry.is_symlink() || entry.encrypted() || !plain_name(&name) {
            return Err(bad(format!("unexpected entry {name:?}")));
        }
        if !names.insert(name.clone()) {
            return Err(bad(format!("duplicate entry {name:?}")));
        }
        sizes.push((name, entry.size()));
    }
    let manifest_size = sizes
        .iter()
        .find(|(n, _)| n == MANIFEST)
        .map(|(_, s)| *s)
        .ok_or_else(|| bad("manifest.json is missing"))?;
    if manifest_size > limits.max_manifest_bytes {
        return Err(bad("manifest.json is too large"));
    }
    let manifest: Manifest = {
        let entry = archive.by_name(MANIFEST).map_err(zip_err)?;
        let mut text = Vec::new();
        entry
            .take(limits.max_manifest_bytes + 1)
            .read_to_end(&mut text)
            .map_err(|e| bad(format!("manifest.json unreadable: {e}")))?;
        if text.len() as u64 > limits.max_manifest_bytes {
            return Err(bad("manifest.json is too large"));
        }
        serde_json::from_slice(&text).map_err(|e| bad(format!("manifest.json: {e}")))?
    };
    if manifest.format == 0 || manifest.format > FORMAT {
        return Err(Error::Newer(format!(
            "this backup uses archive format {} but this Superhet reads up to {FORMAT}: update Superhet first",
            manifest.format
        )));
    }
    check_manifest(&manifest, &sizes, &limits)?;
    for entry in &manifest.files {
        let file = archive.by_name(&entry.path).map_err(zip_err)?;
        let (bytes, sha) = digest(file, entry.bytes)?;
        if bytes != entry.bytes || sha != entry.sha256 {
            return Err(bad(format!("{} does not match its checksum", entry.path)));
        }
    }
    for blob in &manifest.blobs {
        let file = archive.by_name(&blob.path).map_err(zip_err)?;
        let (bytes, sha) = digest(file, blob.bytes)?;
        if bytes != blob.bytes || sha != blob.sha256 {
            return Err(bad(format!("{} does not match its checksum", blob.path)));
        }
    }
    Ok(Opened {
        archive,
        manifest,
        limits,
    })
}

fn check_manifest(m: &Manifest, sizes: &[(String, u64)], limits: &Limits) -> Result<(), Error> {
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for f in &m.files {
        let table =
            tables::table(&f.table).ok_or_else(|| bad(format!("unknown table {:?}", f.table)))?;
        if f.path != format!("data/{}.jsonl", f.table) || f.area != table.area {
            return Err(bad(format!("inconsistent manifest entry {:?}", f.path)));
        }
        if !seen.insert(f.path.as_str()) {
            return Err(bad(format!("duplicate manifest entry {:?}", f.path)));
        }
        if f.bytes > limits.max_entry_bytes {
            return Err(bad(format!("{} is larger than the limit", f.path)));
        }
        total = total.saturating_add(f.bytes);
        let size = sizes.iter().find(|(n, _)| *n == f.path).map(|(_, s)| *s);
        if size != Some(f.bytes) {
            return Err(bad(format!(
                "{} is missing or has another size than declared",
                f.path
            )));
        }
    }
    if total > limits.max_total_bytes {
        return Err(bad("the backup is larger than the limit"));
    }
    let mut blob_total = 0u64;
    for b in &m.blobs {
        let parts = blob_parts(&b.path);
        let consistent = parts == Some((b.kind.as_str(), b.stem.as_str(), b.part.as_str()));
        if !consistent || !seen.insert(b.path.as_str()) {
            return Err(bad(format!(
                "inconsistent or duplicate manifest entry {:?}",
                b.path
            )));
        }
        if b.bytes > limits.max_blob_bytes {
            return Err(bad(format!("{} is larger than the limit", b.path)));
        }
        blob_total = blob_total.saturating_add(b.bytes);
        let size = sizes.iter().find(|(n, _)| *n == b.path).map(|(_, s)| *s);
        if size != Some(b.bytes) {
            return Err(bad(format!(
                "{} is missing or has another size than declared",
                b.path
            )));
        }
    }
    if blob_total > limits.max_blob_total_bytes {
        return Err(bad("the recording files are larger than the limit"));
    }
    // Nothing besides the manifest and its files.
    if sizes.len() != m.files.len() + m.blobs.len() + 1 {
        return Err(bad(
            "the archive contains entries the manifest does not list",
        ));
    }
    Ok(())
}

/// Reads the rows of one data file in batches.
pub struct Batches<'a> {
    reader: BufReader<io::Take<zip::read::ZipFile<'a, File>>>,
    max_line: usize,
    buf: Vec<u8>,
}

impl Opened {
    /// A reader over a recording file listed in the manifest.
    ///
    /// # Errors
    /// Fails if the entry cannot be opened.
    pub fn blob_reader(&mut self, path: &str) -> Result<impl Read + '_, Error> {
        let declared = self
            .manifest
            .blobs
            .iter()
            .find(|b| b.path == path)
            .map(|b| b.bytes)
            .ok_or_else(|| bad(format!("{path} is not in the backup")))?;
        let file = self.archive.by_name(path).map_err(zip_err)?;
        Ok(file.take(declared))
    }

    /// Iterate the rows of the file of `table` (the caller checked that it is listed).
    ///
    /// # Errors
    /// Fails if the entry cannot be opened.
    pub fn batches(&mut self, table: &str) -> Result<Batches<'_>, Error> {
        let declared = self
            .manifest
            .files
            .iter()
            .find(|f| f.table == table)
            .map(|f| f.bytes)
            .ok_or_else(|| bad(format!("{table} is not in the backup")))?;
        let file = self
            .archive
            .by_name(&format!("data/{table}.jsonl"))
            .map_err(zip_err)?;
        Ok(Batches {
            reader: BufReader::new(file.take(declared)),
            max_line: self.limits.max_line_bytes,
            buf: Vec::new(),
        })
    }
}

impl Batches<'_> {
    /// Up to `max_rows` rows (JSON objects); `None` at the end.
    ///
    /// # Errors
    /// [`Error::BadArchive`] for an over-long line or a row that is not a JSON object.
    pub fn next_batch(&mut self, max_rows: usize) -> Result<Option<Vec<Value>>, Error> {
        let mut rows = Vec::new();
        while rows.len() < max_rows {
            self.buf.clear();
            let n = (&mut self.reader)
                .take(self.max_line as u64 + 1)
                .read_until(b'\n', &mut self.buf)
                .map_err(|e| bad(format!("unreadable entry: {e}")))?;
            if n == 0 {
                break;
            }
            if self.buf.len() > self.max_line {
                return Err(bad("a row is longer than the limit"));
            }
            let line = self.buf.strip_suffix(b"\n").unwrap_or(&self.buf);
            if line.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_slice(line)
                .map_err(|e| bad(format!("row is not valid JSON: {e}")))?;
            if !value.is_object() {
                return Err(bad("a row is not a JSON object"));
            }
            rows.push(value);
        }
        Ok(if rows.is_empty() { None } else { Some(rows) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_file;

    fn manifest() -> Manifest {
        Manifest {
            format: FORMAT,
            app_version: "0.0.0".into(),
            schema_version: 17,
            created_at: "2026-01-01T00:00:00Z".into(),
            created_by: "test".into(),
            includes_credentials: false,
            areas: vec!["profiles".into()],
            files: Vec::new(),
            blobs: Vec::new(),
        }
    }

    fn build(rows: &[&str]) -> File {
        let mut w = Writer::new(temp_file().expect("tmp"));
        w.begin("profiles", "profiles").expect("begin");
        for r in rows {
            w.row(r).expect("row");
        }
        w.end().expect("end");
        w.finish(manifest()).expect("finish").0
    }

    /// A zip with the given raw entries (for hostile archives).
    fn raw_zip(entries: &[(&str, &[u8])]) -> File {
        let mut z = ZipWriter::new(temp_file().expect("tmp"));
        for (name, data) in entries {
            z.start_file(*name, SimpleFileOptions::default())
                .expect("start");
            z.write_all(data).expect("write");
        }
        let mut f = z.finish().expect("finish");
        f.rewind().expect("rewind");
        f
    }

    #[test]
    fn written_archive_opens_and_yields_rows() {
        let mut opened = open(
            build(&[r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#]),
            Limits::default(),
        )
        .expect("open");
        assert_eq!(opened.manifest.files.len(), 1);
        assert_eq!(opened.manifest.files[0].rows, 3);
        let mut batches = opened.batches("profiles").expect("batches");
        assert_eq!(batches.next_batch(2).expect("b").expect("some").len(), 2);
        assert_eq!(batches.next_batch(2).expect("b").expect("some").len(), 1);
        assert!(batches.next_batch(2).expect("b").is_none());
    }

    #[test]
    fn path_traversal_and_foreign_entries_are_refused() {
        let good = br#"{"format":1,"app_version":"x","schema_version":1,"created_at":"","created_by":"","includes_credentials":false,"areas":[],"files":[]}"#;
        for name in [
            "../evil",
            "/etc/passwd",
            "data/../../evil.jsonl",
            "data\\x.jsonl",
            "data/a.b.jsonl",
            "data/",
            "other.txt",
            "data/Upper.jsonl",
        ] {
            let f = raw_zip(&[(MANIFEST, good), (name, b"x")]);
            let err = open(f, Limits::default())
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(err.contains("unexpected entry"), "{name}: {err}");
        }
    }

    #[test]
    fn unlisted_and_unknown_files_are_refused() {
        let m = br#"{"format":1,"app_version":"x","schema_version":1,"created_at":"","created_by":"","includes_credentials":false,"areas":[],"files":[]}"#;
        let err = open(
            raw_zip(&[(MANIFEST, m), ("data/profiles.jsonl", b"{}\n")]),
            Limits::default(),
        )
        .err();
        assert!(err.is_some_and(|e| e.to_string().contains("does not list")));
        let m = br#"{"format":1,"app_version":"x","schema_version":1,"created_at":"","created_by":"","includes_credentials":false,"areas":[],"files":[{"path":"data/pg_shadow.jsonl","table":"pg_shadow","area":"x","rows":0,"bytes":0,"sha256":""}]}"#;
        let err = open(
            raw_zip(&[(MANIFEST, m), ("data/pg_shadow.jsonl", b"")]),
            Limits::default(),
        )
        .err();
        assert!(err.is_some_and(|e| e.to_string().contains("unknown table")));
    }

    #[test]
    fn modified_content_is_detected() {
        let content = b"{\"a\":1}\n";
        let sha = hex(&Sha256::digest(content));
        let m = format!(
            r#"{{"format":1,"app_version":"x","schema_version":1,"created_at":"","created_by":"","includes_credentials":false,"areas":[],"files":[{{"path":"data/profiles.jsonl","table":"profiles","area":"profiles","rows":1,"bytes":{},"sha256":"{sha}"}}]}}"#,
            content.len()
        );
        let mut z = ZipWriter::new(temp_file().expect("tmp"));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        z.start_file(MANIFEST, stored).expect("start");
        z.write_all(m.as_bytes()).expect("write");
        z.start_file("data/profiles.jsonl", stored).expect("start");
        z.write_all(content).expect("write");
        let mut f = z.finish().expect("finish");
        f.rewind().expect("rewind");
        let mut raw = Vec::new();
        f.read_to_end(&mut raw).expect("read");
        let mut again = temp_file().expect("tmp");
        again.write_all(&raw).expect("write");
        again.rewind().expect("rewind");
        assert!(open(again, Limits::default()).is_ok());
        // Flip one byte of the stored (uncompressed) row.
        let at = raw
            .windows(content.len())
            .position(|w| w == content)
            .expect("content");
        raw[at + 5] = b'9';
        let mut tampered = temp_file().expect("tmp");
        tampered.write_all(&raw).expect("write");
        tampered.rewind().expect("rewind");
        assert!(open(tampered, Limits::default()).is_err());
    }

    #[test]
    fn declared_size_and_hash_must_match() {
        // Same bytes, but the manifest claims another checksum.
        let m = br#"{"format":1,"app_version":"x","schema_version":1,"created_at":"","created_by":"","includes_credentials":false,"areas":[],"files":[{"path":"data/profiles.jsonl","table":"profiles","area":"profiles","rows":1,"bytes":3,"sha256":"00"}]}"#;
        let err = open(
            raw_zip(&[(MANIFEST, m), ("data/profiles.jsonl", b"{}\n")]),
            Limits::default(),
        )
        .err();
        assert!(err.is_some_and(|e| e.to_string().contains("checksum")));
    }

    #[test]
    fn entry_and_size_limits_hold() {
        let f = build(&[r#"{"a":1}"#]);
        let tight = Limits {
            max_entry_bytes: 2,
            ..Limits::default()
        };
        assert!(open(f, tight).is_err());
        let f = build(&[r#"{"a":1}"#]);
        let few = Limits {
            max_entries: 1,
            ..Limits::default()
        };
        assert!(open(f, few).is_err());
    }

    #[test]
    fn newer_format_is_refused_with_its_own_error() {
        let m = br#"{"format":99,"app_version":"x","schema_version":1,"created_at":"","created_by":"","includes_credentials":false,"areas":[],"files":[]}"#;
        assert!(matches!(
            open(raw_zip(&[(MANIFEST, m)]), Limits::default()),
            Err(Error::Newer(_))
        ));
    }

    #[test]
    fn a_decompression_bomb_cannot_exceed_the_declared_size() {
        // The manifest declares 3 bytes; the entry really holds far more. The size check against
        // the central directory fires before anything is inflated.
        let big = vec![b'a'; 5_000_000];
        let m = br#"{"format":1,"app_version":"x","schema_version":1,"created_at":"","created_by":"","includes_credentials":false,"areas":[],"files":[{"path":"data/profiles.jsonl","table":"profiles","area":"profiles","rows":1,"bytes":3,"sha256":"00"}]}"#;
        let err = open(
            raw_zip(&[(MANIFEST, m), ("data/profiles.jsonl", &big)]),
            Limits::default(),
        )
        .err();
        assert!(err.is_some_and(|e| e.to_string().contains("another size")));
    }

    #[test]
    fn long_rows_and_non_objects_are_refused() {
        let long = format!(r#"{{"a":"{}"}}"#, "x".repeat(200));
        let mut opened = open(
            build(&[&long]),
            Limits {
                max_line_bytes: 64,
                ..Limits::default()
            },
        )
        .expect("open");
        assert!(
            opened
                .batches("profiles")
                .expect("b")
                .next_batch(10)
                .is_err()
        );
        let mut opened = open(build(&["[1,2]"]), Limits::default()).expect("open");
        assert!(
            opened
                .batches("profiles")
                .expect("b")
                .next_batch(10)
                .is_err()
        );
    }
}
