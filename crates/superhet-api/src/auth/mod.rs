//! Local authentication: one admin account, server-side sessions in PostgreSQL.
//!
//! Design and policy: docs/adr/0008-local-auth.md, wire contract: docs/api-auth.md.
//! Everything except `/healthz`, `/readyz` and the `/api/v1/auth/*` endpoints sits behind
//! [`protect`]; there is no anonymous mode.

// Handlers use `Result<_, Response>` so `?` short-circuits straight into the HTTP answer; the
// error path is cold and the size is irrelevant there.
#![allow(clippy::result_large_err)]

mod limiter;
mod password;
mod routes;
mod token;

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::{
    Router,
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use superhet_db::{PgPool, auth::Lifetimes};
use tokio::sync::Semaphore;

use crate::origin::OriginPolicy;
pub use limiter::Limits;
pub use routes::router;

/// Environment variable: `auto` (default), `true` or `false`.
pub const ENV_COOKIE_SECURE: &str = "SUPERHET_COOKIE_SECURE";
/// Environment variable: trust `X-Forwarded-For` / `X-Forwarded-Proto` from a reverse proxy.
pub const ENV_TRUST_PROXY: &str = "SUPERHET_TRUST_PROXY";
/// Environment variable: fixed first-run setup token instead of a generated one.
pub const ENV_SETUP_TOKEN: &str = "SUPERHET_SETUP_TOKEN";

/// Shortest accepted `SUPERHET_SETUP_TOKEN`.
const SETUP_TOKEN_MIN: usize = 16;

/// Idle timeout (sliding), absolute lifetime and `last_seen_at` refresh interval.
const LIFETIMES: Lifetimes = Lifetimes {
    idle: Duration::from_secs(7 * 24 * 3600),
    absolute: Duration::from_secs(token::COOKIE_MAX_AGE_SECS),
    touch_interval: Duration::from_secs(60),
};

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
    /// Honour `X-Forwarded-*` headers (only behind a proxy that sets them).
    pub trust_proxy: bool,
    /// Operator-provided setup token; generated at startup when absent.
    pub setup_token: Option<String>,
    /// Failed-attempt limiter thresholds.
    pub limits: Limits,
}

impl AuthConfig {
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
        let dummy_hash = password::hash(DUMMY_PASSWORD)?;
        Ok(Self(Arc::new(Inner {
            limiter: limiter::Limiter::new(config.limits),
            db,
            origins,
            config,
            hash_slots: Semaphore::new(MAX_CONCURRENT_HASHES),
            dummy_hash,
            setup_token: Mutex::new(None),
            setup_done: AtomicBool::new(false),
        })))
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
            }
        });
    }

    fn db(&self) -> Result<&PgPool, Response> {
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
        }))
    }

    /// Hash on the blocking pool, at most [`MAX_CONCURRENT_HASHES`] at a time.
    async fn hash_password(&self, plain: String) -> Result<String, Response> {
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

/// Who is calling: address, user agent and whether the connection counts as https.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    ip: Option<IpAddr>,
    user_agent: Option<String>,
    secure: bool,
}

/// Longest stored user agent, in characters.
const USER_AGENT_MAX: usize = 256;

fn forwarded_ip(headers: &HeaderMap) -> Option<IpAddr> {
    // The rightmost entry is the one appended by our (single, trusted) proxy.
    headers
        .get("x-forwarded-for")?
        .to_str()
        .ok()?
        .rsplit(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("https"))
}

fn is_secure(config: &AuthConfig, headers: &HeaderMap, uri: &Uri) -> bool {
    match config.cookie_secure {
        CookieSecure::Always => true,
        CookieSecure::Never => false,
        CookieSecure::Auto => {
            uri.scheme_str() == Some("https") || (config.trust_proxy && forwarded_https(headers))
        }
    }
}

impl FromRequestParts<AuthState> for ClientInfo {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AuthState,
    ) -> Result<Self, Self::Rejection> {
        let config = &state.0.config;
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let ip = if config.trust_proxy {
            forwarded_ip(&parts.headers).or(peer)
        } else {
            peer
        };
        let user_agent = parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.chars().take(USER_AGENT_MAX).collect());
        Ok(Self {
            ip,
            user_agent,
            secure: is_secure(config, &parts.headers, &parts.uri),
        })
    }
}

/// JSON error body `{"error": code[, "message": ...]}`; never cached.
fn error(status: StatusCode, code: &str, message: Option<&str>) -> Response {
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

fn unavailable() -> Response {
    error(StatusCode::SERVICE_UNAVAILABLE, "unavailable", None)
}

fn internal() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", None)
}

/// Middleware: only requests with a valid session reach the inner router.
async fn require_session(
    State(auth): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    match auth.authenticate(request.headers()).await {
        Ok(Some(session)) => {
            request.extensions_mut().insert(session);
            next.run(request).await
        }
        Ok(None) => unauthenticated(),
        Err(err) => {
            tracing::warn!(%err, "session lookup failed");
            unavailable()
        }
    }
}

async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", None)
}

/// Put `routes` behind the session check. The check also covers unknown paths, so an
/// unauthenticated client cannot probe which routes exist.
pub fn protect(routes: Router, auth: AuthState) -> Router {
    routes
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(auth, require_session))
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
        assert!(!is_secure(&cfg, &https, &plain));
        cfg.trust_proxy = true;
        assert!(is_secure(&cfg, &https, &plain));
        assert!(!is_secure(
            &cfg,
            &headers(&[("x-forwarded-proto", "http")]),
            &plain
        ));
        cfg.cookie_secure = CookieSecure::Always;
        assert!(is_secure(&cfg, &HeaderMap::new(), &plain));
        cfg.cookie_secure = CookieSecure::Never;
        assert!(!is_secure(&cfg, &https, &plain));
        let abs: Uri = "https://sdr.example/x".parse().expect("uri");
        assert!(is_secure(&AuthConfig::default(), &HeaderMap::new(), &abs));
    }

    #[test]
    fn forwarded_for_uses_the_rightmost_entry() {
        let h = headers(&[("x-forwarded-for", "6.6.6.6, 203.0.113.9")]);
        assert_eq!(forwarded_ip(&h), "203.0.113.9".parse().ok());
        assert_eq!(forwarded_ip(&headers(&[("x-forwarded-for", "junk")])), None);
    }
}
