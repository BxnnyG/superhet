//! Local authentication: accounts with roles, server-side sessions in PostgreSQL, optional
//! anonymous guest sessions.
//!
//! Design and policy: docs/adr/0008-local-auth.md and docs/adr/0011-users-roles-and-guest-access.md,
//! wire contract: docs/api-auth.md. Everything except `/healthz`, `/readyz` and the
//! `/api/v1/auth/*` endpoints sits behind [`protect`], which resolves the caller to a
//! [`Principal`] and checks the route's declared access (`access::RULES`).

// Handlers use `Result<_, Response>` so `?` short-circuits straight into the HTTP answer; the
// error path is cold and the size is irrelevant there.
#![allow(clippy::result_large_err)]

mod accounts;
pub(crate) mod bans;
mod limiter;
pub(crate) mod password;
mod routes;
pub(crate) mod token;

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::{
    Router,
    extract::{ConnectInfo, FromRequestParts, MatchedPath, Request, State},
    http::{Extensions, HeaderMap, HeaderValue, StatusCode, Uri, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use superhet_bus::audit::Outcome;
use superhet_db::{PgPool, auth::Lifetimes};
use tokio::sync::Semaphore;

use crate::access::{self, Actor, Principal, Role};
use crate::audit::Audit;
use crate::clientinfo::Enricher;
use crate::clientinfo::addr::{self, ProxyTrust};
use crate::origin::OriginPolicy;
use crate::registry::Registry;
use crate::settings::SettingsStore;
pub use limiter::Limits;
pub use routes::router;
pub(crate) use routes::{Reply, db_failed, flatten, guard_origin, invalid, json_reply, parse_json};

/// Environment variable: `auto` (default), `true` or `false`.
pub const ENV_COOKIE_SECURE: &str = "SUPERHET_COOKIE_SECURE";
/// Environment variable: trust `X-Forwarded-For` / `X-Forwarded-Proto` from a reverse proxy.
pub const ENV_TRUST_PROXY: &str = "SUPERHET_TRUST_PROXY";
/// Environment variable: fixed first-run setup token instead of a generated one.
/// Environment variable: networks of trusted reverse proxies (comma or space separated).
pub const ENV_TRUSTED_PROXIES: &str = "SUPERHET_TRUSTED_PROXIES";
/// Environment variable: one-time setup token.
pub const ENV_SETUP_TOKEN: &str = "SUPERHET_SETUP_TOKEN";

/// Shortest accepted `SUPERHET_SETUP_TOKEN`.
const SETUP_TOKEN_MIN: usize = 16;

/// Idle timeout (sliding), absolute lifetime and `last_seen_at` refresh interval.
const LIFETIMES: Lifetimes = Lifetimes {
    idle: Duration::from_secs(7 * 24 * 3600),
    absolute: Duration::from_secs(token::COOKIE_MAX_AGE_SECS),
    touch_interval: Duration::from_secs(60),
};

/// Lifted and expired bans stay listed ("all") this long.
const BAN_HISTORY_DAYS: i32 = 30;

/// How often expired sessions are pruned in the background.
const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);

/// Argon2 runs (19 MiB each) at most this many at a time; excess requests wait.
const MAX_CONCURRENT_HASHES: usize = 4;

/// Password used to burn the same CPU time when the username is unknown.
const DUMMY_PASSWORD: &str = "superhet-dummy-password-for-unknown-users";

/// When the session cookie carries the `Secure` attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CookieSecure {
    /// Secure if the request arrived over https (directly, or via a trusted proxy).
    #[default]
    Auto,
    /// Always Secure.
    Always,
    /// Never Secure (plain-http LAN setups).
    Never,
}

impl CookieSecure {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "true" | "1" | "yes" => Ok(Self::Always),
            "false" | "0" | "no" => Ok(Self::Never),
            other => Err(format!(
                "{ENV_COOKIE_SECURE} must be auto, true or false, got {other:?}"
            )),
        }
    }
}

/// Runtime configuration of the auth layer.
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    /// Cookie `Secure` policy.
    pub cookie_secure: CookieSecure,
    /// Honour `X-Forwarded-*` headers from every peer (only behind a proxy that sets them and
    /// when the published port is reachable from that proxy alone).
    pub trust_proxy: bool,
    /// Networks of reverse proxies whose forwarding headers are believed
    /// (`SUPERHET_TRUSTED_PROXIES`); everybody else's are ignored.
    pub trusted_proxies: Vec<bans::Cidr>,
    /// Operator-provided setup token; generated at startup when absent.
    pub setup_token: Option<String>,
    /// Failed-attempt limiter thresholds.
    pub limits: Limits,
}

