//! A mock OpenID Connect provider for the OIDC tests: discovery, key set and token endpoint on a
//! loopback port, ID tokens signed with ES256. The test plays the browser and the user at the
//! provider (`authorize` stands in for the login/consent screen). No real identity provider is
//! ever contacted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use openidconnect::url::Url;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const CLIENT_ID: &str = "superhet-test";
pub const CLIENT_SECRET: &str = "mock-client-secret";

/// What is wrong with the ID token the provider issues (none: nothing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flaw {
    None,
    WrongNonce,
    WrongAudience,
    Expired,
    WrongSignature,
    WrongIssuer,
    /// The provider answers the token request with an error.
    TokenError,
    /// The PKCE challenge the provider remembered is not the one the client's verifier matches.
    WrongChallenge,
}

struct Grant {
    nonce: String,
    challenge: String,
    redirect_uri: String,
    claims: Value,
    flaw: Flaw,
}

struct Inner {
    key: EcdsaKeyPair,
    other_key: EcdsaKeyPair,
    grants: HashMap<String, Grant>,
    base: String,
    counter: u64,
    /// Number of token requests received (a refused callback must not reach this).
    token_requests: u64,
}

pub struct MockIdp {
    pub base: String,
    inner: Arc<Mutex<Inner>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockIdp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn key_pair() -> EcdsaKeyPair {
    let rng = SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).expect("generate key");
    EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
        .expect("load key")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn sign(key: &EcdsaKeyPair, claims: &Value) -> String {
    let header = json!({"alg": "ES256", "typ": "JWT", "kid": "mock-1"});
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let signature = key
        .sign(&SystemRandom::new(), signing_input.as_bytes())
        .expect("sign");
    format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    )
}

impl MockIdp {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind idp");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let inner = Arc::new(Mutex::new(Inner {
            key: key_pair(),
            other_key: key_pair(),
            grants: HashMap::new(),
            base: base.clone(),
            counter: 0,
            token_requests: 0,
        }));
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .with_state(Arc::clone(&inner));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { base, inner, task }
    }

    /// Number of token requests the provider has received.
    pub fn token_requests(&self) -> u64 {
        self.inner.lock().expect("lock").token_requests
    }

    /// The user signs in at the provider: remember what the authorization request (the
    /// `Location` of the API's login redirect) asked for and hand out a code for it.
    pub fn authorize(&self, location: &str, claims: Value, flaw: Flaw) -> String {
        let url = Url::parse(location).expect("authorization URL");
        assert!(
            location.starts_with(&format!("{}/authorize", self.base)),
            "redirected to the provider: {location}"
        );
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(q.get("client_id").map(String::as_str), Some(CLIENT_ID));
        assert_eq!(
            q.get("code_challenge_method").map(String::as_str),
            Some("S256"),
            "PKCE with S256 is mandatory"
        );
        assert!(
            q.get("state").is_some_and(|s| s.len() >= 16),
            "state: {q:?}"
        );
        assert!(
            q.get("nonce").is_some_and(|s| s.len() >= 16),
            "nonce: {q:?}"
        );
        let scope = q.get("scope").cloned().unwrap_or_default();
        assert!(scope.split(' ').any(|s| s == "openid"), "scope: {scope}");
        let mut inner = self.inner.lock().expect("lock");
        inner.counter += 1;
        let code = format!("code-{}", inner.counter);
        let mut challenge = q["code_challenge"].clone();
        if flaw == Flaw::WrongChallenge {
            challenge = "tampered".to_owned();
        }
        inner.grants.insert(
            code.clone(),
            Grant {
                nonce: q["nonce"].clone(),
                challenge,
                redirect_uri: q["redirect_uri"].clone(),
                claims,
                flaw,
            },
        );
        code
    }
}

async fn discovery(State(inner): State<Arc<Mutex<Inner>>>) -> Response {
    let base = inner.lock().expect("lock").base.clone();
    axum::Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "jwks_uri": format!("{base}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
        "scopes_supported": ["openid", "email", "profile", "groups"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        "claims_supported": ["sub", "email", "email_verified", "preferred_username", "groups"],
    }))
    .into_response()
}

async fn jwks(State(inner): State<Arc<Mutex<Inner>>>) -> Response {
    let inner = inner.lock().expect("lock");
    let point = inner.key.public_key().as_ref().to_vec();
    assert_eq!(point.len(), 65);
    axum::Json(json!({"keys": [{
        "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": "mock-1",
        "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
        "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
    }]}))
    .into_response()
}

fn oauth_error(code: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json")],
        json!({"error": code}).to_string(),
    )
        .into_response()
}

async fn token(
    State(inner): State<Arc<Mutex<Inner>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let form: HashMap<String, String> =
        Url::parse(&format!("http://x/?{}", String::from_utf8_lossy(&body)))
            .expect("form body")
            .query_pairs()
            .into_owned()
            .collect();
    let mut inner = inner.lock().expect("lock");
    inner.token_requests += 1;
    // Client authentication: the secret arrives in a Basic header or in the body.
    let basic = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|v| STANDARD.decode(v).ok())
        .map(|v| String::from_utf8_lossy(&v).into_owned());
    let secret_ok = basic
        .as_deref()
        .is_some_and(|b| b == format!("{CLIENT_ID}:{CLIENT_SECRET}"))
        || form.get("client_secret").map(String::as_str) == Some(CLIENT_SECRET);
    if !secret_ok {
        return oauth_error("invalid_client");
    }
    if form.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return oauth_error("unsupported_grant_type");
    }
    let Some(grant) = form.get("code").and_then(|c| inner.grants.remove(c)) else {
        return oauth_error("invalid_grant");
    };
    if form.get("redirect_uri") != Some(&grant.redirect_uri) {
        return oauth_error("invalid_grant");
    }
    // PKCE: base64url(SHA-256(verifier)) must equal the challenge from the authorization request.
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if computed != grant.challenge {
        return oauth_error("invalid_grant");
    }
    if grant.flaw == Flaw::TokenError {
        return oauth_error("server_error");
    }
    let mut claims = json!({
        "iss": inner.base,
        "aud": CLIENT_ID,
        "iat": now(),
        "exp": now() + 300,
        "nonce": grant.nonce,
    });
    if let (Some(target), Some(extra)) = (claims.as_object_mut(), grant.claims.as_object()) {
        target.extend(extra.clone());
    }
    match grant.flaw {
        Flaw::WrongNonce => claims["nonce"] = json!("not-the-nonce"),
        Flaw::WrongAudience => claims["aud"] = json!("someone-else"),
        Flaw::Expired => {
            claims["iat"] = json!(now() - 7200);
            claims["exp"] = json!(now() - 3600);
        }
        Flaw::WrongIssuer => claims["iss"] = json!("https://evil.example"),
        _ => {}
    }
    let key = if grant.flaw == Flaw::WrongSignature {
        &inner.other_key
    } else {
        &inner.key
    };
    let id_token = sign(key, &claims);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        json!({
            "access_token": "mock-access-token",
            "token_type": "Bearer",
            "expires_in": 300,
            "id_token": id_token,
        })
        .to_string(),
    )
        .into_response()
}
