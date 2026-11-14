//! The share link model: what a link allows, how an admin's input is validated, and the
//! lookup store (database behind a short cache). Everything a token may do is decided here,
//! in pure functions, so the rules can be tested without a server (ADR-0029).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use superhet_db::{PgPool, share_links};

use crate::registry::now_ms;

/// How long a read link table is reused. Writes through this process refresh at once.
const CACHE_TTL: Duration = Duration::from_secs(3);
/// Most text a link carries (the database enforces the same bounds).
const LABEL_MAX: usize = 80;
const TITLE_MAX: usize = 120;
const DESCRIPTION_MAX: usize = 1000;
const LOCATION_MAX: usize = 200;
/// Most origins one link may allow to frame its embed page.
pub const MAX_EMBED_ORIGINS: usize = 20;
/// Default and largest `max_listeners`.
pub const DEFAULT_MAX_LISTENERS: u32 = 5;
const MAX_LISTENERS_LIMIT: u32 = 1000;
/// Highest frequency the daemon accepts (1 THz).
const MAX_FREQ_HZ: u64 = 1_000_000_000_000;
/// Demodulation modes a VFO knows.
pub const MODES: [&str; 6] = ["wfm", "nfm", "am", "usb", "lsb", "cw"];

/// What a token may open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// The spectrum and waterfall stream.
    Waterfall,
    /// Demodulated audio (the followed VFO, or the listener's own on a fixed link).
    Audio,
    /// The live decoder event feed.
    DecoderEvents,
    /// Audio of plugin instances (a DAB service, for example).
    PluginAudio,
}

impl Feature {
    /// The spelling used in the API and the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Waterfall => "waterfall",
            Self::Audio => "audio",
            Self::DecoderEvents => "decoder_events",
            Self::PluginAudio => "plugin_audio",
        }
    }

    /// Parse the API spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        [
            Self::Waterfall,
            Self::Audio,
            Self::DecoderEvents,
            Self::PluginAudio,
        ]
        .into_iter()
        .find(|f| f.as_str() == text)
    }
}

/// What listeners of a link are tuned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tuning {
    /// They hear what the operator is tuned to.
    Follow,
    /// Every audio listener gets a VFO of their own.
    Fixed {
        /// Start frequency.
        freq_hz: u64,
        /// Demodulation mode.
        mode: String,
        /// When set, the listener may retune inside this range (inclusive).
        range: Option<(u64, u64)>,
    },
}

/// A share link, token excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Row id.
    pub id: i64,
    /// Last characters of the token.
    pub token_hint: String,
    /// Admin-only name.
    pub label: String,
    /// Station name for the page.
    pub title: String,
    /// Free text for the page.
    pub description: String,
    /// Free location text for the page.
    pub location: String,
    /// The one device the link serves.
    pub device: String,
    /// What listeners are tuned to.
    pub tuning: Tuning,
    /// Allowed features.
    pub features: BTreeSet<Feature>,
    /// Concurrent connections per kind.
    pub max_listeners: u32,
    /// Origins that may frame the embed page (empty = nobody).
    pub embed_origins: Vec<String>,
    /// Expiry, Unix milliseconds.
    pub expires_at_ms: Option<i64>,
    /// Revocation, Unix milliseconds.
    pub revoked_at_ms: Option<i64>,
    /// Creation, Unix milliseconds.
    pub created_at_ms: i64,
    /// Admin who made it.
    pub created_by: Option<String>,
}

/// Whether a link can be used right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Usable.
    Active,
    /// Past its expiry.
    Expired,
    /// Revoked by an admin.
    Revoked,
}

impl Status {
    /// The spelling used in the API.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }
}

/// Why a retune request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuneRefusal {
    /// The link has no tuning range (or follows the operator).
    NotAllowed,
    /// The frequency lies outside the range.
    OutOfRange,
}

impl Link {
    /// Usable now? Revocation wins over expiry.
    #[must_use]
    pub fn status(&self, now_ms: i64) -> Status {
        if self.revoked_at_ms.is_some() {
            Status::Revoked
        } else if self.expires_at_ms.is_some_and(|at| at <= now_ms) {
            Status::Expired
        } else {
            Status::Active
        }
    }

