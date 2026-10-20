//! Environment configuration of the daemon.

use superhet_bus::subjects;

/// Errors in the environment configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A variable holds an unusable value.
    #[error("{var}: {reason}")]
    Invalid {
        /// Variable name.
        var: &'static str,
        /// What is wrong with it.
        reason: String,
    },
}

/// Spectrum pipeline settings.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumSettings {
    /// FFT length.
    pub fft_size: usize,
    /// Published bins per frame.
    pub bins: usize,
    /// Target frames per second.
    pub fps: f32,
    /// dBFS mapped to quantised 0.
    pub min_db: f32,
    /// dBFS mapped to quantised 255.
    pub max_db: f32,
}

/// Daemon configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Device spec (`SUPERHET_DEVICE`); `None` runs health-only.
    pub device_spec: Option<String>,
    /// Device id used in subjects (`SUPERHET_DEVICE_ID`, default `dev0`).
    pub device_id: String,
    /// Spectrum settings.
    pub spectrum: SpectrumSettings,
}

fn parse<T: std::str::FromStr>(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
    default: T,
) -> Result<T, ConfigError> {
    match get(var) {
        None => Ok(default),
        Some(v) => v.trim().parse().map_err(|_| ConfigError::Invalid {
            var,
            reason: format!("cannot parse {v:?}"),
        }),
    }
}

impl Config {
    /// Read from the process environment.
    ///
    /// # Errors
    /// Fails on unparsable or inconsistent values.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Read through an arbitrary variable lookup (testable).
    ///
    /// # Errors
    /// Fails on unparsable or inconsistent values.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let device_id = get("SUPERHET_DEVICE_ID").unwrap_or_else(|| "dev0".to_owned());
        subjects::spectrum(&device_id).map_err(|e| ConfigError::Invalid {
            var: "SUPERHET_DEVICE_ID",
            reason: e.to_string(),
        })?;
        let spectrum = SpectrumSettings {
            fft_size: parse(&get, "SUPERHET_FFT_SIZE", 4096)?,
            bins: parse(&get, "SUPERHET_SPECTRUM_BINS", 2048)?,
            fps: parse(&get, "SUPERHET_SPECTRUM_FPS", 25.0)?,
            min_db: parse(&get, "SUPERHET_SPECTRUM_MIN_DB", -120.0)?,
            max_db: parse(&get, "SUPERHET_SPECTRUM_MAX_DB", 0.0)?,
        };
        if spectrum.min_db >= spectrum.max_db {
            return Err(ConfigError::Invalid {
                var: "SUPERHET_SPECTRUM_MIN_DB",
                reason: "must be below SUPERHET_SPECTRUM_MAX_DB".into(),
            });
        }
        Ok(Self {
            device_spec: get("SUPERHET_DEVICE").filter(|s| !s.trim().is_empty()),
            device_id,
            spectrum,
        })
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
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults() {
        let c = Config::from_lookup(lookup(&[])).expect("defaults");
        assert_eq!(c.device_spec, None);
        assert_eq!(c.device_id, "dev0");
        assert_eq!(c.spectrum.fft_size, 4096);
        assert_eq!(c.spectrum.bins, 2048);
    }

    #[test]
    fn overrides_and_validation() {
        let c = Config::from_lookup(lookup(&[
            ("SUPERHET_DEVICE", "file:/x.cu8?rate=1"),
            ("SUPERHET_DEVICE_ID", "rtl1"),
            ("SUPERHET_SPECTRUM_FPS", "10"),
        ]))
        .expect("valid");
        assert_eq!(c.device_spec.as_deref(), Some("file:/x.cu8?rate=1"));
        assert_eq!(c.spectrum.fps, 10.0);
        assert!(Config::from_lookup(lookup(&[("SUPERHET_DEVICE_ID", "a.b")])).is_err());
        assert!(Config::from_lookup(lookup(&[("SUPERHET_FFT_SIZE", "big")])).is_err());
        assert!(Config::from_lookup(lookup(&[("SUPERHET_SPECTRUM_MIN_DB", "5")])).is_err());
    }
}
