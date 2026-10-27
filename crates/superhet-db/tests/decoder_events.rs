//! Decoder event queries against a real PostgreSQL; runs only when `DATABASE_URL` is set, in a
//! throwaway database.

use serde_json::json;
use superhet_db::{
    PgPool,
    decoder_events::{self, NewEvent, Query},
    plugin_instances,
};

struct TestDb {
    admin: PgPool,
    name: String,
    pool: PgPool,
}

impl TestDb {
    async fn create(tag: &str) -> Option<Self> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let admin = superhet_db::connect(&url).await.expect("connect admin");
        let name = format!("sh_test_{tag}_{}", std::process::id());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {name}"
        )))
        .execute(&admin)
        .await
        .expect("drop stale");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("create db");
        let (base, _) = url.rsplit_once('/').expect("url with database");
        let pool = superhet_db::connect(&format!("{base}/{name}"))
            .await
            .expect("connect test db");
        superhet_db::migrate(&pool).await.expect("migrate");
        Some(Self { admin, name, pool })
    }

    async fn drop(self) {
        self.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {} WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin)
        .await
        .expect("drop db");
    }
}

fn ev(ts_ms: i64, vfo: i32, kind: &str, value: &str) -> NewEvent {
    NewEvent {
        ts_ms,
        device: "rtl0".into(),
        vfo: Some(vfo),
        freq_hz: Some(100_000_000 + i64::from(vfo)),
        mode: Some("nfm".into()),
        kind: kind.into(),
        source: "core".into(),
        value: value.into(),
        meta: json!({"k": 1}),
    }
}

#[tokio::test]
async fn insert_list_filter_page_prune_clear() {
    let Some(db) = TestDb::create("devents").await else {
        eprintln!("DATABASE_URL not set; skipping database integration test");
        return;
    };
    let pool = &db.pool;
    let base = 1_700_000_000_000_i64;
    let first = decoder_events::insert(pool, &ev(base + 123, 1, "dtmf", "123#"))
        .await
        .expect("insert");
    assert_eq!(first.data.ts_ms, base + 123, "millisecond round trip");
    assert_eq!(first.data.meta, json!({"k": 1}));
    decoder_events::insert(pool, &ev(base + 1_000, 2, "ctcss", "88.5 Hz"))
        .await
        .expect("insert");
    decoder_events::insert(pool, &ev(base + 2_000, 1, "rds_ps", "RADIO X"))
        .await
        .expect("insert");
    decoder_events::insert(pool, &ev(base + 3_000, 1, "dtmf", "*0"))
        .await
        .expect("insert");

    let q = Query {
        limit: 100,
        ..Query::default()
    };
    let all = decoder_events::list(pool, &q).await.expect("list");
    let values: Vec<&str> = all.iter().map(|e| e.data.value.as_str()).collect();
    assert_eq!(values, ["*0", "RADIO X", "88.5 Hz", "123#"]);

    let dtmf = Query {
        kinds: vec!["dtmf".into()],
        ..q.clone()
    };
    assert_eq!(decoder_events::list(pool, &dtmf).await.expect("l").len(), 2);
    let vfo2 = Query {
        vfo: Some(2),
        ..q.clone()
    };
    assert_eq!(decoder_events::list(pool, &vfo2).await.expect("l").len(), 1);
    let range = Query {
        from_ms: Some(base + 1_000),
        to_ms: Some(base + 3_000),
        ..q.clone()
    };
    assert_eq!(
        decoder_events::list(pool, &range).await.expect("l").len(),
        2
    );
    let search = Query {
        search: Some("radio".into()),
        ..q.clone()
    };
    assert_eq!(
        decoder_events::list(pool, &search).await.expect("l").len(),
        1
    );
    let search = Query {
        search: Some("%".into()),
        ..q.clone()
    };
    assert!(
        decoder_events::list(pool, &search)
            .await
            .expect("l")
            .is_empty()
    );

    // Cursor paging walks the same order without gaps or repeats.
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = decoder_events::list(
            pool,
            &Query {
                limit: 3,
                after,
                ..Query::default()
            },
        )
        .await
        .expect("page");
        let Some(last) = page.last() else { break };
        after = Some((last.data.ts_ms, last.id));
        seen.extend(page.iter().map(|e| e.id));
    }
    assert_eq!(seen, all.iter().map(|e| e.id).collect::<Vec<_>>());

    // Prune drops what is older than the cutoff; clear drops the rest.
    assert_eq!(
        decoder_events::prune(pool, base + 1_500).await.expect("p"),
        2
    );
    assert_eq!(decoder_events::clear(pool).await.expect("clear"), 2);
    assert!(decoder_events::list(pool, &q).await.expect("l").is_empty());

    // The CHECK constraint rejects unknown core kinds.
    assert!(
        decoder_events::insert(pool, &ev(base, 1, "bogus", "x"))
            .await
            .is_err()
    );
    db.drop().await;
}

