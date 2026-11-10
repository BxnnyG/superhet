//! The provider side: discovery, the authorization URL (PKCE), the code exchange and ID token
//! verification (signature, issuer, audience, expiry, nonce) through the `openidconnect` crate,
//! over a `reqwest` client that does not follow redirects and is bounded in time and size.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openidconnect::core::{CoreClient, CoreJsonWebKeySet, CoreResponseType};
use openidconnect::http::Response;
use openidconnect::url::Url;
use openidconnect::{
    AsyncHttpClient, AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    EndpointMaybeSet, EndpointNotSet, EndpointSet, HttpRequest, HttpResponse, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, ProviderMetadataWithLogout, RedirectUrl, Scope,
    TokenResponse,
};
use serde_json::Value;

use super::config::{OidcSettings, Secret};
use super::logout::{self, Expect, LogoutClaims, Rejection};

/// Discovery documents and key sets are reused this long.
const PROVIDER_TTL: Duration = Duration::from_secs(15 * 60);
/// A failed verification refreshes the cached provider (key rotation) at most this often.
const REFRESH_MIN_AGE: Duration = Duration::from_secs(30);
/// Largest response accepted from the provider.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Errors of the provider round trips (messages are for logs, never for the browser).
#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    /// Settings do not describe a usable provider.
    #[error("configuration: {0}")]
    Config(String),
    /// Discovery failed.
    #[error("discovery failed: {0}")]
    Discovery(String),
    /// The token request failed.
    #[error("token request failed: {0}")]
    Exchange(String),
    /// The ID token is missing or did not verify.
    #[error("ID token rejected: {0}")]
    Token(String),
}

/// Transport error of the HTTP adapter.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The request could not be completed.
    #[error("{0}")]
    Request(String),
    /// The answer exceeded [`MAX_RESPONSE_BYTES`].
    #[error("response larger than {MAX_RESPONSE_BYTES} bytes")]
    TooLarge,
}

/// `reqwest` as an `openidconnect` HTTP client with a response size cap.
#[derive(Clone)]
pub struct Http(reqwest::Client);

impl Http {
    /// The client used for every provider request: no redirects (they would move a request to
    /// a host nobody configured), short timeouts.
    #[must_use]
    pub fn new() -> Self {
        // reqwest is built without a bundled crypto provider; the workspace uses ring. Already
        // installed (by another component) is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "Superhet/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/BxnnyG/superhet)"
            ))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self(client)
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

type HttpFuture<'c> = Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + 'c>>;

impl<'c> AsyncHttpClient<'c> for Http {
    type Error = HttpError;
    type Future = HttpFuture<'c>;

