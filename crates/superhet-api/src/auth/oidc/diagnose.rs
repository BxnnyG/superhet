//! The admin "test connection": run discovery against the saved settings and, when it fails,
//! explain why and look for the issuer the admin probably meant. The look-up asks only a small
//! fixed set of candidates on the same origin ([`hints::candidates`]) through the same HTTP
//! client as every other provider request (no redirects, short timeouts, https unless the
//! issuer itself is plain http), and a candidate is offered only when the document it serves
//! names exactly that candidate as its `issuer`.

use std::time::Duration;

use serde_json::{Value, json};

use super::client::{Http, OidcError, Provider};
use super::config::{OidcSettings, Secret};
use super::hints;

/// The whole candidate search gives up after this long.
const PROBE_BUDGET: Duration = Duration::from_secs(20);

/// What discovery against the configured issuer came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    /// Discovery worked.
    pub ok: bool,
    /// The issuer the provider announced.
    pub issuer: Option<String>,
    /// The provider advertises an end session endpoint.
    pub end_session: bool,
    /// Why it did not work.
    pub error: Option<String>,
    /// The provider's HTTP status, if it answered.
    pub status: Option<u16>,
    /// The discovery URL that was requested.
    pub url: Option<String>,
    /// Explanation when the issuer looks like an endpoint URL.
    pub hint: Option<String>,
    /// An issuer that works: the provider's own document confirmed it.
    pub suggested_issuer: Option<String>,
}

impl Diagnosis {
    fn failed(error: String) -> Self {
        Self {
            ok: false,
            issuer: None,
            end_session: false,
            error: Some(error),
            status: None,
            url: None,
            hint: None,
            suggested_issuer: None,
        }
    }

    /// The admin API shape (absent fields are left out).
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut out = json!({"ok": self.ok});
        let fields = [
            ("issuer", self.issuer.as_ref()),
            ("error", self.error.as_ref()),
            ("url", self.url.as_ref()),
            ("hint", self.hint.as_ref()),
            ("suggested_issuer", self.suggested_issuer.as_ref()),
        ];
        for (name, value) in fields {
            if let Some(v) = value {
                out[name] = json!(v);
            }
        }
        if self.ok {
            out["end_session"] = json!(self.end_session);
        }
        if let Some(status) = self.status {
            out["status"] = json!(status);
        }
        out
    }
}

/// Discover the provider for `settings` and, on failure, collect the explanation.
pub async fn diagnose(
    http: &Http,
    settings: &OidcSettings,
    secret: Option<&Secret>,
    allow_http: bool,
) -> Diagnosis {
    let failure = match Provider::discover(http, settings, secret, allow_http).await {
        Ok(provider) => {
            return Diagnosis {
                ok: true,
                issuer: Some(provider.issuer().to_owned()),
                end_session: provider.has_end_session(),
                error: None,
                status: None,
                url: None,
                hint: None,
                suggested_issuer: None,
            };
        }
        Err(OidcError::Discovery(failure)) => failure,
        Err(other) => return Diagnosis::failed(other.to_string()),
    };
    let hint = hints::issuer_hint(&settings.issuer);
    // A provider that cannot be reached at all will not answer for another path either.
    let suggested_issuer = if failure.unreachable {
        None
    } else {
        probe(http, settings, secret, allow_http).await
    };
    Diagnosis {
        ok: false,
        issuer: None,
        end_session: false,
        error: Some(OidcError::Discovery(failure.clone()).to_string()),
        status: failure.status,
        url: failure.url,
        hint: hint.map(|h| h.message),
        suggested_issuer,
    }
}

