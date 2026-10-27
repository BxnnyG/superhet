//! Admin-editable settings stored in `superhet.settings`: the guest policy (ADR-0011), audit
//! retention and IP handling (ADR-0012) and the per-role VFO limits. Read through a short-lived
//! cache so the access check on every request does not hit the database.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use superhet_db::PgPool;

/// Settings key of the guest policy.
pub const KEY_GUEST: &str = "guest_policy";
/// Settings key of the audit settings.
pub const KEY_AUDIT: &str = "audit";
/// Settings key of the VFO limits.
pub const KEY_LIMITS: &str = "limits";
/// Settings key of the client report policy.
pub const KEY_CLIENT_REPORT: &str = "client_report";
/// Lookup service offered for "ask clients for their public IP" (the service sees the address).
pub const DEFAULT_PUBLIC_IP_SERVICE: &str = "https://api.ipify.org?format=json";
/// Longest accepted service URL, in characters.
pub const SERVICE_URL_MAX: usize = 200;

/// How long a read value is reused. Writes through this process refresh at once.
const CACHE_TTL: Duration = Duration::from_secs(5);

/// Demodulation modes a policy may allow.
pub const MODES: [&str; 6] = ["wfm", "nfm", "am", "usb", "lsb", "cw"];
/// Longest privacy text, in characters.
pub const PRIVACY_TEXT_MAX: usize = 4000;
/// Largest retention accepted, in days (100 years).
pub const RETENTION_MAX_DAYS: u32 = 36_500;

/// How client addresses are stored in the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IpMode {
    /// The address as seen.
    Full,
    /// IPv4 /24, IPv6 /48.
    Truncated,
    /// Not stored.
    None,
}

/// Retention and address handling of the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuditSettings {
    /// Days to keep events of accounts and the system (`0` = forever).
    pub retention_user_days: u32,
    /// Days to keep guest events (`0` = forever, discouraged).
    pub retention_guest_days: u32,
    /// Address mode for events of accounts.
    pub ip_mode_user: IpMode,
    /// Address mode for guest events.
    pub ip_mode_guest: IpMode,
}

impl Default for AuditSettings {
    fn default() -> Self {
        Self {
            retention_user_days: 90,
            retention_guest_days: 7,
            ip_mode_user: IpMode::Full,
            ip_mode_guest: IpMode::Truncated,
        }
    }
}

impl AuditSettings {
    /// Check the value ranges.
    ///
    /// # Errors
    /// A message naming the first invalid field.
    pub fn validate(&self) -> Result<(), String> {
        if self.retention_user_days > RETENTION_MAX_DAYS
            || self.retention_guest_days > RETENTION_MAX_DAYS
        {
            return Err(format!("retention must be 0..={RETENTION_MAX_DAYS} days"));
        }
        Ok(())
    }
}

/// Whose browsers are asked to report device details at session start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectScope {
    /// Nobody (the default).
    Off,
    /// Signed-in accounts only.
    Users,
    /// Accounts and guests.
    All,
}

/// Client-reported device info and public address (`docs/client-addresses.md`, "Client-reported
/// information"). Everything is off until the admin turns it on; the data is unverified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ClientReportPolicy {
    /// Whose browsers report device details (User-Agent Client Hints, screen, languages ...).
    pub collect: CollectScope,
    /// Also make the browser look up its public address at `public_ip_service` and report it.
    /// Only for the browsers `collect` covers.
    pub ask_public_ip: bool,
    /// HTTPS URL that answers `{"ip": "..."}` or a bare address in plain text.
    pub public_ip_service: String,
}

impl Default for ClientReportPolicy {
    fn default() -> Self {
        Self {
            collect: CollectScope::Off,
            ask_public_ip: false,
            public_ip_service: DEFAULT_PUBLIC_IP_SERVICE.to_owned(),
        }
    }
}

impl ClientReportPolicy {
    /// Check the service URL: `https://`, a host, no credentials, no fragment.
    ///
    /// # Errors
    /// A message naming the problem.
    pub fn validate(&self) -> Result<(), String> {
        self.service_origin().map(|_| ())
    }

