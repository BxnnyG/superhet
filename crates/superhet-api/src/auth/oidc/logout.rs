//! Single logout, the two halves (docs/adr/0024-oidc-logout.md):
//!
//! - RP-initiated logout: the URL that sends the browser to the provider's `end_session_endpoint`
//!   after the local session ended.
//! - Back-channel logout: validation of the provider's `logout_token` (OpenID Connect
//!   Back-Channel Logout 1.0, section 2.6) and the replay protection for its `jti`.
//!
//! Everything here is pure (no I/O, time passed in) so each rejection can be tested on its own.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openidconnect::core::{CoreJsonWebKey, CoreJsonWebKeySet, CoreJwsSigningAlgorithm};
use openidconnect::url::Url;
use openidconnect::{JsonWebKey, JsonWebKeyAlgorithm, JsonWebKeyUse};
use serde_json::Value;

use crate::auth::token;

/// The member of a logout token's `events` claim that marks it as a back-channel logout.
pub const BACKCHANNEL_EVENT: &str = "http://schemas.openid.net/event/backchannel-logout";
/// A logout token is refused when it was issued longer ago than this.
pub const MAX_AGE_SECS: i64 = 5 * 60;
/// Clock difference tolerated between the provider and this instance.
pub const CLOCK_SKEW_SECS: i64 = 60;
/// Largest logout token accepted, in bytes.
pub const MAX_TOKEN_BYTES: usize = 12 * 1024;
/// Longest `jti`, `sid` and `sub` accepted, in bytes.
const ID_MAX: usize = 512;
/// Most remembered `jti` values.
const MAX_JTIS: usize = 10_000;

/// The URL that ends the provider's session: the endpoint with `id_token_hint` (when the
/// session kept the ID token), `client_id`, `post_logout_redirect_uri` and `state` added to
/// whatever query the endpoint already has.
#[must_use]
pub fn end_session_url(
    endpoint: &Url,
    id_token_hint: Option<&str>,
    client_id: &str,
    post_logout_redirect_uri: &str,
    state: &str,
) -> String {
    let mut url = endpoint.clone();
    {
        let mut query = url.query_pairs_mut();
        if let Some(hint) = id_token_hint {
            query.append_pair("id_token_hint", hint);
        }
        query.append_pair("client_id", client_id);
        query.append_pair("post_logout_redirect_uri", post_logout_redirect_uri);
        query.append_pair("state", state);
    }
    url.to_string()
}

/// Why a logout token is refused. [`Rejection::code`] is stable (audit log, answer to the
/// provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// Not a compact JWT with JSON header and payload, or too large.
    Malformed,
    /// The header's `alg` is missing, `none`, symmetric or unknown.
    Algorithm,
    /// No key in the key set fits the header's `kid` and `alg` (rotated keys: refresh).
    UnknownKey,
    /// The signature does not verify.
    Signature,
    /// `iss` is not the provider's issuer.
    Issuer,
    /// `aud` does not contain our client id.
    Audience,
    /// `iat` is missing, too old or in the future.
    IssuedAt,
    /// `exp` is present and in the past.
    Expired,
    /// `events` lacks the back-channel logout member.
    Event,
    /// Neither `sub` nor `sid` is present.
    NoSubject,
    /// A `nonce` is present: this is an ID token, not a logout token.
    Nonce,
    /// `jti` is missing.
    NoJti,
    /// The same `jti` was seen before.
    Replay,
}

impl Rejection {
    /// Stable machine-readable name.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Algorithm => "algorithm",
            Self::UnknownKey => "unknown_key",
            Self::Signature => "signature",
            Self::Issuer => "issuer",
            Self::Audience => "audience",
            Self::IssuedAt => "issued_at",
            Self::Expired => "expired",
            Self::Event => "event",
            Self::NoSubject => "no_subject",
            Self::Nonce => "nonce",
            Self::NoJti => "no_jti",
            Self::Replay => "replay",
        }
    }

    /// Whether fetching the provider's keys again could change the verdict.
    #[must_use]
    pub fn may_be_key_rotation(self) -> bool {
        matches!(self, Self::UnknownKey | Self::Signature)
    }
}

