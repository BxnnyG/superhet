//! Public receiver share links (`/api/v1/admin/share-links`, ADR-0029, `docs/api-share.md`).
//!
//! The token is generated here and shown exactly once, in the reply of the call that made it
//! (create or rotate); the database keeps only its SHA-256.

use axum::{
    Extension,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;

use super::settings_routes::changed_keys;
use super::{AdminState, bad, not_found};
use crate::access::Principal;
use crate::auth::{Reply, db_failed, flatten, guard_origin, internal, json_reply, parse_json};
use crate::registry::now_ms;
use crate::share::links::{Link, LinkInput, Status, Tuning};

/// A link as the admin sees it (never the token).
fn view(state: &AdminState, link: &Link) -> Value {
    let (tuning, freq_hz, mode, range) = match &link.tuning {
        Tuning::Follow => ("follow", None, None, None),
        Tuning::Fixed {
            freq_hz,
            mode,
            range,
        } => ("fixed", Some(*freq_hz), Some(mode.as_str()), *range),
    };
    json!({
        "id": link.id,
        "token_hint": link.token_hint,
        "label": link.label,
        "title": link.title,
        "description": link.description,
        "location": link.location,
        "device": link.device,
        "tuning": tuning,
        "freq_hz": freq_hz,
        "mode": mode,
        "tune_min_hz": range.map(|r| r.0),
        "tune_max_hz": range.map(|r| r.1),
        "features": link.features.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
        "max_listeners": link.max_listeners,
        "embed_origins": link.embed_origins,
        "expires_at_ms": link.expires_at_ms,
        "revoked_at_ms": link.revoked_at_ms,
        "created_at_ms": link.created_at_ms,
        "created_by": link.created_by,
        "status": link.status(now_ms()).as_str(),
        "listeners": state.auth.share_live().count(link.id, link.primary_kind()),
    })
}

fn pool(state: &AdminState) -> Result<&superhet_db::PgPool, Response> {
    state
        .auth
        .share_links()
        .db()
        .ok_or_else(crate::auth::unavailable)
}

/// The stored links, read fresh.
async fn fresh(state: &AdminState) -> Result<Vec<std::sync::Arc<Link>>, Response> {
    state.auth.share_links().invalidate();
    state
        .auth
        .share_links()
        .all()
        .await
        .map_err(|_| crate::auth::unavailable())
}

fn parse_id(raw: &str) -> Result<i64, Response> {
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| bad("invalid link id"))
}

/// A fresh token and its digest and hint.
fn new_token() -> Result<(String, [u8; 32], String), Response> {
    let token = crate::auth::token::generate_token().map_err(|err| {
        tracing::error!(%err, "no random bytes for a share token");
        internal()
    })?;
    let hash = crate::auth::token::hash_token(&token);
    let hint: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Ok((token, hash, hint))
}

pub(super) async fn list(State(state): State<AdminState>) -> Response {
    flatten(list_inner(&state).await)
}

async fn list_inner(state: &AdminState) -> Reply {
    let links = fresh(state).await?;
    let views: Vec<Value> = links.iter().map(|l| view(state, l)).collect();
    Ok(json_reply(StatusCode::OK, &json!({"v": 1, "links": views})))
}

pub(super) async fn create(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(create_inner(&state, &me, &headers, &body).await)
}

async fn create_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let input: LinkInput = parse_json(headers, body)?;
    let mut link = input.validate(now_ms()).map_err(|m| bad(&m))?;
    let (token, hash, hint) = new_token()?;
    link.token_hint = hint;
    link.created_by = Some(me.name.clone());
    link.id = superhet_db::share_links::insert(pool(state)?, &link.to_record(hash))
        .await
        .map_err(db_failed)?;
    state.auth.share_links().invalidate();
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "share.link_create",
            Some(&link.id.to_string()),
            Outcome::Ok,
            json!({
                "label": link.label,
                "device": link.device,
                "features": link.features.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
                "expires_at_ms": link.expires_at_ms,
                "embeddable": !link.embed_origins.is_empty(),
            }),
        )
        .await;
    Ok(json_reply(
        StatusCode::CREATED,
        &json!({"v": 1, "link": view(state, &link), "token": token}),
    ))
}

