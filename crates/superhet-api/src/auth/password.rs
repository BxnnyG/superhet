//! Argon2id hashing and input validation.

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHasher, PasswordVerifier},
};

/// OWASP-recommended Argon2id parameters: 19 MiB, 2 iterations, 1 lane.
const MEMORY_KIB: u32 = 19_456;
const ITERATIONS: u32 = 2;
const LANES: u32 = 1;

/// Longest accepted username.
pub const USERNAME_MAX: usize = 64;
/// Password length bounds (characters).
pub const PASSWORD_MIN: usize = 12;
pub const PASSWORD_MAX: usize = 1024;

fn hasher() -> Result<Argon2<'static>, argon2::Error> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, LANES, None)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Hash a password into a PHC string with a random salt. CPU and memory heavy: call from a
/// blocking context.
pub fn hash(password: &str) -> Result<String, String> {
    let hasher = hasher().map_err(|e| e.to_string())?;
    hasher
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Verify a password against a PHC string (the parameters in the string are authoritative).
/// CPU and memory heavy: call from a blocking context.
#[must_use]
pub fn verify(password: &str, phc: &str) -> bool {
    match hasher() {
        Ok(h) => h.verify_password(password.as_bytes(), phc).is_ok(),
        Err(_) => false,
    }
}

/// Usernames are 1-64 characters from `[A-Za-z0-9._-]`.
pub fn validate_username(username: &str) -> Result<(), &'static str> {
    if username.is_empty() || username.len() > USERNAME_MAX {
        return Err("username must be 1-64 characters");
    }
    if !username
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    {
        return Err("username may only contain letters, digits, '.', '_' and '-'");
    }
    Ok(())
}

/// Passwords are 12-1024 characters; no composition rules (length matters, see NIST 800-63B).
pub fn validate_password(password: &str) -> Result<(), &'static str> {
    let len = password.chars().count();
    if !(PASSWORD_MIN..=PASSWORD_MAX).contains(&len) {
        return Err("password must be 12-1024 characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_roundtrip_with_owasp_parameters() {
        let phc = hash("correct horse battery").expect("hash");
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
        assert!(verify("correct horse battery", &phc));
        assert!(!verify("correct horse batterz", &phc));
        assert_ne!(phc, hash("correct horse battery").expect("hash"), "salted");
        assert!(!verify("x", "not a phc string"));
    }

    #[test]
    fn username_rules() {
        assert!(validate_username("admin").is_ok());
        assert!(validate_username("A.b_c-9").is_ok());
        assert!(validate_username(&"a".repeat(64)).is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username(&"a".repeat(65)).is_err());
        assert!(validate_username("white space").is_err());
        assert!(validate_username("ümlaut").is_err());
        assert!(validate_username("a/b").is_err());
    }

    #[test]
    fn password_rules() {
        assert!(validate_password(&"x".repeat(12)).is_ok());
        assert!(validate_password(&"x".repeat(1024)).is_ok());
        assert!(validate_password(&"x".repeat(11)).is_err());
        assert!(validate_password(&"x".repeat(1025)).is_err());
        assert!(
            validate_password("äöüäöüäöüäöü").is_ok(),
            "counts characters"
        );
    }
}