/// A validated logout token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogoutClaims {
    /// Token id (replay protection).
    pub jti: String,
    /// The user at the provider, if named.
    pub subject: Option<String>,
    /// The provider's session, if named.
    pub sid: Option<String>,
}

/// What a logout token is checked against.
#[derive(Debug, Clone, Copy)]
pub struct Expect<'a> {
    /// The provider's issuer.
    pub issuer: &'a str,
    /// Our client id (must be in `aud`).
    pub client_id: &'a str,
    /// Now, in Unix seconds.
    pub now: i64,
}

fn decode_part(part: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(part).ok()
}

/// Validate `jwt` as a back-channel logout token: signature against `keys`, then every check of
/// section 2.6 (issuer, audience, `iat`, `exp` if present, the `events` member, `sub`/`sid`, no
/// `nonce`, `jti`). `jti` replay is the caller's business ([`ReplayStore`]).
///
/// # Errors
/// The first [`Rejection`] that applies.
pub fn validate(
    jwt: &str,
    keys: &CoreJsonWebKeySet,
    expect: &Expect<'_>,
) -> Result<LogoutClaims, Rejection> {
    if jwt.len() > MAX_TOKEN_BYTES {
        return Err(Rejection::Malformed);
    }
    let mut parts = jwt.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Rejection::Malformed);
    };
    let header_json: Value = decode_part(header)
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or(Rejection::Malformed)?;
    let claims: Value = decode_part(payload)
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or(Rejection::Malformed)?;
    let signature = decode_part(signature).ok_or(Rejection::Malformed)?;
    if !claims.is_object() || !header_json.is_object() {
        return Err(Rejection::Malformed);
    }

    let alg = signing_alg(&header_json)?;
    let kid = header_json.get("kid").and_then(Value::as_str);
    let signing_input = format!("{header}.{payload}");
    verify_signature(keys, &alg, kid, signing_input.as_bytes(), &signature)?;

    check_claims(&claims, expect)
}

/// The header's algorithm, if it is an asymmetric signature algorithm. `none` and the HMAC
/// family are refused (a public key set cannot vouch for them).
fn signing_alg(header: &Value) -> Result<CoreJwsSigningAlgorithm, Rejection> {
    let alg: CoreJwsSigningAlgorithm = header
        .get("alg")
        .and_then(Value::as_str)
        .and_then(|a| serde_json::from_value(Value::String(a.to_owned())).ok())
        .ok_or(Rejection::Algorithm)?;
    match alg {
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256
        | CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384
        | CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512
        | CoreJwsSigningAlgorithm::EcdsaP256Sha256
        | CoreJwsSigningAlgorithm::EcdsaP384Sha384
        | CoreJwsSigningAlgorithm::EcdsaP521Sha512
        | CoreJwsSigningAlgorithm::RsaSsaPssSha256
        | CoreJwsSigningAlgorithm::RsaSsaPssSha384
        | CoreJwsSigningAlgorithm::RsaSsaPssSha512
        | CoreJwsSigningAlgorithm::EdDsa => Ok(alg),
        _ => Err(Rejection::Algorithm),
    }
}

fn usable(key: &CoreJsonWebKey, alg: &CoreJwsSigningAlgorithm, kid: Option<&str>) -> bool {
    if let Some(kid) = kid
        && key.key_id().is_some_and(|k| k.as_str() != kid)
    {
        return false;
    }
    if key.key_use().is_some_and(|u| !u.allows_signature()) {
        return false;
    }
    match key.signing_alg() {
        JsonWebKeyAlgorithm::Algorithm(a) => a == alg,
        JsonWebKeyAlgorithm::Unspecified => true,
        JsonWebKeyAlgorithm::Unsupported => false,
    }
}