impl AuthConfig {
    /// Which peers may speak for a client.
    #[must_use]
    pub fn proxy_trust(&self) -> ProxyTrust {
        ProxyTrust {
            all: self.trust_proxy,
            nets: self.trusted_proxies.clone(),
        }
    }

    /// Read the `SUPERHET_COOKIE_SECURE`, `SUPERHET_TRUST_PROXY` and `SUPERHET_SETUP_TOKEN`
    /// variables.
    ///
    /// # Errors
    /// Fails on an unparsable value or a setup token that is too short to be safe.
    pub fn from_env() -> Result<Self, String> {
        let cookie_secure =
            CookieSecure::parse(&std::env::var(ENV_COOKIE_SECURE).unwrap_or_default())?;
        let trust_proxy = matches!(
            std::env::var(ENV_TRUST_PROXY)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "true" | "1" | "yes"
        );
        let trusted_proxies =
            ProxyTrust::parse_list(&std::env::var(ENV_TRUSTED_PROXIES).unwrap_or_default())
                .map_err(|err| format!("{ENV_TRUSTED_PROXIES}: {err}"))?;
        let setup_token = match std::env::var(ENV_SETUP_TOKEN) {
            Ok(t) if !t.trim().is_empty() => {
                let t = t.trim().to_owned();
                if t.chars().count() < SETUP_TOKEN_MIN {
                    return Err(format!(
                        "{ENV_SETUP_TOKEN} must be at least {SETUP_TOKEN_MIN} characters"
                    ));
                }
                Some(t)
            }
            _ => None,
        };
        Ok(Self {
            cookie_secure,
            trust_proxy,
            trusted_proxies,
            setup_token,
            limits: Limits::default(),
        })
    }
}

struct Inner {
    db: Option<PgPool>,
    origins: OriginPolicy,
    config: AuthConfig,
    limiter: limiter::Limiter,
    hash_slots: Semaphore,
    dummy_hash: String,
    setup_token: Mutex<Option<String>>,
    setup_done: AtomicBool,
    /// Failed logins per user name, across all addresses.
    name_limiter: limiter::NameLimiter,
    settings: Arc<SettingsStore>,
    audit: Audit,
    registry: Registry,
    bans: Arc<bans::Bans>,
    enricher: Enricher,
}

/// Shared state of the auth layer (cheap to clone).
#[derive(Clone)]
pub struct AuthState(Arc<Inner>);

/// An authenticated browser session.
#[derive(Debug, Clone)]
pub struct Session {
    /// Owning user.
    pub user_id: i64,
    /// Login name of the owner.
    pub username: String,
    /// SHA-256 of the cookie token (the database key).
    pub token_hash: [u8; 32],
    /// Role of the owner.
    pub role: Role,
    /// The owner still has to change the password.
    pub must_change_password: bool,
}

impl AuthState {
    /// Build the auth state. Without a database nothing can be authenticated: every protected
    /// route answers 401 and the auth endpoints answer 503.
    ///
    /// # Errors
    /// Fails if Argon2 cannot be initialised.
    pub fn new(
        db: Option<PgPool>,
        origins: OriginPolicy,
        config: AuthConfig,
    ) -> Result<Self, String> {
        Self::with_enricher(db, origins, config, Enricher::from_env())
    }

    /// Like [`AuthState::new`] with explicit enrichment sources.
    ///
    /// # Errors
    /// Fails if Argon2 cannot be initialised.
    pub fn with_enricher(
        db: Option<PgPool>,
        origins: OriginPolicy,
        config: AuthConfig,
        enricher: Enricher,
    ) -> Result<Self, String> {
        let dummy_hash = password::hash(DUMMY_PASSWORD)?;
        let settings = Arc::new(SettingsStore::new(db.clone()));
        let audit = Audit::new(db.clone(), Arc::clone(&settings));
        let registry = Registry::new(audit.clone());
        Ok(Self(Arc::new(Inner {
            limiter: limiter::Limiter::new(config.limits),
            name_limiter: limiter::NameLimiter::new(config.limits.window, config.limits.per_name),
            db,
            origins,
            config,
            hash_slots: Semaphore::new(MAX_CONCURRENT_HASHES),
            dummy_hash,
            setup_token: Mutex::new(None),
            setup_done: AtomicBool::new(false),
            settings,
            audit,
            registry,
            bans: Arc::new(bans::Bans::default()),
            enricher,
        })))
    }

    /// An auth layer without database for unit tests of single routes.
    #[cfg(test)]
    #[must_use]
    pub fn for_tests() -> Self {
        Self::with_enricher(
            None,
            OriginPolicy::default(),
            AuthConfig::default(),
            Enricher::disabled(),
        )
        .expect("auth state")
    }

