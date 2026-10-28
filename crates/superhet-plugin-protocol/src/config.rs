//! Checking an instance configuration against the plugin's `config.schema`.
//!
//! The host runs this before it applies a new configuration, so a typo or an out-of-range value
//! is refused with the offending field instead of crashing the plugin on its next start.

use serde_json::Value;

use crate::event::SchemaError;

/// The configuration breaks the plugin's schema.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.describe())]
pub struct ConfigIssue {
    /// The offending key as a dotted path (`baud.1`, `timing.max_ms`); empty for the whole
    /// document.
    pub field: String,
    /// What is wrong, without the field name (`700 is not one of [512, 1200, 2400]`).
    pub message: String,
}

impl ConfigIssue {
    fn describe(&self) -> String {
        if self.field.is_empty() {
            self.message.clone()
        } else {
            format!("{}: {}", self.field, self.message)
        }
    }
}

/// Validates configurations against one schema.
pub struct ConfigValidator {
    schema: jsonschema::Validator,
}

impl std::fmt::Debug for ConfigValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigValidator").finish_non_exhaustive()
    }
}

impl ConfigValidator {
    /// Build a validator from the plugin's `config.schema` document.
    ///
    /// # Errors
    /// [`SchemaError`] when the document is not a valid JSON Schema.
    pub fn new(schema: &Value) -> Result<Self, SchemaError> {
        let schema =
            jsonschema::validator_for(schema).map_err(|err| SchemaError(err.to_string()))?;
        Ok(Self { schema })
    }

    /// Check a configuration (`null` means "the defaults" and is checked as `{}`).
    ///
    /// # Errors
    /// The first violation, with the field it concerns.
    pub fn check(&self, config: &Value) -> Result<(), ConfigIssue> {
        let empty = Value::Object(serde_json::Map::new());
        let config = if config.is_null() { &empty } else { config };
        let Some(error) = self.schema.iter_errors(config).next() else {
            return Ok(());
        };
        let mut field = dotted(&error.instance_path.to_string());
        let mut message = error.to_string();
        // "Additional properties are not allowed ('x' was unexpected)" and a missing required
        // key name the key in the message; point at it instead of at the parent object.
        if let Some(key) = unexpected_key(&message) {
            field = join(&field, &key);
            message = "unknown setting".to_owned();
        } else if let Some(key) = missing_key(&message) {
            field = join(&field, &key);
            message = "is required".to_owned();
        }
        Err(ConfigIssue { field, message })
    }
}

fn join(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_owned()
    } else {
        format!("{parent}.{key}")
    }
}

/// `/timing/max_ms` -> `timing.max_ms`.
fn dotted(pointer: &str) -> String {
    pointer
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| part.replace("~1", "/").replace("~0", "~"))
        .collect::<Vec<_>>()
        .join(".")
}

fn unexpected_key(message: &str) -> Option<String> {
    let rest = message.strip_prefix("Additional properties are not allowed (")?;
    // "'x' was unexpected)" for one key, "'x', 'y' were unexpected)" for several: report the first.
    let rest = rest
        .strip_suffix(" was unexpected)")
        .or_else(|| rest.strip_suffix(" were unexpected)"))?;
    let first = rest.split(", ").next()?;
    Some(first.trim_matches('\'').to_owned())
}

fn missing_key(message: &str) -> Option<String> {
    let rest = message.strip_suffix(" is a required property")?;
    Some(rest.trim_matches('"').to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn validator() -> ConfigValidator {
        ConfigValidator::new(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "baud": {"type": "array", "items": {"enum": [512, 1200, 2400]}},
                "flex": {"type": "boolean"},
                "timing": {
                    "type": "object",
                    "properties": {"max_ms": {"type": "integer", "minimum": 1}},
                    "additionalProperties": false
                }
            },
            "required": [],
            "additionalProperties": false
        }))
        .expect("schema")
    }

    #[test]
    fn accepts_valid_configs_and_null() {
        let v = validator();
        assert!(v.check(&Value::Null).is_ok());
        assert!(v.check(&json!({})).is_ok());
        assert!(
            v.check(&json!({"baud": [512, 2400], "flex": false}))
                .is_ok()
        );
    }

    #[test]
    fn names_the_field_of_a_wrong_value() {
        let issue = validator()
            .check(&json!({"baud": [512, 700]}))
            .expect_err("700 is no baud rate");
        assert_eq!(issue.field, "baud.1");
        assert!(issue.message.contains("700"), "{}", issue.message);
        assert!(issue.to_string().starts_with("baud.1: "));
    }

    #[test]
    fn names_the_type_error_and_nested_fields() {
        let v = validator();
        let issue = v.check(&json!({"flex": "yes"})).expect_err("type");
        assert_eq!(issue.field, "flex");
        let issue = v
            .check(&json!({"timing": {"max_ms": 0}}))
            .expect_err("minimum");
        assert_eq!(issue.field, "timing.max_ms");
    }

    #[test]
    fn points_at_unknown_keys() {
        let v = validator();
        let issue = v.check(&json!({"flx": true})).expect_err("typo");
        assert_eq!(
            (issue.field.as_str(), issue.message.as_str()),
            ("flx", "unknown setting")
        );
        let issue = v
            .check(&json!({"timing": {"max": 1}}))
            .expect_err("nested typo");
        assert_eq!(issue.field, "timing.max");
    }

    #[test]
    fn a_config_that_is_not_an_object_is_refused_as_a_whole() {
        let issue = validator().check(&json!([1])).expect_err("array");
        assert_eq!(issue.field, "");
        assert!(!issue.to_string().starts_with(": "));
    }

    #[test]
    fn names_missing_required_keys() {
        let v =
            ConfigValidator::new(&json!({"type": "object", "required": ["lat"]})).expect("schema");
        let issue = v.check(&json!({})).expect_err("required");
        assert_eq!(
            (issue.field.as_str(), issue.message.as_str()),
            ("lat", "is required")
        );
    }

    #[test]
    fn refuses_an_unusable_schema() {
        assert!(ConfigValidator::new(&json!({"type": "nonsense"})).is_err());
    }
}
