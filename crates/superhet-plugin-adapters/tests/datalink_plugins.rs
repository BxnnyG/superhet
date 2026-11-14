//! The AIS, ACARS and VDL2 adapters end to end (ADR-0031): the real `superhet-plugin-adapter`
//! binary the way the host starts it, with stand-in tools (`tests/ais/AIS-catcher`,
//! `tests/acars/acarsdec`, `tests/vdl2/dumpvdl2`, POSIX shell scripts) that print committed sample
//! output. The tests check the command line the tool got, that the host's samples reach its
//! stdin, and that the events satisfy the manifest's schema (`contract::check`).
//!
//! The real tools are built into the image (`scripts/check-plugin-image.sh` runs them there); no
//! recordings of AIS, ACARS or VDL2 are committed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use superhet_plugin_protocol::contract::{check, check_static, run_simple_with_env};

const TIMEOUT: Duration = Duration::from_secs(30);

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures(dir: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(dir)
}

struct Outcome {
    events: Vec<(String, Value)>,
    args: String,
    stdin_bytes: u64,
}

/// Run `plugin` with the stand-in tool of `fixture_dir` in front of `PATH`, feeding `input`.
fn run_plugin(plugin: &str, fixture_dir: &str, config: &Value, input: &[u8]) -> Outcome {
    let package = check_static(&repo().join("plugins").join(plugin))
        .unwrap_or_else(|problems| panic!("{plugin}: {problems:?}"));
    let adapter = Path::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"))
        .parent()
        .expect("bin dir")
        .to_path_buf();
    let path = std::env::join_paths([fixtures(fixture_dir), adapter].into_iter().chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("PATH");
    let path = path.to_string_lossy().into_owned();
    static RUN: AtomicUsize = AtomicUsize::new(0);
    let scratch = std::env::temp_dir().join(format!(
        "superhet-datalink-test-{plugin}-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let args_out = scratch.join("args").display().to_string();
    let stdin_out = scratch.join("stdin").display().to_string();
    let report = run_simple_with_env(
        &package,
        input,
        config,
        TIMEOUT,
        &[
            ("PATH", &path),
            ("FAKE_ARGS_OUT", &args_out),
            ("FAKE_STDIN_OUT", &stdin_out),
        ],
    )
    .expect("run");
    let problems = check(&report);
    assert!(
        problems.is_empty(),
        "{plugin}: {problems:?}\nstderr: {}",
        report.stderr
    );
    let outcome = Outcome {
        events: report
            .events
            .iter()
            .map(|e| (e.event_type.clone(), Value::Object(e.data.clone())))
            .collect(),
        args: std::fs::read_to_string(&args_out)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        stdin_bytes: std::fs::read_to_string(&stdin_out)
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    outcome
}

#[test]
fn ais_vessels_from_nmea() {
    let samples = vec![0x80u8; 192_000];
    let out = run_plugin("ais", "ais", &Value::Null, &samples);
    assert_eq!(out.args, "-r cu8 . -s 192000 -o 1");
    assert_eq!(out.stdin_bytes, 192_000, "the samples must reach the tool");
    // The repeated report of the first vessel is rationed, the bad checksum and the GPS sentence
    // are dropped, the static data of the first vessel is news.
    let kinds: Vec<(&str, u64)> = out
        .events
        .iter()
        .map(|(t, d)| (t.as_str(), d["mmsi"].as_u64().expect("mmsi")))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("ais.vessel", 244_670_316),
            ("ais.vessel", 477_553_000),
            ("ais.vessel", 244_670_316),
            ("ais.vessel", 211_234_560),
        ]
    );
    let first = &out.events[0].1;
    assert_eq!(first["class"], "A");
    assert_eq!(first["sog_kn"], 12.3);
    assert_eq!(first["sog_kmh"], 22.8);
    assert_eq!(first["cog_deg"], 270.4);
    assert_eq!(first["lat"], 52.0123);
    assert_eq!(first["lon"], 4.1234);
    let named = &out.events[2].1;
    assert_eq!(named["name"], "EVER GIVEN");
    assert_eq!(named["ship_type_text"], "Cargo");
    assert_eq!(named["destination"], "NLRTM");
    assert_eq!(named["lat"], 52.0123, "the earlier position is merged in");
    assert_eq!(out.events[1].1["status_text"], "Moored");
}

#[test]
fn ais_event_interval_is_configurable() {
    let out = run_plugin("ais", "ais", &json!({"event_interval_s": 5}), b"");
    // Replayed in an instant, so the interval makes no difference here; the config is accepted.
    assert_eq!(out.events.len(), 4);
}

#[test]
fn acars_messages() {
    let samples = vec![0x80u8; 240_000];
    let out = run_plugin("acars", "acars", &Value::Null, &samples);
    // The host started the instance on 100 MHz (contract test environment).
    assert_eq!(
        out.args,
        "-e --output json:file:path=- --iqfile file=/dev/stdin,sr=240000,format=U8 -c 100.000000 100.000000"
    );
    assert_eq!(out.stdin_bytes, 240_000);
    // The object without a label and the non-JSON line are dropped.
    assert_eq!(out.events.len(), 2);
    let (kind, d) = &out.events[0];
    assert_eq!(kind, "acars.message");
    assert_eq!(d["tail"], "D-AIPA");
    assert_eq!(d["flight"], "LH1234");
    assert_eq!(d["label"], "H1");
    assert_eq!(d["direction"], "downlink");
    assert_eq!(d["text"], "#CFBFLT01/PSN 4811.2N01131.5E");
    assert_eq!(out.events[1].1["direction"], "uplink");
}

#[test]
fn acars_extra_channels_reach_the_command_line() {
    let out = run_plugin(
        "acars",
        "acars",
        &json!({"frequencies_mhz": [100.1, 100.05], "include_empty": true}),
        b"",
    );
    assert_eq!(
        out.args,
        "--output json:file:path=- --iqfile file=/dev/stdin,sr=240000,format=U8 -c 100.000000 100.050000 100.100000"
    );
}

#[test]
fn vdl2_messages() {
    let samples = vec![0x80u8; 420_000];
    let out = run_plugin("vdl2", "vdl2", &Value::Null, &samples);
    assert_eq!(
        out.args,
        "--iq-file - --sample-format U8 --oversample 4 --centerfreq 100000000 \
         --output decoded:json:file:path=- 100000000"
    );
    assert_eq!(out.stdin_bytes, 420_000);
    // The XID frame carries no message and is left out.
    assert_eq!(out.events.len(), 2);
    let (kind, d) = &out.events[0];
    assert_eq!(kind, "vdl2.message");
    assert_eq!(d["kind"], "acars");
    assert_eq!(d["icao"], "3c6444");
    assert_eq!(d["tail"], "D-AIPA");
    assert_eq!(d["freq_mhz"], 136.975);
    let (_, x25) = &out.events[1];
    assert_eq!(x25["kind"], "x25");
    assert_eq!(x25["icao"], "3c6444", "the aircraft is the destination");
    assert_eq!(x25["decoded"]["clnp"]["cpdlc"]["msg"], "CLEARED TO LAND");
}

#[test]
fn vdl2_all_frames_includes_link_management() {
    let out = run_plugin("vdl2", "vdl2", &json!({"all_frames": true}), b"");
    assert_eq!(out.events.len(), 3);
    assert_eq!(out.events[1].1["kind"], "xid");
}

#[test]
fn missing_tools_exit_127_and_bad_config_exits_2() {
    let adapter = env!("CARGO_BIN_EXE_superhet-plugin-adapter");
    for mode in ["ais", "acars", "vdl2"] {
        let status = std::process::Command::new(adapter)
            .arg(mode)
            .env("PATH", "")
            .env("SUPERHET_FREQ_HZ", "131725000")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("spawn");
        assert_eq!(status.code(), Some(127), "{mode}: tool not installed");
        let status = std::process::Command::new(adapter)
            .arg(mode)
            .env("PATH", "")
            .env("SUPERHET_FREQ_HZ", "131725000")
            .env("SUPERHET_CONFIG", r#"{"nonsense":1}"#)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("spawn");
        assert_eq!(status.code(), Some(2), "{mode}: bad config");
    }
    // The decoders that read a channel need the receiver's frequency.
    for mode in ["acars", "vdl2"] {
        let status = std::process::Command::new(adapter)
            .arg(mode)
            .env("PATH", "")
            .env_remove("SUPERHET_FREQ_HZ")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("spawn");
        assert_eq!(status.code(), Some(2), "{mode}: no frequency");
    }
}
