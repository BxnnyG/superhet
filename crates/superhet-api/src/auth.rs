//! Authentication hook for protected routes.
//!
//! TODO(P1 local auth): validate the session cookie (server-side session in PostgreSQL, see
//! docs/ARCHITECTURE.md section 7) and answer 401 without a valid session. Until the local
//! auth item lands this is a pass-through, so every protected route is already wired behind
//! the one place that has to change. Do not expose the API publicly before then.

use axum::{extract::Request, middleware::Next, response::Response};

/// Middleware guarding authenticated routes (currently a pass-through, see module docs).
pub async fn require_session(request: Request, next: Next) -> Response {
    next.run(request).await
}
