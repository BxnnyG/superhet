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
//! - `sh.v1.events.<plugin>.<type>` – plugin events
//! - `sh.v1.lookup.<key>` – lookup requests
//! - `sh.v1.plugin.<id>.ctl` – plugin control

pub mod audio;
pub mod config;
#[cfg(feature = "opus")]
pub mod opus;
pub mod profile;
pub mod rds;
pub mod spectrum;
pub mod subjects;
pub mod zoom;

pub use config::{Auth, BusConfig, ConfigError};

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

fn options(config: &BusConfig) -> async_nats::ConnectOptions {
    let options = async_nats::ConnectOptions::new().name(config.client_name.as_str());
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
    fn backoff_grows_and_is_capped() {
        assert_eq!(backoff(0), Duration::from_millis(250));
        assert_eq!(backoff(1), Duration::from_millis(250));
        assert_eq!(backoff(2), Duration::from_millis(500));
        assert_eq!(backoff(3), Duration::from_secs(1));
        assert_eq!(backoff(100), BACKOFF_MAX);
        assert_eq!(backoff(usize::MAX), BACKOFF_MAX);
    }
}
