//! Optional OpenID Connect login next to the local accounts (docs/adr/0019-oidc-login.md,
//! docs/api-oidc.md): Authorization Code flow with PKCE, discovery through the issuer URL, a
//! group-to-role mapping, optional provisioning and linking. Sessions after a successful OIDC
//! login are the same server-side sessions as for password logins.

pub(crate) mod client;
pub(crate) mod config;
pub(crate) mod flow;
pub(crate) mod link;
pub(crate) mod logout;
mod routes;
mod routes_logout;

use std::time::Duration;

use superhet_db::oidc as db;

pub(crate) use config::{OidcEnv, OidcSettings, Source};
pub use routes::router;
pub(crate) use routes_logout::provider_logout_url;

use super::AuthState;
use super::limiter::{Limiter, Limits};

/// Back-channel logout requests that failed validation, per client address, before further
/// requests are refused for a while. Separate from the login limiter: a misconfigured provider
/// must not be able to lock people out of password sign-in.
const BACKCHANNEL_LIMITS: Limits = Limits {
    window: Duration::from_secs(5 * 60),
    per_ip: 20,
    global: 200,
    per_name: 0,
    max_tracked_ips: 1000,
};

/// Per-process OIDC state inside the auth layer.
pub struct OidcState {
    pub(crate) env: OidcEnv,
    pub(crate) pending: flow::PendingStore,
    pub(crate) providers: client::ProviderCache,
    pub(crate) replay: logout::ReplayStore,
    pub(crate) backchannel_limiter: Limiter,
}

impl Default for OidcState {
    fn default() -> Self {
        Self {
            env: OidcEnv::default(),
            pending: flow::PendingStore::default(),
            providers: client::ProviderCache::default(),
            replay: logout::ReplayStore::default(),
            backchannel_limiter: Limiter::new(BACKCHANNEL_LIMITS),
        }
    }
}

impl OidcState {
    /// State for the given environment.
    #[must_use]
    pub fn new(env: OidcEnv) -> Self {
        Self {
            env,
            ..Self::default()
        }
    }
}

impl AuthState {
    pub(crate) fn oidc(&self) -> &OidcState {
        &self.0.oidc
    }

    /// The effective OIDC settings and where they come from: the environment wins over the
    /// stored document.
    pub(crate) async fn oidc_settings(&self) -> (OidcSettings, Source) {
        match &self.oidc().env.settings {
            Some(settings) => (settings.clone(), Source::Env),
            None => (self.settings().oidc().await, Source::Database),
        }
    }

    /// The settings if OIDC login is on and usable.
    pub(crate) async fn oidc_active(&self) -> Option<OidcSettings> {
        self.0.db.as_ref()?;
        let (settings, _) = self.oidc_settings().await;
        let usable = settings.enabled && settings.validate(self.oidc().env.allow_http).is_ok();
        usable.then_some(settings)
    }

    /// Whether password logins are accepted right now. They are, unless the admin switched them
    /// off, OIDC is usable, and an enabled admin can still sign in through OIDC (so the switch
    /// can never lock everybody out). `SUPERHET_OIDC_KEEP_LOCAL_LOGIN` overrides it.
    pub(crate) async fn local_login_allowed(&self) -> bool {
        if self.oidc().env.keep_local_login {
            return true;
        }
        let Some(settings) = self.oidc_active().await else {
            return true;
        };
        if settings.local_login {
            return true;
        }
        let Some(db) = self.0.db.as_ref() else {
            return true;
        };
        // Fail open: if the check itself fails, do not lock anybody out.
        !db::has_active_oidc_admin(db).await.unwrap_or(false)
    }

    /// Public description for the login page (`/api/v1/auth/status`).
    pub(crate) async fn oidc_public(&self) -> serde_json::Value {
        match self.oidc_active().await {
            Some(settings) => serde_json::json!({
                "enabled": true,
                "name": settings.name,
                "login_url": config::LOGIN_PATH,
            }),
            None => serde_json::json!({"enabled": false, "name": null, "login_url": null}),
        }
    }
}
