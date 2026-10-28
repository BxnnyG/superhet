//! Plugin manifest (`plugin.toml`), contract v1.
//!
//! [`Manifest::from_toml`] parses and validates; a manifest that passes is safe for the host to
//! act on (paths stay inside the plugin directory, exclusive claims are complete, the transport
//! fits the plugin type). The human-readable description is `docs/plugin-contract.md`; the JSON
//! Schema of this structure is `schema/manifest.schema.json`.

use std::path::{Component, Path};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::PROTOCOL_VERSION;

/// File name of the manifest inside a plugin directory.
pub const MANIFEST_FILE: &str = "plugin.toml";

/// Largest manifest the host reads (bytes).
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// A manifest could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// The file is not valid TOML or does not match the structure.
    #[error("manifest syntax: {0}")]
    Parse(String),
    /// The manifest parses but breaks a contract rule; one entry per problem.
    #[error("manifest invalid: {}", .0.join("; "))]
    Invalid(Vec<String>),
}

/// What a plugin is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginType {
    /// Turns samples or audio into events (and optionally audio / voice frames).
    Decoder,
    /// Turns voice frames into PCM.
    Codec,
    /// Resolves identifiers (callsign, ICAO address, ...).
    Lookup,
    /// Frontend module; has no process.
    Ui,
}

/// How the plugin is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// Child process of the plugin host.
    #[default]
    Process,
    /// Separate container (declared in `compose.plugins.yaml`; not started by the host yet).
    Container,
}

/// How the host talks to the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// stdin = samples, stdout = JSON Lines events, stderr = log.
    #[default]
    Simple,
    /// Length-prefixed frames on a Unix domain socket (`SUPERHET_SOCKET`).
    Framed,
}

/// What the host feeds into a decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    /// Complex baseband around the instance frequency.
    Iq,
    /// FM discriminator output.
    Discriminator,
    /// Demodulated audio.
    Audio,
    /// Voice frames of another decoder (codec plugins).
    VoiceFrames,
}

/// Sample encoding on the input stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SampleFormat {
    /// Interleaved 32-bit float I/Q.
    Cf32,
    /// Interleaved signed 16-bit I/Q.
    Cs16,
    /// Interleaved unsigned 8-bit I/Q.
    Cu8,
    /// Signed 16-bit little-endian real samples.
    S16le,
    /// 32-bit float little-endian real samples.
    F32le,
}

/// Identity and licensing (`[plugin]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginMeta {
    /// Reverse-DNS id, at least three lowercase segments (`org.superhet.decoder.pocsag`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Semantic version of the plugin.
    pub version: String,
    /// Contract version this plugin was written for; must equal the host's.
    pub protocol: u32,
    /// SPDX licence expression of the plugin and the tools it wraps.
    pub license: String,
    /// Plugin type.
    #[serde(rename = "type")]
    pub kind: PluginType,
    /// One-line description.
    #[serde(default)]
    pub description: String,
    /// Project page.
    #[serde(default)]
    pub homepage: Option<String>,
}

/// Execution (`[runtime]`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    /// Process or container.
    #[serde(default)]
    pub kind: RuntimeKind,
    /// Simple or framed.
    #[serde(default)]
    pub transport: Transport,
    /// Program and arguments. A first element containing `/` is relative to the plugin directory;
    /// a bare name is looked up in `PATH`. Required for processes, not allowed for UI plugins.
    #[serde(default)]
    pub command: Vec<String>,
    /// Image reference (container plugins).
    #[serde(default)]
    pub image: Option<String>,
    /// Kill and restart the instance when it was silent this long (seconds). Any stdout line
    /// (simple) or any frame (framed) counts as activity. Default: no watchdog (simple), 15 s
    /// (framed).
    #[serde(default)]
    pub heartbeat_timeout_s: Option<u32>,
}

