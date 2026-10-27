//! The public authentication endpoints under `/api/v1/auth`.

use std::sync::atomic::Ordering;
use std::time::Instant;

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use superhet_bus::audit::Outcome;
use superhet_db::users::Role;

use super::{
    AuthState, ClientInfo, error, internal, password, token, unauthenticated, unavailable,
};
use crate::audit::{ActorKind, Who};

/// Request bodies are tiny JSON documents; passwords are capped at 1024 characters.
const MAX_BODY_BYTES: usize = 8 * 1024;

/// Routes of the auth API (their own state; mounted outside the session check).
pub fn router(auth: AuthState) -> Router {
    Router::new()
        .merge(super::accounts::router())
        .route("/api/v1/auth/status", get(status))
        .route("/api/v1/auth/setup", post(setup))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/password", post(change_password))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(auth)
}

#[derive(Deserialize)]
struct SetupBody {
    username: String,
    password: String,
    setup_token: String,
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct PasswordBody {
    current_password: String,
    new_password: String,
}

pub(crate) type Reply = Result<Response, Response>;

pub(crate) fn flatten(reply: Reply) -> Response {
    match reply {
        Ok(r) | Err(r) => r,
    }
}

pub(crate) fn invalid(message: &str) -> Response {
    error(StatusCode::BAD_REQUEST, "invalid", Some(message))
}

pub(crate) fn invalid_credentials() -> Response {
    error(StatusCode::UNAUTHORIZED, "invalid_credentials", None)
}

pub(crate) fn db_failed(err: superhet_db::Error) -> Response {
    tracing::warn!(%err, "auth database error");
    unavailable()
}

/// CSRF protection for state-changing routes: the browser `Origin` must be allowed.
pub(crate) fn guard_origin(auth: &AuthState, headers: &HeaderMap) -> Result<(), Response> {
    if auth.0.origins.allows(headers) {
        Ok(())
    } else {
        tracing::warn!("auth: origin rejected");
        Err(error(
            StatusCode::FORBIDDEN,
            "forbidden",
            Some("origin not allowed"),
        ))
    }
}

pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<T, Response> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        return Err(error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid",
            Some("content type must be application/json"),
        ));
    }
    serde_json::from_slice(body)
        .map_err(|_| invalid("body is not valid JSON of the expected shape"))
}

pub(crate) fn rate_limited(wait: std::time::Duration) -> Response {
    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    let mut response = error(StatusCode::TOO_MANY_REQUESTS, "rate_limited", None);
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(secs.max(1)));
    response
}

pub(crate) fn check_rate(auth: &AuthState, client: &ClientInfo) -> Result<(), Response> {
    auth.0
        .limiter
        .check(client.ip, Instant::now())
        .map_err(rate_limited)
}

pub(crate) fn record_failure(auth: &AuthState, client: &ClientInfo) {
    auth.0.limiter.record_failure(client.ip, Instant::now());
}

pub(crate) fn no_content(cookie: &str) -> Reply {
    no_content_cookies(&[cookie])
}

