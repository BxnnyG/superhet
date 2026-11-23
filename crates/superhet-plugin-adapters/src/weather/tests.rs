use super::*;
use serde_json::json;

#[test]
fn config_defaults_and_pipelines() {
    assert_eq!(Config::from_json(&Value::Null), Ok(Config::default()));
    assert_eq!(
        Config::from_json(&json!({})).map(|c| c.pipeline),
        Ok(Pipeline::Lrpt72)
    );
    let c = Config::from_json(&json!({"symbol_rate": "80k", "satellite": " METEOR-M2 4 ",
        "threshold_db": 4.5, "min_seconds": 90, "max_minutes": 15, "arm_seconds": 3, "hang_seconds": 20}))
    .expect("config");
    assert_eq!(c.pipeline, Pipeline::Lrpt80);
    assert_eq!(c.satellite.as_deref(), Some("METEOR-M2 4"));
    assert!((c.threshold_db - 4.5).abs() < f64::EPSILON);
    assert_eq!(c.params().arm_blocks, 12);
    assert_eq!(c.params().hang_blocks, 80);
    // APT wins over a symbol rate (which only LRPT has).
    let apt = Config::from_json(&json!({"pipeline": "apt", "symbol_rate": "80k"})).expect("apt");
    assert_eq!(apt.pipeline, Pipeline::Apt);
    assert_eq!(apt.pipeline.satdump_name(), "noaa_apt");
    assert_eq!(apt.pipeline.kind(), "apt");
}

#[test]
fn config_rejects_bad_values_naming_the_key() {
    for (bad, key) in [
        (json!({"pipeline": "hrpt"}), "pipeline"),
        (json!({"symbol_rate": "72"}), "symbol_rate"),
        (json!({"satellite": ""}), "satellite"),
        (json!({"threshold_db": 0.1}), "threshold_db"),
        (json!({"min_seconds": 1}), "min_seconds"),
        (json!({"max_minutes": 100}), "max_minutes"),
        (json!({"arm_seconds": "x"}), "arm_seconds"),
        (json!({"hang_seconds": 0}), "hang_seconds"),
        (json!({"bogus": 1}), "bogus"),
        (json!(5), "object"),
    ] {
        let err = Config::from_json(&bad).expect_err("must fail");
        assert!(err.contains(key), "{err}");
    }
}

#[test]
fn satdump_command_line() {
    let args = satdump_args(
        Pipeline::Lrpt72,
        Path::new("/w/in.cs16"),
        Path::new("/w/out"),
        192_000,
        "cs16",
    );
    assert_eq!(
        args,
        [
            "meteor_m2-x_lrpt",
            "baseband",
            "/w/in.cs16",
            "/w/out",
            "--samplerate",
            "192000",
            "--baseband_format",
            "cs16"
        ]
    );
}

#[test]
fn the_tool_is_found_by_path_directory_or_name() {
    assert_eq!(locate(None), PathBuf::from("satdump"));
    assert_eq!(locate(Some("  ")), PathBuf::from("satdump"));
    assert_eq!(
        locate(Some("/opt/x/satdump")),
        PathBuf::from("/opt/x/satdump")
    );
    let dir = std::env::temp_dir();
    assert_eq!(locate(dir.to_str()), dir.join("satdump"));
}

#[test]
fn segment_sidecars_round_trip() {
    let meta = SegmentMeta {
        instance: "i-1".into(),
        device: Some("dev0".into()),
        started_ms: 1_795_436_348_000,
        ended_ms: None,
        peak_db: Some(6.5),
        frequency_hz: Some(137_900_000),
        pipeline: Pipeline::Lrpt80,
        satellite: None,
        sample_rate: 192_000,
        format: "cs16".into(),
    };
    assert_eq!(SegmentMeta::from_json(&meta.to_json()), Some(meta));
    assert_eq!(SegmentMeta::from_json(&json!({"pipeline": "other"})), None);
}

#[test]
fn pass_ids_are_directory_safe() {
    let id = pass_id(1_795_436_348_123);
    let (secs, hex) = id.split_once('-').expect("dash");
    assert_eq!(secs, "1795436348");
    assert_eq!(hex.len(), 4);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
}
