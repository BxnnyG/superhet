//! Web Push (RFC 8030) with VAPID (RFC 8292) and `aes128gcm` payload encryption (RFC 8291),
//! built on `ring`, which is in the dependency tree anyway.
//!
//! The server owns one ECDSA P-256 key pair. Its public half is handed to browsers as the
//! `applicationServerKey` when they subscribe; the private half signs a short-lived JWT that
//! tells the push service who is sending. The payload is encrypted to the browser's own key, so
//! the push service (Google, Mozilla, Apple, ...) never sees the notification text.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use reqwest::Method;
use reqwest::header::{
    AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue,
};
use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::agreement::{ECDH_P256, EphemeralPrivateKey, UnparsedPublicKey, agree_ephemeral};
use ring::hkdf::{HKDF_SHA256, KeyType, Salt};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::json;
use url::Url;

use crate::egress::{Egress, Request, SendError};
use crate::message::Message;
use crate::ssrf;
use crate::target::PushDoc;

/// Record size announced in the payload header; the message must fit one record.
const RECORD_SIZE: u32 = 4096;
/// Largest plaintext in one record: record size minus the AEAD tag and the delimiter byte.
const MAX_PLAINTEXT: usize = RECORD_SIZE as usize - 16 - 1;
/// Lifetime of a VAPID token (the limit is 24 h).
const JWT_LIFETIME_SECS: i64 = 12 * 3600;

/// A new VAPID key pair: `(private PKCS#8 document, public key point)`, both base64url.
///
/// # Errors
/// A message if the system random source fails.
pub fn generate_vapid() -> Result<(String, String), String> {
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
        .map_err(|_| "could not generate a VAPID key".to_owned())?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
        .map_err(|_| "generated VAPID key is not usable".to_owned())?;
    Ok((
        B64.encode(pkcs8.as_ref()),
        B64.encode(pair.public_key().as_ref()),
    ))
}

/// The `Authorization` header value for a push service at `audience` (`scheme://host[:port]`).
fn vapid_authorization(
    private_pkcs8: &str,
    public_key: &str,
    audience: &str,
    subject: &str,
    now_secs: i64,
) -> Result<String, SendError> {
    let rng = SystemRandom::new();
    let der = B64
        .decode(private_pkcs8)
        .map_err(|_| SendError::permanent("stored VAPID key is damaged"))?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
        .map_err(|_| SendError::permanent("stored VAPID key is damaged"))?;
    let head = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = B64.encode(
        json!({"aud": audience, "exp": now_secs + JWT_LIFETIME_SECS, "sub": subject}).to_string(),
    );
    let signing_input = format!("{head}.{claims}");
    let signature = pair
        .sign(&rng, signing_input.as_bytes())
        .map_err(|_| SendError::permanent("signing with the VAPID key failed"))?;
    Ok(format!(
        "vapid t={signing_input}.{}, k={public_key}",
        B64.encode(signature.as_ref())
    ))
}

struct Len(usize);
impl KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn expand(prk: &ring::hkdf::Prk, info: &[u8], out: &mut [u8]) -> Result<(), SendError> {
    prk.expand(&[info], Len(out.len()))
        .and_then(|okm| okm.fill(out))
        .map_err(|_| SendError::permanent("key derivation failed"))
}

/// Keys derived from the shared secret (RFC 8291 section 3.4).
fn derive(
    ecdh: &[u8],
    auth_secret: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8],
) -> Result<([u8; 16], [u8; 12]), SendError> {
    let prk_key = Salt::new(HKDF_SHA256, auth_secret).extract(ecdh);
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let mut ikm = [0u8; 32];
    expand(&prk_key, &key_info, &mut ikm)?;
    let prk = Salt::new(HKDF_SHA256, salt).extract(&ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    expand(&prk, b"Content-Encoding: aes128gcm\0", &mut cek)?;
    expand(&prk, b"Content-Encoding: nonce\0", &mut nonce)?;
    Ok((cek, nonce))
}

/// Encrypt `plaintext` for a browser (`p256dh`: its 65-byte public key, `auth`: its 16-byte
/// secret), producing the complete `aes128gcm` request body.
///
/// # Errors
/// A permanent error for unusable keys or a payload that does not fit one record.
pub fn encrypt(plaintext: &[u8], p256dh: &[u8], auth: &[u8]) -> Result<Vec<u8>, SendError> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(SendError::permanent(
            "the notification is too large for one push record",
        ));
    }
    let rng = SystemRandom::new();
    let private = EphemeralPrivateKey::generate(&ECDH_P256, &rng)
        .map_err(|_| SendError::permanent("could not generate a key"))?;
    let as_public = private
        .compute_public_key()
        .map_err(|_| SendError::permanent("could not generate a key"))?;
    let as_public = as_public.as_ref().to_vec();
    let mut salt = [0u8; 16];
    rng.fill(&mut salt)
        .map_err(|_| SendError::permanent("no random data"))?;
    let (cek, nonce) = agree_ephemeral(
        private,
        &UnparsedPublicKey::new(&ECDH_P256, p256dh),
        |ecdh| derive(ecdh, auth, p256dh, &as_public, &salt),
    )
    .map_err(|_| SendError::permanent("the subscription's public key is not valid"))??;
    let mut record = plaintext.to_vec();
    record.push(0x02); // last record, no padding
    let key = LessSafeKey::new(
        UnboundKey::new(&AES_128_GCM, &cek)
            .map_err(|_| SendError::permanent("cipher setup failed"))?,
    );
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::empty(),
        &mut record,
    )
    .map_err(|_| SendError::permanent("encryption failed"))?;
    let mut body = Vec::with_capacity(21 + as_public.len() + record.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(u8::try_from(as_public.len()).unwrap_or(65));
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&record);
    Ok(body)
}