    /// Address, user agent, reverse DNS and GeoIP enrichment for the admin views.
    #[must_use]
    pub fn enricher(&self) -> &Enricher {
        &self.0.enricher
    }

    /// The audit writer.
    #[must_use]
    pub fn audit(&self) -> &Audit {
        &self.0.audit
    }

    /// The admin-editable settings.
    #[must_use]
    pub fn settings(&self) -> &SettingsStore {
        &self.0.settings
    }

    /// Guest sessions and live connections.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.0.registry
    }

    /// First-run housekeeping: if no admin exists yet, arm the setup token and log it.
    ///
    /// # Errors
    /// Fails if the database cannot be queried or no random bytes are available.
    pub async fn prepare_setup(&self) -> Result<(), Box<dyn std::error::Error>> {
        let Some(db) = &self.0.db else {
            tracing::warn!("no database configured: authentication is unavailable");
            return Ok(());
        };
        if superhet_db::auth::has_user(db).await? {
            self.0.setup_done.store(true, Ordering::Relaxed);
            return Ok(());
        }
        let (token, generated) = match &self.0.config.setup_token {
            Some(t) => (t.clone(), false),
            None => (token::generate_setup_token()?, true),
        };
        if generated {
            tracing::warn!(
                "first run: no admin account exists yet. Open the web UI and create it with this \
                 one-time setup token: {token}  (read it again with: docker compose logs \
                 superhet-api; it changes on every restart until the account exists, or pin it \
                 with {ENV_SETUP_TOKEN})"
            );
        } else {
            tracing::warn!(
                "first run: no admin account exists yet. Open the web UI and create it with the \
                 setup token from {ENV_SETUP_TOKEN}"
            );
        }
        *self
            .0
            .setup_token
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(token);
        Ok(())
    }

    /// Periodically delete expired sessions in the background.
    pub fn spawn_maintenance(&self) {
        let Some(db) = self.0.db.clone() else {
            return;
        };
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(PRUNE_INTERVAL);
            loop {
                ticker.tick().await;
                match superhet_db::auth::prune_sessions(&db, LIFETIMES).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(removed = n, "pruned expired sessions"),
                    Err(err) => tracing::warn!(%err, "session pruning failed"),
                }
                if let Err(err) = superhet_db::bans::prune(&db, BAN_HISTORY_DAYS).await {
                    tracing::warn!(%err, "ban history pruning failed");
                }
            }
        });
    }

    pub(crate) fn db(&self) -> Result<&PgPool, Response> {
        self.0.db.as_ref().ok_or_else(unavailable)
    }

    /// Resolve the session cookie of a request.
    async fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<Session>, superhet_db::Error> {
        let (Some(db), Some(raw)) = (&self.0.db, token::token_from_headers(headers)) else {
            return Ok(None);
        };
        let token_hash = token::hash_token(&raw);
        let Some(active) =
            superhet_db::auth::find_active_session(db, &token_hash, LIFETIMES).await?
        else {
            return Ok(None);
        };
        if active.needs_touch
            && let Err(err) = superhet_db::auth::touch_session(db, &token_hash).await
        {
            tracing::warn!(%err, "could not refresh session activity");
        }
        Ok(Some(Session {
            user_id: active.user_id,
            username: active.username,
            token_hash,
            role: active.role,
            must_change_password: active.must_change_password,
        }))
    }

    /// Resolve the caller of a request: an account session first, then a guest session (only
    /// while guest mode is on). `None` means unauthenticated.
    async fn resolve_principal(
        &self,
        headers: &HeaderMap,
        client: &ClientInfo,
    ) -> Result<Option<Principal>, superhet_db::Error> {
        if let Some(session) = self.authenticate(headers).await? {
            return Ok(Some(Principal {
                actor: Actor::User(session.user_id),
                name: session.username,
                role: session.role,
                session_key: token::hex(&session.token_hash),
                ip: client.ip,
                peer: client.peer,
                via: client.chain.clone(),
                user_agent: client.user_agent.clone(),
                must_change_password: session.must_change_password,
            }));
        }
        let Some(raw) = token::guest_token_from_headers(headers) else {
            return Ok(None);
        };
        let policy = self.0.settings.guest_policy().await;
        if !policy.enabled {
            return Ok(None);
        }
        let idle = Duration::from_secs(policy.idle_timeout_secs);
        let Some(guest) = self.0.registry.guest_touch(&token::hash_token(&raw), idle) else {
            return Ok(None);
        };
        Ok(Some(Principal {
            session_key: crate::registry::guest_key(&guest.id),
            actor: Actor::Guest(guest.id),
            name: "guest".to_owned(),
            role: Role::Listener,
            ip: client.ip,
            peer: client.peer,
            via: client.chain.clone(),
            user_agent: client.user_agent.clone(),
            must_change_password: false,
        }))
    }

    /// Hash on the blocking pool, at most [`MAX_CONCURRENT_HASHES`] at a time.
    pub(crate) async fn hash_password(&self, plain: String) -> Result<String, Response> {
        let _slot = self.0.hash_slots.acquire().await.map_err(|_| internal())?;
        tokio::task::spawn_blocking(move || password::hash(&plain))
            .await
            .map_err(|_| internal())?
            .map_err(|err| {
                tracing::error!(%err, "password hashing failed");
                internal()
            })
    }

    /// Verify on the blocking pool; `None` verifies against a dummy hash so that unknown
    /// users cost the same time as wrong passwords.
    async fn verify_password(&self, plain: String, phc: Option<String>) -> bool {
        let real = phc.is_some();
        let phc = phc.unwrap_or_else(|| self.0.dummy_hash.clone());
        let Ok(_slot) = self.0.hash_slots.acquire().await else {
            return false;
        };
        let ok = tokio::task::spawn_blocking(move || password::verify(&plain, &phc))
            .await
            .unwrap_or(false);
        ok && real
    }
}

