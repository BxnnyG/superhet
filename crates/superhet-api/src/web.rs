//! Static web UI (the built `web/dist`), served without authentication: the files are public,
//! the data behind `/api/v1/*` is not. It runs as an outermost layer for GET/HEAD requests, so
//! the API, the health probes and the auth middleware never see UI paths, and unknown API
//! paths keep their JSON errors.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower::ServiceExt;

use crate::share::links::ShareLinks;
use tower_http::services::{ServeDir, ServeFile};

/// Default directory of the built UI; override with `SUPERHET_WEB_DIR`.
pub const DEFAULT_WEB_DIR: &str = "/usr/share/superhet/web";

/// Same-origin only; `connect-src 'self'` covers the same-origin WebSockets. Styles need
/// `'unsafe-inline'` because React sets `style` attributes; map tiles of the ADS-B view come from
/// the caching tile proxy of this server (`tiles`, no extra origin); only with
/// `SUPERHET_MAP_TILE_MODE=direct` the tile server's origin (OpenStreetMap unless
/// `SUPERHET_MAP_TILE_URL` says otherwise) replaces the one in `img-src`, ADR-0017; the AudioWorklet module is a same-origin file
/// under `/assets/`.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; connect-src 'self'; worker-src 'self' blob:; object-src 'none'; \
base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

/// The one extra origin the page may fetch from: the public-address lookup service, while the
/// admin asks clients for their public address (`ClientReportPolicy`). Process-wide because the
/// static file layer has no access to the settings.
static LOOKUP_ORIGIN: RwLock<Option<String>> = RwLock::new(None);

/// Allow (or stop allowing) the page to connect to `origin` (`https://host[:port]`).
pub fn set_lookup_origin(origin: Option<String>) {
    *LOOKUP_ORIGIN
        .write()
        .unwrap_or_else(PoisonError::into_inner) = origin;
}

/// The CSP with the configured tile origin in `img-src` and the lookup origin added to
/// `connect-src`, when there is one.
#[cfg(test)]
fn csp_header() -> HeaderValue {
    csp_framed_by(&[])
}

/// An origin that is safe to put into `frame-ancestors` (a link's list was validated when it was
/// saved; this refuses anything that could end the directive anyway).
fn frameable(origin: &str) -> bool {
    !origin.is_empty()
        && !origin.contains([';', ',', '\'', '"', ' '])
        && (!origin.contains('*') || origin.contains("://*."))
}

/// [`csp_header`] with `frame-ancestors` limited to `framers` (none = `'none'`).
fn csp_framed_by(framers: &[String]) -> HeaderValue {
    // Proxy mode (default): tiles come from our own origin, `img-src` needs nothing extra.
    let csp = match crate::map::tile_origin() {
        Some(tiles) => CSP.replace("blob:; connect-src", &format!("blob: {tiles}; connect-src")),
        None => CSP.to_owned(),
    };
    let origin = LOOKUP_ORIGIN
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let csp = match origin {
        // The origin comes from a validated https URL; refuse anything that could end the
        // directive or smuggle another one.
        Some(o) if !o.contains([';', ',', '\'', '"', ' ']) => {
            csp.replace("connect-src 'self';", &format!("connect-src 'self' {o};"))
        }
        _ => csp,
    };
    let framers: Vec<&str> = framers
        .iter()
        .map(String::as_str)
        .filter(|o| frameable(o))
        .collect();
    let csp = if framers.is_empty() {
        csp
    } else {
        csp.replace(
            "frame-ancestors 'none'",
            &format!("frame-ancestors {}", framers.join(" ")),
        )
    };
    HeaderValue::from_str(&csp).unwrap_or_else(|_| HeaderValue::from_static(CSP))
}

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
    /// Share links, for the `frame-ancestors` list of `/embed/<token>`.
    shares: Option<Arc<ShareLinks>>,
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
            shares: None,
        }))
    }

    /// Let `/embed/<token>` pages be framed by the origins their share link allows.
    #[must_use]
    pub fn with_shares(self, shares: Arc<ShareLinks>) -> Self {
        match self {
            Self::Dir(site) => Self::Dir(Arc::new(Site {
                files: site.files.clone(),
                spa: site.spa.clone(),
                shares: Some(shares),
            })),
            missing => missing,
        }
    }
}

/// The token of an embed page `/embed/<token>`.
fn embed_token(path: &str) -> Option<&str> {
    let token = path.strip_prefix("/embed/")?.trim_end_matches('/');
    (!token.is_empty() && !token.contains('/')).then_some(token)
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
    // Only an embed page whose link is active and lists origins may be framed, and only by
    // those; every other page (the public page `/r/<token>` included) stays unframeable.
    let framers = match (&site.shares, embed_token(&path)) {
        (Some(shares), Some(token)) => {
            shares
                .embed_origins(&crate::auth::token::hash_token(token))
                .await
        }
        _ => Vec::new(),
    };
    decorate(&mut response, assets, &framers);
    response
}

