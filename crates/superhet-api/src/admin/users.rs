//! Users, passwords and invites (`/api/v1/admin/users`, `/api/v1/admin/invites`).

use std::time::Duration;

use axum::{
    Extension,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;
use superhet_db::users::{Created, Guarded, InviteInfo, Role, UserInfo};

use super::{AdminState, bad, conflict, not_found};
use crate::access::Principal;
use crate::auth::{
    Reply, db_failed, error, flatten, guard_origin, internal, invalid, json_reply, parse_json,
    password, token,
};

/// Length of a generated one-time password.
const GENERATED_PASSWORD_LEN: usize = 20;
/// Default and largest invite lifetime in hours.
const INVITE_DEFAULT_HOURS: u64 = 72;
const INVITE_MAX_HOURS: u64 = 720;
/// Longest invite note, in characters.
const NOTE_MAX: usize = 200;

fn parse_role(value: &str) -> Result<Role, Response> {
    Role::parse(value).ok_or_else(|| bad("role must be admin, operator or listener"))
}

fn user_json(user: &UserInfo, sessions: usize) -> Value {
    json!({
        "id": user.id,
        "username": user.username,
        "role": user.role.as_str(),
        "disabled": user.disabled,
        "must_change_password": user.must_change_password,
        "created_at_ms": user.created_at_ms,
        "last_login_ms": user.last_login_ms,
        "sessions": sessions,
    })
}

fn invite_json(invite: &InviteInfo) -> Value {
    json!({
        "id": invite.id,
        "role": invite.role.as_str(),
        "note": invite.note,
        "created_by": invite.created_by_name,
        "created_at_ms": invite.created_at_ms,
        "expires_at_ms": invite.expires_at_ms,
        "state": invite.state.as_str(),
        "used_by": invite.used_by_name,
    })
}

fn parse_id(raw: &str) -> Result<i64, Response> {
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| bad("invalid id"))
}

/// Close the sockets of every session of `user_id` (before the rows are deleted).
async fn kick_user(state: &AdminState, user_id: i64) -> Result<(), Response> {
    let db = state.auth.db()?;
    for session in superhet_db::users::list_sessions(db, Some(user_id))
        .await
        .map_err(db_failed)?
    {
        state.auth.registry().kick(&token::hex(&session.id));
    }
    Ok(())
}

pub(super) async fn list(State(state): State<AdminState>) -> Response {
    flatten(list_inner(&state).await)
}

async fn list_inner(state: &AdminState) -> Reply {
    let db = state.auth.db()?;
    let users = superhet_db::users::list(db).await.map_err(db_failed)?;
    let sessions = superhet_db::users::list_sessions(db, None)
        .await
        .map_err(db_failed)?;
    let rows: Vec<Value> = users
        .iter()
        .map(|u| user_json(u, sessions.iter().filter(|s| s.user_id == u.id).count()))
        .collect();
    Ok(json_reply(StatusCode::OK, &json!({"users": rows})))
}

pub(super) async fn get(State(state): State<AdminState>, Path(id): Path<String>) -> Response {
    flatten(get_inner(&state, &id).await)
}

async fn get_inner(state: &AdminState, id: &str) -> Reply {
    let db = state.auth.db()?;
    let id = parse_id(id)?;
    let user = superhet_db::users::get(db, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    let sessions = superhet_db::users::list_sessions(db, Some(id))
        .await
        .map_err(db_failed)?;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"user": user_json(&user, sessions.len())}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    username: String,
    role: String,
    password: Option<String>,
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
    let role = parse_role(&req.role)?;
    password::validate_username(&req.username).map_err(invalid)?;
    let (plain, generated) = match req.password {
        Some(p) => (p, false),
        None => (
            token::generate_password(GENERATED_PASSWORD_LEN).map_err(|_| internal())?,
            true,
        ),
    };
    password::validate_password(&plain).map_err(invalid)?;
    let phc = state.auth.hash_password(plain.clone()).await?;
    // A password the admin chose or saw is a one-time password: the user must replace it.
    let created = superhet_db::users::create(db, &req.username, &phc, role, true)
        .await
        .map_err(db_failed)?;
    let Created::Id(id) = created else {
        state
            .auth
            .audit()
            .log(
                &me.who(),
                "user.create",
                Some(&req.username),
                Outcome::Denied,
                json!({"reason": "username_taken"}),
            )
            .await;
        return Err(conflict("username_taken", "that username exists already"));
    };
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "user.create",
            Some(&req.username),
            Outcome::Ok,
            json!({"id": id, "role": role.as_str()}),
        )
        .await;
    let user = superhet_db::users::get(db, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(internal)?;
    let mut reply = json!({"user": user_json(&user, 0)});
    if generated {
        reply["password"] = Value::String(plain);
    }
    Ok(json_reply(StatusCode::CREATED, &reply))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    role: Option<String>,
    disabled: Option<bool>,
}