fn verify_signature(
    keys: &CoreJsonWebKeySet,
    alg: &CoreJwsSigningAlgorithm,
    kid: Option<&str>,
    message: &[u8],
    signature: &[u8],
) -> Result<(), Rejection> {
    let mut candidates = keys
        .keys()
        .iter()
        .filter(|k| usable(k, alg, kid))
        .peekable();
    if candidates.peek().is_none() {
        return Err(Rejection::UnknownKey);
    }
    if candidates.any(|key| key.verify_signature(alg, message, signature).is_ok()) {
        Ok(())
    } else {
        Err(Rejection::Signature)
    }
}

fn text(claims: &Value, name: &str) -> Option<String> {
    claims
        .get(name)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= ID_MAX)
        .map(str::to_owned)
}

#[allow(clippy::cast_possible_truncation)]
fn seconds(claims: &Value, name: &str) -> Option<i64> {
    let v = claims.get(name)?;
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// The checks of section 2.6 on an already authenticated payload.
fn check_claims(claims: &Value, expect: &Expect<'_>) -> Result<LogoutClaims, Rejection> {
    if claims.get("iss").and_then(Value::as_str) != Some(expect.issuer) {
        return Err(Rejection::Issuer);
    }
    let audience = match claims.get("aud") {
        Some(Value::String(a)) => a == expect.client_id,
        Some(Value::Array(list)) => list.iter().any(|a| a.as_str() == Some(expect.client_id)),
        _ => false,
    };
    if !audience {
        return Err(Rejection::Audience);
    }
    let iat = seconds(claims, "iat").ok_or(Rejection::IssuedAt)?;
    if iat > expect.now + CLOCK_SKEW_SECS || iat < expect.now - MAX_AGE_SECS - CLOCK_SKEW_SECS {
        return Err(Rejection::IssuedAt);
    }
    if claims.get("exp").is_some() {
        match seconds(claims, "exp") {
            Some(exp) if exp + CLOCK_SKEW_SECS >= expect.now => {}
            _ => return Err(Rejection::Expired),
        }
    }
    let event_ok = claims
        .get("events")
        .and_then(|e| e.get(BACKCHANNEL_EVENT))
        .is_some_and(Value::is_object);
    if !event_ok {
        return Err(Rejection::Event);
    }
    if claims.get("nonce").is_some() {
        return Err(Rejection::Nonce);
    }
    let subject = text(claims, "sub");
    let sid = text(claims, "sid");
    if subject.is_none() && sid.is_none() {
        return Err(Rejection::NoSubject);
    }
    let jti = text(claims, "jti").ok_or(Rejection::NoJti)?;
    Ok(LogoutClaims { jti, subject, sid })
}

/// The `jti` values seen lately: a logout token is accepted once. Entries live as long as the
/// token could still pass the age check, so nothing older is ever replayable.
#[derive(Default)]
pub struct ReplayStore {
    seen: Mutex<HashMap<[u8; 32], i64>>,
}

impl ReplayStore {
    /// Remember `jti` of `issuer` until `now` plus the validity window.
    ///
    /// # Errors
    /// [`Rejection::Replay`] when it is already remembered (or the store is full of live
    /// entries, which only validly signed tokens can cause).
    pub fn claim(&self, issuer: &str, jti: &str, now: i64) -> Result<(), Rejection> {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.len() >= MAX_JTIS {
            seen.retain(|_, until| *until > now);
        }
        if seen.len() >= MAX_JTIS {
            return Err(Rejection::Replay);
        }
        let key = token::hash_token(&format!("{issuer}\n{jti}"));
        match seen.get(&key) {
            Some(until) if *until > now => Err(Rejection::Replay),
            _ => {
                seen.insert(key, now + MAX_AGE_SECS + 2 * CLOCK_SKEW_SECS);
                Ok(())
            }
        }
    }

    /// Forget `jti` (the logout it announced could not be carried out; the provider may retry).
    pub fn release(&self, issuer: &str, jti: &str) {
        let key = token::hash_token(&format!("{issuer}\n{jti}"));
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
    use serde_json::json;

    const ISS: &str = "https://idp.example";
    const CLIENT: &str = "superhet";
    const NOW: i64 = 1_800_000_000;

    struct Signer {
        key: EcdsaKeyPair,
    }

    fn signer() -> Signer {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .expect("generate key");
        Signer {
            key: EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                .expect("load key"),
        }
    }

    impl Signer {
        fn jwks(&self, kid: &str) -> CoreJsonWebKeySet {
            let point = self.key.public_key().as_ref().to_vec();
            serde_json::from_value(json!({"keys": [{
                "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": kid,
                "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
                "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
            }]}))
            .expect("key set")
        }

        fn sign_with_header(&self, header: &Value, claims: &Value) -> String {
            let input = format!(
                "{}.{}",
                URL_SAFE_NO_PAD.encode(header.to_string()),
                URL_SAFE_NO_PAD.encode(claims.to_string())
            );
            let sig = self
                .key
                .sign(&SystemRandom::new(), input.as_bytes())
                .expect("sign");
            format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref()))
        }

        fn sign(&self, claims: &Value) -> String {
            self.sign_with_header(&json!({"alg": "ES256", "typ": "JWT", "kid": "k1"}), claims)
        }
    }

    fn good() -> Value {
        json!({
            "iss": ISS,
            "aud": CLIENT,
            "iat": NOW - 5,
            "jti": "jti-1",
            "sid": "sid-1",
            "sub": "user-1",
            "events": {BACKCHANNEL_EVENT: {}},
        })
    }

    fn expect() -> Expect<'static> {
        Expect {
            issuer: ISS,
            client_id: CLIENT,
            now: NOW,
        }
    }

    fn check(s: &Signer, claims: &Value) -> Result<LogoutClaims, Rejection> {
        validate(&s.sign(claims), &s.jwks("k1"), &expect())
    }

    fn without(claim: &str) -> Value {
        let mut c = good();
        c.as_object_mut().expect("object").remove(claim);
        c
    }

    fn with(claim: &str, value: Value) -> Value {
        let mut c = good();
        c[claim] = value;
        c
    }

    #[test]
    fn a_proper_token_is_accepted() {
        let s = signer();
        let claims = check(&s, &good()).expect("valid");
        assert_eq!(claims.jti, "jti-1");
        assert_eq!(claims.sid.as_deref(), Some("sid-1"));
        assert_eq!(claims.subject.as_deref(), Some("user-1"));
        // Either of sub and sid is enough, aud may be a list, exp is optional but honoured.
        assert!(check(&s, &without("sid")).is_ok());
        assert!(check(&s, &without("sub")).is_ok());
        assert!(check(&s, &with("aud", json!(["other", CLIENT]))).is_ok());
        assert!(check(&s, &with("exp", json!(NOW + 60))).is_ok());
    }

    #[test]
    fn the_signature_must_come_from_the_providers_keys() {
        let s = signer();
        let other = signer();
        let forged = other.sign(&good());
        assert_eq!(
            validate(&forged, &s.jwks("k1"), &expect()),
            Err(Rejection::Signature)
        );
        // Tampered payload with the original signature.
        let token = s.sign(&good());
        let mut parts: Vec<&str> = token.split('.').collect();
        let swapped = URL_SAFE_NO_PAD.encode(with("sid", json!("sid-2")).to_string());
        parts[1] = &swapped;
        assert_eq!(
            validate(&parts.join("."), &s.jwks("k1"), &expect()),
            Err(Rejection::Signature)
        );
        // A key id the provider does not publish: the keys may have rotated.
        let rejection = validate(&token, &s.jwks("k2"), &expect()).expect_err("unknown kid");
        assert_eq!(rejection, Rejection::UnknownKey);
        assert!(rejection.may_be_key_rotation());
        assert!(!Rejection::Issuer.may_be_key_rotation());
    }

    #[test]
    fn unsigned_and_symmetric_tokens_are_refused() {
        let s = signer();
        for alg in ["none", "HS256", "HS512", "nonsense"] {
            let token = s.sign_with_header(&json!({"alg": alg, "kid": "k1"}), &good());
            assert_eq!(
                validate(&token, &s.jwks("k1"), &expect()),
                Err(Rejection::Algorithm),
                "{alg}"
            );
        }
        let no_alg = s.sign_with_header(&json!({"kid": "k1"}), &good());
        assert_eq!(
            validate(&no_alg, &s.jwks("k1"), &expect()),
            Err(Rejection::Algorithm)
        );
        // `alg: none` with an empty signature part.
        let none = format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#),
            URL_SAFE_NO_PAD.encode(good().to_string())
        );
        assert_eq!(
            validate(&none, &s.jwks("k1"), &expect()),
            Err(Rejection::Algorithm)
        );
        // A header claiming RS256 against an EC key is no way around the key type.
        let confused = s.sign_with_header(&json!({"alg": "RS256", "kid": "k1"}), &good());
        assert!(validate(&confused, &s.jwks("k1"), &expect()).is_err());
    }

    #[test]
    fn garbage_is_malformed() {
        let s = signer();
        let keys = s.jwks("k1");
        for token in ["", "a.b", "a.b.c.d", "!!.!!.!!", "e30.e30"] {
            assert_eq!(
                validate(token, &keys, &expect()),
                Err(Rejection::Malformed),
                "{token}"
            );
        }
        let not_json = format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode("{}"),
            URL_SAFE_NO_PAD.encode("[1]"),
            URL_SAFE_NO_PAD.encode("x")
        );
        assert_eq!(
            validate(&not_json, &keys, &expect()),
            Err(Rejection::Malformed)
        );
        let huge = "a".repeat(MAX_TOKEN_BYTES + 1);
        assert_eq!(validate(&huge, &keys, &expect()), Err(Rejection::Malformed));
    }

    #[test]
    fn issuer_and_audience_must_match() {
        let s = signer();
        assert_eq!(
            check(&s, &with("iss", json!("https://evil.example"))),
            Err(Rejection::Issuer)
        );
        assert_eq!(check(&s, &without("iss")), Err(Rejection::Issuer));
        assert_eq!(
            check(&s, &with("aud", json!("someone-else"))),
            Err(Rejection::Audience)
        );
        assert_eq!(
            check(&s, &with("aud", json!(["a", "b"]))),
            Err(Rejection::Audience)
        );
        assert_eq!(check(&s, &without("aud")), Err(Rejection::Audience));
    }

    #[test]
    fn the_token_must_be_fresh() {
        let s = signer();
        assert_eq!(check(&s, &without("iat")), Err(Rejection::IssuedAt));
        let stale = NOW - MAX_AGE_SECS - CLOCK_SKEW_SECS - 1;
        assert_eq!(
            check(&s, &with("iat", json!(stale))),
            Err(Rejection::IssuedAt)
        );
        let future = NOW + CLOCK_SKEW_SECS + 1;
        assert_eq!(
            check(&s, &with("iat", json!(future))),
            Err(Rejection::IssuedAt)
        );
        assert!(check(&s, &with("iat", json!(NOW - MAX_AGE_SECS))).is_ok());
        assert_eq!(
            check(&s, &with("exp", json!(NOW - CLOCK_SKEW_SECS - 1))),
            Err(Rejection::Expired)
        );
        assert_eq!(
            check(&s, &with("exp", json!("soon"))),
            Err(Rejection::Expired)
        );
    }

    #[test]
    fn the_events_claim_must_announce_a_backchannel_logout() {
        let s = signer();
        assert_eq!(check(&s, &without("events")), Err(Rejection::Event));
        assert_eq!(
            check(&s, &with("events", json!({"http://other/event": {}}))),
            Err(Rejection::Event)
        );
        // The member must be a JSON object.
        assert_eq!(
            check(&s, &with("events", json!({BACKCHANNEL_EVENT: "yes"}))),
            Err(Rejection::Event)
        );
        assert_eq!(
            check(&s, &with("events", json!([BACKCHANNEL_EVENT]))),
            Err(Rejection::Event)
        );
    }

    #[test]
    fn an_id_token_is_not_a_logout_token() {
        let s = signer();
        // Looks like an ID token: has a nonce (and no events).
        let id_token = json!({"iss": ISS, "aud": CLIENT, "iat": NOW, "exp": NOW + 300,
                              "sub": "user-1", "nonce": "abc"});
        assert_eq!(check(&s, &id_token), Err(Rejection::Event));
        // Even with the event, a nonce disqualifies the token.
        assert_eq!(
            check(&s, &with("nonce", json!("abc"))),
            Err(Rejection::Nonce)
        );
        assert_eq!(
            check(&s, &with("nonce", Value::Null)),
            Err(Rejection::Nonce)
        );
    }

    #[test]
    fn a_subject_or_session_and_a_jti_are_required() {
        let s = signer();
        let mut neither = without("sid");
        neither.as_object_mut().expect("object").remove("sub");
        assert_eq!(check(&s, &neither), Err(Rejection::NoSubject));
        assert_eq!(
            check(&s, &with("sid", json!(""))).map(|c| c.sid),
            Ok(None),
            "an empty sid names nothing"
        );
        assert_eq!(check(&s, &without("jti")), Err(Rejection::NoJti));
        assert_eq!(check(&s, &with("jti", json!(""))), Err(Rejection::NoJti));
        assert_eq!(check(&s, &with("jti", json!(7))), Err(Rejection::NoJti));
    }

    #[test]
    fn a_jti_is_accepted_once_until_it_ages_out() {
        let store = ReplayStore::default();
        assert_eq!(store.claim(ISS, "j1", NOW), Ok(()));
        assert_eq!(store.claim(ISS, "j1", NOW + 1), Err(Rejection::Replay));
        // Same id at another provider is a different token.
        assert_eq!(store.claim("https://other.example", "j1", NOW), Ok(()));
        assert_eq!(store.claim(ISS, "j2", NOW), Ok(()));
        // Released (the logout failed): the provider may retry.
        store.release(ISS, "j1");
        assert_eq!(store.claim(ISS, "j1", NOW + 2), Ok(()));
        // Once no token with that jti can pass the age check any more, it is forgotten.
        let later = NOW + MAX_AGE_SECS + 2 * CLOCK_SKEW_SECS + 1;
        assert_eq!(store.claim(ISS, "j2", later), Ok(()));
    }

    #[test]
    fn the_replay_store_is_bounded() {
        let store = ReplayStore::default();
        for i in 0..MAX_JTIS {
            store.claim(ISS, &format!("j{i}"), NOW).expect("room");
        }
        assert_eq!(store.claim(ISS, "one-more", NOW), Err(Rejection::Replay));
        // Expired entries make room again.
        let later = NOW + MAX_AGE_SECS + 2 * CLOCK_SKEW_SECS + 1;
        assert_eq!(store.claim(ISS, "one-more", later), Ok(()));
    }

    #[test]
    fn the_end_session_url_carries_the_hint_and_keeps_the_endpoints_query() {
        let endpoint = Url::parse("https://idp.example/logout?realm=r1").expect("url");
        let url = end_session_url(
            &endpoint,
            Some("h.p.s"),
            CLIENT,
            "https://sdr.example/",
            "st4te",
        );
        let parsed = Url::parse(&url).expect("valid URL");
        assert_eq!(parsed.path(), "/logout");
        let q: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["realm"], "r1");
        assert_eq!(q["id_token_hint"], "h.p.s");
        assert_eq!(q["client_id"], CLIENT);
        assert_eq!(q["post_logout_redirect_uri"], "https://sdr.example/");
        assert_eq!(q["state"], "st4te");
        // The redirect URI is encoded, not pasted into the query.
        assert!(url.contains("post_logout_redirect_uri=https%3A%2F%2Fsdr.example%2F"));
        let plain = Url::parse("https://idp.example/end").expect("url");
        let bare = end_session_url(&plain, None, CLIENT, "https://sdr.example/", "s");
        assert!(!bare.contains("id_token_hint"), "{bare}");
        assert!(bare.starts_with("https://idp.example/end?client_id="));
    }
}
