//! The ADS-B aircraft database: reading the Mictronics export, downloading it on request and
//! importing it into PostgreSQL (ADR-0023, `docs/plugins-adsb.md`).
//!
//! Superhet ships no aircraft data. The admin either presses "Download" (one fixed URL, the
//! Mictronics aircraft database, ODC-By 1.0) or points `SUPERHET_AIRCRAFT_DB_FILE` at a copy of
//! that export (a zip, or a directory with its files) for installations without internet access.
//! Lookups afterwards are local queries; nothing is asked from a third party per aircraft.
//!
//! The export is `indexedDB.zip` of <https://github.com/Mictronics/aircraft-database>:
//! `aircrafts.json` (`{"3C6444": ["D-AIXA", "A359", "00"], ...}`: registration, ICAO type
//! designator, two flag characters), `types.json` (`{"A359": ["AIRBUS A-350-900", "L2J", "H"]}`:
//! description, class, wake category), `operators.json` (`{"DLH": ["Lufthansa", "Germany",
//! "LUFTHANSA"]}`: name, country, radio callsign) and `dbversion.json`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde::de::{MapAccess, Visitor};
use serde_json::Value;
use superhet_db::adsb::{AircraftRow, Dataset, OperatorRow, Source, TypeRow};

/// Where the download comes from: the Mictronics aircraft database, ODC-By 1.0.
pub const DOWNLOAD_URL: &str =
    "https://raw.githubusercontent.com/Mictronics/aircraft-database/main/indexedDB.zip";
/// Project page shown next to the licence.
pub const SOURCE_URL: &str = "https://github.com/Mictronics/aircraft-database";
/// Licence of the Mictronics export (Open Data Commons Attribution License 1.0).
pub const LICENCE: &str = "ODC-By 1.0";
/// Text of the licence.
pub const LICENCE_URL: &str = "https://opendatacommons.org/licenses/by/1-0/";
/// Name of the database in the UI.
pub const SOURCE_NAME: &str = "Mictronics aircraft database";
/// Most bytes a download may have (the real archive is about 3.5 MB).
pub const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// Most bytes one file of the archive may have once unpacked (the real one is about 15 MB).
const MAX_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
/// Most aircraft imported (the real database has about 460,000).
const MAX_AIRCRAFT: usize = 3_000_000;
/// Name of the downloaded archive inside the data directory.
pub const ARCHIVE_NAME: &str = "aircraft-db.zip";
/// First line of the ODC-By licence text that the export carries.
const ODC_HEADER: &str = "ODC Attribution License";

/// A database read from disk, ready to import.
#[derive(Debug)]
pub struct Loaded {
    pub dataset: Dataset,
    pub version: String,
    /// The export carries the ODC-By licence text.
    pub odc_by: bool,
}

/// Why a database could not be read or fetched.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// Reading the file or directory failed.
    #[error("cannot read {0}: {1}")]
    Io(String, String),
    /// The archive or one of its files is not what the Mictronics export looks like.
    #[error("{0}")]
    Format(String),
    /// The download failed.
    #[error("download failed: {0}")]
    Download(String),
}

fn clean(text: &str, max: usize) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect()
}

/// `aircrafts.json` as rows. Streams the object so the 15 MB document is never held twice.
struct Aircraft(Vec<AircraftRow>);

impl<'de> Deserialize<'de> for Aircraft {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Aircraft;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object of ICAO address to [registration, type, flags]")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Aircraft, A::Error> {
                let mut rows = Vec::with_capacity(map.size_hint().unwrap_or(0).min(MAX_AIRCRAFT));
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if rows.len() >= MAX_AIRCRAFT {
                        return Err(serde::de::Error::custom("too many aircraft"));
                    }
                    let Some(icao) = (key.len() == 6)
                        .then(|| u32::from_str_radix(&key, 16).ok())
                        .flatten()
                    else {
                        continue;
                    };
                    let Some(fields) = value.as_array() else {
                        continue;
                    };
                    let text = |i: usize, max: usize| {
                        fields
                            .get(i)
                            .and_then(Value::as_str)
                            .map(|s| clean(s, max))
                            .unwrap_or_default()
                    };
                    let flags = text(2, 8);
                    let mut bits = 0i16;
                    // Two characters: military, then "interesting".
                    for (i, c) in flags.chars().take(2).enumerate() {
                        if c == '1' {
                            bits |= 1 << i;
                        }
                    }
                    rows.push(AircraftRow {
                        icao,
                        registration: text(0, 32),
                        type_code: text(1, 16),
                        flags: bits,
                    });
                }
                Ok(Aircraft(rows))
            }
        }
        d.deserialize_map(V)
    }
}

