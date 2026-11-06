//! Login attempts in flight: the `state` value sent to the provider, the nonce and PKCE verifier
//! behind it, and the browser binding that stops a stranger's callback from completing a
//! victim's login (login CSRF).
//!
//! Kept in memory: an attempt lives a few minutes, a restart merely makes users start over.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::auth::token;

/// How long a started login may take to come back from the provider.
pub const TTL: Duration = Duration::from_secs(10 * 60);
/// Most attempts in flight; further starts are refused until some expire.
pub const MAX_PENDING: usize = 2000;
/// Name of the cookie that ties the attempt to the browser that started it.
pub const BINDING_COOKIE: &str = "superhet_oidc";

/// What is remembered per attempt.
#[derive(Debug)]
pub struct Pending {
    /// Nonce that must come back in the ID token.
    pub nonce: String,
    /// PKCE code verifier (the challenge went to the provider).
    pub verifier: String,
    binding: [u8; 32],
    started: Instant,
}

/// Why an attempt cannot be completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowError {
    /// The state is unknown (never issued, already used, or lost in a restart).
    UnknownState,
    /// The attempt took longer than [`TTL`].
    Expired,
    /// The browser does not carry the binding cookie of the attempt.
    BindingMismatch,
    /// Too many attempts in flight.
    Full,
}

/// The attempts in flight.
#[derive(Default)]
pub struct PendingStore {
    map: Mutex<HashMap<[u8; 32], Pending>>,
}

impl PendingStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<[u8; 32], Pending>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Remember an attempt under `state`, bound to the browser holding `binding`.
    ///
    /// # Errors
    /// [`FlowError::Full`] when [`MAX_PENDING`] live attempts exist.
    pub fn insert(
        &self,
        state: &str,
        binding: &str,
        nonce: String,
        verifier: String,
        now: Instant,
    ) -> Result<(), FlowError> {
        let mut map = self.lock();
        if map.len() >= MAX_PENDING {
            map.retain(|_, p| now.saturating_duration_since(p.started) < TTL);
            if map.len() >= MAX_PENDING {
                return Err(FlowError::Full);
            }
        }
        map.insert(
            token::hash_token(state),
            Pending {
                nonce,
                verifier,
                binding: token::hash_token(binding),
                started: now,
            },
        );
        Ok(())
    }

    /// Take the attempt for `state`. The entry is gone afterwards whatever the outcome, so a
    /// state can be tried once.
    ///
    /// # Errors
    /// Unknown, expired or not bound to `binding`.
    pub fn take(
        &self,
        state: &str,
        binding: Option<&str>,
        now: Instant,
    ) -> Result<Pending, FlowError> {
        let pending = self
            .lock()
            .remove(&token::hash_token(state))
            .ok_or(FlowError::UnknownState)?;
        if now.saturating_duration_since(pending.started) >= TTL {
            return Err(FlowError::Expired);
        }
        let presented = binding.map(token::hash_token);
        // Both sides are hashes of secrets, so a plain comparison leaks nothing useful; the
        // constant-time helper keeps it uniform anyway.
        let ok = presented
            .is_some_and(|p| token::secrets_equal(&token::hex(&p), &token::hex(&pending.binding)));
        if !ok {
            return Err(FlowError::BindingMismatch);
        }
        Ok(pending)
    }

    /// Number of attempts in flight.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// No attempt in flight.
    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// `Set-Cookie` value for the binding cookie. `SameSite=Lax` because the provider sends the
/// browser back with a cross-site top-level navigation; scoped to the OIDC endpoints.
#[must_use]
pub fn binding_cookie(value: &str, secure: bool) -> String {
    format!(
        "{BINDING_COOKIE}={value}; Path=/api/v1/auth/oidc; Max-Age={}; HttpOnly; SameSite=Lax{}",
        TTL.as_secs(),
        if secure { "; Secure" } else { "" }
    )
}