    fn call(&'c self, request: HttpRequest) -> Self::Future {
        let client = self.0.clone();
        Box::pin(async move {
            let request = reqwest::Request::try_from(request)
                .map_err(|e| HttpError::Request(e.to_string()))?;
            let mut response = client
                .execute(request)
                .await
                .map_err(|e| HttpError::Request(e.to_string()))?;
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| HttpError::Request(e.to_string()))?
            {
                if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return Err(HttpError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            let mut out: HttpResponse = Response::new(body);
            *out.status_mut() = response.status();
            *out.headers_mut() = response.headers().clone();
            Ok::<_, HttpError>(out)
        })
    }
}

type Client = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// A discovered provider, ready to build authorization URLs and verify tokens.
pub struct Provider {
    client: Client,
    issuer: String,
    client_id: String,
    /// The provider's `end_session_endpoint`, if it advertises one (RP-initiated logout).
    end_session: Option<Url>,
    /// The key set the provider published at discovery (back-channel logout tokens).
    jwks: CoreJsonWebKeySet,
    fetched: Instant,
}

/// Everything needed to send the browser to the provider.
pub struct AuthRequest {
    /// Where to redirect the browser.
    pub url: String,
    /// The `state` parameter.
    pub state: String,
    /// The nonce expected in the ID token.
    pub nonce: String,
    /// The PKCE verifier to present at the token endpoint.
    pub verifier: String,
}

/// A verified ID token.
#[derive(Debug)]
pub struct Verified {
    /// Issuer as stated by the token (equals the discovered issuer).
    pub issuer: String,
    /// `sub`.
    pub subject: String,
    /// The token's claims as JSON (read after the signature check).
    pub payload: Value,
    /// The ID token as received (kept as `id_token_hint` for the provider logout).
    pub id_token: String,
}

fn require_https(label: &str, url: &str, allow_plain: bool) -> Result<(), OidcError> {
    let is_https = url.starts_with("https://");
    if is_https || allow_plain {
        Ok(())
    } else {
        Err(OidcError::Discovery(format!("{label} is not https: {url}")))
    }
}

impl Provider {
    /// Discover the provider described by `settings`.
    ///
    /// # Errors
    /// A configuration problem, an unreachable provider or unusable metadata.
    pub async fn discover(
        http: &Http,
        settings: &OidcSettings,
        secret: Option<&Secret>,
        allow_http: bool,
    ) -> Result<Self, OidcError> {
        let issuer_text = settings.issuer.trim();
        let issuer_url = IssuerUrl::new(issuer_text.to_owned())
            .map_err(|e| OidcError::Config(format!("issuer: {e}")))?;
        let redirect = RedirectUrl::new(settings.redirect_url().map_err(OidcError::Config)?)
            .map_err(|e| OidcError::Config(format!("public_url: {e}")))?;
        let metadata = ProviderMetadataWithLogout::discover_async(issuer_url, http)
            .await
            .map_err(|e| OidcError::Discovery(chain(&e)))?;
        // The endpoints a login talks to must be as private as the issuer itself.
        let plain_ok = allow_http || issuer_text.starts_with("http://");
        require_https(
            "authorization endpoint",
            metadata.authorization_endpoint().as_str(),
            plain_ok,
        )?;
        if let Some(token) = metadata.token_endpoint() {
            require_https("token endpoint", token.as_str(), plain_ok)?;
        }
        let end_session = metadata
            .additional_metadata()
            .end_session_endpoint
            .as_ref()
            .map(|e| e.url().clone());
        if let Some(endpoint) = &end_session {
            require_https("end session endpoint", endpoint.as_str(), plain_ok)?;
        }
        let issuer = metadata.issuer().as_str().to_owned();
        let jwks = metadata.jwks().clone();
        let client_id = settings.client_id.trim().to_owned();
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(client_id.clone()),
            secret.map(|s| ClientSecret::new(s.expose().to_owned())),
        )
        .set_redirect_uri(redirect);
        Ok(Self {
            client,
            issuer,
            client_id,
            end_session,
            jwks,
            fetched: Instant::now(),
        })
    }