/// `{"KEY": ["a", "b", "c"]}` objects as `(key, [a, b, c])`.
fn triples(doc: &Value, what: &str) -> Result<Vec<(String, [String; 3])>, LoadError> {
    let map = doc
        .as_object()
        .ok_or_else(|| LoadError::Format(format!("{what} is not a JSON object")))?;
    Ok(map
        .iter()
        .filter_map(|(key, value)| {
            let fields = value.as_array()?;
            let text = |i: usize, max: usize| {
                fields
                    .get(i)
                    .and_then(Value::as_str)
                    .map(|s| clean(s, max))
                    .unwrap_or_default()
            };
            let key = clean(key, 16);
            (!key.is_empty()).then(|| (key, [text(0, 128), text(1, 64), text(2, 64)]))
        })
        .collect())
}

/// Build a dataset from the contents of the four files. `aircrafts` is required, the others are
/// optional (types and operators only add descriptions).
///
/// # Errors
/// [`LoadError::Format`] when `aircrafts` is not the expected JSON.
pub fn parse(
    aircrafts: &[u8],
    types: Option<&[u8]>,
    operators: Option<&[u8]>,
    version: Option<&[u8]>,
    licence: Option<&[u8]>,
) -> Result<Loaded, LoadError> {
    let Aircraft(aircraft) = serde_json::from_slice(aircrafts)
        .map_err(|e| LoadError::Format(format!("aircrafts.json: {e}")))?;
    if aircraft.is_empty() {
        return Err(LoadError::Format(
            "aircrafts.json holds no aircraft".to_owned(),
        ));
    }
    let types = match types {
        Some(bytes) => {
            let doc: Value = serde_json::from_slice(bytes)
                .map_err(|e| LoadError::Format(format!("types.json: {e}")))?;
            triples(&doc, "types.json")?
                .into_iter()
                .map(|(code, [description, class, wake])| TypeRow {
                    code,
                    description,
                    class: class.chars().take(8).collect(),
                    wake: wake.chars().take(2).collect(),
                })
                .collect()
        }
        None => Vec::new(),
    };
    let operators = match operators {
        Some(bytes) => {
            let doc: Value = serde_json::from_slice(bytes)
                .map_err(|e| LoadError::Format(format!("operators.json: {e}")))?;
            triples(&doc, "operators.json")?
                .into_iter()
                .map(|(code, [name, country, callsign])| OperatorRow {
                    code,
                    name,
                    country,
                    callsign,
                })
                .collect()
        }
        None => Vec::new(),
    };
    let version = version
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .and_then(|doc| match &doc["version"] {
            Value::Number(n) => Some(n.to_string()),
            Value::String(s) => Some(clean(s, 32)),
            _ => None,
        })
        .unwrap_or_default();
    let odc_by = licence.is_some_and(|bytes| {
        String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).contains(ODC_HEADER)
    });
    Ok(Loaded {
        dataset: Dataset {
            aircraft,
            types,
            operators,
        },
        version,
        odc_by,
    })
}

fn read_limited(reader: impl Read, name: &str) -> Result<Vec<u8>, LoadError> {
    let mut out = Vec::new();
    reader
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|e| LoadError::Io(name.to_owned(), e.to_string()))?;
    if out.len() as u64 > MAX_ENTRY_BYTES {
        return Err(LoadError::Format(format!("{name} is too large")));
    }
    Ok(out)
}