#[tokio::test]
async fn plugin_events_and_instances() {
    let Some(db) = TestDb::create("plugins").await else {
        eprintln!("DATABASE_URL not set; skipping database integration test");
        return;
    };
    let pool = &db.pool;
    let base = 1_700_000_000_000_i64;
    let plugin = NewEvent {
        vfo: None,
        kind: "pocsag.message".into(),
        source: "org.superhet.decoder.pocsag".into(),
        ..ev(base, 1, "dtmf", "ALPHA 1234")
    };
    let stored = decoder_events::insert(pool, &plugin).await.expect("plugin");
    assert_eq!(stored.data.vfo, None);
    assert_eq!(stored.to_json()["source"], "org.superhet.decoder.pocsag");
    decoder_events::insert(pool, &ev(base + 1, 1, "dtmf", "5"))
        .await
        .expect("core");

    let q = Query {
        limit: 10,
        ..Query::default()
    };
    assert_eq!(decoder_events::list(pool, &q).await.expect("l").len(), 2);
    let only = Query {
        source: Some("org.superhet.decoder.pocsag".into()),
        ..q.clone()
    };
    let rows = decoder_events::list(pool, &only).await.expect("l");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].data.kind, "pocsag.message");
    let by_type = Query {
        kinds: vec!["pocsag.message".into()],
        ..q.clone()
    };
    assert_eq!(
        decoder_events::list(pool, &by_type).await.expect("l").len(),
        1
    );

    // Plugin kinds must look like event types; core events keep the fixed list.
    for (kind, source) in [("Bad Kind", "org.x.y"), ("pocsag.message", "core")] {
        let bad = NewEvent {
            kind: kind.into(),
            source: source.into(),
            ..plugin.clone()
        };
        assert!(
            decoder_events::insert(pool, &bad).await.is_err(),
            "{kind} from {source}"
        );
    }

    // Plugin instances: upsert replaces, list keeps creation order, delete reports.
    let record = |id: &str, vfo| plugin_instances::Record {
        id: id.into(),
        plugin_id: "org.superhet.decoder.pocsag".into(),
        device: Some("rtl0".into()),
        vfo,
        freq_hz: Some(153_350_000),
        config: json!({"baud": 1200}),
    };
    plugin_instances::upsert(pool, &record("i-1", Some(1)))
        .await
        .expect("1");
    plugin_instances::upsert(pool, &record("i-2", None))
        .await
        .expect("2");
    plugin_instances::upsert(pool, &record("i-1", Some(3)))
        .await
        .expect("again");
    let all = plugin_instances::list(pool).await.expect("list");
    assert_eq!(all, vec![record("i-1", Some(3)), record("i-2", None)]);
    assert!(plugin_instances::delete(pool, "i-1").await.expect("d"));
    assert!(!plugin_instances::delete(pool, "i-1").await.expect("d"));
    assert_eq!(plugin_instances::list(pool).await.expect("list").len(), 1);
    db.drop().await;
}
