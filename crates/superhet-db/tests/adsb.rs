//! ADS-B aircraft database and range statistics against a real PostgreSQL; runs only when
//! `DATABASE_URL` is set, in a throwaway database.

mod common;

use common::TestDb;
use superhet_db::adsb::{
    self, Activity, AircraftRow, Dataset, OperatorRow, RangeCell, RangeUpdate, Source, TypeRow,
};

fn dataset() -> Dataset {
    Dataset {
        aircraft: vec![
            AircraftRow {
                icao: 0x3c6444,
                registration: "D-AIXA".into(),
                type_code: "A359".into(),
                flags: 0,
            },
            AircraftRow {
                icao: 0xa835af,
                registration: "N628TS".into(),
                type_code: "GLF6".into(),
                flags: 2,
            },
            AircraftRow {
                icao: 0x00_0001,
                registration: String::new(),
                type_code: "ZZZZ".into(),
                flags: 1,
            },
        ],
        types: vec![
            TypeRow {
                code: "A359".into(),
                description: "AIRBUS A-350-900".into(),
                class: "L2J".into(),
                wake: "H".into(),
            },
            TypeRow {
                code: "GLF6".into(),
                description: "GULFSTREAM G650".into(),
                class: "L2J".into(),
                wake: "M".into(),
            },
        ],
        operators: vec![OperatorRow {
            code: "DLH".into(),
            name: "Lufthansa".into(),
            country: "Germany".into(),
            callsign: "LUFTHANSA".into(),
        }],
    }
}

fn source(version: &str) -> Source {
    Source {
        source: "Mictronics aircraft database".into(),
        source_url: "https://example.invalid/db.zip".into(),
        licence: "ODC-By 1.0".into(),
        licence_url: "https://opendatacommons.org/licenses/by/1-0/".into(),
        version: version.into(),
        origin: "file".into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn import_replaces_the_database_and_lookup_joins_the_type() {
    let Some(db) = TestDb::create("adsbdb", true).await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    assert!(adsb::database_info(&db.pool).await.expect("info").is_none());
    assert!(
        adsb::lookup_aircraft(&db.pool, &[0x3c6444])
            .await
            .expect("lookup")
            .is_empty()
    );

    let info = adsb::replace_database(&db.pool, &dataset(), &source("530"))
        .await
        .expect("import");
    assert_eq!((info.aircraft, info.types, info.operators), (3, 2, 1));
    assert_eq!(info.source.version, "530");
    assert_eq!(info.source.licence, "ODC-By 1.0");
    assert!(info.imported_ms > 1_700_000_000_000);

    let found = adsb::lookup_aircraft(&db.pool, &[0xa835af, 0x3c6444, 0x123456, 1])
        .await
        .expect("lookup");
    let icaos: Vec<u32> = found.iter().map(|f| f.icao).collect();
    assert_eq!(icaos, [1, 0x3c6444, 0xa835af]);
    let airbus = &found[1];
    assert_eq!(airbus.registration, "D-AIXA");
    assert_eq!(airbus.type_description, "AIRBUS A-350-900");
    assert_eq!((airbus.class.as_str(), airbus.wake.as_str()), ("L2J", "H"));
    // A type the database has no description for still returns its code.
    assert_eq!(found[0].type_code, "ZZZZ");
    assert_eq!(found[0].type_description, "");
    assert_eq!(found[0].flags, 1);

    let ops = adsb::lookup_operators(&db.pool, &["DLH".into(), "XXX".into()])
        .await
        .expect("operators");
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].name, "Lufthansa");

    // A second import replaces everything.
    let mut next = dataset();
    next.aircraft.truncate(1);
    next.aircraft[0].registration = "D-AIXB".into();
    next.operators.clear();
    let info = adsb::replace_database(&db.pool, &next, &source("531"))
        .await
        .expect("reimport");
    assert_eq!((info.aircraft, info.operators), (1, 0));
    assert_eq!(info.source.version, "531");
    let found = adsb::lookup_aircraft(&db.pool, &[0x3c6444, 0xa835af])
        .await
        .expect("lookup");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].registration, "D-AIXB");

    adsb::clear_database(&db.pool).await.expect("clear");
    assert!(adsb::database_info(&db.pool).await.expect("info").is_none());
    assert!(
        adsb::lookup_aircraft(&db.pool, &[0x3c6444])
            .await
            .expect("lookup")
            .is_empty()
    );
    db.drop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_import_is_chunked() {
    let Some(db) = TestDb::create("adsbbig", true).await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let mut data = Dataset::default();
    for i in 0..12_345u32 {
        data.aircraft.push(AircraftRow {
            icao: i * 7,
            registration: format!("X-{i}"),
            type_code: String::new(),
            flags: 0,
        });
    }
    // Duplicates inside one statement must not fail the import.
    data.aircraft.push(data.aircraft[3].clone());
    let info = adsb::replace_database(&db.pool, &data, &source("1"))
        .await
        .expect("import");
    assert_eq!(info.aircraft, 12_345);
    db.drop().await;
}

fn cell(sector: i16, band: i16, km: f32, icao: &str) -> RangeCell {
    RangeCell {
        sector,
        band,
        max_km: km,
        icao: icao.into(),
    }
}

