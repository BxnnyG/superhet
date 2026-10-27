//! Optional offline GeoIP lookups from MMDB files the operator supplies (`SUPERHET_GEOIP_DB`).
//!
//! Nothing is bundled or downloaded: DB-IP Lite (CC BY 4.0) and MaxMind GeoLite2 (EULA, free
//! account) both ship MMDB files. Several files may be listed (for example a city and an ASN
//! database); every file contributes the fields it has. Lookups never leave the machine.

use std::net::IpAddr;
use std::path::PathBuf;

use maxminddb::{Reader, path};
use serde::Serialize;

/// Environment variable with one or more MMDB paths, separated by commas.
pub const ENV_GEOIP_DB: &str = "SUPERHET_GEOIP_DB";

/// What the databases know about an address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct GeoInfo {
    /// ISO 3166-1 alpha-2 code.
    pub country: Option<String>,
    /// English country name.
    pub country_name: Option<String>,
    /// English city name.
    pub city: Option<String>,
    /// Autonomous system number.
    pub asn: Option<u32>,
    /// Organisation owning the autonomous system.
    pub org: Option<String>,
}

impl GeoInfo {
    /// Only country and network, no city (what guests keep when addresses are truncated).
    #[must_use]
    pub fn coarse(mut self) -> Self {
        self.city = None;
        self
    }
}

/// The opened databases.
pub struct GeoDb {
    readers: Vec<(PathBuf, Reader<Vec<u8>>)>,
}

impl std::fmt::Debug for GeoDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoDb")
            .field("files", &self.readers.len())
            .finish()
    }
}

impl GeoDb {
    /// Open every file of a comma separated list.
    ///
    /// # Errors
    /// A message naming the file that could not be read or is not an MMDB database.
    pub fn open_all(list: &str) -> Result<Self, String> {
        let mut readers = Vec::new();
        for path in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let reader = Reader::open_readfile(path)
                .map_err(|err| format!("{ENV_GEOIP_DB}: cannot open {path}: {err}"))?;
            readers.push((PathBuf::from(path), reader));
        }
        if readers.is_empty() {
            return Err(format!("{ENV_GEOIP_DB} names no file"));
        }
        Ok(Self { readers })
    }

    /// Database descriptions for the start-up log.
    #[must_use]
    pub fn describe(&self) -> Vec<String> {
        self.readers
            .iter()
            .map(|(p, r)| {
                format!(
                    "{} ({})",
                    p.file_name().map_or_else(
                        || p.display().to_string(),
                        |n| n.to_string_lossy().into_owned()
                    ),
                    r.metadata().database_type
                )
            })
            .collect()
    }

    /// Look an address up in all databases.
    #[must_use]
    pub fn lookup(&self, ip: IpAddr) -> GeoInfo {
        let mut info = GeoInfo::default();
        for (_, reader) in &self.readers {
            let Ok(result) = reader.lookup(ip) else {
                continue;
            };
            if !result.has_data() {
                continue;
            }
            fill(
                &mut info.country,
                &result.decode_path(&path!["country", "iso_code"]),
            );
            fill(
                &mut info.country_name,
                &result.decode_path(&path!["country", "names", "en"]),
            );
            fill(
                &mut info.city,
                &result.decode_path(&path!["city", "names", "en"]),
            );
            fill(
                &mut info.org,
                &result.decode_path(&path!["autonomous_system_organization"]),
            );
            if info.asn.is_none()
                && let Ok(Some(n)) = result.decode_path::<u32>(&path!["autonomous_system_number"])
            {
                info.asn = Some(n);
            }
        }
        info
    }
}

fn fill(slot: &mut Option<String>, value: &Result<Option<String>, maxminddb::MaxMindDbError>) {
    if slot.is_none()
        && let Ok(Some(v)) = value
        && !v.is_empty()
    {
        *slot = Some(v.chars().take(128).collect());
    }
}

