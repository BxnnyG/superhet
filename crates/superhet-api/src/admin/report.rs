//! `POST /api/v1/auth/client-report`: the browser tells the server about itself once per page
//! load, when the admin policy asks for it (`ClientReportPolicy`). The data is stored with the
//! session (accounts: `sessions.client_report`; guests: in memory with the guest session) and
//! shown to admins marked as client-reported and unverified. It never feeds bans, rate limits or
//! access decisions.

use axum::{
    Extension,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::de::IgnoredAny;
use serde_json::json;
use superhet_bus::audit::Outcome;

use super::AdminState;
use crate::access::{Actor, Principal};
use crate::auth::{Reply, db_failed, error, flatten, guard_origin, parse_json, token};
use crate::clientinfo::report::{ClientReport, present};
use crate::registry::now_ms;
use crate::settings::IpMode;

pub(super) async fn submit(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(submit_inner(&state, &me, &headers, &body).await)
}

async fn submit_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let _shape: IgnoredAny = parse_json(headers, body)?;
    let guest = me.is_guest();
    let policy = state.auth.settings().client_report().await;
    if !policy.collects(guest) {
        return Err(error(
            StatusCode::CONFLICT,
            "disabled",
            Some("client reports are not requested on this server"),
        ));
    }
    let mut report = ClientReport::parse(body)
        .map_err(|m| error(StatusCode::BAD_REQUEST, "invalid", Some(&m)))?;
    report.apply_policy(policy.asks_public_ip(guest));
    if report.is_empty() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let stored = report.to_stored(now_ms());
    let found = match &me.actor {
        Actor::User(id) => {
            let key = token::unhex(&me.session_key)
                .filter(|b| b.len() == 32)
                .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", None))?;
            superhet_db::users::set_session_client_report(state.auth.db()?, &key, *id, &stored)
                .await
                .map_err(db_failed)?
        }
        Actor::Guest(id) => state.auth.registry().guest_set_report(id, stored.clone()),
    };
    if !found {
        return Err(error(StatusCode::NOT_FOUND, "not_found", None));
    }
    let audit = state.auth.settings().audit().await;
    let mode: IpMode = if guest {
        audit.ip_mode_guest
    } else {
        audit.ip_mode_user
    };
    // The audit row carries the report (public address in the actor class's address mode), so
    // it is still readable after the session is gone, for as long as audit retention allows.
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "session.client_report",
            None,
            Outcome::Ok,
            json!({"client": present(&stored, mode)}),
        )
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}
