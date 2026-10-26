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
}

/// The three settings documents with a read cache.
pub struct SettingsStore {
    db: Option<PgPool>,
    cache: Mutex<Cache>,
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