    /// `https://host[:port]` of the lookup service: what the CSP `connect-src` must allow.
    ///
    /// # Errors
    /// A message naming the problem with the URL.
    pub fn service_origin(&self) -> Result<String, String> {
        let url = self.public_ip_service.trim();
        if url.is_empty() || url.chars().count() > SERVICE_URL_MAX {
            return Err(format!(
                "public_ip_service must be 1..={SERVICE_URL_MAX} characters"
            ));
        }
        if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("public_ip_service must not contain whitespace".into());
        }
        let uri: axum::http::Uri = url
            .parse()
            .map_err(|_| "public_ip_service is not a valid URL".to_owned())?;
        if uri.scheme_str() != Some("https") {
            return Err("public_ip_service must be an https:// URL".into());
        }
        let authority = uri
            .authority()
            .ok_or_else(|| "public_ip_service needs a host".to_owned())?;
        if authority.as_str().contains('@') {
            return Err("public_ip_service must not contain credentials".into());
        }
        if uri.host().is_none_or(str::is_empty) {
            return Err("public_ip_service needs a host".into());
        }
        Ok(format!("https://{authority}"))
    }

    /// Whether the browser of a guest (`guest`) or an account is asked for device details.
    #[must_use]
    pub fn collects(&self, guest: bool) -> bool {
        match self.collect {
            CollectScope::Off => false,
            CollectScope::Users => !guest,
            CollectScope::All => true,
        }
    }

    /// Whether that browser is also asked for its public address.
    #[must_use]
    pub fn asks_public_ip(&self, guest: bool) -> bool {
        self.ask_public_ip && self.collects(guest)
    }
}

/// What anonymous guests may do (ADR-0011). Everything is off until the admin enables it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GuestPolicy {
    /// Guest mode on/off. Off by default.
    pub enabled: bool,
    /// Most concurrent guest sessions.
    pub max_guests: u32,
    /// Most concurrent guest sessions per client address.
    pub max_guests_per_ip: u32,
    /// Most VFOs one guest may own.
    pub max_vfos_per_guest: u32,
    /// Modes guests may use.
    pub allowed_modes: Vec<String>,
    /// Lowest frequency guests may tune to, Hz (`None` = no limit).
    pub freq_min_hz: Option<u64>,
    /// Highest frequency guests may tune to, Hz (`None` = no limit).
    pub freq_max_hz: Option<u64>,
    /// A guest without any request for this long is dropped, seconds.
    pub idle_timeout_secs: u64,
    /// Refuse guests while any tuning lock is held.
    pub only_without_lock: bool,
    /// Whether guests may see decoder events (personal data, off by default).
    pub show_decoder_events: bool,
    /// Privacy notice shown on the guest page.
    pub privacy_text: String,
}

impl Default for GuestPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_guests: 5,
            max_guests_per_ip: 2,
            max_vfos_per_guest: 1,
            allowed_modes: ["wfm", "nfm", "am"].map(str::to_owned).to_vec(),
            freq_min_hz: None,
            freq_max_hz: None,
            idle_timeout_secs: 600,
            only_without_lock: false,
            show_decoder_events: false,
            privacy_text: "Guest sessions are logged for abuse handling: your IP address \
                (shortened) and browser type are stored for 7 days."
                .to_owned(),
        }
    }
}

impl GuestPolicy {
    /// Check the value ranges.
    ///
    /// # Errors
    /// A message naming the first invalid field.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_guests > 1000 {
            return Err("max_guests must be 0..=1000".into());
        }
        if !(1..=100).contains(&self.max_guests_per_ip) {
            return Err("max_guests_per_ip must be 1..=100".into());
        }
        if !(1..=16).contains(&self.max_vfos_per_guest) {
            return Err("max_vfos_per_guest must be 1..=16".into());
        }
        if self
            .allowed_modes
            .iter()
            .any(|m| !MODES.contains(&m.as_str()))
        {
            return Err(format!("allowed_modes must be a subset of {MODES:?}"));
        }
        if let (Some(lo), Some(hi)) = (self.freq_min_hz, self.freq_max_hz)
            && lo > hi
        {
            return Err("freq_min_hz must not exceed freq_max_hz".into());
        }
        if !(30..=86_400).contains(&self.idle_timeout_secs) {
            return Err("idle_timeout_secs must be 30..=86400".into());
        }
        if self.privacy_text.chars().count() > PRIVACY_TEXT_MAX {
            return Err(format!(
                "privacy_text is limited to {PRIVACY_TEXT_MAX} characters"
            ));
        }
        Ok(())
    }

    /// Whether a guest may use `mode`.
    #[must_use]
    pub fn allows_mode(&self, mode: &str) -> bool {
        self.allowed_modes.iter().any(|m| m == mode)
    }

    /// Whether a guest may tune to `freq_hz`.
    #[must_use]
    pub fn allows_freq(&self, freq_hz: u64) -> bool {
        self.freq_min_hz.is_none_or(|lo| freq_hz >= lo)
            && self.freq_max_hz.is_none_or(|hi| freq_hz <= hi)
    }
}

