//! Parsing and validation of the `rtlsdr:` device specification.
//!
//! Kept independent of librtlsdr so it is compiled and tested without the `rtlsdr` feature.

use crate::Error;

/// Default sample rate in Hz.
pub(crate) const DEFAULT_RATE: u32 = 2_048_000;
/// Default samples per emitted block.
const DEFAULT_BLOCK_LEN: usize = 16_384;

/// Which dongle to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelector {
    /// Zero-based index in librtlsdr's enumeration order.
    Index(u32),
    /// EEPROM serial string.
    Serial(String),
}

/// Tuner gain setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gain {
    /// Tuner AGC.
    Auto,
    /// Manual gain in dB (snapped to the nearest value the tuner supports).
    Db(f32),
}

/// Configuration of an RTL-SDR device.
#[derive(Debug, Clone, PartialEq)]
pub struct RtlSdrConfig {
    /// Dongle to open.
    pub selector: DeviceSelector,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Initial centre frequency in Hz.
    pub center_hz: u64,
    /// Tuner gain.
    pub gain: Gain,
    /// Crystal frequency correction in ppm.
    pub ppm: i32,
    /// Enable the bias tee (antenna power).
    pub bias_tee: bool,
    /// Samples per emitted block.
    pub block_len: usize,
}

/// Check a sample rate against the ranges the RTL2832U supports
/// (225001-300000 and 900001-3200000 Hz).
///
/// # Errors
/// Fails if the rate is outside both ranges.
pub fn validate_rate(rate: u32) -> Result<(), Error> {
    if (225_001..=300_000).contains(&rate) || (900_001..=3_200_000).contains(&rate) {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "sample rate {rate} outside 225001-300000 and 900001-3200000 Hz"
        )))
    }
}

impl RtlSdrConfig {
    /// Parse `[<index|serial>][?rate=<Hz>&freq=<Hz>&gain=auto|<dB>&ppm=<n>&bias=0|1&block=<samples>]`.
    /// An empty selector means index 0; an all-digit selector is an index, anything else a serial.
    ///
    /// # Errors
    /// Fails on unknown keys, unparsable values or an unsupported sample rate.
    pub fn parse(rest: &str) -> Result<Self, Error> {
        let (sel, query) = rest.split_once('?').unwrap_or((rest, ""));
        let selector = if sel.is_empty() {
            DeviceSelector::Index(0)
        } else if let Ok(index) = sel.parse::<u32>() {
            DeviceSelector::Index(index)
        } else {
            DeviceSelector::Serial(sel.to_owned())
        };
        let mut cfg = Self {
            selector,
            sample_rate: DEFAULT_RATE,
            center_hz: 100_000_000,
            gain: Gain::Auto,
            ppm: 0,
            bias_tee: false,
            block_len: DEFAULT_BLOCK_LEN,
        };
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| Error::Config(format!("malformed parameter {pair:?}")))?;
            let bad = || Error::Config(format!("invalid value for {key}: {value:?}"));
            match key {
                "rate" => cfg.sample_rate = value.parse().map_err(|_| bad())?,
                "freq" => cfg.center_hz = value.parse().map_err(|_| bad())?,
                "gain" => {
                    cfg.gain = if value == "auto" {
                        Gain::Auto
                    } else {
                        let db: f32 = value.parse().map_err(|_| bad())?;
                        if !db.is_finite() {
                            return Err(bad());
                        }
                        Gain::Db(db)
                    }
                }
                "ppm" => cfg.ppm = value.parse().map_err(|_| bad())?,
                "bias" => {
                    cfg.bias_tee = match value {
                        "0" => false,
                        "1" => true,
                        _ => return Err(bad()),
                    }
                }
                "block" => cfg.block_len = value.parse().map_err(|_| bad())?,
                _ => return Err(Error::Config(format!("unknown parameter {key:?}"))),
            }
        }
        validate_rate(cfg.sample_rate)?;
        if cfg.block_len == 0 {
            return Err(Error::Config("block must be non-zero".into()));
        }
        if cfg.center_hz > u64::from(u32::MAX) {
            return Err(Error::Config("freq exceeds 32 bit".into()));
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = RtlSdrConfig::parse("").expect("valid spec");
        assert_eq!(c.selector, DeviceSelector::Index(0));
        assert_eq!(c.sample_rate, 2_048_000);
        assert_eq!(c.gain, Gain::Auto);
    }

    #[test]
    fn full_spec() {
        let c = RtlSdrConfig::parse(
            "RTL001?rate=1024000&freq=145500000&gain=28.5&ppm=-3&bias=1&block=4096",
        )
        .expect("valid spec");
        assert_eq!(c.selector, DeviceSelector::Serial("RTL001".into()));
        assert_eq!(c.sample_rate, 1_024_000);
        assert_eq!(c.center_hz, 145_500_000);
        assert_eq!(c.gain, Gain::Db(28.5));
        assert_eq!(c.ppm, -3);
        assert!(c.bias_tee);
        assert_eq!(c.block_len, 4096);
        assert_eq!(
            RtlSdrConfig::parse("2").expect("valid spec").selector,
            DeviceSelector::Index(2)
        );
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "0?rate=500000",
            "0?rate=3300000",
            "0?rate=abc",
            "0?gain=loud",
            "0?bias=2",
            "0?block=0",
            "0?wat=1",
            "0?freq",
            "0?freq=5000000000",
        ] {
            assert!(
                matches!(RtlSdrConfig::parse(bad), Err(Error::Config(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn rate_ranges() {
        for ok in [225_001, 300_000, 900_001, 3_200_000] {
            assert!(validate_rate(ok).is_ok(), "{ok}");
        }
        for bad in [225_000, 300_001, 900_000, 3_200_001] {
            assert!(validate_rate(bad).is_err(), "{bad}");
        }
    }
}