/// Input stream (`[input]`, decoders and codecs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// What is fed.
    pub kind: InputKind,
    /// Sample rate (Hz) of the stream the host delivers.
    pub sample_rate: u32,
    /// Sample encoding.
    pub format: SampleFormat,
    /// Channel bandwidth (Hz) the host cuts out around the instance frequency.
    #[serde(default)]
    pub bandwidth_hz: Option<u32>,
}

/// Device claim (`[device]`).
///
/// An exclusive plugin takes the whole device: the host asks the daemon to retune to the claimed
/// centre and rate, pauses everything that does not fit and restores the previous state on
/// release (ARCHITECTURE 3.4). Enforcement arrives with the claim command of the daemon; the
/// manifest already declares it so installs show it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// Claims the device exclusively and receives the full window as IQ.
    #[serde(default)]
    pub exclusive: bool,
    /// Fixed centre frequency (Hz), e.g. 1 090 000 000 for ADS-B.
    #[serde(default)]
    pub center_hz: Option<u64>,
    /// Lowest centre the instance may choose (e.g. a DAB block); with `center_hz_max`.
    #[serde(default)]
    pub center_hz_min: Option<u64>,
    /// Highest centre the instance may choose.
    #[serde(default)]
    pub center_hz_max: Option<u64>,
    /// Required device sample rate (Hz).
    #[serde(default)]
    pub sample_rate: Option<u32>,
}

/// Audio output (`[output.audio]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PcmSpec {
    /// Sample rate (Hz).
    pub sample_rate: u32,
    /// Channel count (1 or 2).
    pub channels: u8,
}

/// Outputs (`[output]`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// JSON Schema (draft 2020-12) file, relative to the plugin directory, that every event
    /// line must satisfy. Without one only the envelope is checked.
    #[serde(default)]
    pub events_schema: Option<String>,
    /// PCM audio the plugin produces (framed transport).
    #[serde(default)]
    pub audio: Option<PcmSpec>,
    /// Codec ids of the voice frames the plugin emits (framed transport).
    #[serde(default)]
    pub voice_frames: Vec<String>,
}

/// Codec plugin section (`[codec]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodecSection {
    /// Codec ids the plugin decodes.
    pub frames: Vec<String>,
    /// PCM it returns.
    pub pcm: PcmSpec,
}

/// Lookup plugin section (`[lookup]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LookupSection {
    /// Keys it resolves (`dmr_id`, `callsign`, `icao24`, ...).
    pub keys: Vec<String>,
    /// How long the host may cache an answer (seconds).
    pub cache_ttl_s: u32,
    /// Remote calls per minute the host must not exceed.
    #[serde(default)]
    pub rate_limit_per_min: Option<u32>,
}

/// UI plugin section (`[ui]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiSection {
    /// ES module relative to the plugin directory.
    pub widget: String,
}

/// Instance configuration (`[config]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigSection {
    /// JSON Schema (draft 2020-12) file, relative to the plugin directory, that the instance
    /// configuration must satisfy. The host checks it on `reconfigure` and answers a violation
    /// with the offending field instead of restarting the plugin with a config it will refuse.
    pub schema: String,
}

/// Notice the admin has to confirm before the plugin is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Acknowledge {
    /// Text shown to the admin (licence, patents, ...).
    pub text: String,
    /// Where to read more.
    #[serde(default)]
    pub url: Option<String>,
}

/// Install information (`[install]`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Install {
    /// Build commands (programs relative to the plugin directory), shown before install.
    #[serde(default)]
    pub build: Vec<String>,
    /// Notice to confirm.
    #[serde(default)]
    pub acknowledge: Option<Acknowledge>,
}

/// A parsed `plugin.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Identity.
    pub plugin: PluginMeta,
    /// Execution.
    #[serde(default)]
    pub runtime: Runtime,
    /// Input stream.
    #[serde(default)]
    pub input: Option<Input>,
    /// Device claim.
    #[serde(default)]
    pub device: Device,
    /// Outputs.
    #[serde(default)]
    pub output: Output,
    /// Instance configuration.
    #[serde(default)]
    pub config: Option<ConfigSection>,
    /// Codec section.
    #[serde(default)]
    pub codec: Option<CodecSection>,
    /// Lookup section.
    #[serde(default)]
    pub lookup: Option<LookupSection>,
    /// UI section.
    #[serde(default)]
    pub ui: Option<UiSection>,
    /// Install information.
    #[serde(default)]
    pub install: Option<Install>,
}

