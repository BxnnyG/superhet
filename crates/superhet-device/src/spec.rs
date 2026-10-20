//! Parsing of the `SUPERHET_DEVICE` specification string.

use crate::{Device, Error, FileConfig, FileDevice};

/// Open a device from a spec such as
/// `file:/path/to/capture.cu8?format=cu8&rate=2048000&freq=100000000`.
///
/// # Errors
/// Fails on unknown schemes, malformed parameters or unreadable files.
pub fn open_from_spec(spec: &str) -> Result<Box<dyn Device>, Error> {
    match spec.split_once(':') {
        Some(("file", rest)) => Ok(Box::new(FileDevice::open(FileConfig::parse(rest)?)?)),
        Some(("rtlsdr", rest)) => open_rtlsdr(rest),
        _ => Err(Error::Config(format!(
            "unsupported device spec {spec:?} (expected file:<path>?... or rtlsdr:<index|serial>?...)"
        ))),
    }
}

#[cfg(feature = "rtlsdr")]
fn open_rtlsdr(rest: &str) -> Result<Box<dyn Device>, Error> {
    Ok(Box::new(crate::RtlSdrDevice::open(
        crate::RtlSdrConfig::parse(rest)?,
    )?))
}

#[cfg(not(feature = "rtlsdr"))]
fn open_rtlsdr(rest: &str) -> Result<Box<dyn Device>, Error> {
    // Still validate the spec so typos are reported before the feature hint.
    crate::RtlSdrConfig::parse(rest)?;
    Err(Error::Config(
        "this build has no RTL-SDR support (rebuild with --features rtlsdr)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_scheme() {
        assert!(matches!(
            open_from_spec("nonsense:0").map(|_| ()),
            Err(Error::Config(_))
        ));
        assert!(matches!(
            open_from_spec("nonsense").map(|_| ()),
            Err(Error::Config(_))
        ));
    }

    #[test]
    #[cfg(not(feature = "rtlsdr"))]
    fn rtlsdr_without_feature_is_config_error() {
        assert!(matches!(
            open_from_spec("rtlsdr:0").map(|_| ()),
            Err(Error::Config(m)) if m.contains("--features rtlsdr")
        ));
        assert!(matches!(
            open_from_spec("rtlsdr:0?rate=1").map(|_| ()),
            Err(Error::Config(m)) if m.contains("sample rate")
        ));
    }
}