    /// The issuer string the provider announced (what identities are stored under).
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Build the authorization request: code flow, PKCE (S256), a fresh state and nonce.
    #[must_use]
    pub fn authorize(&self, scopes: &[String]) -> AuthRequest {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = self
            .client
            .authorize_url(
                AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_pkce_challenge(challenge);
        for scope in scopes {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        let (url, state, nonce) = request.url();
        AuthRequest {
            url: url.to_string(),
            state: state.secret().clone(),
            nonce: nonce.secret().clone(),
            verifier: verifier.secret().clone(),
        }
    }

    /// Redeem the authorization code and verify the ID token against the attempt's nonce.
    ///
    /// # Errors
    /// The token request failed, no ID token came back, or it did not verify.
    pub async fn complete(
        &self,
        http: &Http,
        code: &str,
        verifier: &str,
        nonce: &str,
    ) -> Result<Verified, OidcError> {
        let response = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|e| OidcError::Config(e.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_owned()))
            .request_async(http)
            .await
            .map_err(|e| OidcError::Exchange(chain(&e)))?;
        let id_token = response
            .id_token()
            .ok_or_else(|| OidcError::Token("the response has no id_token".to_owned()))?;
        let nonce = Nonce::new(nonce.to_owned());
        let claims = id_token
            .claims(&self.client.id_token_verifier(), &nonce)
            .map_err(|e| OidcError::Token(chain(&e)))?;
        let subject = claims.subject().as_str().to_owned();
        let issuer = claims.issuer().as_str().to_owned();
        // The signature is checked; the remaining claims are read from the same token.
        let serialized =
            serde_json::to_value(id_token).map_err(|e| OidcError::Token(e.to_string()))?;
        let raw = serialized.as_str().unwrap_or_default();
        let payload = payload_of(raw)
            .ok_or_else(|| OidcError::Token("token payload unreadable".to_owned()))?;
        Ok(Verified {
            issuer,
            subject,
            payload,
            id_token: raw.to_owned(),
        })
    }

    /// Whether the provider advertised an `end_session_endpoint` at discovery.
    #[must_use]
    pub fn has_end_session(&self) -> bool {
        self.end_session.is_some()
    }

    /// The URL that ends the provider's session, `None` when it has no end session endpoint.
    #[must_use]
    pub fn end_session_url(
        &self,
        id_token_hint: Option<&str>,
        post_logout_redirect_uri: &str,
        state: &str,
    ) -> Option<String> {
        let endpoint = self.end_session.as_ref()?;
        Some(logout::end_session_url(
            endpoint,
            id_token_hint,
            &self.client_id,
            post_logout_redirect_uri,
            state,
        ))
    }

    /// Validate a back-channel `logout_token` against this provider (`now` in Unix seconds).
    ///
    /// # Errors
    /// The [`Rejection`] that applies.
    pub fn check_logout_token(&self, jwt: &str, now: i64) -> Result<LogoutClaims, Rejection> {
        logout::validate(
            jwt,
            &self.jwks,
            &Expect {
                issuer: &self.issuer,
                client_id: &self.client_id,
                now,
            },
        )
    }

    fn age(&self) -> Duration {
        self.fetched.elapsed()
    }
}

/// The JSON payload of a compact JWT (no verification: call after the signature check).
fn payload_of(jwt: &str) -> Option<Value> {
    use base64::Engine;
    let part = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// An error with its causes joined (`a: b: c`).
fn chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut cur = err.source();
    while let Some(cause) = cur {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        cur = cause.source();
    }
    text
}

/// The discovered provider, reused for [`PROVIDER_TTL`] and rebuilt when the settings change.
pub struct ProviderCache {
    http: Http,
    slot: tokio::sync::Mutex<Option<(String, Arc<Provider>)>>,
}

impl Default for ProviderCache {
    fn default() -> Self {
        Self {
            http: Http::new(),
            slot: tokio::sync::Mutex::new(None),
        }
    }
}

impl ProviderCache {
    /// The HTTP client for token requests.
    #[must_use]
    pub fn http(&self) -> &Http {
        &self.http
    }

    /// The provider for `settings`; `refresh` forces a new discovery if the cached one is not
    /// brand new (a token signed with a rotated key).
    ///
    /// # Errors
    /// See [`Provider::discover`].
    pub async fn get(
        &self,
        settings: &OidcSettings,
        secret: Option<&Secret>,
        allow_http: bool,
        refresh: bool,
    ) -> Result<Arc<Provider>, OidcError> {
        let key = settings.connection_key();
        let mut slot = self.slot.lock().await;
        if let Some((cached_key, provider)) = slot.as_ref()
            && *cached_key == key
            && provider.age() < PROVIDER_TTL
            && !(refresh && provider.age() > REFRESH_MIN_AGE)
        {
            return Ok(Arc::clone(provider));
        }
        let provider =
            Arc::new(Provider::discover(&self.http, settings, secret, allow_http).await?);
        *slot = Some((key, Arc::clone(&provider)));
        Ok(provider)
    }

    /// Drop the cached provider (settings changed).
    pub async fn clear(&self) {
        *self.slot.lock().await = None;
    }
}
