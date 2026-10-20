//! NATS bus layer shared by all Superhet services.
//!
//! Subject layout (version 1, prefix [`subjects::PREFIX`]):
//!
//! - `sh.v1.dev.<dev>.ctl.<cmd>` – control commands to the daemon owning `<dev>`
//! - `sh.v1.dev.<dev>.state` – device state
//! - `sh.v1.dev.<dev>.spectrum` – spectrum frames
//! - `sh.v1.dev.<dev>.audio.<stream>` – demodulated audio streams
//! - `sh.v1.events.<plugin>.<type>` – plugin events
//! - `sh.v1.lookup.<key>` – lookup requests
//! - `sh.v1.plugin.<id>.ctl` – plugin control

pub mod config;
pub mod subjects;

pub use config::{Auth, BusConfig, ConfigError};

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

/// Connect to NATS using the given configuration.
///
/// # Errors
/// Fails if the server is unreachable or rejects the credentials.
pub async fn connect(config: &BusConfig) -> Result<async_nats::Client, Error> {
    let mut options = async_nats::ConnectOptions::new().name(config.client_name.as_str());
    options = match &config.auth {
        Auth::None => options,
        Auth::UserPassword { user, password } => {
            options.user_and_password(user.clone(), password.clone())
        }
        Auth::Nkey { seed } => options.nkey(seed.clone()),
    };
    Ok(options.connect(config.url.as_str()).await?)
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