/// Open the databases named by the environment, if any. A configured but unusable database is
/// logged and skipped: the admin views just show no location.
#[must_use]
pub fn from_env() -> Option<GeoDb> {
    let list = std::env::var(ENV_GEOIP_DB).ok()?;
    if list.trim().is_empty() {
        return None;
    }
    match GeoDb::open_all(&list) {
        Ok(db) => {
            tracing::info!(databases = ?db.describe(), "GeoIP databases loaded");
            Some(db)
        }
        Err(err) => {
            tracing::error!(%err, "GeoIP disabled");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(out: &mut Vec<u8>, text: &str) {
        // Sizes of 29 and more continue in one extra byte (29 + extra).
        if text.len() >= 29 {
            out.push(0x40 | 29);
            out.push(u8::try_from(text.len() - 29).expect("short"));
        } else {
            out.push(0x40 | u8::try_from(text.len()).expect("short"));
        }
        out.extend_from_slice(text.as_bytes());
    }

    fn map(out: &mut Vec<u8>, entries: usize) {
        out.push(0xe0 | u8::try_from(entries).expect("few"));
    }

    /// A minimal IPv4 MMDB: addresses starting with a 0 bit (0.0.0.0/1) are Berlin, AS 3320.
    fn tiny_mmdb() -> Vec<u8> {
        let mut data = Vec::new();
        map(&mut data, 4);
        string(&mut data, "country");
        map(&mut data, 2);
        string(&mut data, "iso_code");
        string(&mut data, "DE");
        string(&mut data, "names");
        map(&mut data, 1);
        string(&mut data, "en");
        string(&mut data, "Germany");
        string(&mut data, "city");
        map(&mut data, 1);
        string(&mut data, "names");
        map(&mut data, 1);
        string(&mut data, "en");
        string(&mut data, "Berlin");
        string(&mut data, "autonomous_system_number");
        data.extend_from_slice(&[0xc2, 0x0c, 0xf8]); // uint32 3320
        string(&mut data, "autonomous_system_organization");
        string(&mut data, "Example Telecom");

        let mut meta = Vec::new();
        map(&mut meta, 9);
        string(&mut meta, "description");
        map(&mut meta, 0);
        string(&mut meta, "binary_format_major_version");
        meta.extend_from_slice(&[0xa1, 2]);
        string(&mut meta, "binary_format_minor_version");
        meta.push(0xa0);
        string(&mut meta, "build_epoch");
        meta.extend_from_slice(&[0x00, 0x02]); // uint64 0
        string(&mut meta, "database_type");
        string(&mut meta, "Test");
        string(&mut meta, "ip_version");
        meta.extend_from_slice(&[0xa1, 4]);
        string(&mut meta, "languages");
        meta.extend_from_slice(&[0x00, 0x04]); // empty array
        string(&mut meta, "node_count");
        meta.extend_from_slice(&[0xc1, 1]);
        string(&mut meta, "record_size");
        meta.extend_from_slice(&[0xa1, 24]);

        // One node: left record (bit 0) = data at offset 0 (node_count + 16 + 0), right = empty.
        let mut db = vec![0, 0, 17, 0, 0, 1];
        db.extend_from_slice(&[0; 16]);
        db.extend_from_slice(&data);
        db.extend_from_slice(b"\xab\xcd\xefMaxMind.com");
        db.extend_from_slice(&meta);
        db
    }

    #[test]
    fn looks_addresses_up_in_a_database_file() {
        let path = std::env::temp_dir().join(format!("superhet-geo-{}.mmdb", std::process::id()));
        std::fs::write(&path, tiny_mmdb()).expect("write");
        let db = GeoDb::open_all(&path.display().to_string()).expect("open");
        let _ = std::fs::remove_file(&path);
        let hit = db.lookup("8.8.8.8".parse().expect("ip"));
        assert_eq!(hit.country.as_deref(), Some("DE"));
        assert_eq!(hit.country_name.as_deref(), Some("Germany"));
        assert_eq!(hit.city.as_deref(), Some("Berlin"));
        assert_eq!(hit.asn, Some(3320));
        assert_eq!(hit.org.as_deref(), Some("Example Telecom"));
        let coarse = hit.coarse();
        assert_eq!((coarse.city, coarse.country.as_deref()), (None, Some("DE")));
        // second half of the address space has no data
        assert_eq!(
            db.lookup("200.1.1.1".parse().expect("ip")),
            GeoInfo::default()
        );
        // IPv6 into an IPv4-only database is an error inside, an empty answer outside
        assert_eq!(
            db.lookup("2001:db8::1".parse().expect("ip")),
            GeoInfo::default()
        );
    }

    #[tokio::test]
    async fn enricher_applies_geo_and_hides_the_city_for_truncated_addresses() {
        let path = std::env::temp_dir().join(format!("superhet-geo2-{}.mmdb", std::process::id()));
        std::fs::write(&path, tiny_mmdb()).expect("write");
        let db = GeoDb::open_all(&path.display().to_string()).expect("open");
        let _ = std::fs::remove_file(&path);
        let e = super::super::Enricher::from_parts(Some(db), None);
        let precise = super::super::Subject {
            ip: "8.8.8.8".parse().ok(),
            precise: true,
            ..Default::default()
        };
        let truncated = super::super::Subject {
            precise: false,
            ..precise.clone()
        };
        let out = e.describe(&[precise, truncated]).await;
        assert_eq!(out[0]["ip_info"]["city"], "Berlin");
        assert_eq!(out[0]["ip_info"]["asn"], 3320);
        assert!(out[1]["ip_info"].get("city").is_none());
        assert_eq!(out[1]["ip_info"]["country"], "DE");
    }

    #[test]
    fn missing_or_empty_configuration_is_reported() {
        assert!(GeoDb::open_all("").is_err());
        assert!(GeoDb::open_all("/nonexistent/geo.mmdb").is_err());
    }

    #[test]
    fn coarse_drops_the_city_only() {
        let info = GeoInfo {
            country: Some("DE".into()),
            country_name: Some("Germany".into()),
            city: Some("Berlin".into()),
            asn: Some(3320),
            org: Some("Example".into()),
        };
        let c = info.clone().coarse();
        assert_eq!(c.city, None);
        assert_eq!(c.country, info.country);
        assert_eq!(c.asn, info.asn);
    }
}
