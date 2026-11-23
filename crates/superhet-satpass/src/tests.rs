use super::*;

// Meteor-M2 4, fetched from CelesTrak on 2026-11-23.
const L0: &str = "METEOR-M2 4";
const L1: &str = "1 59051U 24039A   26327.50722175 -.00000013  00000+0  13685-4 0  9998";
const L2: &str = "2 59051  98.7256 284.9730 0008115  71.3466 288.8592 14.22444296141910";

const FROM_MS: i64 = 1_795_435_200_000; // 2026-11-23T12:00:00Z

/// Rise, highest point, set (Unix ms) and the maximum elevation of the passes of the next 24 h
/// over 51.0 N, 7.0 E, as Skyfield (an independent SGP4 and frame implementation) finds them for
/// the same elements.
const SKYFIELD: [(i64, i64, i64, f64); 8] = [
    (
        1_795_436_348_415,
        1_795_436_770_963,
        1_795_437_194_922,
        27.74,
    ),
    (
        1_795_442_299_539,
        1_795_442_754_445,
        1_795_443_213_457,
        56.37,
    ),
    (
        1_795_448_499_658,
        1_795_448_826_897,
        1_795_449_156_812,
        8.75,
    ),
    (
        1_795_480_277_141,
        1_795_480_537_785,
        1_795_480_797_262,
        4.65,
    ),
    (
        1_795_486_180_773,
        1_795_486_630_437,
        1_795_487_077_073,
        38.95,
    ),
    (
        1_795_492_190_009,
        1_795_492_631_622,
        1_795_493_072_101,
        38.11,
    ),
    (
        1_795_498_238_235,
        1_795_498_549_738,
        1_795_498_861_279,
        8.40,
    ),
    (
        1_795_515_789_950,
        1_795_516_003_356,
        1_795_516_216_830,
        3.29,
    ),
];

fn sat() -> Satellite {
    let sets = parse_elements(&format!("{L0}\n{L1}\n{L2}\n")).expect("parse");
    Satellite::from_elements(&sets[0]).expect("elements")
}

fn observer() -> Observer {
    Observer::new(51.0, 7.0).expect("observer")
}

#[test]
fn passes_match_an_independent_implementation() {
    let found = passes(&sat(), observer(), FROM_MS, FROM_MS + 86_400_000, 0.0);
    // The ninth pass rises at the end of the window and is reported with its true set.
    assert_eq!(found.len(), SKYFIELD.len() + 1, "{found:#?}");
    assert!(found[SKYFIELD.len()].los_ms > FROM_MS + 86_400_000);
    for (pass, (aos, tca, los, max)) in found.iter().zip(SKYFIELD) {
        // Different Earth rotation models and refinement steps: a few seconds are fine.
        assert!((pass.aos_ms - aos).abs() < 6_000, "aos {pass:?}");
        assert!((pass.tca_ms - tca).abs() < 6_000, "tca {pass:?}");
        assert!((pass.los_ms - los).abs() < 6_000, "los {pass:?}");
        assert!((pass.max_elevation_deg - max).abs() < 0.3, "max {pass:?}");
        assert!(!pass.in_progress);
    }
}

#[test]
fn the_minimum_elevation_filters_low_passes() {
    let found = passes(&sat(), observer(), FROM_MS, FROM_MS + 86_400_000, 10.0);
    assert_eq!(found.len(), 5, "{found:#?}");
    assert!(found.iter().all(|p| p.max_elevation_deg >= 10.0));
}

#[test]
fn a_pass_under_way_is_clipped_to_the_window() {
    // 2 minutes into the first pass, window of 1 minute.
    let from = SKYFIELD[0].0 + 120_000;
    let found = passes(&sat(), observer(), from, from + 60_000, 0.0);
    assert_eq!(found.len(), 1);
    assert!(found[0].in_progress);
    // The true times of the pass, not the edges of the window.
    assert!(
        (found[0].aos_ms - SKYFIELD[0].0).abs() < 6_000,
        "{:?}",
        found[0]
    );
    assert!(
        (found[0].los_ms - SKYFIELD[0].2).abs() < 6_000,
        "{:?}",
        found[0]
    );
    assert!((found[0].max_elevation_deg - SKYFIELD[0].3).abs() < 0.3);
    // A window that ends before the satellite rises finds nothing.
    assert!(passes(&sat(), observer(), FROM_MS, FROM_MS + 60_000, 0.0).is_empty());
    assert!(passes(&sat(), observer(), FROM_MS, FROM_MS, 0.0).is_empty());
}

