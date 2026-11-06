//! Golden test of the dsd-fme log parser and the call tracker: transcripts of the tool's log
//! (`tests/dsd/transcripts/*.log`) must give exactly the events in the matching `*.expected.json`,
//! and every event must satisfy the events schema of `plugins/dsd`.
//!
//! The transcripts are written by hand in the style of dsd-fme's log, not captured from a real
//! receiver (dsd-fme cannot be part of this repository, `docs/plugins-dsd.md`). Regenerate the
//! expected files after an intended change with `UPDATE_GOLDEN=1 cargo test -p
//! superhet-plugin-adapters --test dsd_golden` and read the diff.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use superhet_plugin_adapters::dsd::call::Tracker;
use superhet_plugin_adapters::dsd::lines::{Protocol, parse_line};
use superhet_plugin_protocol::Package;

const START_UNIX_MS: u64 = 1_700_000_000_000;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dsd/transcripts")
}

/// Play a transcript: 100 ms per line, 500 ms per `---`, then three seconds of silence.
fn replay(name: &str, fixed: Option<Protocol>) -> Vec<Value> {
    let text = std::fs::read_to_string(dir().join(name)).expect("transcript");
    let mut tracker = Tracker::new(true, fixed);
    let mut events = Vec::new();
    let mut now = 1000u64;
    let mut push = |emits: Vec<superhet_plugin_adapters::dsd::call::Emit>| {
        for emit in emits {
            events.push(json!({"v": 1, "type": emit.kind, "data": emit.data}));
        }
    };
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if line.trim() == "---" {
            now += 500;
            push(tracker.tick(now));
            continue;
        }
        now += 100;
        if let Some(line) = parse_line(line) {
            push(tracker.line(line, now, START_UNIX_MS + now));
        }
    }
    push(tracker.tick(now + 3000));
    events
}

fn check(name: &str, fixed: Option<Protocol>) {
    let events = replay(&format!("{name}.log"), fixed);
    let expected_path = dir().join(format!("{name}.expected.json"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let text = serde_json::to_string_pretty(&events).expect("json") + "\n";
        std::fs::write(&expected_path, text).expect("write expected");
    }
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(&expected_path).expect("expected file"))
            .expect("expected json");
    assert_eq!(Value::Array(events.clone()), expected, "{name}");

    let package = Package::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/dsd"))
        .expect("plugins/dsd loads");
    for event in events {
        package
            .validator
            .check_value(event.clone())
            .unwrap_or_else(|err| panic!("{event}: {err}"));
    }
}

#[test]
fn dmr_two_slots_with_a_group_and_a_private_call() {
    check("dmr-two-slots", Some(Protocol::Dmr));
}

#[test]
fn an_encrypted_dmr_call_is_flagged_with_its_algorithm() {
    // Auto-detection: the protocol comes from the sync lines.
    check("dmr-encrypted", None);
}

#[test]
fn nxdn_ends_with_a_release_and_dpmr_by_silence() {
    check("nxdn-and-dpmr", None);
}

#[test]
fn a_protocol_the_instance_is_not_set_to_gives_no_events() {
    let events = replay("dmr-two-slots.log", Some(Protocol::Nxdn));
    assert!(events.is_empty(), "{events:?}");
}