/// Check what a browser reports when it subscribes.
///
/// # Errors
/// A message for the user.
pub fn validate_subscription(endpoint: &str, p256dh: &str, auth: &str) -> Result<(), String> {
    let url =
        Url::parse(endpoint).map_err(|_| "the push endpoint is not a valid URL".to_owned())?;
    if url.scheme() != "https" {
        return Err("the push endpoint must be an https URL".to_owned());
    }
    ssrf::check_url(&url, false)?;
    let key = B64
        .decode(p256dh.trim_end_matches('='))
        .map_err(|_| "p256dh is not base64url".to_owned())?;
    if key.len() != 65 || key.first() != Some(&0x04) {
        return Err("p256dh is not an uncompressed P-256 key".to_owned());
    }
    let secret = B64
        .decode(auth.trim_end_matches('='))
        .map_err(|_| "auth is not base64url".to_owned())?;
    if secret.len() != 16 {
        return Err("auth must be 16 bytes".to_owned());
    }
    Ok(())
}

/// A browser subscription as stored.
#[derive(Debug, Clone)]
pub struct Subscription<'a> {
    /// Push service URL.
    pub endpoint: &'a str,
    /// Browser public key, base64url.
    pub p256dh: &'a str,
    /// Browser auth secret, base64url.
    pub auth: &'a str,
}

/// The VAPID identity.
#[derive(Debug, Clone, Copy)]
pub struct Identity<'a> {
    /// PKCS#8 private key, base64url. Secret.
    pub private_pkcs8: &'a str,
    /// Public key, base64url.
    pub public_key: &'a str,
    /// Contact (`mailto:` or `https:` URL) for the push service.
    pub subject: &'a str,
}

/// The JSON a service worker receives.
#[must_use]
pub fn payload(message: &Message) -> Vec<u8> {
    let cut = |text: &str, n: usize| text.chars().take(n).collect::<String>();
    json!({
        "title": cut(&message.title, 120),
        "body": cut(&message.body, 600),
        "tag": cut(&message.rule, 80),
        "url": "/",
    })
    .to_string()
    .into_bytes()
}

