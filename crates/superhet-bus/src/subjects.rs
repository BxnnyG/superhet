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
pub const ALL_AUDIO: &str = "sh.v1.dev.*.audio.>";
/// Wildcard matching the RDS state of every VFO of every device.
pub const ALL_RDS: &str = "sh.v1.dev.*.rds.>";
/// Wildcard matching the tone state (CTCSS, DTMF) of every VFO of every device.
pub const ALL_TONES: &str = "sh.v1.dev.*.tones.>";
/// Wildcard matching every plugin event.
pub const ALL_EVENTS: &str = "sh.v1.events.>";
/// Wildcard matching the decoder history events of every device.
pub const ALL_DECODER_EVENTS: &str = "sh.v1.events.core.decoder.>";
/// Wildcard matching the system audit events of every source (ADR-0012).
pub const ALL_AUDIT: &str = "sh.v1.audit.>";
/// Wildcard matching every lookup request.
pub const ALL_LOOKUP: &str = "sh.v1.lookup.>";
/// Wildcard matching every control command of the plugin host.
pub const ALL_PLUGIN_CTL: &str = "sh.v1.plugin.ctl.>";
/// State changes of plugin instances (JSON `InstanceInfo`, core NATS).
pub const PLUGIN_STATE: &str = "sh.v1.plugin.state";

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

/// Subscription pattern for every control command of one device: `sh.v1.dev.<dev>.ctl.>`.
///
/// # Errors
/// Fails if `device` is not a single valid token.
pub fn ctl_all(device: &str) -> Result<String, InvalidToken> {
    check(device)?;
    Ok(format!("{PREFIX}.dev.{device}.ctl.>"))
}

/// Device VFO control subject: `sh.v1.dev.<dev>.ctl.vfo.<action>` with `action` one of
/// `create`, `update`, `delete`.
///
/// # Errors
/// Fails if a token is invalid.
pub fn ctl_vfo(device: &str, action: &str) -> Result<String, InvalidToken> {
    check(device)?;
    check(action)?;
    Ok(format!("{PREFIX}.dev.{device}.ctl.vfo.{action}"))
}

/// Device lock subject: `sh.v1.dev.<dev>.ctl.lock.<action>` with `action` one of `acquire`,
/// `release`.
///
/// # Errors
/// Fails if a token is invalid.
pub fn ctl_lock(device: &str, action: &str) -> Result<String, InvalidToken> {
    check(device)?;
    check(action)?;
    Ok(format!("{PREFIX}.dev.{device}.ctl.lock.{action}"))
}

/// Profile subject: `sh.v1.dev.<dev>.ctl.profile.<action>` with `action` `load`.
///
/// # Errors
/// Fails if a token is invalid.
pub fn ctl_profile(device: &str, action: &str) -> Result<String, InvalidToken> {
    check(device)?;
    check(action)?;
    Ok(format!("{PREFIX}.dev.{device}.ctl.profile.{action}"))
}

/// Name of the JetStream KV bucket holding the latest state of every device.
pub const STATE_BUCKET: &str = "STATE";

/// Key of a device in the [`STATE_BUCKET`]: `dev.<dev>`.
///
/// Stricter than a subject token because KV keys only allow `A-Za-z0-9`, `-`, `_` and `=`
/// within a token.
///
/// # Errors
/// Fails if `device` is not a single valid token or contains other characters.
pub fn state_key(device: &str) -> Result<String, InvalidToken> {
    check(device)?;
    if !device
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '='))
    {
        return Err(InvalidToken(device.to_owned()));
    }
    Ok(format!("dev.{device}"))
}

