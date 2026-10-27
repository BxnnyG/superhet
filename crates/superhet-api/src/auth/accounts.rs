//! Public endpoints that create sessions without a password login: guest sessions (ADR-0011)
//! and invite redemption. Both are rate limited; the guest endpoint does not exist (404) while
//! guest mode is off.

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use superhet_bus::audit::Outcome;
use superhet_db::users::{Accepted, InviteState};

use super::routes::{
    Reply, account_who, check_rate, db_failed, flatten, guard_origin, invalid, json_reply,
    no_content, parse_json, record_failure, start_session,
};
use super::{AuthState, ClientInfo, error, internal, password, token};
use crate::audit::{ActorKind, Who};
use crate::registry::GuestRefusal;

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/api/v1/auth/guest", post(guest_start))
        .route("/api/v1/auth/invites/{token}", get(invite_preview))
        .route("/api/v1/auth/invites/accept", post(invite_accept))
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", None)
}

async fn guest_start(
    State(auth): State<AuthState>,
    client: ClientInfo,
    headers: HeaderMap,
) -> Response {
    flatten(guest_start_inner(&auth, &client, &headers).await)
}

async fn guest_start_inner(auth: &AuthState, client: &ClientInfo, headers: &HeaderMap) -> Reply {
    guard_origin(auth, headers)?;
    let policy = auth.settings().guest_policy().await;
    if !policy.enabled || auth.0.db.is_none() {
        return Err(not_found());
    }
    let limits = serde_json::json!({
        "idle_timeout_secs": policy.idle_timeout_secs,
        "max_vfos": policy.max_vfos_per_guest,
        "allowed_modes": policy.allowed_modes,
        "freq_min_hz": policy.freq_min_hz,
        "freq_max_hz": policy.freq_max_hz,
        "show_decoder_events": policy.show_decoder_events,
    });
    // A browser that already holds a live guest session keeps it.
    if let Some(raw) = token::guest_token_from_headers(headers)
        && let Some(existing) = auth.registry().guest_touch(
            &token::hash_token(&raw),
            std::time::Duration::from_secs(policy.idle_timeout_secs),
        )
    {
        let body = serde_json::json!({"guest": {"id": existing.id}, "limits": limits});
        return Ok(json_reply(StatusCode::OK, &body));
    }
    let raw = token::generate_token().map_err(|err| {
        tracing::error!(%err, "no randomness for a guest token");
        internal()
    })?;
    let session = auth
        .registry()
        .guest_start(
            token::hash_token(&raw),
            client.ip,
            client.peer,
            client.user_agent.clone(),
            &policy,
        )
        .map_err(|refusal| match refusal {
            GuestRefusal::Full => error(StatusCode::SERVICE_UNAVAILABLE, "guests_full", None),
            GuestRefusal::TooManyFromAddress => {
                error(StatusCode::TOO_MANY_REQUESTS, "too_many_guests", None)
            }
        })?;
    let who = Who {
        kind: ActorKind::Guest,
        id: Some(session.id.clone()),
        name: Some("guest".to_owned()),
        role: Some(superhet_db::users::Role::Listener),
        ip: client.ip,
        peer: client.peer,
        via: client.chain.clone(),
        user_agent: client.user_agent.clone(),
    };
    auth.audit()
        .log(
            &who,
            "guest.session_start",
            None,
            Outcome::Ok,
            serde_json::json!({}),
        )
        .await;
    let mut response = json_reply(
        StatusCode::OK,
        &serde_json::json!({"guest": {"id": session.id}, "limits": limits}),
    );
    let cookie =
        HeaderValue::try_from(token::guest_cookie(&raw, client.secure)).map_err(|_| internal())?;
    response.headers_mut().append(header::SET_COOKIE, cookie);
    Ok(response)
}

async fn invite_preview(
    State(auth): State<AuthState>,
    client: ClientInfo,
    Path(raw): Path<String>,
) -> Response {
    flatten(invite_preview_inner(&auth, &client, &raw).await)
}

fn invalid_invite() -> Response {
    error(StatusCode::NOT_FOUND, "invalid_invite", None)
}

async fn invite_preview_inner(auth: &AuthState, client: &ClientInfo, raw: &str) -> Reply {
    let db = auth.db()?;
    check_rate(auth, client)?;
    let invite = superhet_db::users::find_invite(db, &token::hash_token(raw))
        .await
        .map_err(db_failed)?
        .filter(|i| i.state == InviteState::Open);
    let Some(invite) = invite else {
        record_failure(auth, client);
        return Err(invalid_invite());
    };
    Ok(json_reply(
        StatusCode::OK,
        &serde_json::json!({
            "role": invite.role.as_str(),
            "expires_at_ms": invite.expires_at_ms,
            "invited_by": invite.created_by_name,
        }),
    ))
}

#[derive(Deserialize)]
struct AcceptBody {
    token: String,
    username: String,
    password: String,
}

async fn invite_accept(
    State(auth): State<AuthState>,
    client: ClientInfo,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(invite_accept_inner(&auth, &client, &headers, &body).await)
}

async fn invite_accept_inner(
    auth: &AuthState,
    client: &ClientInfo,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(auth, headers)?;
    let db = auth.db()?;
    let req: AcceptBody = parse_json(headers, body)?;
    check_rate(auth, client)?;
    let hash = token::hash_token(&req.token);
    // Look the invite up before spending an Argon2 run on a hopeless request.
    let open = superhet_db::users::find_invite(db, &hash)
        .await
        .map_err(db_failed)?
        .is_some_and(|i| i.state == InviteState::Open);
    if !open {
        record_failure(auth, client);
        return Err(invalid_invite());
    }
    password::validate_username(&req.username).map_err(invalid)?;
    password::validate_password(&req.password).map_err(invalid)?;
    let phc = auth.hash_password(req.password).await?;
    match superhet_db::users::accept_invite(db, &hash, &req.username, &phc)
        .await
        .map_err(db_failed)?
    {
        Accepted::InvalidToken => {
            record_failure(auth, client);
            Err(invalid_invite())
        }
        Accepted::UsernameTaken => Err(error(StatusCode::CONFLICT, "username_taken", None)),
        Accepted::Created { user_id, role } => {
            auth.audit()
                .log(
                    &account_who(client, Some(user_id), &req.username, Some(role)),
                    "invite.accept",
                    Some(&req.username),
                    Outcome::Ok,
                    serde_json::json!({"role": role.as_str()}),
                )
                .await;
            let cookie = start_session(auth, user_id, client).await?;
            no_content(&cookie)
        }
    }
}
