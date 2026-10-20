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
        _ => Err(Error::Config(format!(
            "unsupported device spec {spec:?} (expected file:<path>?...)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_scheme() {
        assert!(matches!(
            open_from_spec("rtlsdr:0").map(|_| ()),
            Err(Error::Config(_))
        ));
        assert!(matches!(
            open_from_spec("nonsense").map(|_| ()),
            Err(Error::Config(_))
        ));
    }
}