/// VFO limits of accounts (guests have theirs in the policy). `0` = only the daemon's cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    /// Most VFOs one listener may own.
    pub listener_max_vfos: u32,
    /// Most VFOs one operator may own.
    pub operator_max_vfos: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            listener_max_vfos: 4,
            operator_max_vfos: 0,
        }
    }
}

impl Limits {
    /// Check the value ranges.
    ///
    /// # Errors
    /// A message naming the first invalid field.
    pub fn validate(&self) -> Result<(), String> {
        if self.listener_max_vfos > 64 || self.operator_max_vfos > 64 {
            return Err("VFO limits must be 0..=64".into());
        }
        Ok(())
    }
}

/// Why a settings write failed.
#[derive(Debug)]
pub enum SettingsError {
    /// No database is configured.
    Unavailable,
    /// The query failed.
    Db(superhet_db::Error),
}

struct Cached<T> {
    value: T,
    at: Instant,
}

#[derive(Default)]
struct Cache {
    guest: Option<Cached<GuestPolicy>>,
    audit: Option<Cached<AuditSettings>>,
    limits: Option<Cached<Limits>>,
    client_report: Option<Cached<ClientReportPolicy>>,
}

/// The three settings documents with a read cache.
pub struct SettingsStore {
    db: Option<PgPool>,
    cache: Mutex<Cache>,
}

/// The origin the page may connect to for the public-address lookup, if anybody is asked.
fn lookup_origin(policy: &ClientReportPolicy) -> Option<String> {
    if policy.collect == CollectScope::Off || !policy.ask_public_ip {
        return None;
    }
    policy.service_origin().ok()
}

fn fresh<T: Clone>(slot: &Option<Cached<T>>) -> Option<T> {
    slot.as_ref()
        .filter(|c| c.at.elapsed() < CACHE_TTL)
        .map(|c| c.value.clone())
}

