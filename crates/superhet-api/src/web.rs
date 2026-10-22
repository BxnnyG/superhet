//! Static web UI (the built `web/dist`), served without authentication: the files are public,
//! the data behind `/api/v1/*` is not. It runs as an outermost layer for GET/HEAD requests, so
//! the API, the health probes and the auth middleware never see UI paths, and unknown API
//! paths keep their JSON errors.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

/// Default directory of the built UI; override with `SUPERHET_WEB_DIR`.
pub const DEFAULT_WEB_DIR: &str = "/usr/share/superhet/web";

/// Same-origin only; `connect-src 'self'` covers the same-origin WebSockets. Styles need
/// `'unsafe-inline'` because React sets `style` attributes; the AudioWorklet module is a
/// same-origin file under `/assets/`.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; connect-src 'self'; worker-src 'self' blob:; object-src 'none'; \
base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

#[derive(Clone)]
pub enum Web {
    /// UI available in this directory.
    Dir(Arc<Site>),
    /// No UI installed: non-API paths answer a short 404 hint.
    Missing(PathBuf),
}

pub struct Site {
    /// Files only: a miss is a plain 404 (used for `/assets/*` and paths with an extension).
    files: ServeDir,
    /// Files, falling back to `index.html` (client-side routes).
    spa: ServeDir<ServeFile>,
}

impl Web {
    /// `SUPERHET_WEB_DIR` or the default; logs a warning once if there is no `index.html`.
    pub fn from_env() -> Self {
        let dir = std::env::var_os("SUPERHET_WEB_DIR")
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_WEB_DIR), PathBuf::from);
        let web = Self::new(&dir);
        if matches!(web, Self::Missing(_)) {
            tracing::warn!(
                dir = %dir.display(),
                "web UI not found (no index.html); set SUPERHET_WEB_DIR - only the API is served"
            );
        }
        web
    }

    pub fn new(dir: &Path) -> Self {
        let index = dir.join("index.html");
        if !index.is_file() {
            return Self::Missing(dir.to_path_buf());
        }
        Self::Dir(Arc::new(Site {
            files: ServeDir::new(dir).append_index_html_on_directories(false),
            spa: ServeDir::new(dir)
                .append_index_html_on_directories(false)
                .fallback(ServeFile::new(index)),
        }))
    }
}

/// Paths that belong to the backend and are never answered with UI files.
fn is_backend_path(path: &str) -> bool {
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    matches!(first, "api" | "healthz" | "readyz")
}

/// Does the last path segment look like a file (`/favicon.ico`)? Those must 404 when missing
/// instead of returning `index.html`.
fn looks_like_file(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|s| s.contains('.'))
}

pub async fn layer(State(web): State<Web>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    if !matches!(*req.method(), Method::GET | Method::HEAD) || is_backend_path(&path) {
        return next.run(req).await;
    }
    let site = match web {
        Web::Dir(site) => site,
        Web::Missing(dir) => {
            return (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                format!(
                    "Superhet web UI not installed (no index.html in {}); set SUPERHET_WEB_DIR.\n",
                    dir.display()
                ),
            )
                .into_response();
        }
    };
    let assets = path.starts_with("/assets/");
    let (parts, _) = req.into_parts();
    let req = Request::from_parts(parts, Body::empty());
    let result: Result<Response, Infallible> = if assets || looks_like_file(&path) {
        site.files
            .clone()
            .oneshot(req)
            .await
            .map(IntoResponse::into_response)
    } else {
        site.spa
            .clone()
            .oneshot(req)
            .await
            .map(IntoResponse::into_response)
    };
    let mut response = match result {
        Ok(response) => response,
        Err(err) => match err {},
    };
    decorate(&mut response, assets);
    response
}

fn decorate(response: &mut Response, assets: bool) {
    let status = response.status();
    let headers = response.headers_mut();
    if assets && (status.is_success() || status == StatusCode::NOT_MODIFIED) {
        // Vite content-hashes everything under /assets.
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    let is_html = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if is_html {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        );
    }
}
