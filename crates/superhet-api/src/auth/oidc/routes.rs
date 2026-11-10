//! `GET /api/v1/auth/oidc/login` and `GET /api/v1/auth/oidc/callback`: both are browser
//! navigations, so every outcome is a redirect (to `/` when signed in, to
//! `/?oidc_error=<code>` otherwise) and never a JSON error.

use std::time::Instant;

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::json;
use superhet_bus::audit::Outcome;
use superhet_db::oidc as db;
use superhet_db::users::{Guarded, Role};

use super::client::OidcError;
use super::config::{self, Identity, OidcSettings};
use super::flow::{self, FlowError};
use super::link::{self, Decision, Denial, Facts};
use crate::auth::routes::{account_who, check_rate, record_failure, start_oidc_session};
use crate::auth::{AuthState, ClientInfo, token};

pub fn router() -> Router<AuthState> {
    Router::new()
        .route(config::LOGIN_PATH, get(login))
        .route(config::CALLBACK_PATH, get(callback))
        .merge(super::routes_logout::router())
}

/// A redirect with `Cache-Control: no-store` and the given cookies.
fn redirect(location: &str, cookies: &[String]) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::try_from(location) {
        headers.insert(header::LOCATION, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    for cookie in cookies {
        if let Ok(value) = HeaderValue::try_from(cookie.as_str()) {
            headers.append(header::SET_COOKIE, value);
        }
    }
    response
}

/// Back to the login page with an error code the page can word.
fn to_login(code: &str, cookies: &[String]) -> Response {
    redirect(&format!("/?oidc_error={code}"), cookies)
}

async fn login(State(auth): State<AuthState>, client: ClientInfo) -> Response {
    let Some(settings) = auth.oidc_active().await else {
        return to_login("not_enabled", &[]);
    };
    if check_rate(&auth, &client).is_err() {
        return to_login("rate_limited", &[]);
    }
    let oidc = auth.oidc();
    let provider = match oidc
        .providers
        .get(
            &settings,
            oidc.env.client_secret.as_ref(),
            oidc.env.allow_http,
            false,
        )
        .await
    {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(%err, "OIDC login could not start");
            return to_login("unavailable", &[]);
        }
    };
    let request = provider.authorize(&settings.request_scopes());
    let Ok(binding) = token::generate_token() else {
        return to_login("unavailable", &[]);
    };
    if oidc
        .pending
        .insert(
            &request.state,
            &binding,
            request.nonce,
            request.verifier,
            Instant::now(),
        )
        .is_err()
    {
        return to_login("busy", &[]);
    }
    redirect(
        &request.url,
        &[flow::binding_cookie(&binding, client.secure)],
    )
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(auth): State<AuthState>,
    client: ClientInfo,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let clear = [flow::clearing_binding_cookie(client.secure)];
    let Some(settings) = auth.oidc_active().await else {
        return to_login("not_enabled", &clear);
    };
    if check_rate(&auth, &client).is_err() {
        return to_login("rate_limited", &clear);
    }
    match finish(&auth, &client, &headers, &query, &settings).await {
        Ok(session_cookie) => {
            let mut cookies = vec![session_cookie];
            cookies.extend(clear);
            redirect("/", &cookies)
        }
        Err(code) => to_login(code, &clear),
    }
}

/// Audit a refused or failed login.
async fn audit_failure(
    auth: &AuthState,
    client: &ClientInfo,
    name: &str,
    reason: &str,
    extra: serde_json::Value,
) {
    let mut details = json!({"reason": reason});
    if let (Some(d), Some(e)) = (details.as_object_mut(), extra.as_object()) {
        d.extend(e.clone());
    }
    auth.audit()
        .log(
            &account_who(client, None, name, None),
            "auth.oidc_login_failed",
            None,
            Outcome::Denied,
            details,
        )
        .await;
}

