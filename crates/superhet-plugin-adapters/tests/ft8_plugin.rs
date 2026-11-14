//! The FT8 / FT4 / WSPR adapter end to end: the real `superhet-plugin-adapter ft8` binary with
//! a stream of audio on stdin, the way the host feeds it (simple transport).
//!
//! The adapter cuts the audio into UTC slots by the wall clock; `SUPERHET_FT8_STREAM_START_MS`
//! replaces the clock by the sample count so the tests run faster than real time. Two kinds of
//! cases: stand-in `jt9` / `wsprd` scripts (`tests/ft8/fake-*`, POSIX sh) check the slicing, the
//! command lines and the event mapping; the real WSJT-X tools, when they are on `PATH`, decode
//! synthetic signals made here (`SUPERHET_REQUIRE_PLUGIN_TOOLS=1`, set in CI, turns "not
//! installed" into a failure).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use superhet_plugin_protocol::contract::{Report, check, check_static, run_simple_with_env};

const TIMEOUT: Duration = Duration::from_secs(120);
const RATE: usize = 12_000;
/// 2026-11-14T12:00:00Z: a boundary of every slot length.
const T0: u64 = 1_794_657_600_000;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/ft8")
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
}

fn bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// A directory with `jt9` and `wsprd` stand-ins, a scratch directory for logs.
fn fake_tools(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("superhet-ft8-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::copy(fixtures().join("fake-jt9"), dir.join("jt9")).expect("copy jt9");
    std::fs::copy(fixtures().join("fake-wsprd"), dir.join("wsprd")).expect("copy wsprd");
    dir
}

#[derive(Clone, Copy, PartialEq)]
enum System {
    /// Only the adapter (and the directory of stand-ins).
    No,
    /// Enough for `sh`, `wc` and `basename` of the stand-ins, not for the real WSJT-X tools.
    Basic,
    /// The test runner's own `PATH`.
    Full,
}

/// `PATH` with the adapter binary (the manifest calls it by name) and then `first`.
fn path_with(first: Option<&Path>, system: System) -> String {
    let adapter = Path::new(env!("CARGO_BIN_EXE_superhet-plugin-adapter"))
        .parent()
        .expect("bin dir")
        .to_path_buf();
    let mut dirs = vec![adapter];
    dirs.extend(first.map(Path::to_path_buf));
    match system {
        System::No => {}
        System::Basic => dirs.extend(["/usr/bin", "/bin"].map(PathBuf::from)),
        System::Full => {
            dirs.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
        }
    }
    std::env::join_paths(dirs)
        .expect("PATH")
        .to_string_lossy()
        .into_owned()
}

fn run(
    config: &Value,
    samples: &[i16],
    path: &str,
    start_ms: u64,
    extra: &[(&str, &str)],
) -> Report {
    let package = check_static(&repo().join("plugins/ft8"))
        .unwrap_or_else(|problems| panic!("ft8: {problems:?}"));
    let start = start_ms.to_string();
    let mut env = vec![
        ("PATH", path),
        ("SUPERHET_FT8_STREAM_START_MS", start.as_str()),
        ("SUPERHET_FREQ_HZ", "14074000"),
    ];
    env.extend_from_slice(extra);
    run_simple_with_env(&package, &bytes(samples), config, TIMEOUT, &env).expect("run")
}

/// Audio that is not silence: a quiet square wave.
fn noise_like(seconds: usize) -> Vec<i16> {
    (0..seconds * RATE)
        .map(|i| if (i / 40) % 2 == 0 { 800 } else { -800 })
        .collect()
}

fn data(report: &Report, index: usize) -> &serde_json::Map<String, Value> {
    &report.events[index].data
}

#[test]
fn ft8_slots_are_cut_at_utc_boundaries_and_decoded() {
    let dir = fake_tools("ft8");
    let log = dir.join("log.txt");
    // 31 s from 12:00:00: the slots 12:00:00 and 12:00:15 are complete, 12:00:30 is not.
    let report = run(
        &json!({"mode": "ft8", "depth": 3}),
        &noise_like(31),
        &path_with(Some(&dir), System::Basic),
        T0,
        &[("FAKE_FT8_LOG", log.to_str().expect("utf8"))],
    );
    let problems = check(&report);
    assert!(
        problems.is_empty(),
        "{problems:?}\nstderr: {}",
        report.stderr
    );
    assert_eq!(report.events.len(), 4, "stderr: {}", report.stderr);

    let first = data(&report, 0);
    assert_eq!(report.events[0].event_type, "ft8.message");
    assert_eq!(first["mode"], "ft8");
    assert_eq!(first["time"], "2026-11-14T12:00:00Z");
    assert_eq!(first["snr"], -12);
    assert_eq!(first["dt"], -0.3);
    assert_eq!(first["freq_hz"], 1234);
    assert_eq!(first["dial_hz"], 14_074_000u64);
    assert_eq!(first["rf_hz"], 14_075_234u64);
    assert_eq!(first["callsign"], "DL1ABC");
    assert_eq!(first["grid"], "JO62");
    assert_eq!(first["cq"], true);
    let second = data(&report, 1);
    assert_eq!(second["to"], "DL1ABC");
    assert_eq!(second["callsign"], "K1ABC");
    assert_eq!(second["message"], "DL1ABC K1ABC R-05");
    assert_eq!(data(&report, 2)["time"], "2026-11-14T12:00:15Z");

    // One run per slot, with the file the tool names the time after, 15 s of 16-bit audio.
    let calls = std::fs::read_to_string(&log).expect("log");
    let calls: Vec<&str> = calls.lines().collect();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[0].starts_with("-8 -p 15 -d 3 -a "), "{}", calls[0]);
    assert!(calls[0].contains("261114_120000.wav"), "{}", calls[0]);
    assert!(calls[1].contains("261114_120015.wav"), "{}", calls[1]);
    assert!(
        calls[0].ends_with(&format!("size={}", 44 + 15 * RATE * 2)),
        "{}",
        calls[0]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stream_that_starts_mid_slot_waits_for_the_next_one() {
    let dir = fake_tools("late");
    let report = run(
        &json!({}),
        &noise_like(25),
        &path_with(Some(&dir), System::Basic),
        T0 + 7_000,
        &[],
    );
    assert!(check(&report).is_empty(), "{}", report.stderr);
    // 12:00:07 to 12:00:32: only the slot 12:00:15 to 12:00:30 is complete.
    assert_eq!(report.events.len(), 2);
    assert_eq!(data(&report, 0)["time"], "2026-11-14T12:00:15Z");
}

#[test]
fn ft4_uses_short_slots_and_its_own_flag() {
    let dir = fake_tools("ft4");
    let log = dir.join("log.txt");
    let report = run(
        &json!({"mode": "ft4", "depth": 1}),
        &noise_like(16),
        &path_with(Some(&dir), System::Basic),
        T0,
        &[("FAKE_FT8_LOG", log.to_str().expect("utf8"))],
    );
    assert!(check(&report).is_empty(), "{}", report.stderr);
    assert_eq!(data(&report, 0)["mode"], "ft4");
    assert_eq!(data(&report, 2)["time"], "2026-11-14T12:00:07.500Z");
    let calls = std::fs::read_to_string(&log).expect("log");
    assert_eq!(calls.lines().count(), 2);
    assert!(calls.starts_with("-5 -p 7 -d 1 -a "), "{calls}");
    assert!(
        calls
            .lines()
            .next()
            .expect("line")
            .ends_with(&format!("size={}", 44 + 90_000 * 2))
    );
}

#[test]
fn wspr_slots_are_two_minutes() {
    let dir = fake_tools("wspr");
    let log = dir.join("log.txt");
    let report = run(
        &json!({"mode": "wspr", "dial_hz": 14_095_600}),
        &noise_like(121),
        &path_with(Some(&dir), System::Basic),
        T0,
        &[("FAKE_FT8_LOG", log.to_str().expect("utf8"))],
    );
    assert!(check(&report).is_empty(), "{}", report.stderr);
    assert_eq!(report.events.len(), 2, "{}", report.stderr);
    let spot = data(&report, 0);
    assert_eq!(report.events[0].event_type, "wspr.spot");
    assert_eq!(spot["time"], "2026-11-14T12:00:00Z");
    assert_eq!(spot["callsign"], "K1ABC");
    assert_eq!(spot["grid"], "FN42");
    assert_eq!(spot["power_dbm"], 37);
    assert_eq!(spot["freq_hz"], 1500);
    // dial_hz in the config wins over the frequency the instance was started on.
    assert_eq!(spot["rf_hz"], 14_097_100u64);
    assert_eq!(data(&report, 1)["callsign"], "PJ4/K1ABC");
    assert_eq!(data(&report, 1)["drift"], -1);
    let calls = std::fs::read_to_string(&log).expect("log");
    assert!(calls.starts_with("-a "), "{calls}");
    assert!(calls.contains("261114_1200.wav"), "{calls}");
    assert!(
        calls
            .trim_end()
            .ends_with(&format!("size={}", 44 + 120 * RATE * 2))
    );
}

#[test]
fn silence_is_not_decoded_and_a_failing_tool_does_not_end_the_plugin() {
    let dir = fake_tools("quiet");
    let log = dir.join("log.txt");
    let silence = vec![0i16; 31 * RATE];
    let report = run(
        &json!({}),
        &silence,
        &path_with(Some(&dir), System::Basic),
        T0,
        &[("FAKE_FT8_LOG", log.to_str().expect("utf8"))],
    );
    assert!(report.events.is_empty());
    assert!(!log.exists(), "the decoder ran on silence");

    let report = run(
        &json!({}),
        &noise_like(31),
        &path_with(Some(&dir), System::Basic),
        T0,
        &[("FAKE_FT8_FAIL", "1")],
    );
    assert!(report.events.is_empty());
    assert!(report.stderr.contains("ended with"), "{}", report.stderr);
    assert!(
        report.exit.is_some_and(|s| s.success()),
        "the adapter ended with {:?}",
        report.exit
    );
}

#[test]
fn a_missing_tool_is_reported() {
    // Neither the stand-ins nor WSJT-X are on this PATH.
    let report = run(
        &json!({}),
        &noise_like(1),
        &path_with(None, System::No),
        T0,
        &[],
    );
    assert_eq!(
        report.exit.and_then(|s| s.code()),
        Some(127),
        "{}",
        report.stderr
    );
    assert!(
        report.stderr.contains("cannot find jt9"),
        "{}",
        report.stderr
    );
}

#[test]
fn a_bad_configuration_is_refused() {
    let report = run(
        &json!({"mode": "jt65"}),
        &[],
        &path_with(None, System::No),
        T0,
        &[],
    );
    assert_eq!(report.exit.and_then(|s| s.code()), Some(2));
    assert!(report.stderr.contains("mode must be"), "{}", report.stderr);
}

// ---------- the real WSJT-X tools on synthetic signals ----------

/// FSK tones of "CQ DL1ABC JO62" (ft8_lib, `gen_ft8`).
const FT8_TONES: &str =
    "3140652000000001045311305210577534623140652674424751471436372416647501333140652";
/// Channel symbols of the WSPR message "K1ABC FN42 37" (`wsprsim -c`).
const WSPR_SYMBOLS: &str = "330020001020131222100323133220200032012322002232110233210221321222033030301210212032132003323032203020201023021112330231212221332000010320132222202332323320031222";

struct Noise(u64);

impl Noise {
    /// Roughly normal noise with the given standard deviation (sum of uniforms).
    fn next(&mut self, sigma: f64) -> f64 {
        let mut sum = 0.0;
        for _ in 0..12 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            sum += (self.0 >> 11) as f64 / (1u64 << 53) as f64;
        }
        (sum - 6.0) * sigma
    }
}

/// Continuous-phase FSK of `tones`: `spacing` Hz apart from `base` Hz, `symbol` samples each,
/// starting `start` samples into a `total` sample slot, over noise.
fn fsk(
    tones: &[f64],
    base: f64,
    spacing: f64,
    symbol: usize,
    start: usize,
    total: usize,
    amp: f64,
) -> Vec<i16> {
    let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
    let mut out: Vec<f64> = (0..total).map(|_| noise.next(1000.0)).collect();
    let mut phase = 0.0f64;
    let mut at = start;
    for tone in tones {
        let freq = base + tone * spacing;
        let step = 2.0 * std::f64::consts::PI * freq / RATE as f64;
        for _ in 0..symbol {
            out[at] += amp * phase.sin();
            phase += step;
            at += 1;
        }
        phase %= 2.0 * std::f64::consts::PI;
    }
    out.iter()
        .map(|v| v.round().clamp(-32_000.0, 32_000.0) as i16)
        .collect()
}

fn digits(text: &str) -> Vec<f64> {
    text.bytes().map(|b| f64::from(b - b'0')).collect()
}

fn require_tool(tool: &str) -> bool {
    if on_path(tool) {
        return true;
    }
    assert!(
        std::env::var_os("SUPERHET_REQUIRE_PLUGIN_TOOLS").is_none(),
        "{tool} is required (SUPERHET_REQUIRE_PLUGIN_TOOLS is set) but not on PATH"
    );
    eprintln!("SKIP: {tool} is not installed");
    false
}

#[test]
fn real_jt9_decodes_a_synthetic_ft8_signal() {
    if !require_tool("jt9") {
        return;
    }
    // 15 s slot, the signal starts 0.5 s in; a second slot of noise only.
    let mut samples = fsk(
        &digits(FT8_TONES),
        1500.0,
        6.25,
        1920,
        RATE / 2,
        15 * RATE,
        500.0,
    );
    samples.extend(fsk(&[], 0.0, 0.0, 0, 0, 16 * RATE, 0.0));
    let report = run(
        &json!({"mode": "ft8"}),
        &samples,
        &path_with(None, System::Full),
        T0,
        &[],
    );
    assert!(check(&report).is_empty(), "{}", report.stderr);
    assert_eq!(report.events.len(), 1, "{}", report.stderr);
    let d = data(&report, 0);
    assert_eq!(d["message"], "CQ DL1ABC JO62");
    assert_eq!(d["callsign"], "DL1ABC");
    assert_eq!(d["grid"], "JO62");
    assert_eq!(d["time"], "2026-11-14T12:00:00Z");
    assert_eq!(d["freq_hz"], 1500);
    assert_eq!(d["rf_hz"], 14_075_500u64);
}

#[test]
fn real_wsprd_decodes_a_synthetic_wspr_signal() {
    if !require_tool("wsprd") {
        return;
    }
    // One 2 minute slot; the transmission starts a second in. 12000/8192 Hz spacing, centred on 1500.
    let tones: Vec<f64> = digits(WSPR_SYMBOLS).iter().map(|s| s - 1.5).collect();
    let mut samples = fsk(
        &tones,
        1500.0,
        12_000.0 / 8_192.0,
        8_192,
        RATE,
        120 * RATE,
        500.0,
    );
    samples.extend(vec![0i16; RATE]);
    let report = run(
        &json!({"mode": "wspr"}),
        &samples,
        &path_with(None, System::Full),
        T0,
        &[("SUPERHET_FREQ_HZ", "14095600")],
    );
    assert!(check(&report).is_empty(), "{}", report.stderr);
    assert_eq!(report.events.len(), 1, "{}", report.stderr);
    let d = data(&report, 0);
    assert_eq!(d["callsign"], "K1ABC");
    assert_eq!(d["grid"], "FN42");
    assert_eq!(d["power_dbm"], 37);
    assert_eq!(d["freq_hz"], 1500);
    assert_eq!(d["rf_hz"], 14_097_100u64);
}