    /// Whether the link allows `feature`.
    #[must_use]
    pub fn allows(&self, feature: Feature) -> bool {
        self.features.contains(&feature)
    }

    /// Whether the link serves audio of any kind.
    #[must_use]
    pub fn allows_any_audio(&self) -> bool {
        self.allows(Feature::Audio) || self.allows(Feature::PluginAudio)
    }

    /// The tuning range of a fixed link that allows retuning.
    #[must_use]
    pub fn tune_range(&self) -> Option<(u64, u64)> {
        match &self.tuning {
            Tuning::Fixed { range, .. } => *range,
            Tuning::Follow => None,
        }
    }

    /// May a listener retune to `freq_hz`?
    ///
    /// # Errors
    /// [`TuneRefusal`] naming why not. Retuning is only ever possible on a fixed link with a
    /// range, and only while the link serves audio.
    pub fn check_tune(&self, freq_hz: u64) -> Result<(), TuneRefusal> {
        match self.tune_range() {
            Some((lo, hi)) if self.allows(Feature::Audio) => {
                if (lo..=hi).contains(&freq_hz) {
                    Ok(())
                } else {
                    Err(TuneRefusal::OutOfRange)
                }
            }
            _ => Err(TuneRefusal::NotAllowed),
        }
    }

    /// The primary kind of connection: what counts as a listener.
    #[must_use]
    pub fn primary_kind(&self) -> Kind {
        if self.allows_any_audio() {
            Kind::Audio
        } else if self.allows(Feature::Waterfall) {
            Kind::Spectrum
        } else {
            Kind::Events
        }
    }

    fn from_record(r: &share_links::Record) -> Self {
        let freq = r.freq_hz.and_then(|f| u64::try_from(f).ok());
        let tuning = match (r.follow, freq, &r.mode) {
            (false, Some(freq_hz), Some(mode)) => Tuning::Fixed {
                freq_hz,
                mode: mode.clone(),
                range: r
                    .tune_min_hz
                    .zip(r.tune_max_hz)
                    .and_then(|(lo, hi)| Some((u64::try_from(lo).ok()?, u64::try_from(hi).ok()?))),
            },
            _ => Tuning::Follow,
        };
        Self {
            id: r.id,
            token_hint: r.token_hint.clone(),
            label: r.label.clone(),
            title: r.title.clone(),
            description: r.description.clone(),
            location: r.location.clone(),
            device: r.device.clone(),
            tuning,
            // An unknown feature name (never written by this code) grants nothing.
            features: r
                .features
                .iter()
                .filter_map(|f| Feature::parse(f))
                .collect(),
            max_listeners: u32::try_from(r.max_listeners).unwrap_or(1).max(1),
            embed_origins: r.embed_origins.clone(),
            expires_at_ms: r.expires_at_ms,
            revoked_at_ms: r.revoked_at_ms,
            created_at_ms: r.created_at_ms,
            created_by: r.created_by.clone(),
        }
    }

    /// The row for this link with the given token digest.
    #[must_use]
    pub fn to_record(&self, token_hash: [u8; 32]) -> share_links::Record {
        let (follow, freq_hz, mode, range) = match &self.tuning {
            Tuning::Follow => (true, None, None, None),
            Tuning::Fixed {
                freq_hz,
                mode,
                range,
            } => (false, Some(*freq_hz), Some(mode.clone()), *range),
        };
        let signed = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        share_links::Record {
            id: self.id,
            token_hash,
            token_hint: self.token_hint.clone(),
            label: self.label.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            location: self.location.clone(),
            device: self.device.clone(),
            follow,
            freq_hz: freq_hz.map(signed),
            mode,
            tune_min_hz: range.map(|(lo, _)| signed(lo)),
            tune_max_hz: range.map(|(_, hi)| signed(hi)),
            features: self
                .features
                .iter()
                .map(|f| f.as_str().to_owned())
                .collect(),
            max_listeners: i32::try_from(self.max_listeners).unwrap_or(i32::MAX),
            embed_origins: self.embed_origins.clone(),
            expires_at_ms: self.expires_at_ms,
            revoked_at_ms: self.revoked_at_ms,
            created_at_ms: self.created_at_ms,
            created_by: self.created_by.clone(),
        }
    }
}