/// Read an export: a zip archive or a directory holding its files.
///
/// # Errors
/// [`LoadError`] when the path cannot be read or does not hold an export.
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let shown = path.display().to_string();
    let io = |e: std::io::Error| LoadError::Io(shown.clone(), e.to_string());
    let wanted = [
        "aircrafts.json",
        "types.json",
        "operators.json",
        "dbversion.json",
        "LICENSE",
    ];
    let mut files: [Option<Vec<u8>>; 5] = [None, None, None, None, None];
    if path.is_dir() {
        for (slot, name) in files.iter_mut().zip(wanted) {
            let file = path.join(name);
            if file.is_file() {
                *slot = Some(read_limited(std::fs::File::open(&file).map_err(io)?, name)?);
            }
        }
    } else {
        let file = std::fs::File::open(path).map_err(io)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| LoadError::Format(format!("{shown} is not a zip archive: {e}")))?;
        for index in 0..archive.len() {
            let entry = archive
                .by_index(index)
                .map_err(|e| LoadError::Format(format!("{shown}: {e}")))?;
            // Only the base name counts; a path inside the archive is never used on disk.
            let base = entry
                .name()
                .map(|n| n.rsplit('/').next().unwrap_or("").to_owned())
                .unwrap_or_default();
            if let Some(i) = wanted.iter().position(|w| *w == base)
                && entry.is_file()
                && files[i].is_none()
            {
                files[i] = Some(read_limited(entry, &base)?);
            }
        }
    }
    let [aircrafts, types, operators, version, licence] = files;
    let aircrafts = aircrafts.ok_or_else(|| {
        LoadError::Format(format!(
            "{shown} has no aircrafts.json (expected the Mictronics indexedDB.zip export)"
        ))
    })?;
    parse(
        &aircrafts,
        types.as_deref(),
        operators.as_deref(),
        version.as_deref(),
        licence.as_deref(),
    )
}

/// The source description stored with an import.
#[must_use]
pub fn source_for(loaded: &Loaded, origin: &str) -> Source {
    let (name, licence) = if origin == "download" || loaded.odc_by {
        (SOURCE_NAME.to_owned(), LICENCE.to_owned())
    } else {
        (
            "Imported aircraft database".to_owned(),
            "unknown (supplied by the operator)".to_owned(),
        )
    };
    let known = origin == "download" || loaded.odc_by;
    Source {
        source: name,
        source_url: if known { SOURCE_URL } else { "" }.to_owned(),
        licence,
        licence_url: if known { LICENCE_URL } else { "" }.to_owned(),
        version: loaded.version.clone(),
        origin: origin.to_owned(),
    }
}

/// Download `url` into `dir/`[`ARCHIVE_NAME`] (through a temporary file, so a failed download
/// never replaces a good archive) and return its path. Redirects are not followed.
///
/// # Errors
/// [`LoadError::Download`] or [`LoadError::Io`].
pub async fn download(url: &str, dir: &Path, max_bytes: u64) -> Result<PathBuf, LoadError> {
    use tokio::io::AsyncWriteExt;

    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .user_agent(concat!(
            "Superhet/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/BxnnyG/superhet)"
        ))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| LoadError::Download(e.to_string()))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| LoadError::Download(e.without_url().to_string()))?;
    if !response.status().is_success() {
        return Err(LoadError::Download(format!(
            "the server answered {}",
            response.status()
        )));
    }
    if response.content_length().is_some_and(|n| n > max_bytes) {
        return Err(LoadError::Download("the file is too large".to_owned()));
    }
    let io = |e: std::io::Error| LoadError::Io(dir.display().to_string(), e.to_string());
    tokio::fs::create_dir_all(dir).await.map_err(io)?;
    let tmp = dir.join(format!("{ARCHIVE_NAME}.part"));
    let result = async {
        let mut file = tokio::fs::File::create(&tmp).await.map_err(io)?;
        let mut total = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| LoadError::Download(e.without_url().to_string()))?
        {
            total += chunk.len() as u64;
            if total > max_bytes {
                return Err(LoadError::Download("the file is too large".to_owned()));
            }
            file.write_all(&chunk).await.map_err(io)?;
        }
        file.flush().await.map_err(io)?;
        Ok(())
    }
    .await;
    if let Err(err) = result {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(err);
    }
    let dest = dir.join(ARCHIVE_NAME);
    tokio::fs::rename(&tmp, &dest).await.map_err(io)?;
    Ok(dest)
}

