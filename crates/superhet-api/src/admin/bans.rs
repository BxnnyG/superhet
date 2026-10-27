//! Address bans (`/api/v1/admin/bans`): list, create (optionally from a live session, which
//! also kicks it) and lift. Enforcement lives in `auth::bans` and runs before authentication.

use std::collections::HashMap;
use std::net::IpAddr;

use axum::{
    Extension,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;
use superhet_db::bans::Ban;

use super::{AdminState, bad, conflict, not_found};
use crate::access::{Actor, Principal};
use crate::auth::bans::{Cidr, MAX_ACTIVE, canonical_ip};
use crate::auth::{Reply, db_failed, flatten, guard_origin, json_reply, parse_json, token};
use crate::registry::now_ms;

/// Longest ban in minutes (one year).
const MAX_MINUTES: u64 = 525_600;
/// Longest reason, in characters.
const REASON_MAX: usize = 200;

fn ban_json(ban: &Ban) -> Value {
    let state = if ban.lifted_at_ms.is_some() {
        "lifted"
    } else if ban.active {
        "active"
    } else {
        "expired"
    };
    json!({
        "id": ban.id,
        "network": ban.network,
        "reason": ban.reason,
        "state": state,
        "created_at_ms": ban.created_at_ms,
        "created_by": ban.created_by,
        "expires_at_ms": ban.expires_at_ms,
        "lifted_at_ms": ban.lifted_at_ms,
        "lifted_by": ban.lifted_by,
    })
}

pub(super) async fn list(
    State(state): State<AdminState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    flatten(list_inner(&state, &query).await)
}

async fn list_inner(state: &AdminState, query: &HashMap<String, String>) -> Reply {
    let db = state.auth.db()?;
    let only_active = match query.get("state").map(String::as_str) {
        None | Some("active") => true,
        Some("all") => false,
        Some(_) => return Err(bad("state must be active or all")),
    };
    let bans = superhet_db::bans::list(db, only_active)
        .await
        .map_err(db_failed)?;
    let rows: Vec<Value> = bans.iter().map(ban_json).collect();
    Ok(json_reply(StatusCode::OK, &json!({"bans": rows})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    /// Host or network to ban (`203.0.113.5`, `203.0.113.0/24`, `2001:db8::/48`).
    network: Option<String>,
    /// Alternatively a live session (`g.<id>` / `u.<hex>`): its exact client address is banned.
    session: Option<String>,
    reason: Option<String>,
    /// Minutes until the ban ends; absent = permanent.
    expires_in_minutes: Option<u64>,
    /// End matching sessions at once (default true).
    kick: Option<bool>,
}

/// The raw client address of a live session.
async fn session_ip(state: &AdminState, sid: &str) -> Result<IpAddr, Response> {
    let ip = if let Some(id) = sid.strip_prefix("g.") {
        state
            .auth
            .registry()
            .guests()
            .into_iter()
            .find(|(g, _)| g.id == id)
            .map(|(g, _)| g.ip)
            .ok_or_else(not_found)?
    } else if let Some(hexed) = sid.strip_prefix("u.") {
        let bytes = token::unhex(hexed)
            .filter(|b| b.len() == 32)
            .ok_or_else(|| bad("invalid session id"))?;
        superhet_db::users::list_sessions(state.auth.db()?, None)
            .await
            .map_err(db_failed)?
            .into_iter()
            .find(|s| s.id == bytes)
            .ok_or_else(not_found)?
            .ip
            .and_then(|ip| ip.parse().ok())
    } else {
        return Err(bad("invalid session id"));
    };
    ip.ok_or_else(|| conflict("no_address", "the address of that session is not known"))
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
    let db = state.auth.db()?;
    let req: CreateBody = parse_json(headers, body)?;
    let admin_id = match me.actor {
        Actor::User(id) => id,
        Actor::Guest(_) => return Err(bad("only accounts can ban")),
    };
    let via_session = req.session.clone();
    let net = match (&req.network, &req.session) {
        (Some(text), None) => Cidr::parse(text).map_err(|m| bad(&m))?,
        (None, Some(sid)) => {
            Cidr::parse(&session_ip(state, sid).await?.to_string()).map_err(|m| bad(&m))?
        }
        _ => return Err(bad("give exactly one of network and session")),
    };
    if let Some(reason) = &req.reason
        && (reason.chars().count() > REASON_MAX || reason.chars().any(char::is_control))
    {
        return Err(bad("reason is limited to 200 printable characters"));
    }
    let expires_at_ms = match req.expires_in_minutes {
        None => None,
        Some(m) if (1..=MAX_MINUTES).contains(&m) => {
            // Bounded by MAX_MINUTES, so the product fits an i64.
            Some(now_ms() + i64::try_from(m * 60_000).map_err(|_| bad("duration too long"))?)
        }
        Some(_) => return Err(bad("expires_in_minutes must be 1..=525600")),
    };
    if me.ip.is_some_and(|ip| net.contains(ip)) {
        return Err(conflict(
            "would_ban_self",
            "that would also block your own address",
        ));
    }
    if superhet_db::bans::list(db, true)
        .await
        .map_err(db_failed)?
        .len()
        >= MAX_ACTIVE
    {
        return Err(conflict("too_many_bans", "too many active bans"));
    }
    let network = net.to_string();
    let ban = superhet_db::bans::insert(
        db,
        &network,
        req.reason.as_deref(),
        expires_at_ms,
        admin_id,
        &me.name,
    )
    .await
    .map_err(db_failed)?
    .ok_or_else(|| conflict("already_banned", "that network is already banned"))?;
    state.auth.reload_bans().await.map_err(db_failed)?;
    let kicked = if req.kick.unwrap_or(true) {
        kick_matching(state, &net).await?
    } else {
        0
    };
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "ban.create",
            Some(&network),
            Outcome::Ok,
            json!({
                "reason": req.reason,
                "expires_at_ms": expires_at_ms,
                "kicked": kicked,
                "session": via_session,
            }),
        )
        .await;
    Ok(json_reply(
        StatusCode::CREATED,
        &json!({"ban": ban_json(&ban), "kicked": kicked}),
    ))
}

/// End every session (guest or account) whose address lies in `net`; returns how many.
async fn kick_matching(state: &AdminState, net: &Cidr) -> Result<usize, Response> {
    let registry = state.auth.registry();
    let mut kicked = 0;
    for (guest, _) in registry.guests() {
        if guest.ip.is_some_and(|ip| net.contains(canonical_ip(ip))) {
            registry.guest_remove_by_id(&guest.id);
            kicked += 1;
        }
    }
    let db = state.auth.db()?;
    for session in superhet_db::users::list_sessions(db, None)
        .await
        .map_err(db_failed)?
    {
        let inside = session
            .ip
            .as_deref()
            .and_then(|ip| ip.parse::<IpAddr>().ok())
            .is_some_and(|ip| net.contains(ip));
        if inside {
            registry.kick(&token::hex(&session.id));
            if superhet_db::users::revoke_session(db, &session.id, None)
                .await
                .map_err(db_failed)?
            {
                kicked += 1;
            }
        }
    }
    Ok(kicked)
}

pub(super) async fn lift(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    flatten(lift_inner(&state, &me, &id, &headers).await)
}

async fn lift_inner(state: &AdminState, me: &Principal, id: &str, headers: &HeaderMap) -> Reply {
    guard_origin(&state.auth, headers)?;
    let db = state.auth.db()?;
    let id: i64 = id.parse().map_err(|_| bad("invalid ban id"))?;
    let ban = superhet_db::bans::lift(db, id, &me.name)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    state.auth.reload_bans().await.map_err(db_failed)?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "ban.lift",
            Some(&ban.network),
            Outcome::Ok,
            json!({"ban": ban.id}),
        )
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}