impl SettingsStore {
    /// A store over `db` (without one, every document is its default and writes fail).
    #[must_use]
    pub fn new(db: Option<PgPool>) -> Self {
        Self {
            db,
            cache: Mutex::new(Cache::default()),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn load<T: Default + serde::de::DeserializeOwned>(&self, key: &str) -> T {
        let Some(db) = &self.db else {
            return T::default();
        };
        match superhet_db::settings::get(db, key).await {
            Ok(Some(value)) => serde_json::from_value(value).unwrap_or_else(|err| {
                tracing::error!(%err, key, "stored settings are unreadable, using defaults");
                T::default()
            }),
            Ok(None) => T::default(),
            Err(err) => {
                tracing::warn!(%err, key, "could not read settings, using defaults");
                T::default()
            }
        }
    }

    /// The guest policy (guest mode off when nothing is stored or the database is down).
    pub async fn guest_policy(&self) -> GuestPolicy {
        if let Some(v) = fresh(&self.cache().guest) {
            return v;
        }
        let value: GuestPolicy = self.load(KEY_GUEST).await;
        self.cache().guest = Some(Cached {
            value: value.clone(),
            at: Instant::now(),
        });
        value
    }

    /// The audit settings.
    pub async fn audit(&self) -> AuditSettings {
        if let Some(v) = fresh(&self.cache().audit) {
            return v;
        }
        let value: AuditSettings = self.load(KEY_AUDIT).await;
        self.cache().audit = Some(Cached {
            value: value.clone(),
            at: Instant::now(),
        });
        value
    }

    /// The VFO limits.
    pub async fn limits(&self) -> Limits {
        if let Some(v) = fresh(&self.cache().limits) {
            return v;
        }
        let value: Limits = self.load(KEY_LIMITS).await;
        self.cache().limits = Some(Cached {
            value: value.clone(),
            at: Instant::now(),
        });
        value
    }

    /// The client report policy (everything off when nothing is stored). Reading it also keeps
    /// the page's content security policy in step with the configured lookup service.
    pub async fn client_report(&self) -> ClientReportPolicy {
        if let Some(v) = fresh(&self.cache().client_report) {
            return v;
        }
        let value: ClientReportPolicy = self.load(KEY_CLIENT_REPORT).await;
        self.cache().client_report = Some(Cached {
            value: value.clone(),
            at: Instant::now(),
        });
        crate::web::set_lookup_origin(lookup_origin(&value));
        value
    }

    /// Replace the client report policy.
    ///
    /// # Errors
    /// Fails if the database is unavailable.
    pub async fn put_client_report(
        &self,
        value: &ClientReportPolicy,
        by: &str,
    ) -> Result<(), SettingsError> {
        self.store(
            KEY_CLIENT_REPORT,
            serde_json::to_value(value).unwrap_or_default(),
            by,
        )
        .await?;
        self.cache().client_report = Some(Cached {
            value: value.clone(),
            at: Instant::now(),
        });
        crate::web::set_lookup_origin(lookup_origin(value));
        Ok(())
    }

    async fn store(
        &self,
        key: &str,
        value: serde_json::Value,
        by: &str,
    ) -> Result<(), SettingsError> {
        let Some(db) = &self.db else {
            return Err(SettingsError::Unavailable);
        };
        superhet_db::settings::put(db, key, &value, by)
            .await
            .map_err(SettingsError::Db)
    }

    /// Replace the guest policy.
    ///
    /// # Errors
    /// Fails if the database is unavailable.
    pub async fn put_guest_policy(
        &self,
        value: &GuestPolicy,
        by: &str,
    ) -> Result<(), SettingsError> {
        self.store(
            KEY_GUEST,
            serde_json::to_value(value).unwrap_or_default(),
            by,
        )
        .await?;
        self.cache().guest = Some(Cached {
            value: value.clone(),
            at: Instant::now(),
        });
        Ok(())
    }

    /// Replace the audit settings.
    ///
    /// # Errors
    /// Fails if the database is unavailable.
    pub async fn put_audit(&self, value: &AuditSettings, by: &str) -> Result<(), SettingsError> {
        self.store(
            KEY_AUDIT,
            serde_json::to_value(value).unwrap_or_default(),
            by,
        )
        .await?;
        self.cache().audit = Some(Cached {
            value: value.clone(),
            at: Instant::now(),
        });
        Ok(())
    }

    /// Replace the VFO limits.
    ///
    /// # Errors
    /// Fails if the database is unavailable.
    pub async fn put_limits(&self, value: &Limits, by: &str) -> Result<(), SettingsError> {
        self.store(
            KEY_LIMITS,
            serde_json::to_value(value).unwrap_or_default(),
            by,
        )
        .await?;
        self.cache().limits = Some(Cached {
            value: value.clone(),
            at: Instant::now(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_conservative() {
        let g = GuestPolicy::default();
        assert!(!g.enabled && !g.show_decoder_events && !g.only_without_lock);
        assert_eq!(g.allowed_modes, ["wfm", "nfm", "am"]);
        assert_eq!(g.validate(), Ok(()));
        let a = AuditSettings::default();
        assert_eq!((a.retention_user_days, a.retention_guest_days), (90, 7));
        assert_eq!(
            (a.ip_mode_user, a.ip_mode_guest),
            (IpMode::Full, IpMode::Truncated)
        );
        assert_eq!(Limits::default().validate(), Ok(()));
    }

    #[test]
    fn policy_validation_and_checks() {
        let mut g = GuestPolicy::default();
        g.allowed_modes.push("tetra".into());
        assert!(g.validate().is_err());
        g = GuestPolicy {
            freq_min_hz: Some(88_000_000),
            freq_max_hz: Some(108_000_000),
            ..GuestPolicy::default()
        };
        assert!(
            g.allows_freq(100_000_000) && !g.allows_freq(87_999_999) && !g.allows_freq(108_000_001)
        );
        assert!(g.allows_mode("wfm") && !g.allows_mode("usb"));
        g.freq_min_hz = Some(109_000_000);
        assert!(g.validate().is_err());
        for bad in [
            GuestPolicy {
                idle_timeout_secs: 5,
                ..GuestPolicy::default()
            },
            GuestPolicy {
                max_vfos_per_guest: 0,
                ..GuestPolicy::default()
            },
            GuestPolicy {
                max_guests: 5000,
                ..GuestPolicy::default()
            },
            GuestPolicy {
                privacy_text: "x".repeat(PRIVACY_TEXT_MAX + 1),
                ..GuestPolicy::default()
            },
        ] {
            assert!(bad.validate().is_err());
        }
        assert!(
            AuditSettings {
                retention_user_days: 40_000,
                ..AuditSettings::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn client_reports_are_off_by_default_and_follow_the_scope() {
        let p = ClientReportPolicy::default();
        assert!(!p.collects(false) && !p.collects(true) && !p.asks_public_ip(false));
        assert_eq!(p.public_ip_service, DEFAULT_PUBLIC_IP_SERVICE);
        assert_eq!(p.validate(), Ok(()));
        let users = ClientReportPolicy {
            collect: CollectScope::Users,
            ask_public_ip: true,
            ..p.clone()
        };
        assert!(users.collects(false) && !users.collects(true));
        assert!(users.asks_public_ip(false) && !users.asks_public_ip(true));
        let all = ClientReportPolicy {
            collect: CollectScope::All,
            ..users.clone()
        };
        assert!(all.collects(true) && all.asks_public_ip(true));
        // Asking for the address without collecting at all asks nobody.
        let no_collect = ClientReportPolicy {
            ask_public_ip: true,
            ..p
        };
        assert!(!no_collect.asks_public_ip(false));
    }

    #[test]
    fn the_lookup_service_must_be_a_plain_https_url() {
        let with = |url: &str| ClientReportPolicy {
            public_ip_service: url.to_owned(),
            ..ClientReportPolicy::default()
        };
        assert_eq!(
            with("https://api.ipify.org?format=json").service_origin(),
            Ok("https://api.ipify.org".to_owned())
        );
        assert_eq!(
            with("https://ip.example.net:8443/ip").service_origin(),
            Ok("https://ip.example.net:8443".to_owned())
        );
        for bad in [
            "",
            "http://api.ipify.org",
            "ftp://x.example",
            "api.ipify.org",
            "https://user:pw@x.example/",
            "https://x.example/a b",
            "https:///path",
            "https://x.example;script-src *",
            "javascript:alert(1)",
        ] {
            assert!(with(bad).validate().is_err(), "{bad}");
        }
        let long = format!("https://x.example/{}", "a".repeat(200));
        assert!(with(&long).validate().is_err());
    }

    #[test]
    fn lookup_origin_only_while_somebody_is_asked() {
        let mut p = ClientReportPolicy::default();
        assert_eq!(lookup_origin(&p), None);
        p.ask_public_ip = true;
        assert_eq!(lookup_origin(&p), None, "collect is off");
        p.collect = CollectScope::Users;
        assert_eq!(lookup_origin(&p).as_deref(), Some("https://api.ipify.org"));
    }

    #[test]
    fn documents_reject_unknown_fields_and_fill_defaults() {
        assert!(serde_json::from_str::<GuestPolicy>(r#"{"enabled":true,"nope":1}"#).is_err());
        let g: GuestPolicy = serde_json::from_str(r#"{"enabled":true}"#).expect("partial");
        assert!(g.enabled);
        assert_eq!(g.max_guests, 5);
        let a: AuditSettings =
            serde_json::from_str(r#"{"ip_mode_guest":"none"}"#).expect("partial");
        assert_eq!(a.ip_mode_guest, IpMode::None);
    }
}