#[test]
fn directly_overhead_is_ninety_degrees() {
    let s = sat();
    let (lat, lon, height) = s.subpoint(FROM_MS).expect("subpoint");
    // Sun-synchronous orbit at about 820 km.
    assert!((780.0..860.0).contains(&height), "{height}");
    let obs = Observer::new(lat, lon).expect("observer");
    let look = look_angle(&s, obs, FROM_MS).expect("look");
    assert!(look.elevation_deg > 89.9, "{look:?}");
    assert!((look.range_km - height).abs() < 1.0, "{look:?}");
}

#[test]
fn the_far_side_of_the_earth_is_below_the_horizon() {
    let s = sat();
    let (lat, lon, _) = s.subpoint(FROM_MS).expect("subpoint");
    let antipode =
        Observer::new(-lat, if lon > 0.0 { lon - 180.0 } else { lon + 180.0 }).expect("observer");
    let look = look_angle(&s, antipode, FROM_MS).expect("look");
    assert!(look.elevation_deg < -60.0, "{look:?}");
}

#[test]
fn azimuth_is_a_compass_bearing() {
    let found = passes(&sat(), observer(), FROM_MS, FROM_MS + 86_400_000, 0.0);
    // The first pass of the reference rises towards north-east (see SKYFIELD: azimuth at the
    // highest point 59 degrees).
    let p = &found[0];
    assert!((p.tca_azimuth_deg - 59.3).abs() < 2.0, "{p:?}");
    for p in &found {
        for az in [p.aos_azimuth_deg, p.tca_azimuth_deg, p.los_azimuth_deg] {
            assert!((0.0..360.0).contains(&az));
        }
    }
}

#[test]
fn observer_must_be_on_earth() {
    assert!(Observer::new(91.0, 0.0).is_err());
    assert!(Observer::new(0.0, 181.0).is_err());
    assert!(Observer::new(f64::NAN, 0.0).is_err());
    assert!(Observer::new(-90.0, -180.0).is_ok());
}

#[test]
fn parses_two_and_three_line_sets_and_omm_json() {
    let two = parse_elements(&format!("{L1}\n{L2}\n")).expect("two lines");
    assert_eq!(two.len(), 1);
    assert_eq!(two[0].norad_id, 59051);
    let three =
        parse_elements(&format!("{L0}  \n{L1}\n{L2}\n\n{L0}\n{L1}\n{L2}\n")).expect("three");
    assert_eq!(three.len(), 2);
    assert_eq!(three[0].object_name.as_deref(), Some(L0));

    let omm = r#"[{"OBJECT_NAME":"METEOR-M2 3","OBJECT_ID":"2023-091A","EPOCH":"2026-11-23T12:33:08.382816",
        "MEAN_MOTION":14.24057105,"ECCENTRICITY":0.0004988,"INCLINATION":98.5884,
        "RA_OF_ASC_NODE":19.0681,"ARG_OF_PERICENTER":73.2184,"MEAN_ANOMALY":286.9542,
        "EPHEMERIS_TYPE":0,"CLASSIFICATION_TYPE":"U","NORAD_CAT_ID":57166,"ELEMENT_SET_NO":999,
        "REV_AT_EPOCH":17719,"BSTAR":1.5745e-5,"MEAN_MOTION_DOT":-7.0e-8,"MEAN_MOTION_DDOT":0}]"#;
    let sets = parse_elements(omm).expect("omm");
    assert_eq!(sets[0].norad_id, 57166);
    let s = Satellite::from_elements(&sets[0]).expect("elements");
    assert_eq!(s.name, "METEOR-M2 3");
    assert!(!passes(&s, observer(), FROM_MS, FROM_MS + 86_400_000, 10.0).is_empty());
    // A single object instead of an array is fine too.
    let one = omm.trim().trim_start_matches('[').trim_end_matches(']');
    assert_eq!(parse_elements(one).expect("object").len(), 1);
}

#[test]
fn bad_input_is_refused_with_a_reason() {
    for bad in [
        "",
        "   \n",
        "hello",
        "1 59051U\n2 59051",
        &format!("{L1}\n"),
        "[1, 2]",
        "{\"OBJECT_NAME\": 1}",
        "[",
    ] {
        assert!(parse_elements(bad).is_err(), "{bad:?}");
    }
    // A corrupted checksum digit is caught by the TLE parser.
    let corrupt = L1.replace("9998", "9999");
    assert!(parse_elements(&format!("{corrupt}\n{L2}\n")).is_err());
}
