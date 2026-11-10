//! The two logout halves that need the HTTP layer (docs/adr/0024-oidc-logout.md):
//! `POST /api/v1/auth/oidc/backchannel-logout`, which the provider calls to end Superhet
//! sessions, and the helper that turns a just-ended OIDC session into the provider's
//! `end_session_endpoint` URL for the browser.

use std::time::{Duration, Instant};

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use openidconnect::url::form_urlencoded;
use serde_json::json;
use superhet_bus::audit::Outcome;
use superhet_db::oidc as db;

use super::client::Provider;
use super::config::{self, OidcSettings};
use super::logout::Rejection;
use crate::auth::routes::account_who;
use crate::auth::{AuthState, ClientInfo, token};

/// A logout token is a few hundred bytes; this leaves room for large claim sets.
const MAX_BODY_BYTES: usize = 16 * 1024;
/// Discovery for the logout redirect may take this long before sign-out gives up on it.
const DISCOVERY_WAIT: Duration = Duration::from_secs(4);
/// Audit action of a back-channel logout.
const ACTION: &str = "auth.oidc_backchannel_logout";

pub fn router() -> Router<AuthState> {
    Router::new().route(
        config::BACKCHANNEL_PATH,
        post(backchannel).layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
    )
}

/// The URL that ends the provider's session for a session that just ended locally, or `None`
/// when the session did not come from OIDC, the admin switched provider logout off, OIDC is
/// off or the issuer changed since, or the provider has no `end_session_endpoint`.
pub(crate) async fn provider_logout_url(
    auth: &AuthState,
    ended: &db::EndedSession,
) -> Option<String> {
    let issuer = ended.issuer.as_deref()?;
    let settings = auth.oidc_active().await.filter(|s| s.rp_logout)?;
    let provider = tokio::time::timeout(DISCOVERY_WAIT, provider_for(auth, &settings, false))
        .await
        .ok()?
        .map_err(|err| tracing::warn!(%err, "provider logout skipped: discovery failed"))
        .ok()?;
    if provider.issuer() != issuer {
        return None;
    }
    let state = token::generate_token().ok()?;
    provider.end_session_url(
        ended.id_token.as_deref(),
        &settings.post_logout_url().ok()?,
        &state,
    )
}

async fn provider_for(
    auth: &AuthState,
    settings: &OidcSettings,
    refresh: bool,
) -> Result<std::sync::Arc<Provider>, super::client::OidcError> {
    let oidc = auth.oidc();
    oidc.providers
        .get(
            settings,
            oidc.env.client_secret.as_ref(),
            oidc.env.allow_http,
            refresh,
        )
        .await
}

/// How a back-channel request failed.
struct Failure {
    status: StatusCode,
    code: &'static str,
}

impl Failure {
    fn invalid(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
        }
    }

    fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "unavailable",
        }
    }
}

fn answer(status: StatusCode, body: Option<serde_json::Value>) -> Response {
    let mut response = match body {
        Some(body) => (status, body.to_string()).into_response(),
        None => status.into_response(),
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if status != StatusCode::OK {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    response
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn logout_token_of(headers: &HeaderMap, body: &Bytes) -> Option<String> {
    let form = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';').next().is_some_and(|m| {
                m.trim()
                    .eq_ignore_ascii_case("application/x-www-form-urlencoded")
            })
        });
    if !form {
        return None;
    }
    form_urlencoded::parse(body)
        .find(|(k, _)| k == "logout_token")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

async fn backchannel(
    State(auth): State<AuthState>,
    client: ClientInfo,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match handle(&auth, &client, &headers, &body).await {
        Ok(()) => answer(StatusCode::OK, None),
        Err(fail) => answer(fail.status, Some(json!({"error": fail.code}))),
    }
}

async fn handle(
    auth: &AuthState,
    client: &ClientInfo,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(), Failure> {
    let settings = auth
        .oidc_active()
        .await
        .filter(|s| s.backchannel_logout)
        .ok_or_else(|| Failure::invalid("not_enabled"))?;
    let limiter = &auth.oidc().backchannel_limiter;
    limiter
        .check(client.ip, Instant::now())
        .map_err(|_| Failure {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
        })?;
    let Some(jwt) = logout_token_of(headers, body) else {
        limiter.record_failure(client.ip, Instant::now());
        deny(auth, client, "missing_token", json!({}));
        return Err(Failure::invalid("invalid_request"));
    };
    let provider = provider_for(auth, &settings, false).await.map_err(|err| {
        tracing::warn!(%err, "back-channel logout: provider unavailable");
        Failure::unavailable()
    })?;
    let mut checked = provider.check_logout_token(&jwt, now_secs());
    if checked.as_ref().is_err_and(|r| r.may_be_key_rotation()) {
        // The provider may have rotated its keys since the cached discovery.
        if let Ok(fresh) = provider_for(auth, &settings, true).await {
            checked = fresh.check_logout_token(&jwt, now_secs());
        }
    }
    let claims = match checked {
        Ok(c) => c,
        Err(reason) => {
            limiter.record_failure(client.ip, Instant::now());
            deny(auth, client, reason.code(), json!({}));
            return Err(Failure::invalid(reason.code()));
        }
    };
    let issuer = provider.issuer();
    let details = json!({"issuer": issuer, "sid": claims.sid, "sub": claims.subject});
    if auth
        .oidc()
        .replay
        .claim(issuer, &claims.jti, now_secs())
        .is_err()
    {
        deny(auth, client, Rejection::Replay.code(), details);
        return Err(Failure::invalid(Rejection::Replay.code()));
    }
    let pool = auth.db().map_err(|_| Failure::unavailable())?;
    let ended = db::end_sessions_for_logout(
        pool,
        issuer,
        claims.sid.as_deref(),
        claims.subject.as_deref(),
    )
    .await
    .map_err(|err| {
        // The provider may retry; do not let the retry look like a replay.
        auth.oidc().replay.release(issuer, &claims.jti);
        tracing::warn!(%err, "back-channel logout: could not end the sessions");
        Failure::unavailable()
    })?;
    // Open WebSockets of the ended sessions close with them.
    for id in &ended {
        auth.registry().kick(&token::hex(id));
    }
    auth.audit()
        .log(
            &account_who(client, None, "oidc", None),
            ACTION,
            None,
            Outcome::Ok,
            json!({
                "issuer": issuer,
                "sid": claims.sid,
                "sub": claims.subject,
                "sessions": ended.len(),
            }),
        )
        .await;
    Ok(())
}

/// Audit a refused back-channel logout (unauthenticated callers can make many: coalesced).
fn deny(auth: &AuthState, client: &ClientInfo, reason: &str, extra: serde_json::Value) {
    let mut details = json!({"reason": reason});
    if let (Some(d), Some(e)) = (details.as_object_mut(), extra.as_object()) {
        d.extend(e.clone());
    }
    auth.audit().log_coalesced(
        &account_who(client, None, "oidc", None),
        ACTION,
        None,
        Outcome::Denied,
        details,
    );
}
