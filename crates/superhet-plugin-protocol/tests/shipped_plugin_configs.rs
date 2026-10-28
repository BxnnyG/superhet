//! The `config.schema` of the first-party plugins accepts what the web UI and the adapters
//! accept, and names the field of what they refuse.

use std::path::PathBuf;

use serde_json::{Value, json};
use superhet_plugin_protocol::Package;

fn check(plugin: &str, config: &Value) -> Result<(), (String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
    let package = Package::load(&dir.join(plugin)).expect("plugin loads");
    let validator = package
        .config_validator
        .expect("plugin declares config.schema");
    validator
        .check(config)
        .map_err(|issue| (issue.field, issue.message))
}

#[test]
fn pocsag() {
    for ok in [
        Value::Null,
        json!({}),
        json!({"baud": [512, 1200], "flex": false, "invert": true, "charset": "DE"}),
        json!({"invert": "auto"}),
        json!({"baud": [], "flex": true}),
    ] {
        assert_eq!(check("pocsag", &ok), Ok(()), "{ok}");
    }
    let field = |config: Value| check("pocsag", &config).expect_err("refused").0;
    assert_eq!(field(json!({"baud": [512, 700]})), "baud.1");
    assert_eq!(field(json!({"charset": "XX"})), "charset");
    assert_eq!(field(json!({"flex": "yes"})), "flex");
    assert_eq!(field(json!({"invert": "maybe"})), "invert");
    assert_eq!(field(json!({"flx": true})), "flx");
    // No demodulator left.
    assert_eq!(field(json!({"baud": [], "flex": false})), "flex");
}

#[test]
fn rtl433() {
    assert_eq!(check("rtl433", &json!({"protocols": [19, 20]})), Ok(()));
    assert_eq!(check("rtl433", &Value::Null), Ok(()));
    let field = |config: Value| check("rtl433", &config).expect_err("refused").0;
    assert_eq!(field(json!({"protocols": [0]})), "protocols.0");
    assert_eq!(field(json!({"protocols": "19"})), "protocols");
    assert_eq!(field(json!({"x": 1})), "x");
}

#[test]
fn aprs_takes_no_options() {
    assert_eq!(check("aprs", &Value::Null), Ok(()));
    assert_eq!(check("aprs", &json!({})), Ok(()));
    let (field, message) = check("aprs", &json!({"x": 1})).expect_err("refused");
    assert_eq!((field.as_str(), message.as_str()), ("x", "unknown setting"));
}

#[test]
fn adsb() {
    assert_eq!(
        check(
            "adsb",
            &json!({"lat": 51.5, "lon": 7.0, "event_interval_s": 30})
        ),
        Ok(())
    );
    let field = |config: Value| check("adsb", &config).expect_err("refused").0;
    assert_eq!(field(json!({"lat": 95.0, "lon": 7.0})), "lat");
    assert_eq!(field(json!({"event_interval_s": 1})), "event_interval_s");
    assert!(!field(json!({"lat": 50.0})).starts_with(':'));
}
