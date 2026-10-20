//! Versioned plugin contract for Superhet: manifest and wire types shared by
//! the server and out-of-process plugins.

/// Version of the plugin protocol. `0` means unstable pre-alpha.
pub const PROTOCOL_VERSION: u32 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_pre_alpha() {
        assert_eq!(PROTOCOL_VERSION, 0);
    }
}