fn decorate(response: &mut Response, assets: bool, framers: &[String]) {
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
        headers.insert(header::CONTENT_SECURITY_POLICY, csp_framed_by(framers));
        // `X-Frame-Options` cannot name origins: it is only left off where the CSP does.
        if framers.is_empty() {
            headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        }
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_csp_gains_the_lookup_origin_only_while_one_is_set() {
        // One test owns the process-wide value so parallel tests cannot interleave.
        set_lookup_origin(None);
        assert_eq!(csp_header().to_str().unwrap_or_default(), CSP);
        set_lookup_origin(Some("https://api.ipify.org".into()));
        let csp = csp_header();
        let csp = csp.to_str().unwrap_or_default();
        assert!(
            csp.contains("connect-src 'self' https://api.ipify.org;"),
            "{csp}"
        );
        assert!(csp.contains("default-src 'self'"), "{csp}");
        // A value that could end the directive is refused rather than injected.
        set_lookup_origin(Some("https://x.example; script-src *".into()));
        assert_eq!(csp_header().to_str().unwrap_or_default(), CSP);
        set_lookup_origin(None);
        the_csp_img_src_follows_the_configured_tile_server();
    }

    fn html() -> Response {
        let mut response = Response::new(Body::empty());
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        response
    }

    fn header_of(response: &Response, name: header::HeaderName) -> String {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    fn pages_are_unframeable_unless_a_link_lists_origins() {
        let mut plain = html();
        decorate(&mut plain, false, &[]);
        assert!(
            header_of(&plain, header::CONTENT_SECURITY_POLICY).contains("frame-ancestors 'none'")
        );
        assert_eq!(header_of(&plain, header::X_FRAME_OPTIONS), "DENY");

        let mut embed = html();
        let origins = vec![
            "https://blog.example.org".to_owned(),
            "https://*.example.net".to_owned(),
        ];
        decorate(&mut embed, false, &origins);
        let csp = header_of(&embed, header::CONTENT_SECURITY_POLICY);
        assert!(
            csp.contains("frame-ancestors https://blog.example.org https://*.example.net"),
            "{csp}"
        );
        assert!(!csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(embed.headers().get(header::X_FRAME_OPTIONS).is_none());
        // Everything else in the policy stays as strict as on every page.
        assert!(csp.contains("default-src 'self'") && csp.contains("script-src 'self'"));
    }

    #[test]
    fn a_framing_origin_cannot_smuggle_directives() {
        for evil in [
            "https://x.example; script-src *",
            "https://x.example, https://y.example",
            "*",
            "https://a b",
            "",
        ] {
            assert!(!frameable(evil), "{evil}");
            let mut response = html();
            decorate(&mut response, false, &[evil.to_owned()]);
            let csp = header_of(&response, header::CONTENT_SECURITY_POLICY);
            assert!(csp.contains("frame-ancestors 'none'"), "{evil}: {csp}");
            assert!(!csp.contains("script-src *"), "{evil}: {csp}");
        }
    }

    #[test]
    fn embed_paths_are_recognised_exactly() {
        assert_eq!(embed_token("/embed/abc"), Some("abc"));
        assert_eq!(embed_token("/embed/abc/"), Some("abc"));
        assert_eq!(embed_token("/embed/"), None);
        assert_eq!(embed_token("/embed/a/b"), None);
        assert_eq!(embed_token("/r/abc"), None);
        assert_eq!(embed_token("/embedded/abc"), None);
    }

    async fn page(web: Web, path: &str) -> Response {
        use axum::{Router, middleware};
        use tower::ServiceExt;
        Router::new()
            .layer(middleware::from_fn_with_state(web, layer))
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible")
    }

    #[tokio::test]
    async fn only_the_embed_page_of_an_active_link_may_be_framed_and_only_by_its_origins() {
        use crate::share::links::{ShareLinks, test_link};
        let dir = std::env::temp_dir().join(format!("superhet-web-csp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("index.html"), "<!doctype html><title>x</title>").expect("index");
        let mut open = test_link(1);
        open.embed_origins = vec!["https://blog.example.org".into()];
        let closed = test_link(2); // embeddable by nobody
        let mut revoked = test_link(3);
        revoked.embed_origins = vec!["https://blog.example.org".into()];
        revoked.revoked_at_ms = Some(1);
        let shares = Arc::new(ShareLinks::for_tests(vec![
            ("open", open),
            ("closed", closed),
            ("revoked", revoked),
        ]));
        let web = Web::new(&dir).with_shares(shares);
        let csp_of = |r: &Response| header_of(r, header::CONTENT_SECURITY_POLICY);

        let r = page(web.clone(), "/embed/open").await;
        assert!(
            csp_of(&r).contains("frame-ancestors https://blog.example.org"),
            "{}",
            csp_of(&r)
        );
        assert!(r.headers().get(header::X_FRAME_OPTIONS).is_none());
        for denied in [
            "/embed/closed",
            "/embed/revoked",
            "/embed/unknown",
            "/r/open",
            "/",
            "/admin",
        ] {
            let r = page(web.clone(), denied).await;
            assert!(
                csp_of(&r).contains("frame-ancestors 'none'"),
                "{denied}: {}",
                csp_of(&r)
            );
            assert_eq!(header_of(&r, header::X_FRAME_OPTIONS), "DENY", "{denied}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Part of the test above: both change process-wide values that `csp_header` reads.
    fn the_csp_img_src_follows_the_configured_tile_server() {
        crate::map::set_for_test(None);
        let csp = csp_header();
        let csp = csp.to_str().unwrap_or_default();
        assert!(csp.contains("img-src 'self' data: blob:;"), "{csp}");
        assert!(!csp.contains("openstreetmap"), "{csp}");
        let direct = crate::map::MapConfig {
            mode: crate::map::TileMode::Direct,
            ..Default::default()
        };
        crate::map::set_for_test(Some(direct.clone()));
        let csp = csp_header();
        assert!(
            csp.to_str()
                .unwrap_or_default()
                .contains("https://tile.openstreetmap.org;")
        );
        let custom = crate::map::MapConfig {
            tile_url: "http://tiles.lan:8080/{z}/{x}/{y}.png".into(),
            ..direct
        };
        crate::map::set_for_test(Some(custom));
        let csp = csp_header();
        let csp = csp.to_str().unwrap_or_default();
        assert!(
            csp.contains("img-src 'self' data: blob: http://tiles.lan:8080;"),
            "{csp}"
        );
        assert!(!csp.contains("openstreetmap"), "{csp}");
        crate::map::set_for_test(None);
    }
}