/// 204 with several `Set-Cookie` headers.
pub(crate) fn no_content_cookies(cookies: &[&str]) -> Reply {
    let mut response = (
        StatusCode::NO_CONTENT,
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
    )
        .into_response();
    for cookie in cookies {
        let value = HeaderValue::try_from(*cookie).map_err(|_| internal())?;
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(response)
}

/// A JSON body with `no-store`.
pub(crate) fn json_reply(status: StatusCode, body: &serde_json::Value) -> Response {
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

/// The audit actor for an account that just authenticated (or tried to).
pub(crate) fn account_who(
    client: &ClientInfo,
    id: Option<i64>,
    name: &str,
    role: Option<Role>,
) -> Who {
    Who {
        kind: ActorKind::User,
        id: id.map(|i| i.to_string()),
        name: Some(name.chars().take(64).collect()),
        role,
        ip: client.ip,
        peer: client.peer,
        via: client.chain.clone(),
        user_agent: client.user_agent.clone(),
    }
}

/// Persist a new session for `user_id` and return the `Set-Cookie` value for it.
pub(super) async fn start_session(
    auth: &AuthState,
    user_id: i64,
    client: &ClientInfo,
) -> Result<String, Response> {
    let db = auth.db()?;
    let raw = token::generate_session_token().map_err(|err| {
        tracing::error!(%err, "no randomness for a session token");
        internal()
    })?;
    let ip = client.ip.map(|ip| ip.to_string());
    let peer = client.peer.map(|ip| ip.to_string());
    superhet_db::auth::create_session_from(
        db,
        &token::hash_token(&raw),
        user_id,
        client.user_agent.as_deref(),
        ip.as_deref(),
        peer.as_deref(),
    )
    .await
    .map_err(db_failed)?;
    Ok(token::session_cookie(&raw, client.secure))
}

async fn status(State(auth): State<AuthState>, client: ClientInfo, headers: HeaderMap) -> Response {
    flatten(status_inner(&auth, &client, &headers).await)
}

async fn status_inner(auth: &AuthState, client: &ClientInfo, headers: &HeaderMap) -> Reply {
    let db = auth.db()?;
    let setup_required = if auth.0.setup_done.load(Ordering::Relaxed) {
        false
    } else if superhet_db::auth::has_user(db).await.map_err(db_failed)? {
        auth.0.setup_done.store(true, Ordering::Relaxed);
        false
    } else {
        true
    };
    let principal = auth
        .resolve_principal(headers, client)
        .await
        .map_err(db_failed)?;
    let guest = auth.settings().guest_policy().await;
    let report = auth.settings().client_report().await;
    let guest_notice = guest
        .enabled
        .then(|| client_report_notice(&report, true))
        .flatten();
    let (user_id, username) = match principal.as_ref().map(|p| &p.actor) {
        Some(crate::access::Actor::User(id)) => {
            (Some(*id), principal.as_ref().map(|p| p.name.clone()))
        }
        _ => (None, None),
    };
    let body = serde_json::json!({
        "setup_required": setup_required,
        "authenticated": principal.is_some(),
        "username": username,
        "user_id": user_id,
        "role": principal.as_ref().map(|p| p.role.as_str()),
        "kind": principal.as_ref().map(|p| if p.is_guest() { "guest" } else { "user" }),
        "must_change_password": principal.as_ref().is_some_and(|p| p.must_change_password),
        "guest": {
            "enabled": guest.enabled,
            "privacy_text": guest.enabled.then_some(guest.privacy_text),
            // Tells the guest page up front what the browser will be asked to report.
            "client_report": guest_notice,
        },
        // What this browser is asked to report about itself (null: nothing).
        "client_report": principal
            .as_ref()
            .and_then(|p| client_report_notice(&report, p.is_guest())),
    });
    Ok(json_reply(StatusCode::OK, &body))
}

/// The client report request for a browser, or `None` when it is not asked for anything.
fn client_report_notice(
    policy: &crate::settings::ClientReportPolicy,
    guest: bool,
) -> Option<serde_json::Value> {
    policy.collects(guest).then(|| {
        serde_json::json!({
            "public_ip_service": policy.asks_public_ip(guest).then(|| policy.public_ip_service.clone()),
        })
    })
}

async fn setup(
    State(auth): State<AuthState>,
    client: ClientInfo,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(setup_inner(&auth, &client, &headers, &body).await)
}

async fn setup_inner(
    auth: &AuthState,
    client: &ClientInfo,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(auth, headers)?;
    let db = auth.db()?;
    let req: SetupBody = parse_json(headers, body)?;
    check_rate(auth, client)?;

    if superhet_db::auth::has_user(db).await.map_err(db_failed)? {
        return Err(error(StatusCode::CONFLICT, "already_setup", None));
    }
    let token_ok = auth
        .0
        .setup_token
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_deref()
        .is_some_and(|expected| token::secrets_equal(expected, &req.setup_token));
    if !token_ok {
        record_failure(auth, client);
        tracing::warn!(ip = ?client.ip, "setup rejected: wrong setup token");
        return Err(error(StatusCode::FORBIDDEN, "invalid_setup_token", None));
    }
    password::validate_username(&req.username).map_err(invalid)?;
    password::validate_password(&req.password).map_err(invalid)?;

    let phc = auth.hash_password(req.password).await?;
    let Some(user_id) = superhet_db::auth::create_admin(db, &req.username, &phc)
        .await
        .map_err(db_failed)?
    else {
        return Err(error(StatusCode::CONFLICT, "already_setup", None));
    };
    auth.0.setup_done.store(true, Ordering::Relaxed);
    *auth
        .0
        .setup_token
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    auth.0.limiter.record_success(client.ip);
    tracing::info!(username = %req.username, "admin account created");
    auth.audit()
        .log(
            &account_who(client, Some(user_id), &req.username, Some(Role::Admin)),
            "auth.setup",
            Some(&req.username),
            Outcome::Ok,
            serde_json::json!({}),
        )
        .await;

    let cookie = start_session(auth, user_id, client).await?;
    no_content(&cookie)
}

async fn login(
    State(auth): State<AuthState>,
    client: ClientInfo,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(login_inner(&auth, &client, &headers, &body).await)
}

async fn login_inner(
    auth: &AuthState,
    client: &ClientInfo,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(auth, headers)?;
    let db = auth.db()?;
    let req: LoginBody = parse_json(headers, body)?;
    let who = account_who(client, None, &req.username, None);
    let blocked = check_rate(auth, client).and_then(|()| {
        auth.0
            .name_limiter
            .check(&req.username, Instant::now())
            .map_err(rate_limited)
    });
    if let Err(response) = blocked {
        auth.audit().log_coalesced(
            &who,
            "auth.login_failed",
            Some(&who.name.clone().unwrap_or_default()),
            Outcome::Denied,
            serde_json::json!({"reason": "rate_limited"}),
        );
        return Err(response);
    }
    // Oversized input cannot be a valid credential; do not feed it to Argon2.
    if req.username.len() > password::USERNAME_MAX
        || req.password.chars().count() > password::PASSWORD_MAX
    {
        record_failure(auth, client);
        return Err(invalid_credentials());
    }

    let user = superhet_db::auth::find_user_by_name(db, &req.username)
        .await
        .map_err(db_failed)?;
    // Always run a verification, against a dummy hash for unknown users; a disabled account
    // is only rejected afterwards, so it costs the same time as a wrong password.
    let ok = auth
        .verify_password(req.password, user.as_ref().map(|u| u.password_hash.clone()))
        .await;
    let disabled = user.as_ref().is_some_and(|u| u.disabled);
    let Some(user) = user.filter(|u| ok && !u.disabled) else {
        record_failure(auth, client);
        auth.0
            .name_limiter
            .record_failure(&req.username, Instant::now());
        tracing::warn!(ip = ?client.ip, "login failed");
        auth.audit()
            .log(
                &who,
                "auth.login_failed",
                Some(&req.username),
                Outcome::Denied,
                serde_json::json!({"reason": if ok && disabled { "disabled" } else { "bad_credentials" }}),
            )
            .await;
        return Err(invalid_credentials());
    };
    auth.0.limiter.record_success(client.ip);
    auth.0.name_limiter.record_success(&req.username);
    let cookie = start_session(auth, user.id, client).await?;
    if let Err(err) = superhet_db::users::mark_login(db, user.id).await {
        tracing::warn!(%err, "could not record the login time");
    }
    auth.audit()
        .log(
            &account_who(client, Some(user.id), &user.username, Some(user.role)),
            "auth.login",
            Some(&user.username),
            Outcome::Ok,
            serde_json::json!({}),
        )
        .await;
    no_content(&cookie)
}

async fn logout(State(auth): State<AuthState>, client: ClientInfo, headers: HeaderMap) -> Response {
    flatten(logout_inner(&auth, &client, &headers).await)
}

async fn logout_inner(auth: &AuthState, client: &ClientInfo, headers: &HeaderMap) -> Reply {
    guard_origin(auth, headers)?;
    let db = auth.db()?;
    // Resolve first (it also refreshes a guest), then end whatever sessions the cookies name.
    if let Some(principal) = auth
        .resolve_principal(headers, client)
        .await
        .map_err(db_failed)?
    {
        auth.audit()
            .log(
                &principal.who(),
                "auth.logout",
                None,
                Outcome::Ok,
                serde_json::json!({}),
            )
            .await;
    }
    if let Some(raw) = token::token_from_headers(headers) {
        superhet_db::auth::delete_session(db, &token::hash_token(&raw))
            .await
            .map_err(db_failed)?;
    }
    if let Some(raw) = token::guest_token_from_headers(headers) {
        auth.registry().guest_remove(&token::hash_token(&raw));
    }
    no_content_cookies(&[
        &token::clearing_cookie(client.secure),
        &token::clearing_guest_cookie(client.secure),
    ])
}

async fn change_password(
    State(auth): State<AuthState>,
    client: ClientInfo,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(change_password_inner(&auth, &client, &headers, &body).await)
}

async fn change_password_inner(
    auth: &AuthState,
    client: &ClientInfo,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(auth, headers)?;
    let db = auth.db()?;
    let session = auth
        .authenticate(headers)
        .await
        .map_err(db_failed)?
        .ok_or_else(unauthenticated)?;
    let req: PasswordBody = parse_json(headers, body)?;
    check_rate(auth, client)?;
    password::validate_password(&req.new_password).map_err(invalid)?;
    if req.current_password.chars().count() > password::PASSWORD_MAX {
        record_failure(auth, client);
        return Err(invalid_credentials());
    }

    let user = superhet_db::auth::find_user_by_id(db, session.user_id)
        .await
        .map_err(db_failed)?;
    let ok = auth
        .verify_password(req.current_password, user.map(|u| u.password_hash))
        .await;
    let who = account_who(
        client,
        Some(session.user_id),
        &session.username,
        Some(session.role),
    );
    if !ok {
        record_failure(auth, client);
        tracing::warn!(ip = ?client.ip, "password change: wrong current password");
        auth.audit()
            .log(
                &who,
                "auth.password_change",
                None,
                Outcome::Denied,
                serde_json::json!({"reason": "bad_credentials"}),
            )
            .await;
        return Err(invalid_credentials());
    }
    auth.0.limiter.record_success(client.ip);

    let phc = auth.hash_password(req.new_password).await?;
    let revoked =
        superhet_db::auth::change_password(db, session.user_id, &phc, &session.token_hash)
            .await
            .map_err(db_failed)?;
    tracing::info!(revoked, "password changed, other sessions revoked");
    auth.audit()
        .log(
            &who,
            "auth.password_change",
            None,
            Outcome::Ok,
            serde_json::json!({"revoked_sessions": revoked}),
        )
        .await;
    Ok((
        StatusCode::NO_CONTENT,
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_rounds_up_and_is_at_least_one_second() {
        let r = rate_limited(std::time::Duration::from_millis(1500));
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(r.headers()[header::RETRY_AFTER], "2");
        let r = rate_limited(std::time::Duration::ZERO);
        assert_eq!(r.headers()[header::RETRY_AFTER], "1");
    }

    #[test]
    fn json_guard_rejects_wrong_content_type_and_shape() {
        let mut h = HeaderMap::new();
        let body = Bytes::from_static(br#"{"username":"a","password":"b"}"#);
        let err = parse_json::<LoginBody>(&h, &body)
            .err()
            .expect("no content type");
        assert_eq!(err.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(parse_json::<LoginBody>(&h, &body).is_ok());
        let err = parse_json::<LoginBody>(&h, &Bytes::from_static(b"{}"))
            .err()
            .expect("shape");
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }
}