fn update(ts_ms: i64, receiver: Option<(f64, f64)>, cells: Vec<RangeCell>) -> RangeUpdate {
    RangeUpdate {
        ts_ms,
        receiver,
        cells,
        activity: Activity {
            msgs_per_s: 120.0,
            aircraft: 14,
            with_position: 9,
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn range_cells_keep_the_maximum_and_events_are_idempotent() {
    let Some(db) = TestDb::create("adsbrange", true).await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let rx = Some((51.0, 7.0));
    let first = update(
        60_000,
        rx,
        vec![cell(3, 2, 120.5, "3c6444"), cell(10, 4, 200.0, "a835af")],
    );
    let recorded = adsb::record_range(&db.pool, "dev0", &first)
        .await
        .expect("record");
    assert!(!recorded.restarted);
    // Again (a redelivery) and a smaller value for the same cell change nothing; a larger one wins.
    adsb::record_range(&db.pool, "dev0", &first)
        .await
        .expect("again");
    adsb::record_range(
        &db.pool,
        "dev0",
        &update(120_000, rx, vec![cell(3, 2, 80.0, "aaaaaa")]),
    )
    .await
    .expect("smaller");
    adsb::record_range(
        &db.pool,
        "dev0",
        &update(180_000, rx, vec![cell(3, 2, 150.25, "bbbbbb")]),
    )
    .await
    .expect("larger");
    // Another device has its own statistics.
    adsb::record_range(
        &db.pool,
        "dev1",
        &update(60_000, Some((10.0, 10.0)), vec![cell(0, 0, 5.0, "")]),
    )
    .await
    .expect("dev1");

    let summary = adsb::range_summary(&db.pool, "dev0")
        .await
        .expect("summary");
    assert_eq!(summary.receiver, rx);
    assert!(summary.since_ms.is_some());
    assert_eq!(
        summary.cells,
        vec![cell(3, 2, 150.25, "bbbbbb"), cell(10, 4, 200.0, "a835af")]
    );
    assert_eq!(
        adsb::range_summary(&db.pool, "dev1")
            .await
            .expect("dev1")
            .cells
            .len(),
        1
    );

    // One row per minute, the repeated minute counted once.
    let points = adsb::activity(&db.pool, "dev0", 0, 60_000)
        .await
        .expect("activity");
    assert_eq!(points.len(), 3);
    assert_eq!(points[0].aircraft, 14);
    assert_eq!(points[0].with_position, 9);
    assert!((points[0].msgs_per_s - 120.0).abs() < f32::EPSILON);
    // Bucketed into five minutes: one point.
    let coarse = adsb::activity(&db.pool, "dev0", 0, 300_000)
        .await
        .expect("coarse");
    assert_eq!(coarse.len(), 1);
    // Since filters.
    assert_eq!(
        adsb::activity(&db.pool, "dev0", 150_000, 60_000)
            .await
            .expect("since")
            .len(),
        1
    );
    db.drop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_the_receiver_starts_over_and_reset_forgets_everything() {
    let Some(db) = TestDb::create("adsbmove", true).await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    adsb::record_range(
        &db.pool,
        "dev0",
        &update(60_000, Some((51.0, 7.0)), vec![cell(3, 2, 120.0, "x")]),
    )
    .await
    .expect("record");
    // A few hundred metres: same place.
    let r = adsb::record_range(
        &db.pool,
        "dev0",
        &update(120_000, Some((51.002, 7.001)), vec![cell(4, 2, 90.0, "y")]),
    )
    .await
    .expect("near");
    assert!(!r.restarted);
    assert_eq!(
        adsb::range_summary(&db.pool, "dev0")
            .await
            .expect("s")
            .cells
            .len(),
        2
    );
    // 100 km away: new statistics.
    let r = adsb::record_range(
        &db.pool,
        "dev0",
        &update(180_000, Some((52.0, 8.0)), vec![cell(7, 1, 30.0, "z")]),
    )
    .await
    .expect("far");
    assert!(r.restarted);
    let summary = adsb::range_summary(&db.pool, "dev0").await.expect("s");
    assert_eq!(summary.receiver, Some((52.0, 8.0)));
    assert_eq!(summary.cells, vec![cell(7, 1, 30.0, "z")]);
    // No position in an event: the activity is stored, the position stays.
    adsb::record_range(&db.pool, "dev0", &update(240_000, None, vec![]))
        .await
        .expect("no position");
    assert_eq!(
        adsb::range_summary(&db.pool, "dev0")
            .await
            .expect("s")
            .receiver,
        Some((52.0, 8.0))
    );

    assert_eq!(
        adsb::prune_activity(&db.pool, 150_000)
            .await
            .expect("prune"),
        2
    );
    let removed = adsb::reset_range(&db.pool, "dev0").await.expect("reset");
    assert_eq!(removed, 1);
    let summary = adsb::range_summary(&db.pool, "dev0").await.expect("s");
    assert!(summary.cells.is_empty() && summary.receiver.is_none());
    assert!(
        adsb::activity(&db.pool, "dev0", 0, 60_000)
            .await
            .expect("a")
            .is_empty()
    );
    db.drop().await;
}