impl Manifest {
    /// Parse and validate a manifest.
    ///
    /// # Errors
    /// [`ManifestError::Parse`] for syntax or unknown keys, [`ManifestError::Invalid`] listing
    /// every broken rule.
    pub fn from_toml(text: &str) -> Result<Self, ManifestError> {
        if text.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::Invalid(vec![format!(
                "manifest larger than {MAX_MANIFEST_BYTES} bytes"
            )]));
        }
        let manifest: Self =
            toml::from_str(text).map_err(|err| ManifestError::Parse(err.to_string()))?;
        let problems = manifest.problems();
        if problems.is_empty() {
            Ok(manifest)
        } else {
            Err(ManifestError::Invalid(problems))
        }
    }

    /// The effective heartbeat timeout in seconds, `None` for no watchdog.
    #[must_use]
    pub fn heartbeat_timeout_s(&self) -> Option<u32> {
        match (self.runtime.heartbeat_timeout_s, self.runtime.transport) {
            (Some(0), _) => None,
            (Some(secs), _) => Some(secs),
            (None, Transport::Framed) => Some(15),
            (None, Transport::Simple) => None,
        }
    }

    /// Whether instances of this plugin claim the device exclusively.
    #[must_use]
    pub fn is_exclusive(&self) -> bool {
        self.device.exclusive
    }

    /// Every rule the manifest breaks (empty when valid).
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.check_meta(&mut out);
        self.check_runtime(&mut out);
        self.check_streams(&mut out);
        self.check_device(&mut out);
        self.check_sections(&mut out);
        out
    }

    fn check_meta(&self, out: &mut Vec<String>) {
        let meta = &self.plugin;
        if meta.protocol != PROTOCOL_VERSION {
            out.push(format!(
                "plugin.protocol is {} but this host speaks {PROTOCOL_VERSION}",
                meta.protocol
            ));
        }
        if !is_valid_plugin_id(&meta.id) {
            out.push(format!(
                "plugin.id {:?} must be reverse-DNS with at least three segments of [a-z0-9-] (max 128 chars)",
                meta.id
            ));
        }
        if semver::Version::parse(&meta.version).is_err() {
            out.push(format!(
                "plugin.version {:?} is not a semantic version",
                meta.version
            ));
        }
        if meta.name.trim().is_empty() || meta.name.len() > 80 {
            out.push("plugin.name must be 1-80 characters".to_owned());
        }
        if meta.license.trim().is_empty() {
            out.push("plugin.license must be an SPDX expression".to_owned());
        }
    }

    fn check_runtime(&self, out: &mut Vec<String>) {
        let rt = &self.runtime;
        let ty = self.plugin.kind;
        if ty == PluginType::Ui {
            if !rt.command.is_empty() || rt.image.is_some() {
                out.push("ui plugins have no runtime.command or runtime.image".to_owned());
            }
            return;
        }
        match rt.kind {
            RuntimeKind::Process => {
                match rt.command.first() {
                    None => out.push("runtime.command is required for processes".to_owned()),
                    Some(program) if program.trim().is_empty() => {
                        out.push("runtime.command[0] is empty".to_owned());
                    }
                    Some(program) if program.contains('/') && !is_safe_relative(program) => {
                        out.push(format!(
                            "runtime.command[0] {program:?} must stay inside the plugin directory"
                        ));
                    }
                    Some(_) => {}
                }
                if rt.image.is_some() {
                    out.push("runtime.image is only for container plugins".to_owned());
                }
            }
            RuntimeKind::Container => {
                if rt.image.as_deref().is_none_or(|i| i.trim().is_empty()) {
                    out.push("runtime.image is required for container plugins".to_owned());
                }
                if rt.transport != Transport::Framed {
                    out.push("container plugins use the framed transport".to_owned());
                }
            }
        }
        if matches!(ty, PluginType::Codec | PluginType::Lookup) && rt.transport != Transport::Framed
        {
            out.push(format!(
                "{} plugins require runtime.transport = \"framed\"",
                if ty == PluginType::Codec {
                    "codec"
                } else {
                    "lookup"
                }
            ));
        }
        if (self.output.audio.is_some() || !self.output.voice_frames.is_empty())
            && rt.transport != Transport::Framed
        {
            out.push("audio and voice frames require runtime.transport = \"framed\"".to_owned());
        }
    }

    fn check_streams(&self, out: &mut Vec<String>) {
        let ty = self.plugin.kind;
        match (&self.input, ty) {
            (None, PluginType::Decoder | PluginType::Codec) => {
                out.push("[input] is required for decoders and codecs".to_owned());
            }
            (Some(_), PluginType::Lookup | PluginType::Ui) => {
                out.push("[input] is only for decoders and codecs".to_owned());
            }
            (Some(input), _) => {
                if input.sample_rate == 0 {
                    out.push("input.sample_rate must be positive".to_owned());
                }
                if input.bandwidth_hz == Some(0) {
                    out.push("input.bandwidth_hz must be positive".to_owned());
                }
                let complex = matches!(
                    input.format,
                    SampleFormat::Cf32 | SampleFormat::Cs16 | SampleFormat::Cu8
                );
                match input.kind {
                    InputKind::Iq if !complex => {
                        out.push("input.kind iq needs a complex format (cf32, cs16, cu8)".into());
                    }
                    InputKind::Discriminator | InputKind::Audio if complex => {
                        out.push("input.kind discriminator/audio needs a real format".into());
                    }
                    _ => {}
                }
                if ty == PluginType::Decoder && input.kind == InputKind::VoiceFrames {
                    out.push("decoders cannot take voice frames as input".to_owned());
                }
                if ty == PluginType::Codec && input.kind != InputKind::VoiceFrames {
                    out.push("codec plugins take input.kind voice_frames".to_owned());
                }
            }
            (None, _) => {}
        }
        if let Some(path) = &self.output.events_schema
            && !is_safe_relative(path)
        {
            out.push(format!(
                "output.events_schema {path:?} must be a relative path inside the plugin directory"
            ));
        }
        if let Some(config) = &self.config
            && !is_safe_relative(&config.schema)
        {
            out.push(format!(
                "config.schema {:?} must be a relative path inside the plugin directory",
                config.schema
            ));
        }
        if let Some(audio) = &self.output.audio
            && (audio.sample_rate == 0 || !(1..=2).contains(&audio.channels))
        {
            out.push("output.audio needs a positive sample_rate and 1 or 2 channels".to_owned());
        }
        for id in &self.output.voice_frames {
            if !is_valid_codec_id(id) {
                out.push(format!("output.voice_frames: invalid codec id {id:?}"));
            }
        }
    }

    fn check_device(&self, out: &mut Vec<String>) {
        let dev = &self.device;
        let has_range = dev.center_hz_min.is_some() || dev.center_hz_max.is_some();
        if !dev.exclusive {
            if dev.center_hz.is_some() || has_range || dev.sample_rate.is_some() {
                out.push(
                    "device.center_hz / center_hz_min / center_hz_max / sample_rate only apply when device.exclusive = true"
                        .to_owned(),
                );
            }
            return;
        }
        if self.plugin.kind != PluginType::Decoder {
            out.push("only decoders can claim the device".to_owned());
        }
        if dev.sample_rate.is_none_or(|rate| rate == 0) {
            out.push("an exclusive plugin must declare a positive device.sample_rate".to_owned());
        }
        match (dev.center_hz, dev.center_hz_min, dev.center_hz_max) {
            (Some(0), _, _) => out.push("device.center_hz must be positive".to_owned()),
            (Some(_), None, None) => {}
            (None, Some(lo), Some(hi)) if lo > 0 && lo <= hi => {}
            (None, Some(_), Some(_)) => {
                out.push("device.center_hz_min must be positive and <= center_hz_max".to_owned());
            }
            (Some(_), _, _) => {
                out.push("device.center_hz and the center_hz_min/max range exclude each other".into());
            }
            _ => out.push(
                "an exclusive plugin declares device.center_hz or both center_hz_min and center_hz_max"
                    .to_owned(),
            ),
        }
        if self.input.as_ref().is_none_or(|i| i.kind != InputKind::Iq) {
            out.push("an exclusive plugin takes input.kind iq (the whole window)".to_owned());
        }
    }

    fn check_sections(&self, out: &mut Vec<String>) {
        let ty = self.plugin.kind;
        match (&self.codec, ty) {
            (None, PluginType::Codec) => out.push("[codec] is required for codecs".to_owned()),
            (Some(_), t) if t != PluginType::Codec => {
                out.push("[codec] is only for codec plugins".to_owned());
            }
            (Some(codec), _) => {
                if codec.frames.is_empty() || !codec.frames.iter().all(|f| is_valid_codec_id(f)) {
                    out.push("codec.frames needs at least one valid codec id".to_owned());
                }
                if codec.pcm.sample_rate == 0 || !(1..=2).contains(&codec.pcm.channels) {
                    out.push(
                        "codec.pcm needs a positive sample_rate and 1 or 2 channels".to_owned(),
                    );
                }
            }
            (None, _) => {}
        }
        match (&self.lookup, ty) {
            (None, PluginType::Lookup) => out.push("[lookup] is required for lookups".to_owned()),
            (Some(_), t) if t != PluginType::Lookup => {
                out.push("[lookup] is only for lookup plugins".to_owned());
            }
            (Some(lookup), _) if lookup.keys.is_empty() => {
                out.push("lookup.keys must not be empty".to_owned());
            }
            _ => {}
        }
        match (&self.ui, ty) {
            (None, PluginType::Ui) => out.push("[ui] is required for ui plugins".to_owned()),
            (Some(ui), _) if !is_safe_relative(&ui.widget) => {
                out.push(format!(
                    "ui.widget {:?} must be a relative path inside the plugin directory",
                    ui.widget
                ));
            }
            _ => {}
        }
        if let Some(install) = &self.install {
            for cmd in &install.build {
                let program = cmd.split_whitespace().next().unwrap_or("");
                if program.is_empty() || (program.contains('/') && !is_safe_relative(program)) {
                    out.push(format!(
                        "install.build entry {cmd:?} must start with a program inside the plugin directory"
                    ));
                }
            }
        }
    }
}