/// Who is calling: addresses, user agent and whether the connection counts as https.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    /// The visitor's address: the forwarded one behind a trusted proxy, else the peer.
    ip: Option<IpAddr>,
    /// The TCP peer (a proxy, or the Docker bridge gateway when no real address gets through).
    peer: Option<IpAddr>,
    /// Forwarded chain reported by a trusted proxy.
    chain: Vec<IpAddr>,
    user_agent: Option<String>,
    secure: bool,
}

/// Longest stored user agent, in characters.
const USER_AGENT_MAX: usize = 256;

fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("https"))
}

fn is_secure(config: &AuthConfig, peer: Option<IpAddr>, headers: &HeaderMap, uri: &Uri) -> bool {
    match config.cookie_secure {
        CookieSecure::Always => true,
        CookieSecure::Never => false,
        CookieSecure::Auto => {
            uri.scheme_str() == Some("https")
                || (config.proxy_trust().trusts(peer) && forwarded_https(headers))
        }
    }
}

impl ClientInfo {
    fn from_request(
        config: &AuthConfig,
        extensions: &Extensions,
        headers: &HeaderMap,
        uri: &Uri,
    ) -> Self {
        let peer = extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let resolved = addr::resolve(peer, headers, &config.proxy_trust());
        let user_agent = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.chars().take(USER_AGENT_MAX).collect());
        Self {
            ip: resolved.client,
            peer: resolved.peer,
            chain: resolved.chain,
            user_agent,
            secure: is_secure(config, peer, headers, uri),
        }
    }
}

impl FromRequestParts<AuthState> for ClientInfo {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AuthState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self::from_request(
            &state.0.config,
            &parts.extensions,
            &parts.headers,
            &parts.uri,
        ))
    }
}

/// JSON error body `{"error": code[, "message": ...]}`; never cached.
pub(crate) fn error(status: StatusCode, code: &str, message: Option<&str>) -> Response {
    let body = match message {
        Some(m) => serde_json::json!({"error": code, "message": m}),
        None => serde_json::json!({"error": code}),
    };
    (
        status,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body.to_string(),
    )
        .into_response()
}

fn unauthenticated() -> Response {
    error(StatusCode::UNAUTHORIZED, "unauthenticated", None)
}

pub(crate) fn unavailable() -> Response {
    error(StatusCode::SERVICE_UNAVAILABLE, "unavailable", None)
}

pub(crate) fn internal() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", None)
}

fn forbidden() -> Response {
    error(StatusCode::FORBIDDEN, "forbidden", None)
}

