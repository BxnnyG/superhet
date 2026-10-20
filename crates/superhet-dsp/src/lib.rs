//! Digital signal processing for Superhet: FFT, DDC, demodulators and resampling.

/// Returns the crate name; placeholder until real functionality lands.
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_set() {
        assert_eq!(crate_name(), "superhet-dsp");
    }
}