/// Everything after the browser came back; `Err` carries the code for the login page.
async fn finish(
    auth: &AuthState,
    client: &ClientInfo,
    headers: &HeaderMap,
    query: &CallbackQuery,
    settings: &OidcSettings,
) -> Result<String, &'static str> {
    let oidc = auth.oidc();
    let binding = token::cookie_value(headers, flow::BINDING_COOKIE);
    let Some(state) = query.state.as_deref() else {
        record_failure(auth, client);
        audit_failure(auth, client, "oidc", "invalid_state", json!({})).await;
        return Err("invalid_state");
    };
    let pending = match oidc.pending.take(state, binding.as_deref(), Instant::now()) {
        Ok(p) => p,
        Err(err) => {
            record_failure(auth, client);
            let reason = match err {
                FlowError::Expired => "expired",
                FlowError::BindingMismatch => "binding_mismatch",
                FlowError::UnknownState | FlowError::Full => "invalid_state",
            };
            audit_failure(auth, client, "oidc", reason, json!({})).await;
            return Err("invalid_state");
        }
    };
    if let Some(error) = &query.error {
        // The provider refused (access_denied, ...). Log its code, never its free text.
        let code: String = error
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(64)
            .collect();
        audit_failure(
            auth,
            client,
            "oidc",
            "provider_error",
            json!({"error": code}),
        )
        .await;
        return Err("provider_denied");
    }
    let Some(code) = query.code.as_deref().filter(|c| !c.is_empty()) else {
        audit_failure(auth, client, "oidc", "missing_code", json!({})).await;
        return Err("invalid_state");
    };

    let verified = verify(auth, settings, code, &pending).await.map_err(|err| {
        tracing::warn!(%err, "OIDC login failed");
        match err {
            OidcError::Token(_) => "token_invalid",
            _ => "unavailable",
        }
    });
    let verified = match verified {
        Ok(v) => v,
        Err(code) => {
            audit_failure(
                auth,
                client,
                "oidc",
                "provider_failed",
                json!({"stage": code}),
            )
            .await;
            return Err(code);
        }
    };
    let identity = Identity::from_payload(
        &verified.issuer,
        &verified.subject,
        &verified.payload,
        settings,
    );
    let name = identity
        .email
        .clone()
        .unwrap_or_else(|| identity.subject.clone());
    let db = auth.db().map_err(|_| "unavailable")?;
    let role = settings.map_role(&identity.groups);

    let outcome = match resolve(auth, client, settings, &identity, role).await {
        Ok(user) => user,
        Err(Resolve::Denied(denial)) => {
            audit_failure(
                auth,
                client,
                &name,
                denial.code(),
                json!({"issuer": identity.issuer, "subject": identity.subject}),
            )
            .await;
            return Err(denial.code());
        }
        Err(Resolve::Unavailable) => return Err("unavailable"),
    };

    auth.0.limiter.record_success(client.ip);
    let sid = verified
        .payload
        .get("sid")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 512);
    // Kept for the provider logout; a token too large for the column is simply not kept.
    let id_token = Some(verified.id_token.as_str()).filter(|t| t.len() <= 16 * 1024);
    let cookie = start_oidc_session(
        auth,
        outcome.user_id,
        client,
        &db::SessionOidc {
            issuer: &identity.issuer,
            sid,
            id_token,
        },
    )
    .await
    .map_err(|_| "unavailable")?;
    if let Err(err) = superhet_db::users::mark_login(db, outcome.user_id).await {
        tracing::warn!(%err, "could not record the login time");
    }
    auth.audit()
        .log(
            &account_who(
                client,
                Some(outcome.user_id),
                &outcome.username,
                Some(outcome.role),
            ),
            "auth.oidc_login",
            Some(&outcome.username),
            Outcome::Ok,
            json!({
                "issuer": identity.issuer,
                "subject": identity.subject,
                "provisioned": outcome.provisioned,
                "linked": outcome.linked,
            }),
        )
        .await;
    Ok(cookie)
}

async fn verify(
    auth: &AuthState,
    settings: &OidcSettings,
    code: &str,
    pending: &flow::Pending,
) -> Result<super::client::Verified, OidcError> {
    let oidc = auth.oidc();
    let secret = oidc.env.client_secret.as_ref();
    let provider = oidc
        .providers
        .get(settings, secret, oidc.env.allow_http, false)
        .await?;
    match provider
        .complete(
            oidc.providers.http(),
            code,
            &pending.verifier,
            &pending.nonce,
        )
        .await
    {
        // A token signed with a key we do not know yet: the provider may have rotated its keys.
        // The code is spent, so this only refreshes the cache for the next attempt.
        Err(OidcError::Token(reason)) => {
            let _ = oidc
                .providers
                .get(settings, secret, oidc.env.allow_http, true)
                .await;
            Err(OidcError::Token(reason))
        }
        other => other,
    }
}

/// The account a login resolved to.
struct Resolved {
    user_id: i64,
    username: String,
    role: Role,
    provisioned: bool,
    linked: bool,
}

enum Resolve {
    Denied(Denial),
    Unavailable,
}

fn unavailable(err: superhet_db::Error) -> Resolve {
    tracing::warn!(%err, "OIDC account lookup failed");
    Resolve::Unavailable
}

