//! Contract test suite applied to the reference plugin `plugins/example-echo`.
//!
//! Needs `python3` on `PATH` (the reference plugin is a Python script).

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use superhet_plugin_protocol::contract::{check, check_static, run_simple};
use superhet_plugin_protocol::{Package, PluginType, Transport};

fn plugin_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/example-echo")
}

fn load() -> Package {
    check_static(&plugin_dir()).unwrap_or_else(|problems| panic!("static checks: {problems:?}"))
}

const TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn manifest_is_valid_and_simple() {
    let package = load();
    assert_eq!(package.manifest.plugin.id, "org.superhet.example.echo");
    assert_eq!(package.manifest.plugin.kind, PluginType::Decoder);
    assert_eq!(package.manifest.runtime.transport, Transport::Simple);
    assert!(!package.manifest.is_exclusive());
}

#[test]
fn well_behaved_run_passes_the_contract() {
    let package = load();
    let input = vec![0u8; 10_000];
    let report = run_simple(&package, &input, &Value::Null, TIMEOUT).expect("run");
    let problems = check(&report);
    assert!(
        problems.is_empty(),
        "{problems:?}\nstderr: {}",
        report.stderr
    );
    // 10 000 bytes arrive in reads of at most 4096: at least three blocks, one summary.
    let blocks = report
        .events
        .iter()
        .filter(|e| e.event_type == "echo.block")
        .count();
    assert!(blocks >= 3, "{blocks} blocks");
    let summary = report.events.last().expect("summary event");
    assert_eq!(summary.event_type, "echo.summary");
    assert_eq!(summary.data["bytes"], 10_000);
    assert!(report.stderr.contains("starting"), "stderr is captured");
}

#[test]
fn empty_input_still_ends_with_a_summary() {
    let package = load();
    let report = run_simple(&package, &[], &Value::Null, TIMEOUT).expect("run");
    assert!(check(&report).is_empty(), "{:?}", check(&report));
    assert_eq!(report.events.len(), 1);
    assert_eq!(report.events[0].data["blocks"], 0);
}

#[test]
fn invalid_lines_are_detected() {
    let package = load();
    let report =
        run_simple(&package, &[1, 2, 3], &json!({"mode": "invalid"}), TIMEOUT).expect("run");
    assert_eq!(report.violations.len(), 3, "{:?}", report.violations);
    assert!(
        !check(&report).is_empty(),
        "the suite must fail such a plugin"
    );
    // Valid events around the garbage are still delivered.
    assert_eq!(report.events.len(), 2);
}

#[test]
fn crashing_plugin_fails_the_suite() {
    let package = load();
    let report = run_simple(&package, &[], &json!({"mode": "crash"}), TIMEOUT).expect("run");
    let problems = check(&report);
    assert!(
        problems.iter().any(|p| p.contains("exited with")),
        "{problems:?}"
    );
}

#[test]
fn hanging_plugin_hits_the_timeout() {
    let package = load();
    let report = run_simple(
        &package,
        &[],
        &json!({"mode": "hang"}),
        Duration::from_millis(500),
    )
    .expect("run");
    assert!(report.exit.is_none());
    assert!(check(&report).iter().any(|p| p.contains("timeout")));
}
