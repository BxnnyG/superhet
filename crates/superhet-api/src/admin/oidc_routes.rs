//! OIDC settings (`/api/v1/admin/oidc`, `/api/v1/admin/oidc/test`), docs/api-oidc.md.

use axum::{
    Extension,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;

use super::settings_routes::{changed_keys, store_failed, to_value};
use super::{AdminState, bad, conflict};
use crate::access::Principal;
use crate::auth::oidc::{OidcSettings, Source, client::Provider};
use crate::auth::{Reply, db_failed, flatten, guard_origin, json_reply, parse_json};

/// The settings document with everything the admin page shows next to it.
async fn view(state: &AdminState) -> Result<Value, Response> {
    let auth = &state.auth;
    let (settings, source) = auth.oidc_settings().await;
    let env = &auth.oidc().env;
    let oidc_admin = match auth.db() {
        Ok(db) => superhet_db::oidc::has_active_oidc_admin(db)
            .await
            .map_err(db_failed)?,
        Err(_) => false,
    };
    Ok(json!({
        "oidc": to_value(&settings),
        "source": source,
        "redirect_url": settings.redirect_url().ok(),
        "client_secret_set": env.client_secret.is_some(),
        "oidc_admin_exists": oidc_admin,
        "local_login_active": auth.local_login_allowed().await,
        "keep_local_login": env.keep_local_login,
    }))
}

pub(super) async fn get(State(state): State<AdminState>) -> Response {
    flatten(get_inner(&state).await)
}

async fn get_inner(state: &AdminState) -> Reply {
    Ok(json_reply(StatusCode::OK, &view(state).await?))
}

pub(super) async fn put(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(put_inner(&state, &me, &headers, &body).await)
}

async fn put_inner(state: &AdminState, me: &Principal, headers: &HeaderMap, body: &Bytes) -> Reply {
    guard_origin(&state.auth, headers)?;
    let db = state.auth.db()?;
    let (old, source) = state.auth.oidc_settings().await;
    if source == Source::Env {
        return Err(conflict(
            "managed_by_env",
            "OIDC is configured through SUPERHET_OIDC_* environment variables",
        ));
    }
    let new: OidcSettings = parse_json(headers, body)?;
    new.validate(state.auth.oidc().env.allow_http)
        .map_err(|m| bad(&m))?;
    // Password logins may only go away while somebody can still sign in as admin through OIDC.
    let switching_off = new.enabled && !new.local_login;
    if switching_off
        && !superhet_db::oidc::has_active_oidc_admin(db)
            .await
            .map_err(db_failed)?
    {
        return Err(conflict(
            "no_oidc_admin",
            "link at least one enabled admin account to an OIDC identity before switching \
             password logins off",
        ));
    }
    state
        .auth
        .settings()
        .put_oidc(&new, &me.name)
        .await
        .map_err(store_failed)?;
    state.auth.oidc().providers.clear().await;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "settings.change",
            Some("oidc"),
            Outcome::Ok,
            json!({"changed": changed_keys(&to_value(&old), &to_value(&new)), "enabled": new.enabled}),
        )
        .await;
    Ok(json_reply(StatusCode::OK, &view(state).await?))
}

/// Run discovery against the effective settings, enabled or not, and report what the provider
/// says. Provider problems are an answer (`ok: false`), not an error status.
pub(super) async fn test(State(state): State<AdminState>) -> Response {
    flatten(test_inner(&state).await)
}

async fn test_inner(state: &AdminState) -> Reply {
    let auth = &state.auth;
    let (settings, _) = auth.oidc_settings().await;
    let env = &auth.oidc().env;
    settings.validate(env.allow_http).map_err(|m| bad(&m))?;
    if settings.issuer.trim().is_empty() {
        return Err(bad("issuer is not set"));
    }
    let result = Provider::discover(
        auth.oidc().providers.http(),
        &settings,
        env.client_secret.as_ref(),
        env.allow_http,
    )
    .await;
    let body = match result {
        Ok(provider) => json!({"ok": true, "issuer": provider.issuer()}),
        Err(err) => json!({"ok": false, "error": err.to_string()}),
    };
    Ok(json_reply(StatusCode::OK, &body))
}