async fn existing(state: &AdminState, id: i64) -> Result<std::sync::Arc<Link>, Response> {
    fresh(state)
        .await?
        .into_iter()
        .find(|l| l.id == id)
        .ok_or_else(not_found)
}

pub(super) async fn update(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(update_inner(&state, &me, &id, &headers, &body).await)
}

async fn update_inner(
    state: &AdminState,
    me: &Principal,
    id: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let id = parse_id(id)?;
    let input: LinkInput = parse_json(headers, body)?;
    let old = existing(state, id).await?;
    let mut link = input.validate(now_ms()).map_err(|m| bad(&m))?;
    link.id = id;
    link.token_hint.clone_from(&old.token_hint);
    link.revoked_at_ms = old.revoked_at_ms;
    link.created_at_ms = old.created_at_ms;
    link.created_by.clone_from(&old.created_by);
    // The token digest is not edited here; `update` leaves the column alone.
    if !superhet_db::share_links::update(pool(state)?, &link.to_record([0; 32]))
        .await
        .map_err(db_failed)?
    {
        return Err(not_found());
    }
    state.auth.share_links().invalidate();
    // Connections were admitted under the old rules; they reconnect under the new ones.
    let ended = crate::share::kick_link(&state.auth, id);
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "share.link_update",
            Some(&id.to_string()),
            Outcome::Ok,
            json!({
                "changed": changed_keys(&view(state, &old), &view(state, &link)),
                "ended_connections": ended,
            }),
        )
        .await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"v": 1, "link": view(state, &link), "ended_connections": ended}),
    ))
}

pub(super) async fn revoke(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    flatten(revoke_inner(&state, &me, &id, &headers).await)
}

async fn revoke_inner(state: &AdminState, me: &Principal, id: &str, headers: &HeaderMap) -> Reply {
    guard_origin(&state.auth, headers)?;
    let id = parse_id(id)?;
    if !superhet_db::share_links::revoke(pool(state)?, id)
        .await
        .map_err(db_failed)?
    {
        return Err(not_found());
    }
    state.auth.share_links().invalidate();
    let ended = crate::share::kick_link(&state.auth, id);
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "share.link_revoke",
            Some(&id.to_string()),
            Outcome::Ok,
            json!({"ended_connections": ended}),
        )
        .await;
    let link = existing(state, id).await?;
    debug_assert_eq!(link.status(now_ms()), Status::Revoked);
    Ok(json_reply(
        StatusCode::OK,
        &json!({"v": 1, "link": view(state, &link), "ended_connections": ended}),
    ))
}

pub(super) async fn rotate(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    flatten(rotate_inner(&state, &me, &id, &headers).await)
}

async fn rotate_inner(state: &AdminState, me: &Principal, id: &str, headers: &HeaderMap) -> Reply {
    guard_origin(&state.auth, headers)?;
    let id = parse_id(id)?;
    let (token, hash, hint) = new_token()?;
    if !superhet_db::share_links::set_token(pool(state)?, id, &hash, &hint)
        .await
        .map_err(db_failed)?
    {
        return Err(not_found());
    }
    state.auth.share_links().invalidate();
    let ended = crate::share::kick_link(&state.auth, id);
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "share.link_rotate",
            Some(&id.to_string()),
            Outcome::Ok,
            json!({"ended_connections": ended}),
        )
        .await;
    let link = existing(state, id).await?;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"v": 1, "link": view(state, &link), "token": token, "ended_connections": ended}),
    ))
}

pub(super) async fn remove(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    flatten(remove_inner(&state, &me, &id, &headers).await)
}

async fn remove_inner(state: &AdminState, me: &Principal, id: &str, headers: &HeaderMap) -> Reply {
    guard_origin(&state.auth, headers)?;
    let id = parse_id(id)?;
    // Kick before the row is gone: afterwards the table can no longer name the link.
    let ended = crate::share::kick_link(&state.auth, id);
    if !superhet_db::share_links::delete(pool(state)?, id)
        .await
        .map_err(db_failed)?
    {
        return Err(not_found());
    }
    state.auth.share_links().invalidate();
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "share.link_delete",
            Some(&id.to_string()),
            Outcome::Ok,
            json!({"ended_connections": ended}),
        )
        .await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"v": 1, "ended_connections": ended}),
    ))
}