/// The first candidate whose discovery document names itself as the issuer.
async fn probe(
    http: &Http,
    settings: &OidcSettings,
    secret: Option<&Secret>,
    allow_http: bool,
) -> Option<String> {
    let candidates = hints::candidates(&settings.issuer);
    let search = async {
        for candidate in candidates {
            let mut attempt = settings.clone();
            attempt.issuer.clone_from(&candidate);
            if let Ok(provider) = Provider::discover(http, &attempt, secret, allow_http).await
                && provider.issuer() == candidate
            {
                return Some(candidate);
            }
        }
        None
    };
    tokio::time::timeout(PROBE_BUDGET, search)
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::{StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

    use super::*;

    /// A provider stub on a loopback port: a discovery document under `prefix` (naming
    /// `claimed` as its issuer, by default its own URL), 404 everywhere else.
    struct Stub {
        base: String,
        hits: Arc<AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    #[derive(Clone)]
    struct Doc {
        issuer: String,
        base: String,
        hits: Arc<AtomicUsize>,
    }

    async fn document(State(doc): State<Doc>) -> Response {
        doc.hits.fetch_add(1, Ordering::SeqCst);
        axum::Json(json!({
            "issuer": doc.issuer,
            "authorization_endpoint": format!("{}/authorize", doc.base),
            "token_endpoint": format!("{}/token", doc.base),
            "jwks_uri": format!("{}/jwks", doc.base),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"],
        }))
        .into_response()
    }

    async fn keys() -> Response {
        axum::Json(json!({"keys": []})).into_response()
    }

    async fn not_found(State(doc): State<Doc>) -> Response {
        doc.hits.fetch_add(1, Ordering::SeqCst);
        StatusCode::NOT_FOUND.into_response()
    }

    impl Stub {
        async fn start(prefix: &str, claimed: Option<&str>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let base = format!("http://{}", listener.local_addr().expect("addr"));
            let hits = Arc::new(AtomicUsize::new(0));
            let doc = Doc {
                issuer: claimed.map_or_else(|| format!("{base}{prefix}"), str::to_owned),
                base: base.clone(),
                hits: Arc::clone(&hits),
            };
            let app = Router::new()
                .route(
                    &format!("{prefix}/.well-known/openid-configuration"),
                    get(document),
                )
                .route("/jwks", get(keys))
                .fallback(not_found)
                .with_state(doc);
            let task = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Self { base, hits, task }
        }

        fn requests(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    /// A stub that answers every request with a redirect to `target`.
    async fn redirector(target: String) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let app = Router::new().fallback(move || {
            let target = target.clone();
            async move { (StatusCode::FOUND, [(header::LOCATION, target)]).into_response() }
        });
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (base, task)
    }

    fn settings(issuer: &str) -> OidcSettings {
        OidcSettings {
            issuer: issuer.to_owned(),
            client_id: "superhet".to_owned(),
            public_url: "https://sdr.example.org".to_owned(),
            ..OidcSettings::default()
        }
    }

    async fn run(issuer: &str) -> Diagnosis {
        diagnose(&Http::new(), &settings(issuer), None, false).await
    }

    #[tokio::test]
    async fn a_working_issuer_needs_no_help() {
        let stub = Stub::start("/realms/home", None).await;
        let got = run(&format!("{}/realms/home", stub.base)).await;
        assert!(got.ok, "{got:?}");
        assert_eq!(got.issuer, Some(format!("{}/realms/home", stub.base)));
        assert_eq!(got.suggested_issuer, None);
        assert_eq!(got.to_json()["ok"], true);
    }

    #[tokio::test]
    async fn an_endpoint_url_leads_to_the_confirmed_issuer() {
        let stub = Stub::start("/realms/home", None).await;
        let entered = format!("{}/realms/home/protocol/openid-connect/auth", stub.base);
        let got = run(&entered).await;
        assert!(!got.ok);
        assert_eq!(got.status, Some(404));
        assert_eq!(
            got.url.as_deref(),
            Some(format!("{entered}/.well-known/openid-configuration").as_str())
        );
        assert_eq!(
            got.suggested_issuer,
            Some(format!("{}/realms/home", stub.base))
        );
        assert!(got.hint.as_deref().is_some_and(|h| h.contains("issuer")));
        let json = got.to_json();
        assert_eq!(json["ok"], false);
        assert_eq!(json["status"], 404);
    }

    #[tokio::test]
    async fn a_parent_path_and_a_trailing_slash_are_tried() {
        let stub = Stub::start("", None).await;
        let got = run(&format!("{}/oauth/v2/authorize", stub.base)).await;
        assert_eq!(got.suggested_issuer, Some(stub.base.clone()));

        // The document names the issuer without the slash the admin typed.
        let stub = Stub::start("/application/o/app", None).await;
        let got = run(&format!("{}/application/o/app/", stub.base)).await;
        assert_eq!(
            got.suggested_issuer,
            Some(format!("{}/application/o/app", stub.base))
        );
    }

    #[tokio::test]
    async fn a_document_that_names_another_issuer_is_never_suggested() {
        let stub = Stub::start("/realms/home", Some("https://elsewhere.example")).await;
        let got = run(&format!("{}/realms/home/deeper", stub.base)).await;
        assert!(!got.ok);
        assert_eq!(got.suggested_issuer, None);
        // The candidates were asked, though.
        assert!(stub.requests() >= 1);
    }

    #[tokio::test]
    async fn another_origin_is_never_asked() {
        let other = Stub::start("", None).await;
        let (base, task) =
            redirector(format!("{}/.well-known/openid-configuration", other.base)).await;
        let got = run(&format!("{base}/oauth/v2/authorize")).await;
        task.abort();
        assert!(!got.ok);
        assert_eq!(got.suggested_issuer, None);
        assert_eq!(other.requests(), 0, "a redirect must not be followed");
    }

    #[tokio::test]
    async fn an_unreachable_provider_is_reported_without_guessing() {
        let got = run("http://127.0.0.1:9/oauth/v2/authorize").await;
        assert!(!got.ok);
        assert_eq!(got.status, None);
        assert_eq!(got.suggested_issuer, None);
        assert!(got.hint.is_some());
        assert!(got.error.is_some());
    }
}
