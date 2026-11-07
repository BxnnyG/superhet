//! Golden test of the TETRA datagram parser and the call tracker: transcripts of what `tetra-rx`
//! sends (`tests/tetra/transcripts/*.udp`) must give exactly the events in the matching
//! `*.expected.json`, and every event must satisfy the events schema of `plugins/tetra`.
//!
//! `generator-sync.udp` is **captured**: osmo-tetra's own test generator (`build_sb()` in
//! `src/conv_enc_test.c`) made the burst in `tests/tetra/sync-burst.bits`, a real `tetra-rx` decoded
//! it, and its datagrams are the file. The others are written by hand in the formats the sources
//! of `tetra-rx` show (`docs/plugins-tetra.md`): no TETRA network was recorded. Regenerate the
//! expected files after an intended change with `UPDATE_GOLDEN=1 cargo test -p
//! superhet-plugin-adapters --test tetra_golden` and read the diff.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use superhet_plugin_adapters::tetra::call::{Emit, Tracker, Voice};
use superhet_plugin_adapters::tetra::tetmon::{Message, parse_datagram, parse_sysinfo_line};
use superhet_plugin_protocol::Package;

const START_UNIX_MS: u64 = 1_700_000_000_000;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tetra/transcripts")
}

/// Play a transcript: 100 ms per line, 500 ms per `---`, then thirty seconds of silence. Voice is
/// played (not withheld) when the tracker says so; the tracker gets the codec's view too.
fn replay(name: &str, mute_encrypted: bool) -> Vec<Value> {
    let text = std::fs::read_to_string(dir().join(name)).expect("transcript");
    let mut tracker = Tracker::new(mute_encrypted);
    let mut events = Vec::new();
    let mut now = 1000u64;
    let mut push = |emits: Vec<Emit>| {
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
        if let Some(console) = line.strip_prefix("console:") {
            if let Some(info) = parse_sysinfo_line(console.trim()) {
                push(tracker.sysinfo(info, now));
            }
        } else if let Some(usage) = line.strip_prefix("TRA ") {
            let usage: u32 = usage.trim().parse().expect("usage marker");
            let (action, emits) = tracker.voice(usage, now, START_UNIX_MS + now);
            if action == Voice::Play {
                tracker.played(usage);
            }
            push(emits);
        } else if let Some(message) = parse_datagram(line.as_bytes()) {
            assert!(!matches!(message, Message::Voice { .. }));
            push(tracker.message(message, now, START_UNIX_MS + now));
        }
    }
    push(tracker.tick(now + 30_000));
    events
}

fn check(name: &str) {
    let events = replay(&format!("{name}.udp"), true);
    let expected_path = dir().join(format!("{name}.expected.json"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let text = serde_json::to_string_pretty(&events).expect("json") + "\n";
        std::fs::write(&expected_path, text).expect("write expected");
    }
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(&expected_path).expect("expected file"))
            .expect("expected json");
    assert_eq!(Value::Array(events.clone()), expected, "{name}");

    let package = Package::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/tetra"))
        .expect("plugins/tetra loads");
    for event in events {
        package
            .validator
            .check_value(event.clone())
            .unwrap_or_else(|err| panic!("{event}: {err}"));
    }
}

#[test]
fn the_cell_of_osmo_tetras_own_test_burst_is_mcc_262_mnc_42() {
    check("generator-sync");
    let events = replay("generator-sync.udp", true);
    assert_eq!(events.len(), 1, "{events:?}");
    let cell = &events[0]["data"];
    assert_eq!(
        (
            cell["mcc"].as_u64(),
            cell["mnc"].as_u64(),
            cell["cc"].as_u64()
        ),
        (Some(262), Some(42), Some(0))
    );
}

#[test]
fn a_group_call_with_its_cell_and_talker() {
    check("group-call");
}

#[test]
fn an_encrypted_call_is_flagged_and_its_voice_withheld() {
    check("encrypted-call");
    let events = replay("encrypted-call.udp", true);
    let end = events
        .iter()
        .find(|e| e["type"] == "tetra.call.end")
        .expect("call end");
    assert_eq!(end["data"]["encrypted"], true);
    assert_eq!(end["data"]["muted"], true);
    assert_eq!(end["data"]["audio_ms"], 0);
    assert!(end["data"]["voice_ms"].as_u64().is_some_and(|ms| ms > 0));
    // With muting off the same air gives audio.
    let events = replay("encrypted-call.udp", false);
    let end = events
        .iter()
        .find(|e| e["type"] == "tetra.call.end")
        .expect("call end");
    assert!(end["data"]["audio_ms"].as_u64().is_some_and(|ms| ms > 0));
}

#[test]
fn neighbours_status_and_text_messages_without_repeats() {
    check("cell-and-messages");
    let events = replay("cell-and-messages.udp", true);
    let count = |kind: &str| events.iter().filter(|e| e["type"] == kind).count();
    assert_eq!(
        (
            count("tetra.neighbour"),
            count("tetra.status"),
            count("tetra.sds")
        ),
        (1, 1, 1)
    );
}