/// The three kinds of connection a token may open (each is capped on its own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `ws/spectrum`.
    Spectrum,
    /// `ws/audio`.
    Audio,
    /// `ws/events`.
    Events,
}

/// An admin's description of a link, as sent to the API.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkInput {
    /// Admin-only name.
    pub label: String,
    /// Station name for the page.
    #[serde(default)]
    pub title: String,
    /// Free text for the page.
    #[serde(default)]
    pub description: String,
    /// Free location text for the page.
    #[serde(default)]
    pub location: String,
    /// Device id.
    pub device: String,
    /// `follow` (default) or `fixed`.
    #[serde(default)]
    pub tuning: TuningKind,
    /// Fixed: start frequency.
    #[serde(default)]
    pub freq_hz: Option<u64>,
    /// Fixed: mode.
    #[serde(default)]
    pub mode: Option<String>,
    /// Fixed: lowest frequency a listener may retune to.
    #[serde(default)]
    pub tune_min_hz: Option<u64>,
    /// Fixed: highest frequency a listener may retune to.
    #[serde(default)]
    pub tune_max_hz: Option<u64>,
    /// Allowed features (API spelling).
    #[serde(default)]
    pub features: Vec<String>,
    /// Concurrent connections per kind (default 5).
    #[serde(default)]
    pub max_listeners: Option<u32>,
    /// Origins that may frame the embed page.
    #[serde(default)]
    pub embed_origins: Vec<String>,
    /// Expiry, Unix milliseconds (`null` = never).
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
}

/// `tuning` of a [`LinkInput`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TuningKind {
    /// Listeners hear the operator.
    #[default]
    Follow,
    /// Listeners get a VFO at a fixed frequency.
    Fixed,
}

