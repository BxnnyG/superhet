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
fn the_example_ships_a_panel_the_static_checks_find() {
    use superhet_plugin_protocol::manifest::UiPlacement;
    use superhet_plugin_protocol::ui;
    let package = load();
    let section = package.manifest.ui.as_ref().expect("[ui] section");
    assert_eq!(section.placement, UiPlacement::Inspector);
    assert!(section.tune && section.reads.is_empty());
    let (root, page) = ui::split_entry(&section.entry).expect("entry in a subdirectory");
    assert_eq!((root, page), ("ui", "index.html"));
    // Every file of the panel is a served type within the size limit, and the page uses no
    // inline code or styles and no external origins (the panel's CSP would block them).
    let dir = plugin_dir().join(root);
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("ui dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(ui::valid_asset_path(&name), "{name}");
        assert!(ui::mime_for(&name).is_some(), "{name} would not be served");
        assert!(entry.metadata().expect("meta").len() as usize <= ui::MAX_ASSET_BYTES);
        seen += 1;
    }
    assert!(seen >= 3, "page, script and style");
    let html = std::fs::read_to_string(dir.join(page)).expect("page");
    assert!(!html.contains("http://") && !html.contains("https://"));
    assert!(!html.contains("<style") && !html.contains(" style="));
    assert!(!html.contains("onclick") && !html.contains("<script>"));
}

#[test]
fn a_missing_panel_page_fails_the_static_checks() {
    let tmp = std::env::temp_dir().join(format!("superhet-nopage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    for sub in ["ui", "schema", "bin"] {
        std::fs::create_dir_all(tmp.join(sub)).expect("mkdir");
    }
    std::fs::copy(plugin_dir().join("plugin.toml"), tmp.join("plugin.toml")).expect("copy");
    std::fs::copy(
        plugin_dir().join("schema/events.json"),
        tmp.join("schema/events.json"),
    )
    .expect("copy");
    std::fs::write(tmp.join("bin/example-echo"), "").expect("write");
    let problems = check_static(&tmp).expect_err("page is missing");
    assert!(
        problems.iter().any(|p| p.contains("ui.entry")),
        "{problems:?}"
    );
    std::fs::write(tmp.join("ui/index.html"), "<p>x</p>").expect("write");
    assert!(check_static(&tmp).is_ok());
    let _ = std::fs::remove_dir_all(&tmp);
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