/// `Set-Cookie` value removing the binding cookie.
#[must_use]
pub fn clearing_binding_cookie(secure: bool) -> String {
    format!(
        "{BINDING_COOKIE}=; Path=/api/v1/auth/oidc; Max-Age=0; HttpOnly; SameSite=Lax{}",
        if secure { "; Secure" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(state: &str, binding: &str, now: Instant) -> PendingStore {
        let store = PendingStore::default();
        store
            .insert(state, binding, "nonce-1".into(), "verifier-1".into(), now)
            .expect("insert");
        store
    }

    #[test]
    fn the_matching_state_and_binding_return_nonce_and_verifier() {
        let now = Instant::now();
        let store = store_with("state-a", "bind-a", now);
        let p = store.take("state-a", Some("bind-a"), now).expect("take");
        assert_eq!(
            (p.nonce.as_str(), p.verifier.as_str()),
            ("nonce-1", "verifier-1")
        );
        assert!(store.is_empty());
    }

    #[test]
    fn an_unknown_state_is_refused() {
        let now = Instant::now();
        let store = store_with("state-a", "bind-a", now);
        assert_eq!(
            store.take("state-b", Some("bind-a"), now).err(),
            Some(FlowError::UnknownState)
        );
        assert_eq!(store.len(), 1, "a wrong guess does not burn a real attempt");
    }

    #[test]
    fn a_state_works_only_once() {
        let now = Instant::now();
        let store = store_with("state-a", "bind-a", now);
        store.take("state-a", Some("bind-a"), now).expect("first");
        assert_eq!(
            store.take("state-a", Some("bind-a"), now).err(),
            Some(FlowError::UnknownState)
        );
    }

    #[test]
    fn a_callback_from_another_browser_is_refused_and_burns_the_state() {
        let now = Instant::now();
        let store = store_with("state-a", "bind-a", now);
        assert_eq!(
            store.take("state-a", Some("bind-other"), now).err(),
            Some(FlowError::BindingMismatch)
        );
        assert_eq!(
            store.take("state-a", Some("bind-a"), now).err(),
            Some(FlowError::UnknownState),
            "the attacker's attempt consumed it"
        );
        let store = store_with("state-a", "bind-a", now);
        assert_eq!(
            store.take("state-a", None, now).err(),
            Some(FlowError::BindingMismatch),
            "no cookie at all"
        );
    }

    #[test]
    fn attempts_expire() {
        let now = Instant::now();
        let store = store_with("state-a", "bind-a", now);
        assert_eq!(
            store.take("state-a", Some("bind-a"), now + TTL).err(),
            Some(FlowError::Expired)
        );
        let store = store_with("state-a", "bind-a", now);
        assert!(
            store
                .take(
                    "state-a",
                    Some("bind-a"),
                    now + TTL - Duration::from_secs(1)
                )
                .is_ok()
        );
    }

    #[test]
    fn the_store_is_bounded_and_prunes_expired_attempts() {
        let now = Instant::now();
        let store = PendingStore::default();
        for i in 0..MAX_PENDING {
            store
                .insert(&format!("s{i}"), "b", "n".into(), "v".into(), now)
                .expect("room");
        }
        assert_eq!(
            store.insert("one-too-many", "b", "n".into(), "v".into(), now),
            Err(FlowError::Full)
        );
        // Once the old ones expired there is room again.
        store
            .insert("later", "b", "n".into(), "v".into(), now + TTL)
            .expect("pruned");
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn the_binding_cookie_is_scoped_lax_and_http_only() {
        let c = binding_cookie("abc", true);
        assert!(c.starts_with("superhet_oidc=abc; Path=/api/v1/auth/oidc;"));
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Lax") && c.ends_with("; Secure"));
        assert!(!binding_cookie("abc", false).contains("Secure"));
        assert!(clearing_binding_cookie(false).contains("Max-Age=0"));
    }
}