/// Reverse-DNS plugin id: at least three dot-separated segments of `[a-z0-9-]`, each starting
/// with a letter or digit, at most 128 characters. The restricted alphabet keeps ids usable as
/// directory names and (with dots mapped to `_`) as NATS subject tokens.
#[must_use]
pub fn is_valid_plugin_id(id: &str) -> bool {
    if id.len() > 128 {
        return false;
    }
    let mut segments = 0;
    for segment in id.split('.') {
        segments += 1;
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first.is_ascii_lowercase() || first.is_ascii_digit())
            || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return false;
        }
    }
    segments >= 3
}

/// Codec id such as `tetra-acelp` or `ambe2+-3600x2450`.
fn is_valid_codec_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '+' | '_'))
}

/// A relative path that does not leave its base directory.
#[must_use]
pub fn is_safe_relative(path: &str) -> bool {
    if path.is_empty() || path.contains('\0') {
        return false;
    }
    Path::new(path)
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && Path::new(path).is_relative()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[plugin]
id = "org.superhet.decoder.pocsag"
name = "POCSAG"
version = "0.1.0"
protocol = 1
license = "GPL-2.0-only"
type = "decoder"

[runtime]
command = ["bin/adapter"]

[input]
kind = "discriminator"
sample_rate = 22050
format = "s16le"
"#;

    fn problems(toml: &str) -> Vec<String> {
        match Manifest::from_toml(toml) {
            Err(ManifestError::Invalid(p)) => p,
            Err(ManifestError::Parse(p)) => vec![p],
            Ok(_) => Vec::new(),
        }
    }

    #[test]
    fn accepts_minimal_decoder() {
        let m = Manifest::from_toml(GOOD).expect("valid");
        assert_eq!(m.plugin.kind, PluginType::Decoder);
        assert_eq!(m.runtime.transport, Transport::Simple);
        assert!(!m.is_exclusive());
        assert_eq!(m.heartbeat_timeout_s(), None);
    }

    #[test]
    fn rejects_unknown_keys_and_wrong_protocol() {
        assert!(!problems(&GOOD.replace("[runtime]", "[runtime]\nfoo = 1")).is_empty());
        let p = problems(&GOOD.replace("protocol = 1", "protocol = 0"));
        assert!(p.iter().any(|m| m.contains("protocol")), "{p:?}");
    }

    #[test]
    fn rejects_bad_ids_and_paths() {
        for id in [
            "pocsag",
            "a.b",
            "Org.superhet.x",
            "org..x",
            "org.super_het.x",
        ] {
            assert!(!is_valid_plugin_id(id), "{id}");
        }
        assert!(is_valid_plugin_id("org.superhet.decoder.pocsag"));
        let p = problems(&GOOD.replace("bin/adapter", "../evil"));
        assert!(p.iter().any(|m| m.contains("inside")), "{p:?}");
        let p = problems(&GOOD.replace("bin/adapter", "/bin/sh"));
        assert!(p.iter().any(|m| m.contains("inside")), "{p:?}");
        assert!(is_safe_relative("schema/events.json"));
        assert!(!is_safe_relative("a/../../b"));
    }

    #[test]
    fn exclusive_claims_are_complete() {
        let base = GOOD
            .replace("discriminator", "iq")
            .replace("s16le", "cu8")
            .to_owned();
        let fixed = format!(
            "{base}\n[device]\nexclusive = true\ncenter_hz = 1090000000\nsample_rate = 2400000\n"
        );
        assert!(problems(&fixed).is_empty(), "{:?}", problems(&fixed));
        let range = format!(
            "{base}\n[device]\nexclusive = true\ncenter_hz_min = 174928000\ncenter_hz_max = 239200000\nsample_rate = 2048000\n"
        );
        assert!(problems(&range).is_empty(), "{:?}", problems(&range));
        let no_centre = format!("{base}\n[device]\nexclusive = true\nsample_rate = 2400000\n");
        assert!(!problems(&no_centre).is_empty());
        let no_rate = format!("{base}\n[device]\nexclusive = true\ncenter_hz = 1\n");
        assert!(!problems(&no_rate).is_empty());
        let stray = format!("{base}\n[device]\ncenter_hz = 1090000000\n");
        assert!(!problems(&stray).is_empty());
        // Exclusive needs the whole window as IQ.
        let audio_in =
            format!("{GOOD}\n[device]\nexclusive = true\ncenter_hz = 1\nsample_rate = 2\n");
        assert!(!problems(&audio_in).is_empty());
    }

    #[test]
    fn type_specific_rules() {
        let p = problems(&GOOD.replace("type = \"decoder\"", "type = \"codec\""));
        assert!(p.iter().any(|m| m.contains("codec")), "{p:?}");
        let audio = format!("{GOOD}\n[output.audio]\nsample_rate = 8000\nchannels = 1\n");
        assert!(problems(&audio).iter().any(|m| m.contains("framed")));
        let framed = audio.replace("[runtime]", "[runtime]\ntransport = \"framed\"");
        assert!(problems(&framed).is_empty(), "{:?}", problems(&framed));
        let m = Manifest::from_toml(&framed).expect("valid");
        assert_eq!(m.heartbeat_timeout_s(), Some(15));
    }

    #[test]
    fn semver_is_checked() {
        let p = problems(&GOOD.replace("0.1.0", "one"));
        assert!(p.iter().any(|m| m.contains("semantic")), "{p:?}");
    }
}
