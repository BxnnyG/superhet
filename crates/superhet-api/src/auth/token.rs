//! Session tokens, setup tokens and the session cookie.

use axum::http::{HeaderMap, header};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

/// Name of the session cookie.
pub const COOKIE_NAME: &str = "superhet_session";

/// Name of the guest session cookie.
pub const GUEST_COOKIE_NAME: &str = "superhet_guest";

/// Absolute session lifetime, also used as cookie `Max-Age`.
pub const COOKIE_MAX_AGE_SECS: u64 = 30 * 24 * 3600;

/// Raw random bytes of a session token (256 bit).
const SESSION_TOKEN_BYTES: usize = 32;

/// Raw random bytes of a generated setup token (192 bit).
const SETUP_TOKEN_BYTES: usize = 24;

fn random_b64(len: usize) -> Result<String, getrandom::Error> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// A fresh session token from the operating system's CSPRNG, base64url without padding.
pub fn generate_session_token() -> Result<String, getrandom::Error> {
    random_b64(SESSION_TOKEN_BYTES)
}

/// A fresh setup token for the first-run wizard.
pub fn generate_setup_token() -> Result<String, getrandom::Error> {
    random_b64(SETUP_TOKEN_BYTES)
}

/// What the database stores for a token: its SHA-256 digest. A leaked database dump therefore
/// does not yield usable cookies.
#[must_use]
pub fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Constant-time equality of two secrets (compared via their digests, so the length does not
/// leak either).
#[must_use]
pub fn secrets_equal(a: &str, b: &str) -> bool {
    let (a, b) = (hash_token(a), hash_token(b));
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn cookie_value(headers: &HeaderMap, wanted: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, value)| *name == wanted && !value.is_empty())
        .map(|(_, value)| value.to_owned())
}

/// Extract the session token from the `Cookie` header(s).
#[must_use]
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, COOKIE_NAME)
}

/// Extract the guest session token from the `Cookie` header(s).
#[must_use]
pub fn guest_token_from_headers(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, GUEST_COOKIE_NAME)
}

/// `Set-Cookie` value establishing a guest session (a browser-session cookie; the server side
/// idle timeout decides how long it lives).
#[must_use]
pub fn guest_cookie(token: &str, secure: bool) -> String {
    format!(
        "{GUEST_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict{}",
        if secure { "; Secure" } else { "" }
    )
}

/// `Set-Cookie` value removing the guest cookie in the browser.
#[must_use]
pub fn clearing_guest_cookie(secure: bool) -> String {
    format!(
        "{GUEST_COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{}",
        if secure { "; Secure" } else { "" }
    )
}

/// Lower-case hex of `bytes`.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Bytes of a hex string, or `None` if it is not valid hex.
#[must_use]
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// A fresh token for an invite link or a guest session (256 bit).
pub fn generate_token() -> Result<String, getrandom::Error> {
    random_b64(SESSION_TOKEN_BYTES)
}

/// A random one-time password of `len` characters from an unambiguous alphabet.
pub fn generate_password(len: usize) -> Result<String, getrandom::Error> {
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes)?;
    // 256 is not a multiple of the alphabet size; the bias is negligible for a one-time
    // password but rejecting the top slice keeps it exactly uniform.
    let limit = 256 - 256 % ALPHABET.len();
    let mut out = String::with_capacity(len);
    let mut pool = bytes.into_iter();
    while out.len() < len {
        let b = match pool.next() {
            Some(b) => b,
            None => {
                let mut one = [0u8; 1];
                getrandom::fill(&mut one)?;
                one[0]
            }
        };
        if usize::from(b) < limit {
            out.push(char::from(ALPHABET[usize::from(b) % ALPHABET.len()]));
        }
    }
    Ok(out)
}

/// `Set-Cookie` value establishing a session.
#[must_use]
pub fn session_cookie(token: &str, secure: bool) -> String {
    format!(
        "{COOKIE_NAME}={token}; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}; HttpOnly; SameSite=Strict{}",
        if secure { "; Secure" } else { "" }
    )
}

/// `Set-Cookie` value removing the session cookie in the browser.
#[must_use]
pub fn clearing_cookie(secure: bool) -> String {
    format!(
        "{COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{}",
        if secure { "; Secure" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn tokens_are_random_urlsafe_and_long_enough() {
        let a = generate_session_token().expect("rng");
        let b = generate_session_token().expect("rng");
        assert_ne!(a, b);
        assert_eq!(a.len(), 43); // 32 bytes, base64url without padding
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert!(generate_setup_token().expect("rng").len() >= 32);
    }

    #[test]
    fn hash_is_sha256_and_not_the_token() {
        // SHA-256("abc")
        let expected = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];
        assert_eq!(hash_token("abc"), expected);
        assert_ne!(hash_token("abc"), hash_token("abd"));
    }

    #[test]
    fn secrets_compare_exactly() {
        assert!(secrets_equal("same", "same"));
        assert!(!secrets_equal("same", "Same"));
        assert!(!secrets_equal("short", "shorter"));
        assert!(!secrets_equal("", "x"));
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(unhex("000fff"), Some(vec![0, 15, 255]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex("é1"), None);
    }

    #[test]
    fn generated_secrets_have_the_right_shape() {
        let t = generate_token().expect("token");
        assert_eq!(t.len(), 43);
        let p = generate_password(20).expect("password");
        assert_eq!(p.len(), 20);
        assert!(
            p.chars()
                .all(|c| c.is_ascii_alphanumeric() && !"0OIl1".contains(c))
        );
        assert_ne!(p, generate_password(20).expect("another"));
    }

    #[test]
    fn guest_cookie_is_separate_from_the_session_cookie() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("superhet_session=aaa; superhet_guest=bbb"),
        );
        assert_eq!(token_from_headers(&h).as_deref(), Some("aaa"));
        assert_eq!(guest_token_from_headers(&h).as_deref(), Some("bbb"));
        let c = guest_cookie("bbb", true);
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict") && c.contains("Secure"));
        assert!(!c.contains("Max-Age"));
        assert!(clearing_guest_cookie(false).contains("Max-Age=0"));
    }

    #[test]
    fn cookie_flags() {
        let c = session_cookie("tok", false);
        assert!(c.starts_with("superhet_session=tok;"));
        for flag in ["Path=/", "HttpOnly", "SameSite=Strict", "Max-Age=2592000"] {
            assert!(c.contains(flag), "{c} lacks {flag}");
        }
        assert!(!c.contains("Secure"));
        assert!(session_cookie("tok", true).ends_with("; Secure"));
        let gone = clearing_cookie(true);
        assert!(gone.starts_with("superhet_session=;") && gone.contains("Max-Age=0"));
        assert!(gone.contains("HttpOnly") && gone.contains("Secure"));
    }

    #[test]
    fn cookie_is_found_among_others() {
        let mut headers = HeaderMap::new();
        assert_eq!(token_from_headers(&headers), None);
        headers.append(header::COOKIE, HeaderValue::from_static("a=1; b=2"));
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("xsuperhet_session=no; superhet_session=abc_-9"),
        );
        assert_eq!(token_from_headers(&headers).as_deref(), Some("abc_-9"));
        let mut empty = HeaderMap::new();
        empty.insert(
            header::COOKIE,
            HeaderValue::from_static("superhet_session="),
        );
        assert_eq!(token_from_headers(&empty), None);
    }
}
