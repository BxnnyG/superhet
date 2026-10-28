//! Versioned plugin contract for Superhet: manifest and wire types shared by the server and
//! out-of-process plugins.
//!
//! Contract version 1. Prose description: `docs/plugin-contract.md`; JSON Schemas of every
//! structure: `schema/*.schema.json` in this crate (generated from the types, a test fails when
//! they drift). The [`contract`] module is the test suite a plugin has to pass.

pub mod config;
pub mod contract;
pub mod ctl;
pub mod event;
pub mod framed;
pub mod manifest;
pub mod package;
pub mod schema;

pub use config::{ConfigIssue, ConfigValidator};
pub use event::{Event, EventError, EventValidator, Line};
pub use manifest::{Manifest, ManifestError, PluginType, Transport};
pub use package::{Package, PackageError};

/// Version of the plugin protocol. Plugins declare it in `plugin.protocol`; the host refuses
/// every other value. It bumps on incompatible changes only.
pub const PROTOCOL_VERSION: u32 = 1;

/// Environment variables the host sets for every plugin process.
pub mod env {
    /// Plugin id.
    pub const PLUGIN_ID: &str = "SUPERHET_PLUGIN_ID";
    /// Instance id.
    pub const INSTANCE_ID: &str = "SUPERHET_INSTANCE_ID";
    /// Plugin directory (also the working directory).
    pub const PLUGIN_DIR: &str = "SUPERHET_PLUGIN_DIR";
    /// Device name, when the instance is bound to one.
    pub const DEVICE: &str = "SUPERHET_DEVICE";
    /// VFO id, when the instance is fed from one.
    pub const VFO: &str = "SUPERHET_VFO";
    /// Frequency in Hz, when the instance is bound to one.
    pub const FREQ_HZ: &str = "SUPERHET_FREQ_HZ";
    /// Instance configuration as one line of JSON (`null` when none was given).
    pub const CONFIG: &str = "SUPERHET_CONFIG";
    /// Unix socket path (framed transport only).
    pub const SOCKET: &str = "SUPERHET_SOCKET";
}

/// Identity of one plugin instance, as handed to the process.
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceEnv<'a> {
    /// Plugin id.
    pub plugin_id: &'a str,
    /// Instance id.
    pub instance_id: &'a str,
    /// Plugin directory.
    pub plugin_dir: &'a std::path::Path,
    /// Bound device.
    pub device: Option<&'a str>,
    /// Bound VFO.
    pub vfo: Option<u32>,
    /// Bound frequency (Hz).
    pub freq_hz: Option<u64>,
    /// Operator configuration.
    pub config: &'a serde_json::Value,
}

impl InstanceEnv<'_> {
    /// The environment variable assignments (see [`env`]) for the child process.
    #[must_use]
    pub fn vars(&self) -> Vec<(&'static str, String)> {
        let mut vars = vec![
            (env::PLUGIN_ID, self.plugin_id.to_owned()),
            (env::INSTANCE_ID, self.instance_id.to_owned()),
            (env::PLUGIN_DIR, self.plugin_dir.display().to_string()),
            (env::CONFIG, self.config.to_string()),
        ];
        if let Some(device) = self.device {
            vars.push((env::DEVICE, device.to_owned()));
        }
        if let Some(vfo) = self.vfo {
            vars.push((env::VFO, vfo.to_string()));
        }
        if let Some(freq) = self.freq_hz {
            vars.push((env::FREQ_HZ, freq.to_string()));
        }
        vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_one() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn env_vars_skip_unbound_fields() {
        let config = serde_json::json!({"mode": "x"});
        let vars = InstanceEnv {
            plugin_id: "a.b.c",
            instance_id: "i1",
            plugin_dir: std::path::Path::new("/p"),
            device: None,
            vfo: None,
            freq_hz: Some(144_800_000),
            config: &config,
        }
        .vars();
        let names: Vec<_> = vars.iter().map(|(k, _)| *k).collect();
        assert!(names.contains(&env::FREQ_HZ) && !names.contains(&env::DEVICE));
        assert!(vars.contains(&(env::CONFIG, "{\"mode\":\"x\"}".to_owned())));
    }
}
