//! Live sessions and connections with kick (`/api/v1/admin/sessions`) and the self-service
//! list of one's own sessions (`/api/v1/me/sessions`).

use std::collections::HashMap;

use axum::{
    Extension,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;
use superhet_db::users::SessionInfo;

use super::{AdminState, bad, not_found};
use crate::access::{Actor, Principal};
use crate::audit::store_ip;
use crate::auth::{Reply, db_failed, flatten, guard_origin, json_reply, token};
use crate::devices::ctl_state;
use crate::registry::ConnInfo;

/// VFOs of every known device, grouped by owner string.
async fn vfos_by_owner(state: &AdminState) -> HashMap<String, Vec<Value>> {
    let mut by_owner: HashMap<String, Vec<Value>> = HashMap::new();
    for device in state.auth.registry().devices() {
        let Ok(doc) = ctl_state(&state.devices, &device).await else {
            continue;
        };
        for vfo in doc
            .get("vfos")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            let Some(owner) = vfo.get("owner").and_then(Value::as_str) else {
                continue;
            };
            by_owner.entry(owner.to_owned()).or_default().push(json!({
                "device": device,
                "id": vfo.get("id"),
                "freq_hz": vfo.get("freq_hz"),
                "mode": vfo.get("mode"),
                "status": vfo.get("status"),
            }));
        }
    }
    by_owner
}

fn conns_json(conns: &[ConnInfo]) -> Value {
    Value::Array(
        conns
            .iter()
            .map(|c| json!({"kind": c.kind, "device": c.device, "since_ms": c.since_ms}))
            .collect(),
    )
}

fn user_session_json(
    s: &SessionInfo,
    state: &AdminState,
    ip_mode: crate::settings::IpMode,
    vfos: &HashMap<String, Vec<Value>>,
    current: Option<&str>,
) -> Value {
    let key = token::hex(&s.id);
    let ip =
        s.ip.as_deref()
            .and_then(|ip| ip.parse().ok())
            .and_then(|ip| store_ip(Some(ip), ip_mode));
    json!({
        "sid": format!("u.{key}"),
        "kind": "user",
        "user_id": s.user_id,
        "username": s.username,
        "role": s.role.as_str(),
        "ip": ip,
        "user_agent": s.user_agent,
        "created_at_ms": s.created_at_ms,
        "last_seen_ms": s.last_seen_ms,
        "current": current == Some(key.as_str()),
        "connections": conns_json(&state.auth.registry().connections_of(&key)),
        "vfos": vfos.get(&format!("u:{}", s.user_id)).cloned().unwrap_or_default(),
    })
}

pub(super) async fn list(State(state): State<AdminState>) -> Response {
    flatten(list_inner(&state).await)
}

async fn list_inner(state: &AdminState) -> Reply {
    let db = state.auth.db()?;
    let settings = state.auth.settings().audit().await;
    let sessions = superhet_db::users::list_sessions(db, None)
        .await
        .map_err(db_failed)?;
    let vfos = vfos_by_owner(state).await;
    let mut rows: Vec<Value> = sessions
        .iter()
        .map(|s| user_session_json(s, state, settings.ip_mode_user, &vfos, None))
        .collect();
    for (guest, idle_secs) in state.auth.registry().guests() {
        rows.push(json!({
            "sid": format!("g.{}", guest.id),
            "kind": "guest",
            "role": "listener",
            "ip": store_ip(guest.ip, settings.ip_mode_guest),
            "user_agent": guest.user_agent,
            "created_at_ms": guest.created_ms,
            "idle_secs": idle_secs,
            "connections": conns_json(&state.auth.registry().connections_of(&crate::registry::guest_key(&guest.id))),
            "vfos": vfos.get(&format!("g:{}", guest.id)).cloned().unwrap_or_default(),
        }));
    }
    Ok(json_reply(StatusCode::OK, &json!({"sessions": rows})))
}

pub(super) async fn kick(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(sid): Path<String>,
    headers: HeaderMap,
) -> Response {
    flatten(kick_inner(&state, &me, &sid, &headers).await)
}

async fn kick_inner(state: &AdminState, me: &Principal, sid: &str, headers: &HeaderMap) -> Reply {
    guard_origin(&state.auth, headers)?;
    let db = state.auth.db()?;
    let (target, kind) = if let Some(hexed) = sid.strip_prefix("u.") {
        let bytes = token::unhex(hexed)
            .filter(|b| b.len() == 32)
            .ok_or_else(|| bad("invalid session id"))?;
        let name = superhet_db::users::list_sessions(db, None)
            .await
            .map_err(db_failed)?
            .into_iter()
            .find(|s| s.id == bytes)
            .map(|s| s.username)
            .ok_or_else(not_found)?;
        // Close the sockets first, then drop the row, so nothing reconnects in between.
        state.auth.registry().kick(hexed);
        if !superhet_db::users::revoke_session(db, &bytes, None)
            .await
            .map_err(db_failed)?
        {
            return Err(not_found());
        }
        (name, "user")
    } else if let Some(id) = sid.strip_prefix("g.") {
        state
            .auth
            .registry()
            .guest_remove_by_id(id)
            .ok_or_else(not_found)?;
        (format!("guest {id}"), "guest")
    } else {
        return Err(bad("invalid session id"));
    };
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "session.kick",
            Some(&target),
            Outcome::Ok,
            json!({"kind": kind}),
        )
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn own_id(me: &Principal) -> Result<i64, Response> {
    match me.actor {
        Actor::User(id) => Ok(id),
        Actor::Guest(_) => Err(crate::auth::error(StatusCode::FORBIDDEN, "forbidden", None)),
    }
}

pub(super) async fn mine(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
) -> Response {
    flatten(mine_inner(&state, &me).await)
}

async fn mine_inner(state: &AdminState, me: &Principal) -> Reply {
    let db = state.auth.db()?;
    let sessions = superhet_db::users::list_sessions(db, Some(own_id(me)?))
        .await
        .map_err(db_failed)?;
    let none = HashMap::new();
    let rows: Vec<Value> = sessions
        .iter()
        .map(|s| {
            let mut row = user_session_json(
                s,
                state,
                crate::settings::IpMode::Full,
                &none,
                Some(&me.session_key),
            );
            // Other people's connection details are not needed here; keep it to the basics.
            if let Some(obj) = row.as_object_mut() {
                obj.remove("connections");
                obj.remove("vfos");
                obj.remove("user_id");
                obj.remove("role");
            }
            row
        })
        .collect();
    Ok(json_reply(StatusCode::OK, &json!({"sessions": rows})))
}

pub(super) async fn revoke_mine(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(sid): Path<String>,
    headers: HeaderMap,
) -> Response {
    flatten(revoke_mine_inner(&state, &me, &sid, &headers).await)
}

async fn revoke_mine_inner(
    state: &AdminState,
    me: &Principal,
    sid: &str,
    headers: &HeaderMap,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let db = state.auth.db()?;
    let user_id = own_id(me)?;
    let hexed = sid
        .strip_prefix("u.")
        .ok_or_else(|| bad("invalid session id"))?;
    let bytes = token::unhex(hexed)
        .filter(|b| b.len() == 32)
        .ok_or_else(|| bad("invalid session id"))?;
    if !superhet_db::users::revoke_session(db, &bytes, Some(user_id))
        .await
        .map_err(db_failed)?
    {
        return Err(not_found());
    }
    state.auth.registry().kick(hexed);
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "session.revoke",
            Some(&me.name),
            Outcome::Ok,
            json!({"current": hexed == me.session_key}),
        )
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}
