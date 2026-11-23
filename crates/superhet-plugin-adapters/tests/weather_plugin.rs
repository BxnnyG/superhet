//! The weather plugin end to end: a synthetic IQ stream (noise, a loud stretch, noise) goes into
//! the real adapter binary, a stand-in `satdump` script plays the decoder, and the test reads the
//! events and the pass directory the adapter leaves for the API's gallery.

#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

const ADAPTER: &str = env!("CARGO_BIN_EXE_superhet-plugin-adapter");
const RATE: usize = 192_000;

fn scratch(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("superhet-weather-it-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// A stand-in for SatDump: writes one PNG and a dataset.json into the output directory, and logs
/// its arguments.
fn stub_satdump(dir: &Path, png: &Path, exit_code: i32, make_image: bool) -> PathBuf {
    let script = dir.join("satdump-stub");
    let body = format!(
        "#!/bin/sh\necho \"$@\" >> \"{log}\"\nout=\"$4\"\nmkdir -p \"$out/MSU-MR\"\n{image}\nprintf '{{\"satellite\":\"METEOR-M2-4\"}}' > \"$out/dataset.json\"\nexit {exit_code}\n",
        log = dir.join("satdump.log").display(),
        image = if make_image {
            format!(
                "cp \"{}\" \"$out/MSU-MR/msu_mr_rgb_124.png\"",
                png.display()
            )
        } else {
            String::new()
        },
    );
    fs::write(&script, body).expect("write stub");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

fn write_png(path: &Path) {
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(fs::File::create(path).expect("create")),
        64,
        48,
    );
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = (0..64 * 48 * 3).map(|i| (i % 251) as u8).collect();
    enc.write_header()
        .expect("header")
        .write_image_data(&data)
        .expect("data");
}

/// `seconds` of cs16 IQ with the given amplitude (pseudo-random, full-band).
fn iq(seconds: usize, amplitude: i32, seed: &mut u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(seconds * RATE * 4);
    for _ in 0..seconds * RATE * 2 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let v = ((*seed >> 8) as i32 % (2 * amplitude + 1)) - amplitude;
        out.extend_from_slice(&(v as i16).to_le_bytes());
    }
    out
}

struct Run {
    events: Vec<Value>,
    code: Option<i32>,
}

fn run_adapter(store: &Path, satdump: &Path, config: &str, input: &[u8], wait_s: u64) -> Run {
    let mut child = Command::new(ADAPTER)
        .arg("weather")
        .env("SUPERHET_WEATHER_DIR", store)
        .env("SUPERHET_SATDUMP_PATH", satdump)
        .env("SUPERHET_INSTANCE_ID", "i-test")
        .env("SUPERHET_DEVICE", "dev0")
        .env("SUPERHET_FREQ_HZ", "137900000")
        .env("SUPERHET_CONFIG", config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn adapter");
    let mut stdin = child.stdin.take().expect("stdin");
    let data = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
        // Dropping stdin closes it: the host's way to ask the plugin to end.
    });
    let stdout = child.stdout.take().expect("stdout");
    let reader = std::thread::spawn(move || {
        BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
            .collect::<Vec<_>>()
    });
    writer.join().expect("writer");
    // After the input ended the adapter has to exit by itself (it may be decoding for a moment).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait_s);
    let status = loop {
        if let Some(s) = child.try_wait().expect("wait") {
            break Some(s);
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    Run {
        events: reader.join().expect("reader"),
        code: status.and_then(|s| s.code()),
    }
}

const CONFIG: &str = r#"{"arm_seconds":2,"hang_seconds":2,"min_seconds":3,"threshold_db":6}"#;

fn stream() -> Vec<u8> {
    let mut seed = 7;
    let mut data = iq(4, 300, &mut seed); // floor
    data.extend(iq(4, 6000, &mut seed)); // +26 dB: the satellite
    data.extend(iq(3, 300, &mut seed)); // gone again
    data
}

/// Every event satisfies the manifest's schema (the host would drop it otherwise).
fn assert_valid(run: &Run) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/weather");
    let package = superhet_plugin_protocol::Package::load(&dir).expect("manifest");
    for e in &run.events {
        package
            .validator
            .check_line(e.to_string().as_bytes())
            .unwrap_or_else(|err| panic!("{e}: {err:?}"));
    }
}

fn kinds(run: &Run) -> Vec<&str> {
    run.events
        .iter()
        .filter_map(|e| e["type"].as_str())
        .collect()
}

#[test]
fn a_pass_becomes_a_gallery_entry() {
    let dir = scratch("pass");
    let png = dir.join("in.png");
    write_png(&png);
    let satdump = stub_satdump(&dir, &png, 0, true);
    let store = dir.join("store");
    let run = run_adapter(&store, &satdump, CONFIG, &stream(), 30);
    assert_eq!(run.code, Some(0), "{:?}", run.events);
    assert_eq!(
        kinds(&run),
        ["weather.recording", "weather.pass"],
        "{:?}",
        run.events
    );
    let pass = &run.events[1]["data"];
    assert_eq!(pass["satellite"], "METEOR-M2-4");
    assert_eq!(pass["images"], 1);
    assert_eq!(pass["pipeline"], "lrpt");
    assert!(pass["peak_db"].as_f64().expect("peak") > 15.0);
    let id = pass["id"].as_str().expect("id");

    let doc: Value =
        serde_json::from_slice(&fs::read(store.join(id).join("pass.json")).expect("pass.json"))
            .expect("json");
    assert_eq!(doc["frequency_hz"], 137_900_000);
    assert_eq!(doc["device"], "dev0");
    assert_eq!(doc["images"][0]["name"], "msu_mr_rgb_124.png");
    assert_eq!(doc["images"][0]["thumb"], "thumb-msu_mr_rgb_124.png");
    assert!(store.join(id).join("msu_mr_rgb_124.png").is_file());
    assert!(store.join(id).join("thumb-msu_mr_rgb_124.png").is_file());
    // The IQ file and the work directory are cleaned up.
    let left: Vec<_> = fs::read_dir(store.join(".work"))
        .expect("work")
        .flatten()
        .collect();
    assert!(left.is_empty(), "{left:?}");
    // SatDump got the documented command line.
    let log = fs::read_to_string(dir.join("satdump.log")).expect("log");
    assert!(log.starts_with("meteor_m2-x_lrpt baseband "), "{log}");
    assert!(
        log.contains("--samplerate 192000 --baseband_format cs16"),
        "{log}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn quiet_input_records_nothing() {
    let dir = scratch("quiet");
    let png = dir.join("in.png");
    write_png(&png);
    let satdump = stub_satdump(&dir, &png, 0, true);
    let mut seed = 3;
    let data = iq(8, 300, &mut seed);
    let run = run_adapter(&dir.join("store"), &satdump, CONFIG, &data, 20);
    assert_eq!(run.code, Some(0));
    assert!(run.events.is_empty(), "{:?}", run.events);
    assert!(!dir.join("satdump.log").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_decode_without_images_is_reported_not_stored() {
    let dir = scratch("noimage");
    let png = dir.join("in.png");
    write_png(&png);
    let satdump = stub_satdump(&dir, &png, 0, false);
    let store = dir.join("store");
    let run = run_adapter(&store, &satdump, CONFIG, &stream(), 30);
    assert_valid(&run);
    assert_eq!(kinds(&run), ["weather.recording", "weather.failed"]);
    assert_eq!(run.events[1]["data"]["reason"], "no_image");
    let passes: Vec<_> = fs::read_dir(&store)
        .expect("store")
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(passes.is_empty(), "{passes:?}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_decoder_is_reported_with_its_reason() {
    let dir = scratch("fail");
    let png = dir.join("in.png");
    write_png(&png);
    let satdump = stub_satdump(&dir, &png, 3, true);
    let run = run_adapter(&dir.join("store"), &satdump, CONFIG, &stream(), 30);
    let failed = run
        .events
        .iter()
        .find(|e| e["type"] == "weather.failed")
        .expect("failure event");
    assert_valid(&run);
    assert_eq!(failed["data"]["reason"], "tool_failed");
    assert!(
        failed["data"]["detail"]
            .as_str()
            .expect("detail")
            .contains("exit status: 3")
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_decoder_keeps_the_recording_for_later() {
    let dir = scratch("missing");
    let store = dir.join("store");
    let run = run_adapter(&store, &dir.join("no-such-satdump"), CONFIG, &stream(), 30);
    let failed = run
        .events
        .iter()
        .find(|e| e["type"] == "weather.failed")
        .expect("failure event");
    assert_valid(&run);
    assert_eq!(failed["data"]["reason"], "tool_missing");
    let work: Vec<String> = fs::read_dir(store.join(".work"))
        .expect("work")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(work.iter().any(|n| n.ends_with(".cs16")), "{work:?}");
    assert!(work.iter().any(|n| n.ends_with(".json")), "{work:?}");

    // Once the tool is there, the next start decodes what was kept.
    let png = dir.join("in.png");
    write_png(&png);
    let satdump = stub_satdump(&dir, &png, 0, true);
    let mut seed = 1;
    let quiet = iq(2, 300, &mut seed);
    let run = run_adapter(&store, &satdump, CONFIG, &quiet, 30);
    assert_eq!(kinds(&run), ["weather.pass"], "{:?}", run.events);
    let work: Vec<_> = fs::read_dir(store.join(".work"))
        .expect("work")
        .flatten()
        .collect();
    assert!(work.is_empty(), "{work:?}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stopping_during_a_pass_leaves_it_for_the_next_start() {
    let dir = scratch("stop");
    let png = dir.join("in.png");
    write_png(&png);
    let satdump = stub_satdump(&dir, &png, 0, true);
    let store = dir.join("store");
    // The input ends while the signal is still up.
    let mut seed = 5;
    let mut data = iq(4, 300, &mut seed);
    data.extend(iq(5, 6000, &mut seed));
    let run = run_adapter(&store, &satdump, CONFIG, &data, 20);
    assert_eq!(run.code, Some(0));
    assert_eq!(kinds(&run), ["weather.recording"]);
    assert!(!dir.join("satdump.log").exists(), "no decoding at shutdown");
    // The next instance decodes it.
    let quiet = iq(1, 300, &mut seed);
    let run = run_adapter(&store, &satdump, CONFIG, &quiet, 30);
    assert_eq!(kinds(&run), ["weather.pass"], "{:?}", run.events);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_config_is_refused_at_start() {
    let dir = scratch("badconfig");
    let out = Command::new(ADAPTER)
        .arg("weather")
        .env("SUPERHET_WEATHER_DIR", dir.join("store"))
        .env("SUPERHET_CONFIG", r#"{"threshold_db": 0}"#)
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("threshold_db"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn offline_decode_imports_a_recording_and_keeps_the_original() {
    let dir = scratch("import");
    let png = dir.join("in.png");
    write_png(&png);
    let satdump = stub_satdump(&dir, &png, 0, true);
    let iq_file = dir.join("recording.cs16");
    fs::write(&iq_file, [0u8; 4096]).expect("write");
    let store = dir.join("store");
    let out = Command::new(ADAPTER)
        .args([
            "weather-decode",
            "--pipeline",
            "apt",
            "--rate",
            "48000",
            "--format",
            "cs16",
        ])
        .arg(&iq_file)
        .env("SUPERHET_WEATHER_DIR", &store)
        .env("SUPERHET_SATDUMP_PATH", &satdump)
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("stored pass "));
    assert!(iq_file.is_file(), "the user's file must stay");
    let log = fs::read_to_string(dir.join("satdump.log")).expect("log");
    assert!(log.starts_with("noaa_apt baseband "), "{log}");
    assert!(log.contains("--samplerate 48000"), "{log}");
    let _ = fs::remove_dir_all(&dir);
}
