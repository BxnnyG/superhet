//! Guest policy, audit settings and VFO limits (`/api/v1/admin/{guest-policy,audit-settings,limits}`).

use axum::{
    Extension,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;

use super::{AdminState, bad};
use crate::access::Principal;
use crate::auth::{Reply, db_failed, error, flatten, guard_origin, json_reply, parse_json};
use crate::settings::{AuditSettings, ClientReportPolicy, GuestPolicy, Limits, SettingsError};

fn store_failed(err: SettingsError) -> Response {
    match err {
        SettingsError::Unavailable => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable", None),
        SettingsError::Db(err) => db_failed(err),
    }
}

/// Top-level keys whose values differ between two settings documents.
fn changed_keys(old: &Value, new: &Value) -> Vec<String> {
    let (Some(old), Some(new)) = (old.as_object(), new.as_object()) else {
        return Vec::new();
    };
    new.iter()
        .filter(|(k, v)| old.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect()
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

pub(super) async fn get_guest_policy(State(state): State<AdminState>) -> Response {
    json_reply(
        StatusCode::OK,
        &json!({"guest_policy": to_value(&state.auth.settings().guest_policy().await)}),
    )
}

pub(super) async fn put_guest_policy(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(put_guest_policy_inner(&state, &me, &headers, &body).await)
}

async fn put_guest_policy_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let new: GuestPolicy = parse_json(headers, body)?;
    new.validate().map_err(|m| bad(&m))?;
    let old = state.auth.settings().guest_policy().await;
    state
        .auth
        .settings()
        .put_guest_policy(&new, &me.name)
        .await
        .map_err(store_failed)?;
    let changed = changed_keys(&to_value(&old), &to_value(&new));
    if old.enabled && !new.enabled {
        // Switching guest mode off ends every guest session (VFOs are cleaned up by the
        // maintenance task, sockets close at once).
        state.auth.registry().guests_clear();
    }
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "guest.policy_change",
            None,
            Outcome::Ok,
            json!({"changed": changed, "enabled": new.enabled}),
        )
        .await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"guest_policy": to_value(&new)}),
    ))
}

pub(super) async fn get_audit_settings(State(state): State<AdminState>) -> Response {
    flatten(get_audit_settings_inner(&state).await)
}

async fn get_audit_settings_inner(state: &AdminState) -> Reply {
    let db = state.auth.db()?;
    let settings = state.auth.settings().audit().await;
    let stats = superhet_db::audit::stats(db).await.map_err(db_failed)?;
    Ok(json_reply(
        StatusCode::OK,
        &json!({
            "audit_settings": to_value(&settings),
            "stats": {"count": stats.count, "oldest_ms": stats.oldest_ms, "bytes": stats.bytes},
        }),
    ))
}

pub(super) async fn put_audit_settings(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(put_audit_settings_inner(&state, &me, &headers, &body).await)
}

async fn put_audit_settings_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let new: AuditSettings = parse_json(headers, body)?;
    new.validate().map_err(|m| bad(&m))?;
    let old = state.auth.settings().audit().await;
    state
        .auth
        .settings()
        .put_audit(&new, &me.name)
        .await
        .map_err(store_failed)?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "settings.change",
            Some("audit"),
            Outcome::Ok,
            json!({"changed": changed_keys(&to_value(&old), &to_value(&new))}),
        )
        .await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"audit_settings": to_value(&new)}),
    ))
}

pub(super) async fn get_limits(State(state): State<AdminState>) -> Response {
    json_reply(
        StatusCode::OK,
        &json!({"limits": to_value(&state.auth.settings().limits().await)}),
    )
}

pub(super) async fn put_limits(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(put_limits_inner(&state, &me, &headers, &body).await)
}

async fn put_limits_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let new: Limits = parse_json(headers, body)?;
    new.validate().map_err(|m| bad(&m))?;
    let old = state.auth.settings().limits().await;
    state
        .auth
        .settings()
        .put_limits(&new, &me.name)
        .await
        .map_err(store_failed)?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "settings.change",
            Some("limits"),
            Outcome::Ok,
            json!({"changed": changed_keys(&to_value(&old), &to_value(&new))}),
        )
        .await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"limits": to_value(&new)}),
    ))
}

pub(super) async fn get_client_report_policy(State(state): State<AdminState>) -> Response {
    json_reply(
        StatusCode::OK,
        &json!({"client_report": to_value(&state.auth.settings().client_report().await)}),
    )
}

pub(super) async fn put_client_report_policy(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(put_client_report_policy_inner(&state, &me, &headers, &body).await)
}

async fn put_client_report_policy_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let mut new: ClientReportPolicy = parse_json(headers, body)?;
    new.public_ip_service = new.public_ip_service.trim().to_owned();
    new.validate().map_err(|m| bad(&m))?;
    let old = state.auth.settings().client_report().await;
    state
        .auth
        .settings()
        .put_client_report(&new, &me.name)
        .await
        .map_err(store_failed)?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "settings.change",
            Some("client_report"),
            Outcome::Ok,
            json!({"changed": changed_keys(&to_value(&old), &to_value(&new))}),
        )
        .await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"client_report": to_value(&new)}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_keys_lists_differences() {
        let a = json!({"x": 1, "y": [1], "z": "s"});
        let b = json!({"x": 1, "y": [1, 2], "z": "t"});
        let mut keys = changed_keys(&a, &b);
        keys.sort();
        assert_eq!(keys, ["y", "z"]);
        assert!(changed_keys(&a, &a).is_empty());
        assert!(changed_keys(&json!(1), &a).is_empty());
    }
}
