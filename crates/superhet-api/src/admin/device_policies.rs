//! Per-device access policies (`/api/v1/admin/device-policies`, ADR-0023).

use axum::{
    Extension,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;

use super::settings_routes::{changed_keys, store_failed, to_value};
use super::{AdminState, bad};
use crate::access::Principal;
use crate::auth::{Reply, db_failed, flatten, guard_origin, json_reply, parse_json};
use crate::device_access::{DevicePolicy, Need, Subject};

/// A policy as the API shows it: the device id next to the policy fields.
fn view(device: &str, policy: &DevicePolicy) -> Value {
    let mut value = to_value(policy);
    value["device"] = json!(device);
    value
}

fn check_device(raw: &str) -> Result<(), Response> {
    if superhet_bus::subjects::state_key(raw).is_ok() && raw.len() <= 64 {
        Ok(())
    } else {
        Err(bad("invalid device id"))
    }
}

pub(super) async fn list(State(state): State<AdminState>) -> Response {
    flatten(list_inner(&state).await)
}

async fn list_inner(state: &AdminState) -> Reply {
    let table = state
        .auth
        .device_policies()
        .table()
        .await
        .map_err(|_| crate::auth::unavailable())?;
    let mut policies: Vec<Value> = table.iter().map(|(d, p)| view(d, p)).collect();
    policies.sort_by(|a, b| a["device"].as_str().cmp(&b["device"].as_str()));
    Ok(json_reply(
        StatusCode::OK,
        &json!({"v": 1, "policies": policies}),
    ))
}

/// End the sockets of sessions that lost the right to see `device`; they reconnect and are
/// refused by the access check. Returns how many sessions were ended.
fn drop_unwelcome(state: &AdminState, device: &str, policy: &DevicePolicy) -> usize {
    let registry = state.auth.registry();
    let mut ended = 0;
    for (session, who) in registry.sessions_on(device) {
        if !policy.allows(&Subject::from_who(&who), Need::View) {
            registry.kick(&session);
            ended += 1;
        }
    }
    ended
}

pub(super) async fn put(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(device): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(put_inner(&state, &me, &device, &headers, &body).await)
}

async fn put_inner(
    state: &AdminState,
    me: &Principal,
    device: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    check_device(device)?;
    let mut new: DevicePolicy = parse_json(headers, body)?;
    new.normalise().map_err(|m| bad(&m))?;
    // A typo in an account id would silently lock the intended person out: name unknown ids.
    if !new.allowed_users.is_empty() {
        let known = superhet_db::users::list(state.auth.db()?)
            .await
            .map_err(db_failed)?;
        if let Some(unknown) = new
            .allowed_users
            .iter()
            .find(|id| !known.iter().any(|u| u.id == **id))
        {
            return Err(bad(&format!("allowed_users: no account with id {unknown}")));
        }
    }
    let policies = state.auth.device_policies();
    let old = policies
        .policy(device)
        .await
        .map_err(|_| crate::auth::unavailable())?;
    policies
        .put(device, &new, &me.name)
        .await
        .map_err(|e| store_failed(crate::settings::SettingsError::Db(e)))?;
    let ended = drop_unwelcome(state, device, &new);
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "device.policy_change",
            Some(device),
            Outcome::Ok,
            json!({
                "changed": changed_keys(&to_value(&old), &to_value(&new)),
                "view_role": new.view_role.as_str(),
                "control_role": new.control_role.as_str(),
                "guests_visible": new.guests_visible,
                "allowed_users": new.allowed_users.len(),
                "ended_sessions": ended,
            }),
        )
        .await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"v": 1, "policy": view(device, &new), "ended_sessions": ended}),
    ))
}

pub(super) async fn reset(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(device): Path<String>,
    headers: HeaderMap,
) -> Response {
    flatten(reset_inner(&state, &me, &device, &headers).await)
}

async fn reset_inner(
    state: &AdminState,
    me: &Principal,
    device: &str,
    headers: &HeaderMap,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    check_device(device)?;
    let existed = state
        .auth
        .device_policies()
        .delete(device)
        .await
        .map_err(|e| store_failed(crate::settings::SettingsError::Db(e)))?;
    if existed {
        state
            .auth
            .audit()
            .log(
                &me.who(),
                "device.policy_reset",
                Some(device),
                Outcome::Ok,
                json!({}),
            )
            .await;
    }
    Ok(json_reply(
        StatusCode::OK,
        &json!({"v": 1, "policy": view(device, &DevicePolicy::default()), "was_set": existed}),
    ))
}