/// Directory for the downloaded archive: `SUPERHET_AIRCRAFT_DB_DIR`, default `/data/aircraft-db`.
#[must_use]
pub fn data_dir() -> PathBuf {
    std::env::var_os("SUPERHET_AIRCRAFT_DB_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from("/data/aircraft-db"), PathBuf::from)
}

/// `SUPERHET_AIRCRAFT_DB_FILE`: an export to import without internet access.
#[must_use]
pub fn import_file() -> Option<PathBuf> {
    std::env::var_os("SUPERHET_AIRCRAFT_DB_FILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    pub(crate) const AIRCRAFTS: &str = r#"{
        "3C6444": ["D-AIXA", "A359", "00"],
        "A835AF": ["N628TS", "GLF6", "01", "extra"],
        "AE1234": ["", "H60", "10"],
        "ZZZZZZ": ["bad", "A320", "00"],
        "12345": ["short key", "A320", "00"],
        "00BEEF": "not a list"
    }"#;
    pub(crate) const TYPES: &str = r#"{
        "A359": ["AIRBUS A-350-900", "L2J", "H"],
        "GLF6": ["GULFSTREAM G650", "L2J", "M"],
        "H60": ["SIKORSKY UH-60 Black Hawk", "H2T", "M"]
    }"#;
    pub(crate) const OPERATORS: &str = r#"{
        "DLH": ["Lufthansa", "Germany", "LUFTHANSA"],
        "AAL": ["American Airlines", "United States", "AMERICAN"]
    }"#;

    /// An in-memory export like Mictronics' `indexedDB.zip` (with a folder, to check that the
    /// base name is used).
    pub(crate) fn zip_bytes(licence: &str) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, body) in [
                ("export/aircrafts.json", AIRCRAFTS),
                ("export/types.json", TYPES),
                ("export/operators.json", OPERATORS),
                ("export/dbversion.json", r#"{"version":530}"#),
                ("LICENSE", licence),
            ] {
                zip.start_file(name, options).expect("start");
                zip.write_all(body.as_bytes()).expect("write");
            }
            zip.finish().expect("finish");
        }
        out.into_inner()
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sh-aircraft-db-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn parses_the_export_and_skips_bad_entries() {
        let loaded = parse(
            AIRCRAFTS.as_bytes(),
            Some(TYPES.as_bytes()),
            Some(OPERATORS.as_bytes()),
            Some(br#"{"version":530}"#),
            Some(b"# ODC Attribution License (ODC-By)\n"),
        )
        .expect("parse");
        assert_eq!(loaded.version, "530");
        assert!(loaded.odc_by);
        let aircraft = &loaded.dataset.aircraft;
        assert_eq!(aircraft.len(), 3, "{aircraft:?}");
        assert_eq!(aircraft[0].icao, 0x3c6444);
        assert_eq!(aircraft[0].registration, "D-AIXA");
        assert_eq!(aircraft[0].type_code, "A359");
        assert_eq!(aircraft[0].flags, 0);
        // "01": not military, interesting.
        assert_eq!(aircraft[1].flags, 2);
        // "10": military.
        assert_eq!(aircraft[2].flags, 1);
        assert_eq!(loaded.dataset.types.len(), 3);
        assert_eq!(loaded.dataset.operators.len(), 2);
        let dlh = loaded
            .dataset
            .operators
            .iter()
            .find(|o| o.code == "DLH")
            .expect("DLH");
        assert_eq!(
            (
                dlh.name.as_str(),
                dlh.country.as_str(),
                dlh.callsign.as_str()
            ),
            ("Lufthansa", "Germany", "LUFTHANSA")
        );
    }

    #[test]
    fn rejects_documents_that_are_not_an_export() {
        for bad in ["[]", "not json", "{}", r#"{"3C6444": 5}"#] {
            assert!(
                parse(bad.as_bytes(), None, None, None, None).is_err(),
                "{bad}"
            );
        }
        assert!(parse(AIRCRAFTS.as_bytes(), Some(b"[]"), None, None, None).is_err());
    }

    #[test]
    fn long_fields_are_cut_and_controls_dropped() {
        let long = "x".repeat(300);
        let doc = format!(r#"{{"000001": ["A\u0007B{long}", "T", "00"]}}"#);
        let loaded = parse(doc.as_bytes(), None, None, None, None).expect("parse");
        let reg = &loaded.dataset.aircraft[0].registration;
        assert_eq!(reg.chars().count(), 32);
        assert!(reg.starts_with("AB"));
    }

    #[test]
    fn loads_a_zip_and_a_directory() {
        let dir = scratch("load");
        let file = dir.join("export.zip");
        std::fs::write(&file, zip_bytes("# ODC Attribution License (ODC-By)")).expect("write");
        let loaded = load(&file).expect("zip");
        assert_eq!(loaded.dataset.aircraft.len(), 3);
        assert_eq!(loaded.version, "530");
        assert!(loaded.odc_by);

        let unpacked = dir.join("unpacked");
        std::fs::create_dir_all(&unpacked).expect("dir");
        std::fs::write(unpacked.join("aircrafts.json"), AIRCRAFTS).expect("write");
        std::fs::write(unpacked.join("types.json"), TYPES).expect("write");
        let loaded = load(&unpacked).expect("dir");
        assert_eq!(loaded.dataset.aircraft.len(), 3);
        assert_eq!(loaded.dataset.types.len(), 3);
        assert!(loaded.dataset.operators.is_empty());
        assert!(!loaded.odc_by);

        // Neither a zip nor an export.
        let junk = dir.join("junk.zip");
        std::fs::write(&junk, b"hello").expect("write");
        assert!(matches!(load(&junk), Err(LoadError::Format(_))));
        assert!(matches!(
            load(&dir.join("missing.zip")),
            Err(LoadError::Io(..))
        ));
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).expect("dir");
        assert!(matches!(load(&empty), Err(LoadError::Format(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_description_is_honest_about_the_licence() {
        let known = Loaded {
            dataset: Dataset::default(),
            version: "530".into(),
            odc_by: true,
        };
        let s = source_for(&known, "file");
        assert_eq!(s.licence, LICENCE);
        assert_eq!(s.source, SOURCE_NAME);
        let unknown = Loaded {
            odc_by: false,
            ..known
        };
        let s = source_for(&unknown, "file");
        assert!(s.licence.starts_with("unknown"));
        assert!(s.source_url.is_empty() && s.licence_url.is_empty());
        assert_eq!(source_for(&unknown, "download").licence, LICENCE);
    }

    #[tokio::test]
    async fn downloads_from_a_local_server_and_enforces_the_size_cap() {
        use axum::{Router, http::StatusCode, routing::get};
        let body = zip_bytes("# ODC Attribution License");
        let served = body.clone();
        let app = Router::new()
            .route("/db.zip", get(move || async move { served.clone() }))
            .route("/missing.zip", get(|| async { StatusCode::NOT_FOUND }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let dir = scratch("download");

        let path = download(&format!("http://127.0.0.1:{port}/db.zip"), &dir, 1 << 20)
            .await
            .expect("download");
        assert_eq!(std::fs::read(&path).expect("read"), body);
        assert_eq!(load(&path).expect("load").dataset.aircraft.len(), 3);

        // Too large, and not found: an error, and the good archive stays.
        let err = download(&format!("http://127.0.0.1:{port}/db.zip"), &dir, 100)
            .await
            .expect_err("too large");
        assert!(err.to_string().contains("too large"), "{err}");
        let err = download(
            &format!("http://127.0.0.1:{port}/missing.zip"),
            &dir,
            1 << 20,
        )
        .await
        .expect_err("404");
        assert!(err.to_string().contains("404"), "{err}");
        assert_eq!(std::fs::read(&path).expect("read"), body);
        assert!(!dir.join(format!("{ARCHIVE_NAME}.part")).exists());
        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