/// Middleware: resolve the caller, look the route up in the access table and let only
/// permitted requests through (the [`Principal`] travels as a request extension).
async fn enforce(State(auth): State<AuthState>, mut request: Request, next: Next) -> Response {
    let client = ClientInfo::from_request(
        &auth.0.config,
        request.extensions(),
        request.headers(),
        request.uri(),
    );
    let principal = match auth.resolve_principal(request.headers(), &client).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return unauthenticated(),
        Err(err) => {
            tracing::warn!(%err, "session lookup failed");
            return unavailable();
        }
    };
    // Unknown paths have no matched route: let the fallback answer 404 (the caller is signed in).
    let Some(pattern) = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
    else {
        return next.run(request).await;
    };
    let Some(rule) = access::find_rule(request.method(), &pattern) else {
        tracing::error!(method = %request.method(), path = pattern, "route has no access rule: refused");
        return forbidden();
    };
    if principal.must_change_password {
        return error(StatusCode::FORBIDDEN, "password_change_required", None);
    }
    let guests_see_decoder_events = principal.is_guest()
        && rule.access == access::Access::GuestDecoder
        && auth.0.settings.guest_policy().await.show_decoder_events;
    if !rule.access.allows(&principal, guests_see_decoder_events) {
        auth.0.audit.log_coalesced(
            &principal.who(),
            "access.denied",
            Some(&format!("{} {}", rule.method, rule.path)),
            Outcome::Denied,
            serde_json::json!({}),
        );
        return forbidden();
    }
    request.extensions_mut().insert(principal);
    next.run(request).await
}

async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", None)
}

/// Layer that refuses `/api/` requests from banned addresses; apply it outermost so it runs
/// before every route, public or protected.
pub fn ban_gate(router: Router, auth: AuthState) -> Router {
    router.layer(middleware::from_fn_with_state(auth, bans::gate))
}

/// Put `routes` behind the session check. The check also covers unknown paths, so an
/// unauthenticated client cannot probe which routes exist.
pub fn protect(routes: Router, auth: AuthState) -> Router {
    routes
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(auth, enforce))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_secure_modes_parse() {
        assert_eq!(CookieSecure::parse("").expect("ok"), CookieSecure::Auto);
        assert_eq!(CookieSecure::parse("AUTO").expect("ok"), CookieSecure::Auto);
        assert_eq!(
            CookieSecure::parse("true").expect("ok"),
            CookieSecure::Always
        );
        assert_eq!(
            CookieSecure::parse("false").expect("ok"),
            CookieSecure::Never
        );
        assert!(CookieSecure::parse("sometimes").is_err());
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn secure_flag_follows_policy() {
        let plain: Uri = "/x".parse().expect("uri");
        let https = headers(&[("x-forwarded-proto", "https")]);
        let mut cfg = AuthConfig::default();
        // auto without a trusted proxy ignores a spoofable header
        assert!(!is_secure(&cfg, None, &https, &plain));
        cfg.trust_proxy = true;
        assert!(is_secure(&cfg, None, &https, &plain));
        assert!(!is_secure(
            &cfg,
            None,
            &headers(&[("x-forwarded-proto", "http")]),
            &plain
        ));
        cfg.cookie_secure = CookieSecure::Always;
        assert!(is_secure(&cfg, None, &HeaderMap::new(), &plain));
        cfg.cookie_secure = CookieSecure::Never;
        assert!(!is_secure(&cfg, None, &https, &plain));
        let abs: Uri = "https://sdr.example/x".parse().expect("uri");
        assert!(is_secure(
            &AuthConfig::default(),
            None,
            &HeaderMap::new(),
            &abs
        ));
    }

    #[test]
    fn client_info_resolves_peer_and_client() {
        let mut ext = Extensions::new();
        ext.insert(ConnectInfo(SocketAddr::from(([172, 22, 0, 1], 5555))));
        let h = headers(&[("x-forwarded-for", "6.6.6.6, 8.8.4.4")]);
        let uri: Uri = "/x".parse().expect("uri");
        // no proxy trusted: the spoofable header is ignored, the peer is the client
        let plain = ClientInfo::from_request(&AuthConfig::default(), &ext, &h, &uri);
        assert_eq!(plain.ip, "172.22.0.1".parse().ok());
        assert_eq!(plain.peer, "172.22.0.1".parse().ok());
        assert!(plain.chain.is_empty());
        // gateway listed as trusted proxy network
        let cfg = AuthConfig {
            trusted_proxies: ProxyTrust::parse_list("172.16.0.0/12").expect("nets"),
            ..AuthConfig::default()
        };
        let via = ClientInfo::from_request(&cfg, &ext, &h, &uri);
        assert_eq!(via.ip, "8.8.4.4".parse().ok());
        assert_eq!(via.peer, "172.22.0.1".parse().ok());
        assert_eq!(via.chain.len(), 2);
        // a proxy network that does not contain the peer changes nothing
        let other = AuthConfig {
            trusted_proxies: ProxyTrust::parse_list("10.0.0.0/8").expect("nets"),
            ..AuthConfig::default()
        };
        assert_eq!(
            ClientInfo::from_request(&other, &ext, &h, &uri).ip,
            "172.22.0.1".parse().ok()
        );
    }
}
