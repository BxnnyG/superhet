//! Versioned NATS subject constants and builders.
//!
//! All subjects live under `sh.v1`. A breaking change to a payload or subject
//! layout bumps the version token (`sh.v2`) so old and new peers can coexist.
//! Every namespace has its own second token, so ids never collide.

/// Root of every subject, including the protocol version.
pub const PREFIX: &str = "sh.v1";

/// Wildcard matching every control command of every device.
pub const ALL_CTL: &str = "sh.v1.dev.*.ctl.>";
/// Wildcard matching the state subject of every device.
pub const ALL_STATE: &str = "sh.v1.dev.*.state";
/// Wildcard matching the spectrum subject of every device.
pub const ALL_SPECTRUM: &str = "sh.v1.dev.*.spectrum";
/// Wildcard matching every audio stream of every device.
pub const ALL_AUDIO: &str = "sh.v1.dev.*.audio.*";
/// Wildcard matching every plugin event.
pub const ALL_EVENTS: &str = "sh.v1.events.>";
/// Wildcard matching every lookup request.
pub const ALL_LOOKUP: &str = "sh.v1.lookup.>";
/// Wildcard matching the control subject of every plugin.
pub const ALL_PLUGIN_CTL: &str = "sh.v1.plugin.*.ctl";

/// A subject token is invalid if it is empty or contains NATS metacharacters.
fn is_valid_token(token: &str) -> bool {
    !token.is_empty()
        && !token
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '.' | '*' | '>'))
}

/// Subject construction failed because a token is not usable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid subject token {0:?}")]
pub struct InvalidToken(pub String);

fn check(token: &str) -> Result<(), InvalidToken> {
    if is_valid_token(token) {
        Ok(())
    } else {
        Err(InvalidToken(token.to_owned()))
    }
}

/// Control command subject of a device: `sh.v1.dev.<dev>.ctl.<cmd>`.
///
/// # Errors
/// Fails if a token is not a single valid token.
pub fn ctl(device: &str, cmd: &str) -> Result<String, InvalidToken> {
    check(device)?;
    check(cmd)?;
    Ok(format!("{PREFIX}.dev.{device}.ctl.{cmd}"))
}

/// State subject of a device: `sh.v1.dev.<dev>.state`.
///
/// # Errors
/// Fails if `device` is not a single valid token.
pub fn state(device: &str) -> Result<String, InvalidToken> {
    check(device)?;
    Ok(format!("{PREFIX}.dev.{device}.state"))
}

/// Spectrum subject of a device: `sh.v1.dev.<dev>.spectrum`.
///
/// # Errors
/// Fails if `device` is not a single valid token.
pub fn spectrum(device: &str) -> Result<String, InvalidToken> {
    check(device)?;
    Ok(format!("{PREFIX}.dev.{device}.spectrum"))
}

/// Audio subject of a stream: `sh.v1.dev.<dev>.audio.<stream>`.
///
/// # Errors
/// Fails if a token is invalid.
pub fn audio(device: &str, stream: &str) -> Result<String, InvalidToken> {
    check(device)?;
    check(stream)?;
    Ok(format!("{PREFIX}.dev.{device}.audio.{stream}"))
}

/// Plugin event subject: `sh.v1.events.<plugin>.<type>`.
///
/// # Errors
/// Fails if a token is invalid.
pub fn event(plugin: &str, event_type: &str) -> Result<String, InvalidToken> {
    check(plugin)?;
    check(event_type)?;
    Ok(format!("{PREFIX}.events.{plugin}.{event_type}"))
}

/// Lookup request subject: `sh.v1.lookup.<key>`.
///
/// # Errors
/// Fails if `key` is not a single valid token.
pub fn lookup(key: &str) -> Result<String, InvalidToken> {
    check(key)?;
    Ok(format!("{PREFIX}.lookup.{key}"))
}

/// Plugin control subject: `sh.v1.plugin.<id>.ctl`.
///
/// # Errors
/// Fails if `id` is not a single valid token.
pub fn plugin_ctl(id: &str) -> Result<String, InvalidToken> {
    check(id)?;
    Ok(format!("{PREFIX}.plugin.{id}.ctl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_versioned_subjects() {
        assert_eq!(
            ctl("rtl0", "tune").as_deref(),
            Ok("sh.v1.dev.rtl0.ctl.tune")
        );
        assert_eq!(state("rtl0").as_deref(), Ok("sh.v1.dev.rtl0.state"));
        assert_eq!(spectrum("rtl0").as_deref(), Ok("sh.v1.dev.rtl0.spectrum"));
        assert_eq!(
            audio("rtl0", "main").as_deref(),
            Ok("sh.v1.dev.rtl0.audio.main")
        );
        assert_eq!(
            event("adsb", "position").as_deref(),
            Ok("sh.v1.events.adsb.position")
        );
        assert_eq!(lookup("icao").as_deref(), Ok("sh.v1.lookup.icao"));
        assert_eq!(plugin_ctl("adsb").as_deref(), Ok("sh.v1.plugin.adsb.ctl"));
    }

    #[test]
    fn rejects_wildcards_dots_and_empty_tokens() {
        for bad in ["", "a.b", "*", ">", "a b"] {
            assert!(ctl(bad, "x").is_err(), "{bad:?}");
            assert!(ctl("d", bad).is_err(), "{bad:?}");
            assert!(event("p", bad).is_err(), "{bad:?}");
            assert!(lookup(bad).is_err(), "{bad:?}");
            assert!(plugin_ctl(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn any_device_id_is_allowed() {
        assert_eq!(state("events").as_deref(), Ok("sh.v1.dev.events.state"));
    }

    #[test]
    fn wildcards_match_builder_shape() {
        assert!(ALL_CTL.starts_with("sh.v1.dev."));
        assert_eq!(ALL_EVENTS, format!("{PREFIX}.events.>"));
    }
}
