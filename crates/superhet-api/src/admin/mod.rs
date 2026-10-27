//! Administration API under `/api/v1/admin` (role `admin`) plus the self-service session list
//! under `/api/v1/me/sessions`. Contract: `docs/api-users.md`, `docs/api-audit.md`. Role checks
//! happen in the access middleware (`access::RULES`); the handlers only deal with the request.

// Handlers use `Result<_, Response>` so `?` short-circuits straight into the HTTP answer.
#![allow(clippy::result_large_err)]

mod audit_routes;
mod bans;
mod report;
mod sessions;
mod settings_routes;
mod users;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    response::Response,
    routing::{delete, get, post},
};

use crate::auth::{AuthState, error};
use crate::devices::DevicesState;

/// Request bodies are small JSON documents.
const MAX_BODY_BYTES: usize = 16 * 1024;

/// Shared state of the admin routes.
#[derive(Clone)]
pub struct AdminState {
    pub(crate) auth: AuthState,
    pub(crate) devices: DevicesState,
}

impl AdminState {
    /// Create the state from the auth layer and the device routes' state (for VFO lookups).
    #[must_use]
    pub fn new(auth: AuthState, devices: DevicesState) -> Self {
        Self { auth, devices }
    }
}

/// Routes of the admin API, behind the access middleware.
pub fn router(state: AdminState) -> Router {
    let limit = DefaultBodyLimit::max(MAX_BODY_BYTES);
    Router::new()
        .route(
            "/api/v1/admin/users",
            get(users::list).post(users::create).layer(limit),
        )
        .route(
            "/api/v1/admin/users/{id}",
            get(users::get)
                .patch(users::update)
                .delete(users::remove)
                .layer(limit),
        )
        .route(
            "/api/v1/admin/users/{id}/password",
            post(users::reset_password).layer(limit),
        )
        .route(
            "/api/v1/admin/invites",
            get(users::list_invites)
                .post(users::create_invite)
                .layer(limit),
        )
        .route("/api/v1/admin/invites/{id}", delete(users::revoke_invite))
        .route("/api/v1/admin/sessions", get(sessions::list))
        .route("/api/v1/admin/sessions/{sid}", delete(sessions::kick))
        .route(
            "/api/v1/admin/bans",
            get(bans::list).post(bans::create).layer(limit),
        )
        .route("/api/v1/admin/bans/{id}", delete(bans::lift))
        .route(
            "/api/v1/auth/client-report",
            post(report::submit).layer(DefaultBodyLimit::max(
                crate::clientinfo::report::MAX_BODY_BYTES,
            )),
        )
        .route(
            "/api/v1/admin/client-report-policy",
            get(settings_routes::get_client_report_policy)
                .put(settings_routes::put_client_report_policy)
                .layer(limit),
        )
        .route("/api/v1/me/sessions", get(sessions::mine))
        .route("/api/v1/me/sessions/{sid}", delete(sessions::revoke_mine))
        .route("/api/v1/admin/audit", get(audit_routes::query))
        .route("/api/v1/admin/audit/export", get(audit_routes::export))
        .route(
            "/api/v1/admin/guest-policy",
            get(settings_routes::get_guest_policy)
                .put(settings_routes::put_guest_policy)
                .layer(limit),
        )
        .route(
            "/api/v1/admin/audit-settings",
            get(settings_routes::get_audit_settings)
                .put(settings_routes::put_audit_settings)
                .layer(limit),
        )
        .route(
            "/api/v1/admin/limits",
            get(settings_routes::get_limits)
                .put(settings_routes::put_limits)
                .layer(limit),
        )
        .with_state(state)
}

pub(crate) fn bad(message: &str) -> Response {
    error(StatusCode::BAD_REQUEST, "invalid", Some(message))
}

pub(crate) fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", None)
}

pub(crate) fn conflict(code: &str, message: &str) -> Response {
    error(StatusCode::CONFLICT, code, Some(message))
}
