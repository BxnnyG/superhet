//! JSON Schemas of the contract, generated from the Rust types.
//!
//! The committed copies live in `schema/` next to `Cargo.toml`. The test `committed_schemas_match`
//! fails when they drift; run it with `UPDATE_SCHEMAS=1` to rewrite them.

use schemars::JsonSchema;
use serde_json::Value;

use crate::ctl;
use crate::event::Event;
use crate::framed::{HostHello, PluginHello};
use crate::manifest::Manifest;

fn schema_of<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null)
}

/// Every schema as `(file stem, document)`; the file is `<stem>.schema.json`.
#[must_use]
pub fn all() -> Vec<(&'static str, Value)> {
    vec![
        ("manifest", schema_of::<Manifest>()),
        ("event", schema_of::<Event>()),
        ("host-event", schema_of::<crate::event::HostEvent>()),
        ("host-hello", schema_of::<HostHello>()),
        ("plugin-hello", schema_of::<PluginHello>()),
        ("ctl-install-request", schema_of::<ctl::InstallRequest>()),
        ("ctl-start-request", schema_of::<ctl::StartRequest>()),
        ("ctl-instance-ref", schema_of::<ctl::InstanceRef>()),
        ("ctl-remove-request", schema_of::<ctl::RemoveRequest>()),
        ("ctl-removed-reply", schema_of::<ctl::RemovedReply>()),
        ("ctl-error-reply", schema_of::<ctl::ErrorReply>()),
        ("ctl-plugins-reply", schema_of::<ctl::PluginsReply>()),
        ("ctl-plugin-reply", schema_of::<ctl::PluginReply>()),
        ("ctl-instances-reply", schema_of::<ctl::InstancesReply>()),
        ("ctl-instance-reply", schema_of::<ctl::InstanceReply>()),
        ("instance-info", schema_of::<ctl::InstanceInfo>()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema")
    }

    #[test]
    fn committed_schemas_match() {
        let update = std::env::var_os("UPDATE_SCHEMAS").is_some();
        if update {
            std::fs::create_dir_all(dir()).expect("create schema dir");
        }
        for (stem, doc) in all() {
            let path = dir().join(format!("{stem}.schema.json"));
            let mut text = serde_json::to_string_pretty(&doc).expect("serialise");
            text.push('\n');
            if update {
                std::fs::write(&path, &text).expect("write schema");
                continue;
            }
            let committed = std::fs::read_to_string(&path).unwrap_or_else(|err| {
                panic!("{}: {err} (run with UPDATE_SCHEMAS=1)", path.display())
            });
            assert_eq!(
                committed, text,
                "{stem} schema drifted; run `UPDATE_SCHEMAS=1 cargo test -p superhet-plugin-protocol committed_schemas_match`"
            );
        }
    }

    #[test]
    fn manifest_schema_accepts_the_reference_manifest() {
        let manifest = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../plugins/example-echo/plugin.toml"),
        )
        .expect("reference manifest");
        let value: Value = toml::from_str(&manifest).expect("toml");
        let schema = all()
            .into_iter()
            .find(|(stem, _)| *stem == "manifest")
            .expect("manifest schema")
            .1;
        let validator = jsonschema::validator_for(&schema).expect("valid schema");
        assert!(
            validator.is_valid(&value),
            "{:?}",
            validator.validate(&value)
        );
    }
}
