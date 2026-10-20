//! Bus configuration read from the environment.

use std::fmt;

/// Environment variable holding the NATS server URL.
pub const ENV_URL: &str = "SUPERHET_NATS_URL";
/// Environment variable holding the NATS user name.
pub const ENV_USER: &str = "SUPERHET_NATS_USER";
/// Environment variable holding the NATS password.
pub const ENV_PASSWORD: &str = "SUPERHET_NATS_PASSWORD";
/// Environment variable holding an nkey seed (alternative to user/password).
pub const ENV_NKEY_SEED: &str = "SUPERHET_NATS_NKEY_SEED";

/// Invalid bus configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Only one of user and password is set.
    #[error("{ENV_USER} and {ENV_PASSWORD} must be set together")]
    IncompleteUserPassword,
    /// Both user/password and nkey credentials are set.
    #[error("set either {ENV_USER}/{ENV_PASSWORD} or {ENV_NKEY_SEED}, not both")]
    AmbiguousAuth,
}

/// How to authenticate against the server.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    /// No credentials.
    None,
    /// User name and password.
    UserPassword {
        /// User name.
        user: String,
        /// Password.
        password: String,
    },
    /// nkey seed (private key).
    Nkey {
        /// The `S...` seed string.
        seed: String,
    },
}

// Manual impl so secrets never end up in logs.
impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("Auth::None"),
            Self::UserPassword { user, .. } => f
                .debug_struct("Auth::UserPassword")
                .field("user", user)
                .finish_non_exhaustive(),
            Self::Nkey { .. } => f.write_str("Auth::Nkey { .. }"),
        }
    }
}

/// Connection settings for the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusConfig {
    /// Server URL, e.g. `nats://nats:4222`.
    pub url: String,
    /// Credentials.
    pub auth: Auth,
    /// Client name reported to the server.
    pub client_name: String,
}

impl BusConfig {
    /// Read the configuration from the process environment.
    ///
    /// Returns `Ok(None)` when `SUPERHET_NATS_URL` is unset or empty.
    ///
    /// # Errors
    /// Fails when the credential variables are inconsistent.
    pub fn from_env(client_name: &str) -> Result<Option<Self>, ConfigError> {
        Self::from_lookup(client_name, |key| std::env::var(key).ok())
    }

    /// Like [`Self::from_env`] with an injectable variable lookup.
    ///
    /// # Errors
    /// Fails when the credential variables are inconsistent.
    pub fn from_lookup(
        client_name: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, ConfigError> {
        let get = |key: &str| lookup(key).filter(|v| !v.is_empty());
        let Some(url) = get(ENV_URL) else {
            return Ok(None);
        };
        let auth = match (get(ENV_USER), get(ENV_PASSWORD), get(ENV_NKEY_SEED)) {
            (None, None, None) => Auth::None,
            (Some(user), Some(password), None) => Auth::UserPassword { user, password },
            (None, None, Some(seed)) => Auth::Nkey { seed },
            (Some(_), Some(_), Some(_)) => return Err(ConfigError::AmbiguousAuth),
            _ => return Err(ConfigError::IncompleteUserPassword),
        };
        Ok(Some(Self {
            url,
            auth,
            client_name: client_name.to_owned(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn unset_url_means_disabled() {
        assert!(
            BusConfig::from_lookup("t", lookup(&[]))
                .expect("valid")
                .is_none()
        );
    }

    #[test]
    fn user_password_is_parsed() {
        let cfg = BusConfig::from_lookup(
            "t",
            lookup(&[
                (ENV_URL, "nats://x:4222"),
                (ENV_USER, "u"),
                (ENV_PASSWORD, "p"),
            ]),
        )
        .expect("valid")
        .expect("enabled");
        assert_eq!(
            cfg.auth,
            Auth::UserPassword {
                user: "u".into(),
                password: "p".into()
            }
        );
    }

    #[test]
    fn half_credentials_are_rejected() {
        let err = BusConfig::from_lookup("t", lookup(&[(ENV_URL, "nats://x"), (ENV_USER, "u")]));
        assert!(matches!(err, Err(ConfigError::IncompleteUserPassword)));
    }

    #[test]
    fn debug_hides_secrets() {
        let auth = Auth::UserPassword {
            user: "u".into(),
            password: "hunter2".into(),
        };
        assert!(!format!("{auth:?}").contains("hunter2"));
    }
}