/// Look the identity up, decide, and carry the decision out.
async fn resolve(
    auth: &AuthState,
    client: &ClientInfo,
    settings: &OidcSettings,
    identity: &Identity,
    role: Option<Role>,
) -> Result<Resolved, Resolve> {
    let pool = auth.db().map_err(|_| Resolve::Unavailable)?;
    let mut facts = Facts {
        linked: db::find_by_identity(pool, &identity.issuer, &identity.subject)
            .await
            .map_err(unavailable)?,
        ..Facts::default()
    };
    if link::wants_email_lookup(settings, identity, facts.linked.is_some())
        && let Some(email) = &identity.email
    {
        facts.by_email = db::find_by_email(pool, email).await.map_err(unavailable)?;
        if let Some(user) = &facts.by_email {
            facts.by_email_has_identity = db::has_identity_at(pool, user.id, &identity.issuer)
                .await
                .map_err(unavailable)?;
        }
    }
    let email = identity
        .email
        .as_deref()
        .filter(|_| identity.email_verified);
    match link::decide(settings, identity, role, &facts) {
        Decision::Deny(denial) => Err(Resolve::Denied(denial)),
        Decision::SignIn(user) => {
            let role = sync_role(auth, client, settings, &user, role).await;
            touch(pool, identity, email).await;
            Ok(Resolved {
                user_id: user.id,
                username: user.username,
                role,
                provisioned: false,
                linked: false,
            })
        }
        Decision::Link(user) => {
            match db::link(pool, user.id, &identity.issuer, &identity.subject, email)
                .await
                .map_err(unavailable)?
            {
                db::Linked::Done => {}
                db::Linked::Conflict => return Err(Resolve::Denied(Denial::AlreadyLinked)),
            }
            auth.audit()
                .log(
                    &account_who(client, Some(user.id), &user.username, Some(user.role)),
                    "auth.oidc_link",
                    Some(&user.username),
                    Outcome::Ok,
                    json!({"issuer": identity.issuer, "subject": identity.subject, "by": "email"}),
                )
                .await;
            let role = sync_role(auth, client, settings, &user, role).await;
            touch(pool, identity, email).await;
            Ok(Resolved {
                user_id: user.id,
                username: user.username,
                role,
                provisioned: false,
                linked: true,
            })
        }
        Decision::Provision(role) => {
            let created = db::provision(
                pool,
                &identity.username,
                role,
                &identity.issuer,
                &identity.subject,
                email,
            )
            .await
            .map_err(unavailable)?;
            let Some(created) = created else {
                // The same person logged in twice at once; the other request won.
                return match db::find_by_identity(pool, &identity.issuer, &identity.subject)
                    .await
                    .map_err(unavailable)?
                {
                    Some(user) => Ok(Resolved {
                        user_id: user.id,
                        username: user.username,
                        role: user.role,
                        provisioned: false,
                        linked: false,
                    }),
                    None => Err(Resolve::Unavailable),
                };
            };
            auth.audit()
                .log(
                    &account_who(client, Some(created.id), &created.username, Some(role)),
                    "auth.oidc_provision",
                    Some(&created.username),
                    Outcome::Ok,
                    json!({"issuer": identity.issuer, "subject": identity.subject, "role": role.as_str()}),
                )
                .await;
            Ok(Resolved {
                user_id: created.id,
                username: created.username,
                role,
                provisioned: true,
                linked: false,
            })
        }
    }
}

async fn touch(pool: &superhet_db::PgPool, identity: &Identity, email: Option<&str>) {
    if let Err(err) = db::touch_login(pool, &identity.issuer, &identity.subject, email).await {
        tracing::warn!(%err, "could not record the OIDC login");
    }
}

/// Apply the mapped role to a linked account (if the admin wants roles to follow the provider)
/// and return the role the account has afterwards. The last active admin is never demoted.
async fn sync_role(
    auth: &AuthState,
    client: &ClientInfo,
    settings: &OidcSettings,
    user: &db::LinkedUser,
    mapped: Option<Role>,
) -> Role {
    let Some(wanted) = mapped.filter(|r| settings.sync_roles && *r != user.role) else {
        return user.role;
    };
    let Ok(pool) = auth.db() else {
        return user.role;
    };
    let who = account_who(client, Some(user.id), &user.username, Some(user.role));
    match superhet_db::users::set_role(pool, user.id, wanted).await {
        Ok(Guarded::Done) => {
            auth.audit()
                .log(
                    &who,
                    "user.role_change",
                    Some(&user.username),
                    Outcome::Ok,
                    json!({"from": user.role.as_str(), "to": wanted.as_str(), "source": "oidc"}),
                )
                .await;
            wanted
        }
        Ok(Guarded::LastAdmin) => {
            auth.audit()
                .log(
                    &who,
                    "user.role_change",
                    Some(&user.username),
                    Outcome::Denied,
                    json!({"reason": "last_admin", "to": wanted.as_str(), "source": "oidc"}),
                )
                .await;
            user.role
        }
        Ok(Guarded::NotFound) => user.role,
        Err(err) => {
            tracing::warn!(%err, "could not sync the role from the OIDC groups");
            user.role
        }
    }
}