fn text(name: &str, value: &str, max: usize, required: bool) -> Result<String, String> {
    let value = value.trim();
    if required && value.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if value.chars().count() > max {
        return Err(format!("{name}: at most {max} characters"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{name} must not contain control characters"));
    }
    Ok(value.to_owned())
}

/// An origin for `frame-ancestors`: `http(s)://host[:port]`, optionally `https://*.host`. Anything
/// with a path, a query, whitespace or a quote is refused (the value ends up in a header).
///
/// # Errors
/// A message naming the problem.
pub fn normalise_origin(raw: &str) -> Result<String, String> {
    let origin = raw.trim().trim_end_matches('/').to_ascii_lowercase();
    let bad = || format!("embed origin {raw:?} must look like https://example.org");
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .ok_or_else(bad)?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    if port.is_some_and(|p| p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit())) {
        return Err(bad());
    }
    let name = host.strip_prefix("*.").unwrap_or(host);
    let label_ok = |l: &str| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if name.is_empty() || !name.split('.').all(label_ok) {
        return Err(bad());
    }
    Ok(origin)
}

impl LinkInput {
    /// Check the input and build the link (id, hint and creation data are the caller's).
    ///
    /// # Errors
    /// A message naming the first problem.
    pub fn validate(&self, now_ms: i64) -> Result<Link, String> {
        let label = text("label", &self.label, LABEL_MAX, true)?;
        let title = text("title", &self.title, TITLE_MAX, false)?;
        let description = text("description", &self.description, DESCRIPTION_MAX, false)?;
        let location = text("location", &self.location, LOCATION_MAX, false)?;
        let device = self.device.trim().to_owned();
        if device.len() > 64 || superhet_bus::subjects::state_key(&device).is_err() {
            return Err("invalid device id".into());
        }
        let mut features = BTreeSet::new();
        for name in &self.features {
            features.insert(Feature::parse(name).ok_or_else(|| {
                format!("unknown feature {name:?} (waterfall, audio, decoder_events, plugin_audio)")
            })?);
        }
        if features.is_empty() {
            return Err("allow at least one feature".into());
        }
        let max_listeners = self.max_listeners.unwrap_or(DEFAULT_MAX_LISTENERS);
        if !(1..=MAX_LISTENERS_LIMIT).contains(&max_listeners) {
            return Err(format!("max_listeners: 1 to {MAX_LISTENERS_LIMIT}"));
        }
        if self.embed_origins.len() > MAX_EMBED_ORIGINS {
            return Err(format!("embed_origins: at most {MAX_EMBED_ORIGINS}"));
        }
        let mut embed_origins = Vec::new();
        for raw in &self.embed_origins {
            let origin = normalise_origin(raw)?;
            if !embed_origins.contains(&origin) {
                embed_origins.push(origin);
            }
        }
        if self.expires_at_ms.is_some_and(|at| at <= now_ms) {
            return Err("expires_at_ms lies in the past".into());
        }
        let tuning = match self.tuning {
            TuningKind::Follow => {
                if self.freq_hz.is_some()
                    || self.mode.is_some()
                    || self.tune_min_hz.is_some()
                    || self.tune_max_hz.is_some()
                {
                    return Err(
                        "a link that follows the operator has no frequency, mode or tuning range"
                            .into(),
                    );
                }
                Tuning::Follow
            }
            TuningKind::Fixed => self.fixed()?,
        };
        Ok(Link {
            id: 0,
            token_hint: String::new(),
            label,
            title,
            description,
            location,
            device,
            tuning,
            features,
            max_listeners,
            embed_origins,
            expires_at_ms: self.expires_at_ms,
            revoked_at_ms: None,
            created_at_ms: now_ms,
            created_by: None,
        })
    }

    fn fixed(&self) -> Result<Tuning, String> {
        let freq_hz = self.freq_hz.ok_or("a fixed link needs freq_hz")?;
        let mode = self.mode.clone().ok_or("a fixed link needs a mode")?;
        if !MODES.contains(&mode.as_str()) {
            return Err(format!("mode: one of {}", MODES.join(", ")));
        }
        let hz_ok = |hz: u64| (1..=MAX_FREQ_HZ).contains(&hz);
        if !hz_ok(freq_hz) {
            return Err("freq_hz is out of range".into());
        }
        let range = match (self.tune_min_hz, self.tune_max_hz) {
            (None, None) => None,
            (Some(lo), Some(hi)) => {
                if !hz_ok(lo) || !hz_ok(hi) || lo > hi {
                    return Err("tuning range: 1 Hz <= min <= max <= 1 THz".into());
                }
                if !(lo..=hi).contains(&freq_hz) {
                    return Err("freq_hz must lie inside the tuning range".into());
                }
                Some((lo, hi))
            }
            _ => return Err("give both tune_min_hz and tune_max_hz, or neither".into()),
        };
        Ok(Tuning::Fixed {
            freq_hz,
            mode,
            range,
        })
    }
}

/// The link table could not be read; the request is refused.
#[derive(Debug)]
pub struct Unavailable;

type ByHash = Arc<HashMap<[u8; 32], Arc<Link>>>;

struct Cached {
    at: Instant,
    by_hash: ByHash,
}

/// The stored links behind a short-lived cache.
pub struct ShareLinks {
    db: Option<PgPool>,
    cache: Mutex<Option<Cached>>,
    /// Tests: a fixed table that is never refreshed.
    pinned: bool,
}

impl ShareLinks {
    /// Create the store; without a database there are no links.
    #[must_use]
    pub fn new(db: Option<PgPool>) -> Self {
        Self {
            db,
            cache: Mutex::new(None),
            pinned: false,
        }
    }

    /// A store holding exactly these `(token, link)` pairs (unit tests).
    #[cfg(test)]
    #[must_use]
    pub fn for_tests(links: Vec<(&str, Link)>) -> Self {
        let table: HashMap<[u8; 32], Arc<Link>> = links
            .into_iter()
            .map(|(token, link)| (crate::auth::token::hash_token(token), Arc::new(link)))
            .collect();
        Self {
            db: None,
            cache: Mutex::new(Some(Cached {
                at: Instant::now(),
                by_hash: Arc::new(table),
            })),
            pinned: true,
        }
    }

    /// The database, for the admin routes.
    #[must_use]
    pub fn db(&self) -> Option<&PgPool> {
        self.db.as_ref()
    }

    fn cached(&self) -> Option<ByHash> {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .as_ref()
            .filter(|c| self.pinned || c.at.elapsed() < CACHE_TTL)
            .map(|c| Arc::clone(&c.by_hash))
    }

    /// Forget the cached table (after a write).
    pub fn invalidate(&self) {
        if !self.pinned {
            *self.cache.lock().unwrap_or_else(PoisonError::into_inner) = None;
        }
    }

    async fn table(&self) -> Result<ByHash, Unavailable> {
        if let Some(table) = self.cached() {
            return Ok(table);
        }
        let Some(db) = &self.db else {
            return Ok(Arc::default());
        };
        let rows = share_links::list(db).await.map_err(|err| {
            tracing::warn!(%err, "share links unreadable: request refused");
            Unavailable
        })?;
        let table: ByHash = Arc::new(
            rows.iter()
                .map(|r| (r.token_hash, Arc::new(Link::from_record(r))))
                .collect(),
        );
        *self.cache.lock().unwrap_or_else(PoisonError::into_inner) = Some(Cached {
            at: Instant::now(),
            by_hash: Arc::clone(&table),
        });
        Ok(table)
    }

    /// The link a token belongs to, whatever its status.
    ///
    /// # Errors
    /// [`Unavailable`] if the table cannot be read.
    pub async fn find(&self, token_hash: &[u8; 32]) -> Result<Option<Arc<Link>>, Unavailable> {
        Ok(self.table().await?.get(token_hash).cloned())
    }

    /// Every link, newest first (cached; the admin list reads the database directly).
    ///
    /// # Errors
    /// [`Unavailable`] if the table cannot be read.
    pub async fn all(&self) -> Result<Vec<Arc<Link>>, Unavailable> {
        let mut links: Vec<Arc<Link>> = self.table().await?.values().cloned().collect();
        links.sort_by_key(|l| std::cmp::Reverse(l.id));
        Ok(links)
    }

    /// The link with this id (fresh from the database when the cache is stale).
    ///
    /// # Errors
    /// [`Unavailable`] if the table cannot be read.
    pub async fn by_id(&self, id: i64) -> Result<Option<Arc<Link>>, Unavailable> {
        Ok(self.table().await?.values().find(|l| l.id == id).cloned())
    }

    /// The origins allowed to frame the embed page of the link behind a token; empty unless the
    /// link exists and is usable (used by the static layer for the CSP of `/embed/<token>`).
    pub async fn embed_origins(&self, token_hash: &[u8; 32]) -> Vec<String> {
        match self.find(token_hash).await {
            Ok(Some(link)) if link.status(now_ms()) == Status::Active => link.embed_origins.clone(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
pub(crate) fn test_link(id: i64) -> Link {
    Link {
        id,
        token_hint: "abcd".into(),
        label: "test".into(),
        title: "Test station".into(),
        description: String::new(),
        location: String::new(),
        device: "dev0".into(),
        tuning: Tuning::Follow,
        features: BTreeSet::from([Feature::Waterfall, Feature::Audio]),
        max_listeners: 2,
        embed_origins: Vec::new(),
        expires_at_ms: None,
        revoked_at_ms: None,
        created_at_ms: 0,
        created_by: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> LinkInput {
        serde_json::from_value(serde_json::json!({
            "label": "Front page", "device": "dev0", "features": ["waterfall", "audio"]
        }))
        .expect("input")
    }

    #[test]
    fn a_minimal_input_follows_the_operator_with_five_listeners() {
        let link = input().validate(1000).expect("valid");
        assert_eq!(link.tuning, Tuning::Follow);
        assert_eq!(link.max_listeners, DEFAULT_MAX_LISTENERS);
        assert!(link.embed_origins.is_empty());
        assert_eq!(link.status(1000), Status::Active);
    }

    #[test]
    fn invalid_inputs_are_refused() {
        let mut bad = input();
        bad.features.clear();
        assert!(bad.validate(0).is_err());
        let mut bad = input();
        bad.features = vec!["admin".into()];
        assert!(bad.validate(0).is_err());
        let mut bad = input();
        bad.device = "dev 0; drop".into();
        assert!(bad.validate(0).is_err());
        let mut bad = input();
        bad.max_listeners = Some(0);
        assert!(bad.validate(0).is_err());
        let mut bad = input();
        bad.expires_at_ms = Some(10);
        assert!(bad.validate(10).is_err());
        let mut bad = input();
        bad.freq_hz = Some(100_000_000);
        assert!(bad.validate(0).is_err(), "follow links take no frequency");
        let mut fixed = input();
        fixed.tuning = TuningKind::Fixed;
        assert!(fixed.validate(0).is_err(), "fixed needs freq and mode");
        fixed.freq_hz = Some(100_000_000);
        fixed.mode = Some("fm".into());
        assert!(fixed.validate(0).is_err(), "unknown mode");
        fixed.mode = Some("wfm".into());
        fixed.tune_min_hz = Some(99_000_000);
        assert!(fixed.validate(0).is_err(), "half a range");
        fixed.tune_max_hz = Some(99_500_000);
        assert!(fixed.validate(0).is_err(), "start outside the range");
        fixed.tune_max_hz = Some(101_000_000);
        assert!(fixed.validate(0).is_ok());
    }

    #[test]
    fn embed_origins_are_normalised_and_strict() {
        assert_eq!(
            normalise_origin(" HTTPS://Blog.Example.org/ ").as_deref(),
            Ok("https://blog.example.org")
        );
        assert_eq!(
            normalise_origin("https://*.example.org:8443").as_deref(),
            Ok("https://*.example.org:8443")
        );
        for bad in [
            "",
            "*",
            "example.org",
            "ftp://example.org",
            "https://example.org/path",
            "https://example.org; script-src *",
            "https://exa mple.org",
            "https://example.org:80a",
            "https://-x.org",
            "https://",
        ] {
            assert!(normalise_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn status_follows_revocation_and_expiry() {
        let mut link = test_link(1);
        link.expires_at_ms = Some(100);
        assert_eq!(link.status(99), Status::Active);
        assert_eq!(link.status(100), Status::Expired);
        link.revoked_at_ms = Some(5);
        assert_eq!(link.status(1), Status::Revoked);
    }

    #[test]
    fn retuning_needs_a_fixed_link_with_a_range_and_audio() {
        let mut link = test_link(1);
        assert_eq!(link.check_tune(100), Err(TuneRefusal::NotAllowed));
        link.tuning = Tuning::Fixed {
            freq_hz: 150,
            mode: "am".into(),
            range: None,
        };
        assert_eq!(link.check_tune(150), Err(TuneRefusal::NotAllowed));
        link.tuning = Tuning::Fixed {
            freq_hz: 150,
            mode: "am".into(),
            range: Some((100, 200)),
        };
        assert_eq!(link.check_tune(100), Ok(()));
        assert_eq!(link.check_tune(200), Ok(()));
        assert_eq!(link.check_tune(99), Err(TuneRefusal::OutOfRange));
        assert_eq!(link.check_tune(201), Err(TuneRefusal::OutOfRange));
        link.features.remove(&Feature::Audio);
        assert_eq!(link.check_tune(150), Err(TuneRefusal::NotAllowed));
    }

    #[test]
    fn records_round_trip() {
        let mut link = test_link(7);
        link.tuning = Tuning::Fixed {
            freq_hz: 433_920_000,
            mode: "nfm".into(),
            range: Some((433_000_000, 434_000_000)),
        };
        let record = link.to_record([9; 32]);
        assert_eq!(Link::from_record(&record), link);
    }

    #[test]
    fn the_primary_kind_is_what_counts_as_a_listener() {
        let mut link = test_link(1);
        assert_eq!(link.primary_kind(), Kind::Audio);
        link.features = BTreeSet::from([Feature::Waterfall]);
        assert_eq!(link.primary_kind(), Kind::Spectrum);
        link.features = BTreeSet::from([Feature::DecoderEvents]);
        assert_eq!(link.primary_kind(), Kind::Events);
        link.features = BTreeSet::from([Feature::PluginAudio]);
        assert_eq!(link.primary_kind(), Kind::Audio);
    }
}