/// Deliver one message to one browser.
///
/// # Errors
/// A [`SendError`]; `gone` is set when the push service reports the subscription as expired.
pub async fn send(
    egress: &Egress,
    identity: Identity<'_>,
    sub: &Subscription<'_>,
    cfg: &PushDoc,
    message: &Message,
    now_ms: i64,
) -> Result<(), SendError> {
    let url = Url::parse(sub.endpoint)
        .map_err(|_| SendError::permanent("stored endpoint is not a URL"))?;
    let p256dh = B64
        .decode(sub.p256dh.trim_end_matches('='))
        .map_err(|_| SendError::permanent("stored subscription key is damaged"))?;
    let auth = B64
        .decode(sub.auth.trim_end_matches('='))
        .map_err(|_| SendError::permanent("stored subscription secret is damaged"))?;
    let body = encrypt(&payload(message), &p256dh, &auth)?;
    let audience = match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{}://{h}:{p}", url.scheme()),
        (Some(h), None) => format!("{}://{h}", url.scheme()),
        _ => return Err(SendError::permanent("stored endpoint has no host")),
    };
    let authorization = vapid_authorization(
        identity.private_pkcs8,
        identity.public_key,
        &audience,
        identity.subject,
        now_ms / 1000,
    )?;
    let mut headers = HeaderMap::new();
    let mut put = |name: HeaderName, value: &str| {
        if let Ok(v) = HeaderValue::from_str(value) {
            headers.insert(name, v);
        }
    };
    put(AUTHORIZATION, &authorization);
    put(CONTENT_ENCODING, "aes128gcm");
    put(CONTENT_TYPE, "application/octet-stream");
    put(HeaderName::from_static("ttl"), &cfg.ttl_secs.to_string());
    put(HeaderName::from_static("urgency"), &cfg.urgency);
    let status = egress
        .send(&Request {
            method: Method::POST,
            url: &url,
            headers,
            body,
            timeout: std::time::Duration::from_secs(15),
            allow_private: cfg.allow_private,
        })
        .await?;
    match status.as_u16() {
        200..=202 => Ok(()),
        404 | 410 => Err(SendError {
            message: format!("HTTP {}: subscription expired", status.as_u16()),
            retryable: false,
            gone: true,
        }),
        413 => Err(SendError::permanent("HTTP 413: notification too large")),
        400 | 401 | 403 => Err(SendError::permanent(format!(
            "HTTP {}: the push service rejected the request",
            status.as_u16()
        ))),
        _ => Err(crate::egress::status_error(status)
            .unwrap_or_else(|| SendError::transient("unexpected response"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey as VerifyKey};

    /// The browser side: decrypt an `aes128gcm` body with the subscription's private key.
    fn decrypt(
        body: &[u8],
        ua_private: EphemeralPrivateKey,
        ua_public: &[u8],
        auth: &[u8],
    ) -> Vec<u8> {
        let salt = &body[..16];
        assert_eq!(
            u32::from_be_bytes(body[16..20].try_into().expect("rs")),
            RECORD_SIZE
        );
        let id_len = usize::from(body[20]);
        let as_public = &body[21..21 + id_len];
        let record = &body[21 + id_len..];
        let (cek, nonce) = agree_ephemeral(
            ua_private,
            &UnparsedPublicKey::new(&ECDH_P256, as_public),
            |ecdh| derive(ecdh, auth, ua_public, as_public, salt),
        )
        .expect("agree")
        .expect("derive");
        let key = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &cek).expect("key"));
        let mut buf = record.to_vec();
        let plain = key
            .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
            .expect("authentic");
        assert_eq!(plain.last(), Some(&0x02), "delimiter");
        plain[..plain.len() - 1].to_vec()
    }

    fn browser() -> (EphemeralPrivateKey, Vec<u8>, [u8; 16]) {
        let rng = SystemRandom::new();
        let private = EphemeralPrivateKey::generate(&ECDH_P256, &rng).expect("gen");
        let public = private.compute_public_key().expect("pub").as_ref().to_vec();
        let mut auth = [0u8; 16];
        rng.fill(&mut auth).expect("rng");
        (private, public, auth)
    }

    #[test]
    fn payloads_round_trip_through_the_browsers_key() {
        let (private, public, auth) = browser();
        let text = "FIRE at Main Street \u{1F6A8} \u{00e4}\u{00f6}".as_bytes();
        let body = encrypt(text, &public, &auth).expect("encrypt");
        assert_eq!(decrypt(&body, private, &public, &auth), text);
        assert!(
            !body.windows(4).any(|w| w == b"FIRE"),
            "ciphertext hides the text"
        );
    }

    #[test]
    fn a_wrong_auth_secret_cannot_decrypt() {
        let (private, public, auth) = browser();
        let body = encrypt(b"secret", &public, &auth).expect("encrypt");
        let result = std::panic::catch_unwind(|| decrypt(&body, private, &public, &[9u8; 16]));
        assert!(result.is_err());
    }

    #[test]
    fn oversized_payloads_and_bad_keys_are_refused() {
        let (_, public, auth) = browser();
        assert!(encrypt(&vec![b'a'; MAX_PLAINTEXT + 1], &public, &auth).is_err());
        assert!(
            encrypt(b"x", &[4u8; 65], &auth).is_err(),
            "point not on the curve"
        );
        assert!(encrypt(b"x", &[1u8; 10], &auth).is_err());
    }

    #[test]
    fn vapid_tokens_verify_against_the_published_key() {
        let (private, public) = generate_vapid().expect("vapid");
        let header = vapid_authorization(
            &private,
            &public,
            "https://push.example",
            "mailto:a@example.com",
            1_000,
        )
        .expect("header");
        let rest = header.strip_prefix("vapid t=").expect("scheme");
        let (token, key) = rest.split_once(", k=").expect("parts");
        assert_eq!(key, public);
        let (signed, signature) = token.rsplit_once('.').expect("jwt");
        let claims: serde_json::Value = serde_json::from_slice(
            &B64.decode(signed.split_once('.').expect("dot").1)
                .expect("b64"),
        )
        .expect("json");
        assert_eq!(claims["aud"], "https://push.example");
        assert_eq!(claims["sub"], "mailto:a@example.com");
        assert_eq!(claims["exp"], 1_000 + JWT_LIFETIME_SECS);
        let public_bytes = B64.decode(&public).expect("b64");
        VerifyKey::new(&ECDSA_P256_SHA256_FIXED, public_bytes)
            .verify(signed.as_bytes(), &B64.decode(signature).expect("sig"))
            .expect("valid signature");
        // The private half is not part of anything sent.
        assert!(!header.contains(&private));
    }

    #[test]
    fn subscriptions_are_validated() {
        let (_, public, auth) = browser();
        let (p, a) = (B64.encode(&public), B64.encode(auth));
        assert!(validate_subscription("https://fcm.googleapis.com/fcm/send/abc", &p, &a).is_ok());
        assert!(validate_subscription("http://fcm.googleapis.com/x", &p, &a).is_err());
        assert!(validate_subscription("https://127.0.0.1/x", &p, &a).is_err());
        assert!(validate_subscription("https://[::1]/x", &p, &a).is_err());
        assert!(validate_subscription("https://push.example/x", "AAAA", &a).is_err());
        assert!(validate_subscription("https://push.example/x", &p, "AAAA").is_err());
        assert!(validate_subscription("nonsense", &p, &a).is_err());
    }
}