/// Key of the persistent restore snapshot of a device in the [`STATE_BUCKET`]: `restore.<dev>`.
/// Unlike [`state_key`] it survives a graceful daemon shutdown, so the daemon can restore its
/// last tuning, VFOs and lock on start.
///
/// # Errors
/// Same as [`state_key`].
pub fn restore_key(device: &str) -> Result<String, InvalidToken> {
    state_key(device).map(|key| format!("restore.{}", &key["dev.".len()..]))
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

/// Zoom spectrum frames of `device` (see `zoom`): `sh.v1.dev.<dev>.spectrum.zoom`. Not matched
/// by [`ALL_SPECTRUM`], so existing consumers never see them.
///
/// # Errors
/// Fails if `device` is not a valid token.
pub fn spectrum_zoom(device: &str) -> Result<String, InvalidToken> {
    check(device)?;
    Ok(format!("{PREFIX}.dev.{device}.spectrum.zoom"))
}

/// Zoom request (lease) for the daemon owning `device`: `sh.v1.dev.<dev>.ctl.spectrum.zoom`.
///
/// # Errors
/// Fails if `device` is not a valid token.
pub fn ctl_spectrum_zoom(device: &str) -> Result<String, InvalidToken> {
    check(device)?;
    Ok(format!("{PREFIX}.dev.{device}.ctl.spectrum.zoom"))
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

/// RDS state subject of a VFO: `sh.v1.dev.<dev>.rds.<vfo>`.
///
/// # Errors
/// Fails if a token is invalid.
pub fn rds(device: &str, vfo: &str) -> Result<String, InvalidToken> {
    check(device)?;
    check(vfo)?;
    Ok(format!("{PREFIX}.dev.{device}.rds.{vfo}"))
}

/// Tone state subject of a VFO: `sh.v1.dev.<dev>.tones.<vfo>`.
///
/// # Errors
/// Fails if a token is invalid.
pub fn tones(device: &str, vfo: &str) -> Result<String, InvalidToken> {
    check(device)?;
    check(vfo)?;
    Ok(format!("{PREFIX}.dev.{device}.tones.{vfo}"))
}

/// Decoder history event subject (published by the worker for every stored event):
/// `sh.v1.events.core.decoder.<dev>`.
///
/// # Errors
/// Fails if `device` is not a single valid token.
pub fn decoder_events(device: &str) -> Result<String, InvalidToken> {
    check(device)?;
    Ok(format!("{PREFIX}.events.core.decoder.{device}"))
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

/// Event subject of a plugin instance: `sh.v1.events.<plugin>.<type...>`.
///
/// Plugin ids are reverse-DNS and event types dotted; the id becomes one token (dots mapped to
/// `_`, which ids never contain, so the mapping is unambiguous) and the type contributes one
/// token per segment, so `sh.v1.events.org_superhet_example_echo.>` selects one plugin.
///
/// # Errors
/// Fails if the id or a type segment is not a valid token.
pub fn plugin_event(plugin_id: &str, event_type: &str) -> Result<String, InvalidToken> {
    let plugin = plugin_id.replace('.', "_");
    check(&plugin)?;
    event_type.split('.').try_for_each(check)?;
    Ok(format!("{PREFIX}.events.{plugin}.{event_type}"))
}

/// System audit event subject: `sh.v1.audit.<source>` (see [`crate::audit`]).
///
/// # Errors
/// Fails if `source` is not a single valid token.
pub fn audit(source: &str) -> Result<String, InvalidToken> {
    check(source)?;
    Ok(format!("{PREFIX}.audit.{source}"))
}

/// Lookup request subject: `sh.v1.lookup.<key>`.
///
/// # Errors
/// Fails if `key` is not a single valid token.
pub fn lookup(key: &str) -> Result<String, InvalidToken> {
    check(key)?;
    Ok(format!("{PREFIX}.lookup.{key}"))
}

/// Plugin host control subject: `sh.v1.plugin.ctl.<cmd>` (request/reply, `list`, `install`,
/// `instances`, `get`, `start`, `stop`, `restart`). Plugin ids contain dots and cannot be a
/// subject token, so commands name the plugin or instance in the body.
///
/// # Errors
/// Fails if `cmd` is not a single valid token.
pub fn plugin_ctl(cmd: &str) -> Result<String, InvalidToken> {
    check(cmd)?;
    Ok(format!("{PREFIX}.plugin.ctl.{cmd}"))
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
        assert_eq!(audit("sdrd").as_deref(), Ok("sh.v1.audit.sdrd"));
        assert!(audit("a.b").is_err());
        assert_eq!(ctl_all("rtl0").as_deref(), Ok("sh.v1.dev.rtl0.ctl.>"));
        assert_eq!(state_key("rtl-0_a").as_deref(), Ok("dev.rtl-0_a"));
        assert_eq!(spectrum("rtl0").as_deref(), Ok("sh.v1.dev.rtl0.spectrum"));
        assert_eq!(
            spectrum_zoom("rtl0").as_deref(),
            Ok("sh.v1.dev.rtl0.spectrum.zoom")
        );
        assert_eq!(
            ctl_spectrum_zoom("rtl0").as_deref(),
            Ok("sh.v1.dev.rtl0.ctl.spectrum.zoom")
        );
        assert!(spectrum_zoom("a.b").is_err());
        assert!(ctl_spectrum_zoom("*").is_err());
        assert_eq!(
            audio("rtl0", "main").as_deref(),
            Ok("sh.v1.dev.rtl0.audio.main")
        );
        assert_eq!(rds("rtl0", "3").as_deref(), Ok("sh.v1.dev.rtl0.rds.3"));
        assert!(rds("rtl0", "3.4").is_err());
        assert_eq!(tones("rtl0", "3").as_deref(), Ok("sh.v1.dev.rtl0.tones.3"));
        assert!(tones("rtl0", "*").is_err());
        assert_eq!(
            decoder_events("rtl0").as_deref(),
            Ok("sh.v1.events.core.decoder.rtl0")
        );
        assert!(decoder_events("a.b").is_err());
        assert_eq!(
            ctl_vfo("rtl0", "create").as_deref(),
            Ok("sh.v1.dev.rtl0.ctl.vfo.create")
        );
        assert!(ctl_vfo("rtl0", "a.b").is_err());
        assert_eq!(
            ctl_lock("rtl0", "acquire").as_deref(),
            Ok("sh.v1.dev.rtl0.ctl.lock.acquire")
        );
        assert_eq!(
            ctl_profile("rtl0", "load").as_deref(),
            Ok("sh.v1.dev.rtl0.ctl.profile.load")
        );
        assert_eq!(restore_key("rtl0").as_deref(), Ok("restore.rtl0"));
        assert!(restore_key("a.b").is_err());
        assert_eq!(
            event("adsb", "position").as_deref(),
            Ok("sh.v1.events.adsb.position")
        );
        assert_eq!(lookup("icao").as_deref(), Ok("sh.v1.lookup.icao"));
        assert_eq!(plugin_ctl("start").as_deref(), Ok("sh.v1.plugin.ctl.start"));
        assert_eq!(
            plugin_event("org.superhet.example.echo", "echo.block").as_deref(),
            Ok("sh.v1.events.org_superhet_example_echo.echo.block")
        );
        assert!(plugin_event("org.x.y", "a..b").is_err());
        assert!(plugin_event("org.x.y", "a.*").is_err());
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
    fn state_keys_are_restricted_to_kv_safe_characters() {
        for bad in ["", "a.b", "a b", "a:b", "a*", "ä", "a/b"] {
            assert!(state_key(bad).is_err(), "{bad:?}");
        }
        assert!(ctl_all("*").is_err());
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