fn last_admin() -> Response {
    conflict("last_admin", "at least one active admin must remain")
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
    let db = state.auth.db()?;
    let id = parse_id(id)?;
    let req: UpdateBody = parse_json(headers, body)?;
    if req.role.is_none() && req.disabled.is_none() {
        return Err(bad("nothing to change"));
    }
    let new_role = req.role.as_deref().map(parse_role).transpose()?;
    let before = superhet_db::users::get(db, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    let audit = state.auth.audit();

    if let Some(role) = new_role
        && role != before.role
    {
        match superhet_db::users::set_role(db, id, role)
            .await
            .map_err(db_failed)?
        {
            Guarded::Done => {
                audit
                    .log(
                        &me.who(),
                        "user.role_change",
                        Some(&before.username),
                        Outcome::Ok,
                        json!({"from": before.role.as_str(), "to": role.as_str()}),
                    )
                    .await;
            }
            Guarded::LastAdmin => {
                audit
                    .log(
                        &me.who(),
                        "user.role_change",
                        Some(&before.username),
                        Outcome::Denied,
                        json!({"reason": "last_admin", "to": role.as_str()}),
                    )
                    .await;
                return Err(last_admin());
            }
            Guarded::NotFound => return Err(not_found()),
        }
    }
    if let Some(disabled) = req.disabled
        && disabled != before.disabled
    {
        let action = if disabled {
            "user.disable"
        } else {
            "user.enable"
        };
        if disabled {
            kick_user(state, id).await?;
        }
        match superhet_db::users::set_disabled(db, id, disabled)
            .await
            .map_err(db_failed)?
        {
            Guarded::Done => {
                audit
                    .log(
                        &me.who(),
                        action,
                        Some(&before.username),
                        Outcome::Ok,
                        json!({}),
                    )
                    .await;
            }
            Guarded::LastAdmin => {
                audit
                    .log(
                        &me.who(),
                        action,
                        Some(&before.username),
                        Outcome::Denied,
                        json!({"reason": "last_admin"}),
                    )
                    .await;
                return Err(last_admin());
            }
            Guarded::NotFound => return Err(not_found()),
        }
    }
    let user = superhet_db::users::get(db, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    let sessions = superhet_db::users::list_sessions(db, Some(id))
        .await
        .map_err(db_failed)?;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"user": user_json(&user, sessions.len())}),
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
    let db = state.auth.db()?;
    let id = parse_id(id)?;
    let user = superhet_db::users::get(db, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    kick_user(state, id).await?;
    match superhet_db::users::delete(db, id)
        .await
        .map_err(db_failed)?
    {
        Guarded::Done => {
            state
                .auth
                .audit()
                .log(
                    &me.who(),
                    "user.delete",
                    Some(&user.username),
                    Outcome::Ok,
                    json!({"id": id, "role": user.role.as_str()}),
                )
                .await;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        Guarded::LastAdmin => {
            state
                .auth
                .audit()
                .log(
                    &me.who(),
                    "user.delete",
                    Some(&user.username),
                    Outcome::Denied,
                    json!({"reason": "last_admin"}),
                )
                .await;
            Err(last_admin())
        }
        Guarded::NotFound => Err(not_found()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetBody {
    password: Option<String>,
}

pub(super) async fn reset_password(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(reset_inner(&state, &me, &id, &headers, &body).await)
}

async fn reset_inner(
    state: &AdminState,
    me: &Principal,
    id: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let db = state.auth.db()?;
    let id = parse_id(id)?;
    let req: ResetBody = if body.is_empty() {
        ResetBody { password: None }
    } else {
        parse_json(headers, body)?
    };
    let user = superhet_db::users::get(db, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    let (plain, generated) = match req.password {
        Some(p) => (p, false),
        None => (
            token::generate_password(GENERATED_PASSWORD_LEN).map_err(|_| internal())?,
            true,
        ),
    };
    password::validate_password(&plain).map_err(invalid)?;
    let phc = state.auth.hash_password(plain.clone()).await?;
    kick_user(state, id).await?;
    if !superhet_db::users::reset_password(db, id, &phc, true)
        .await
        .map_err(db_failed)?
    {
        return Err(not_found());
    }
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "user.password_reset",
            Some(&user.username),
            Outcome::Ok,
            json!({"generated": generated}),
        )
        .await;
    if generated {
        Ok(json_reply(StatusCode::OK, &json!({"password": plain})))
    } else {
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}

pub(super) async fn list_invites(State(state): State<AdminState>) -> Response {
    flatten(list_invites_inner(&state).await)
}

async fn list_invites_inner(state: &AdminState) -> Reply {
    let db = state.auth.db()?;
    let invites = superhet_db::users::list_invites(db)
        .await
        .map_err(db_failed)?;
    let rows: Vec<Value> = invites.iter().map(invite_json).collect();
    Ok(json_reply(StatusCode::OK, &json!({"invites": rows})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteBody {
    role: String,
    expires_in_hours: Option<u64>,
    note: Option<String>,
}

pub(super) async fn create_invite(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(create_invite_inner(&state, &me, &headers, &body).await)
}

async fn create_invite_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let db = state.auth.db()?;
    let req: InviteBody = parse_json(headers, body)?;
    let role = parse_role(&req.role)?;
    let hours = req.expires_in_hours.unwrap_or(INVITE_DEFAULT_HOURS);
    if !(1..=INVITE_MAX_HOURS).contains(&hours) {
        return Err(bad("expires_in_hours must be 1..=720"));
    }
    if let Some(note) = &req.note
        && (note.chars().count() > NOTE_MAX || note.chars().any(char::is_control))
    {
        return Err(bad("note is limited to 200 printable characters"));
    }
    let secret = token::generate_token().map_err(|_| internal())?;
    let invite = superhet_db::users::create_invite(
        db,
        &token::hash_token(&secret),
        role,
        req.note.as_deref(),
        match me.actor {
            crate::access::Actor::User(id) => id,
            crate::access::Actor::Guest(_) => return Err(internal()),
        },
        &me.name,
        Duration::from_secs(hours * 3600),
    )
    .await
    .map_err(db_failed)?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "invite.create",
            Some(&format!("invite/{}", invite.id)),
            Outcome::Ok,
            json!({"role": role.as_str(), "expires_in_hours": hours}),
        )
        .await;
    Ok(json_reply(
        StatusCode::CREATED,
        &json!({
            "invite": invite_json(&invite),
            "token": secret,
            "path": format!("/invite/{secret}"),
        }),
    ))
}

pub(super) async fn revoke_invite(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    flatten(revoke_invite_inner(&state, &me, &id, &headers).await)
}

async fn revoke_invite_inner(
    state: &AdminState,
    me: &Principal,
    id: &str,
    headers: &HeaderMap,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let db = state.auth.db()?;
    let id = parse_id(id)?;
    if !superhet_db::users::revoke_invite(db, id)
        .await
        .map_err(db_failed)?
    {
        return Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            Some("no open invite with that id"),
        ));
    }
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "invite.revoke",
            Some(&format!("invite/{id}")),
            Outcome::Ok,
            json!({}),
        )
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}
