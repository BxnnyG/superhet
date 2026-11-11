//! Tests of the recording engine without database or bus: ring buffer fed from bus messages,
//! the audio writer task on files, the squelch gate, scanner hit endings and file safety.

use super::*;
use superhet_recording::ogg;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("superhet-rec-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

fn frame(i: u64, open: bool) -> ReplayFrame {
    ReplayFrame {
        capture_unix_ns: 1_000_000_000 + i * 20_000_000,
        channels: 1,
        open,
        payload: Arc::from(vec![(i % 251) as u8; 40]),
    }
}

fn bus_message(i: u64, open: bool) -> Vec<u8> {
    AudioFrame {
        channels: 1,
        flags: u8::from(open),
        sample_rate: 48_000,
        vfo_id: 3,
        seq: i as u32,
        sample_pos: i * 960,
        capture_unix_ns: 1_000_000_000 + i * 20_000_000,
        power_dbfs: -30.0,
        payload: vec![(i % 251) as u8; 40],
    }
    .encode()
}

#[test]
fn the_subject_parser_accepts_only_the_expected_shape() {
    assert_eq!(
        device_of("sh.v1.dev.dev0.audio.3", "audio"),
        Some(("dev0".into(), "3".into()))
    );
    assert_eq!(device_of("sh.v1.dev.dev0.rds.3", "audio"), None);
    assert_eq!(device_of("sh.v1.dev.dev0.audio.3.x", "audio"), None);
    assert_eq!(device_of("sh.v1.other.dev0.audio.3", "audio"), None);
}

#[test]
fn audio_on_the_bus_fills_the_ring_and_replays_as_a_valid_ogg_file() {
    let mgr = Manager::for_tests(temp_dir("ring"));
    for i in 0..500 {
        mgr.on_audio("sh.v1.dev.dev0.audio.3", &bus_message(i, i >= 100));
    }
    // Garbage, plugin pseudo VFOs and other VFOs do not disturb it.
    mgr.on_audio("sh.v1.dev.dev0.audio.3", b"junk");
    mgr.on_audio("sh.v1.dev.dev0.audio.1073741824", &bus_message(1, true));
    assert_eq!(mgr.replay_usage().0, 1);
    let (bytes, frames) = mgr.replay_ogg("dev0", 3, 5, false).expect("replay");
    assert!((250..=251).contains(&frames), "{frames}");
    let file = ogg::parse(&bytes).expect("valid ogg");
    assert_eq!(file.packets.len(), frames);
    assert!(file.ended);
    assert!(mgr.replay_ogg("dev0", 9, 5, false).is_none());
    assert!(mgr.replay_ogg("dev1", 3, 5, false).is_none());
    let (_, gated) = mgr.replay_ogg("dev0", 3, 10, true).expect("replay");
    // 400 open frames plus the 5 frames of pre-roll; the other 95 closed ones are cut.
    assert_eq!(gated, 405);
}

#[test]
fn rds_names_are_remembered_per_vfo() {
    let mgr = Manager::for_tests(temp_dir("rds"));
    let msg = json!({"v": 1, "type": "rds", "vfo": 3, "synced": true, "ps": "RADIO 1 "});
    mgr.on_rds("sh.v1.dev.dev0.rds.3", msg.to_string().as_bytes());
    let tags = lock(&mgr.tags)
        .get(&("dev0".to_owned(), 3))
        .cloned()
        .expect("tags");
    assert_eq!(tags.ps.as_deref(), Some("RADIO 1 "));
}

async fn record(mgr: &Arc<Manager>, gated: bool, pattern: &str, idle: Option<usize>) -> Vec<u8> {
    let dir = mgr.cfg.dir.join("audio");
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("rec1.ogg");
    let file = tokio::fs::File::create(&path).await.expect("file");
    let (tx, rx) = mpsc::channel(1024);
    let (_stop_tx, stop_rx) = watch::channel(None);
    let shared = Arc::new(Shared {
        bytes: AtomicU64::new(0),
        duration_ms: AtomicU64::new(0),
    });
    for (i, c) in pattern.chars().enumerate() {
        tx.send(frame(i as u64, c == '#')).await.expect("send");
    }
    drop(tx);
    let req = AudioStart {
        device: "dev0".into(),
        vfo: 3,
        gated,
        title: "Test".into(),
        source: "manual",
        owner: Owner {
            name: "op".into(),
            id: "u:1".into(),
        },
        freq_hz: Some(145_500_000),
        mode: Some("nfm".into()),
        max_seconds: 3600,
        max_bytes: 1 << 30,
        idle_stop_frames: idle,
    };
    Arc::clone(mgr)
        .run_audio(1, req, file, rx, stop_rx, shared, Tags::default())
        .await;
    std::fs::read(&path).expect("read")
}

#[tokio::test]
async fn an_audio_recording_is_a_valid_ogg_with_the_original_packets() {
    let mgr = Manager::for_tests(temp_dir("audio"));
    let bytes = record(&mgr, false, &"#".repeat(120), None).await;
    let file = ogg::parse(&bytes).expect("valid ogg");
    assert_eq!(file.packets.len(), 120);
    assert_eq!(file.packets[7], vec![7u8; 40]);
    assert_eq!(file.duration_ms(), 2400);
    assert!(file.ended);
    for (key, value) in [
        ("FREQUENCY", "145500000"),
        ("TITLE", "Test"),
        ("MODE", "nfm"),
    ] {
        assert!(
            file.comments.iter().any(|(k, v)| k == key && v == value),
            "{key}"
        );
    }
}

#[tokio::test]
async fn a_gated_recording_skips_silence() {
    let mgr = Manager::for_tests(temp_dir("gated"));
    let pattern = format!("{}{}{}", "-".repeat(200), "#".repeat(50), "-".repeat(200));
    let bytes = record(&mgr, true, &pattern, None).await;
    let file = ogg::parse(&bytes).expect("valid ogg");
    // 5 pre-roll + 50 open + 20 hang.
    assert_eq!(file.packets.len(), 75);
}

#[tokio::test]
async fn a_hit_recording_ends_after_the_silence_limit() {
    let mgr = Manager::for_tests(temp_dir("hit"));
    let pattern = format!("{}{}", "#".repeat(30), "-".repeat(400));
    let bytes = record(&mgr, true, &pattern, Some(50)).await;
    let file = ogg::parse(&bytes).expect("valid ogg");
    // 30 open + 20 hang; the run stops at 50 closed frames.
    assert_eq!(file.packets.len(), 50);
}

fn row(stem: &str) -> db::Record {
    db::Record {
        id: 5,
        kind: "audio".into(),
        source: "manual".into(),
        state: "done".into(),
        stem: stem.into(),
        title: String::new(),
        device: "dev0".into(),
        vfo: None,
        freq_hz: None,
        mode: None,
        owner_name: None,
        owner_id: None,
        started_ms: 0,
        stopped_ms: None,
        duration_ms: 0,
        size_bytes: 0,
        keep: false,
        meta: Value::Null,
    }
}

#[tokio::test]
async fn file_removal_never_leaves_the_recordings_directory() {
    let root = temp_dir("remove");
    let mgr = Manager::for_tests(root.join("recordings"));
    std::fs::create_dir_all(root.join("recordings/audio")).expect("dir");
    let outside = root.join("outside.ogg");
    std::fs::write(&outside, b"keep me").expect("file");
    let inside = root.join("recordings/audio/rec5.ogg");
    std::fs::write(&inside, b"x").expect("file");
    mgr.remove_files(&row("../../outside")).await;
    mgr.remove_files(&row("..")).await;
    assert!(outside.exists());
    assert!(inside.exists());
    mgr.remove_files(&row("rec5")).await;
    assert!(!inside.exists());
}

/// Who may call what: accounts list and play, operators record and manage, guests get nothing.
#[test]
fn access_follows_the_roles() {
    use crate::access::{Actor, Principal, find_rule};
    use crate::device_access::{Need, find_device_rule};
    use axum::http::Method;
    use superhet_db::users::Role;

    let guest = Principal {
        actor: Actor::Guest("ab12".into()),
        ..Principal::test(Role::Listener)
    };
    let listener = Principal::test(Role::Listener);
    let operator = Principal::test(Role::Operator);
    let admin = Principal::test(Role::Admin);
    let reading = [
        (Method::GET, "/api/v1/recordings"),
        (Method::GET, "/api/v1/recordings/config"),
        (Method::GET, "/api/v1/recordings/{id}"),
        (Method::GET, "/api/v1/recordings/{id}/file"),
        (Method::GET, "/api/v1/devices/{dev}/vfos/{id}/replay"),
    ];
    let managing = [
        (Method::PATCH, "/api/v1/recordings/{id}"),
        (Method::DELETE, "/api/v1/recordings/{id}"),
        (Method::POST, "/api/v1/recordings/{id}/stop"),
        (Method::POST, "/api/v1/devices/{dev}/recordings"),
        (Method::POST, "/api/v1/devices/{dev}/vfos/{id}/replay/save"),
    ];
    for (method, path) in &reading {
        let access = find_rule(method, path).expect("rule").access;
        assert!(access.allows(&listener, false), "{path}");
        assert!(access.allows(&operator, false), "{path}");
        assert!(!access.allows(&guest, true), "guests never: {path}");
    }
    for (method, path) in &managing {
        let access = find_rule(method, path).expect("rule").access;
        assert!(!access.allows(&listener, true), "{path}");
        assert!(!access.allows(&guest, true), "{path}");
        assert!(access.allows(&operator, false), "{path}");
        assert!(access.allows(&admin, false), "{path}");
    }
    // Per-device rules: replay needs view of the device, everything that stores needs control.
    let need = |m: &Method, p: &str| find_device_rule(m, p).map(|r| r.need);
    assert_eq!(
        need(&Method::GET, "/api/v1/devices/{dev}/vfos/{id}/replay"),
        Some(Need::View)
    );
    assert_eq!(
        need(&Method::POST, "/api/v1/devices/{dev}/recordings"),
        Some(Need::Control)
    );
    assert_eq!(
        need(&Method::POST, "/api/v1/devices/{dev}/vfos/{id}/replay/save"),
        Some(Need::Control)
    );
}

#[test]
fn the_config_stays_inside_its_limits() {
    let cfg = Config::from_env();
    assert!(cfg.replay_seconds >= ring::MIN_SECONDS && cfg.replay_seconds <= ring::MAX_SECONDS);
}
