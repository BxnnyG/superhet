//! NATS bus layer shared by all Superhet services.
//!
//! Subject layout (version 1, prefix [`subjects::PREFIX`]):
//!
//! - `sh.v1.dev.<dev>.ctl.<cmd>` – control commands to the daemon owning `<dev>`
//! - `sh.v1.dev.<dev>.state` – device state (also kept in KV bucket `STATE`, key `dev.<dev>`)
//! - `sh.v1.dev.<dev>.spectrum` – spectrum frames
//! - `sh.v1.dev.<dev>.spectrum.zoom` – zoom spectrum frames (narrow slice, real resolution)
//! - `sh.v1.dev.<dev>.audio.<stream>` – demodulated audio streams
//! - `sh.v1.dev.<dev>.rds.<vfo>` – decoded RDS state of a WFM VFO (JSON, see [`rds`])
//! - `sh.v1.dev.<dev>.tones.<vfo>` – CTCSS tone and DTMF digits of an NFM / AM VFO (JSON, see [`tones`])
//! - `sh.v1.events.core.decoder.<dev>` – stored decoder history events (JSON, published by the worker)
//! - `sh.v1.events.<plugin>.<type>` – plugin events
//! - `sh.v1.audit.<source>` – system audit events (JSON, see [`audit`]; persisted by the worker)
//! - `sh.v1.lookup.<key>` – lookup requests
//! - `sh.v1.plugin.<id>.ctl` – plugin control

pub mod audio;
pub mod audit;
pub mod config;
pub mod metrics;
#[cfg(feature = "opus")]
pub mod opus;
pub mod profile;
pub mod rds;
pub mod spectrum;
pub mod subjects;
pub mod tones;
pub mod zoom;

pub use config::{Auth, BusConfig, ConfigError};

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Errors raised while connecting to the bus.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration is incomplete or inconsistent.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The NATS connection could not be established.
    #[error("nats connect failed: {0}")]
    Connect(#[from] async_nats::ConnectError),
}

/// First reconnect delay.
const BACKOFF_BASE: Duration = Duration::from_millis(250);
/// Upper bound of the reconnect delay.
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Reconnect delay for the given attempt number: exponential from 250 ms, capped at 10 s.
#[must_use]
pub fn backoff(attempts: usize) -> Duration {
    let shift = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(u32::MAX)
        .min(16);
    BACKOFF_BASE.saturating_mul(1u32 << shift).min(BACKOFF_MAX)
}

/// Maximum number of distinct permission violations remembered (and logged) per process.
const MAX_REPORTED_VIOLATIONS: usize = 32;

/// Whether a server `-ERR` text is a NATS permission violation.
fn is_permission_violation(message: &str) -> bool {
    message
        .to_ascii_lowercase()
        .contains("permissions violation")
}

/// Log every distinct permission violation once at ERROR with an update hint; repeats (a denied
/// 20 Hz publish would otherwise flood the log) are only counted by the server.
fn report_permission_violation(seen: &Mutex<HashSet<String>>, client: &str, message: &str) {
    let first = seen
        .lock()
        .map(|mut seen| seen.len() < MAX_REPORTED_VIOLATIONS && seen.insert(message.to_owned()))
        .unwrap_or(false);
    if first {
        tracing::error!(
            client,
            "NATS denied an operation ({message}): compose.yaml/nats.conf is older than this image; \
             rerun the installer with --update (curl -fsSL \
             https://raw.githubusercontent.com/BxnnyG/superhet/main/scripts/install.sh | sh -s -- --update)"
        );
    }
}

fn options(config: &BusConfig) -> async_nats::ConnectOptions {
    let client = config.client_name.clone();
    let seen = Arc::new(Mutex::new(HashSet::new()));
    let options = async_nats::ConnectOptions::new()
        .name(config.client_name.as_str())
        .event_callback(move |event| {
            let seen = Arc::clone(&seen);
            let client = client.clone();
            async move {
                // async_nats only reports server errors at INFO level, which hides a stale NATS
                // config (e.g. a new subject the old permissions do not allow) in the noise.
                if let async_nats::Event::ServerError(async_nats::ServerError::Other(message)) =
                    &event
                    && is_permission_violation(message)
                {
                    report_permission_violation(&seen, &client, message);
                }
            }
        });
    match &config.auth {
        Auth::None => options,
        Auth::UserPassword { user, password } => {
            options.user_and_password(user.clone(), password.clone())
        }
        Auth::Nkey { seed } => options.nkey(seed.clone()),
    }
}

/// Connect to NATS using the given configuration.
///
/// # Errors
/// Fails if the server is unreachable or rejects the credentials.
pub async fn connect(config: &BusConfig) -> Result<async_nats::Client, Error> {
    Ok(options(config).connect(config.url.as_str()).await?)
}

/// Create a client that connects in the background and never gives up.
///
/// Returns immediately, even if the server is down. Connecting and reconnecting retry with
/// [`backoff`]. Check [`async_nats::Client::connection_state`] before publishing: while the
/// connection is not up, publishes are not delivered and may eventually block.
///
/// # Errors
/// Fails only on a malformed server URL.
pub async fn connect_background(config: &BusConfig) -> Result<async_nats::Client, Error> {
    Ok(options(config)
        .retry_on_initial_connect()
        .reconnect_delay_callback(backoff)
        .max_reconnects(None)
        .connect(config.url.as_str())
        .await?)
}

/// Connect using [`BusConfig::from_env`]; returns `None` when `SUPERHET_NATS_URL` is unset.
///
/// # Errors
/// Fails on invalid configuration or connection errors.
pub async fn connect_from_env(client_name: &str) -> Result<Option<async_nats::Client>, Error> {
    match BusConfig::from_env(client_name)? {
        Some(config) => Ok(Some(connect(&config).await?)),
        None => Ok(None),
    }
}

/// Like [`connect_from_env`] but via [`connect_background`]: does not wait for the server.
///
/// # Errors
/// Fails on invalid configuration or a malformed URL.
pub async fn connect_background_from_env(
    client_name: &str,
) -> Result<Option<async_nats::Client>, Error> {
    match BusConfig::from_env(client_name)? {
        Some(config) => Ok(Some(connect_background(&config).await?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_permission_violations() {
        assert!(is_permission_violation(
            "Permissions Violation for Publish to \"sh.v1.dev.dev0.rds.1\""
        ));
        assert!(!is_permission_violation("Authorization Violation"));
        assert!(!is_permission_violation("Slow Consumer"));
    }

    #[test]
    fn reports_each_violation_once_and_is_bounded() {
        let seen = Mutex::new(HashSet::new());
        report_permission_violation(&seen, "t", "Permissions Violation for Publish to \"a\"");
        report_permission_violation(&seen, "t", "Permissions Violation for Publish to \"a\"");
        assert_eq!(seen.lock().map(|s| s.len()).unwrap_or(0), 1);
        for i in 0..100 {
            report_permission_violation(&seen, "t", &format!("Permissions Violation {i}"));
        }
        assert_eq!(
            seen.lock().map(|s| s.len()).unwrap_or(0),
            MAX_REPORTED_VIOLATIONS
        );
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        assert_eq!(backoff(0), Duration::from_millis(250));
        assert_eq!(backoff(1), Duration::from_millis(250));
        assert_eq!(backoff(2), Duration::from_millis(500));
        assert_eq!(backoff(3), Duration::from_secs(1));
        assert_eq!(backoff(100), BACKOFF_MAX);
        assert_eq!(backoff(usize::MAX), BACKOFF_MAX);
    }
}
