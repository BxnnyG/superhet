//! End-to-end test of the decoder history writer: the real `superhet-worker` binary against a
//! real PostgreSQL and NATS. Runs only when `DATABASE_URL` and `SUPERHET_TEST_NATS_URL` are set.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use superhet_bus::{rds::RdsMessage, subjects, tones::TonesMessage};
use superhet_db::{
    PgPool,
    decoder_events::{self, Event, Query},
};

struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn events(pool: &PgPool, kind: &str) -> Vec<Event> {
    let q = Query {
        kinds: vec![kind.to_owned()],
        limit: 100,
        ..Query::default()
    };
    decoder_events::list(pool, &q).await.expect("list")
}

async fn wait_for(pool: &PgPool, kind: &str, value: &str) -> Event {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(e) = events(pool, kind)
            .await
            .into_iter()
            .find(|e| e.data.value == value)
        {
            return e;
        }
        assert!(
            Instant::now() < deadline,
            "no {kind} event {value:?} within 10 s"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn records_dtmf_ctcss_and_rds_and_announces_them() {
    let (Ok(db_url), Ok(nats_url)) = (
        std::env::var("DATABASE_URL"),
        std::env::var("SUPERHET_TEST_NATS_URL"),
    ) else {
        eprintln!("DATABASE_URL / SUPERHET_TEST_NATS_URL not set; skipping");
        return;
    };
    let admin = superhet_db::connect(&db_url).await.expect("admin");
    let name = format!("sh_test_worker_{}", std::process::id());
    for sql in [
        format!("DROP DATABASE IF EXISTS {name}"),
        format!("CREATE DATABASE {name}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&admin)
            .await
            .expect("db setup");
    }
    let (base, _) = db_url.rsplit_once('/').expect("url with database");
    let url = format!("{base}/{name}");
    let pool = superhet_db::connect(&url).await.expect("connect");
    superhet_db::migrate(&pool).await.expect("migrate");

    let _worker = Proc(
        Command::new(env!("CARGO_BIN_EXE_superhet-worker"))
            .env("SUPERHET_DATABASE_URL", &url)
            .env("SUPERHET_NATS_URL", &nats_url)
            .env("SUPERHET_LISTEN", "127.0.0.1:0")
            .env("SUPERHET_EVENT_RETENTION_DAYS", "30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn worker"),
    );
    let nats = async_nats::connect(&nats_url).await.expect("nats");
    let mut announced = nats
        .subscribe(subjects::decoder_events("itest").expect("subject"))
        .await
        .expect("subscribe");
    nats.flush().await.expect("flush");

    // Readiness probe: keep sending the same RDS state until the worker has stored it.
    let probe = RdsMessage {
        synced: true,
        ps: Some("PROBE".into()),
        ..RdsMessage::empty(99)
    };
    let probe_subject = subjects::rds("itest", "99").expect("subject");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        nats.publish(probe_subject.clone(), probe.encode().into())
            .await
            .expect("publish");
        nats.flush().await.expect("flush");
        if events(&pool, "rds_ps")
            .await
            .iter()
            .any(|e| e.data.value == "PROBE")
        {
            break;
        }
        assert!(Instant::now() < deadline, "worker did not come up");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    // Tuning first, so the events carry the frequency.
    let state = serde_json::json!({
        "v": 1,
        "vfos": [{"id": 1, "freq_hz": 433_500_000, "mode": "nfm"},
                 {"id": 2, "freq_hz": 99_200_000, "mode": "wfm"}],
    });
    nats.publish(
        subjects::state("itest").expect("subject"),
        state.to_string().into(),
    )
    .await
    .expect("publish");
    nats.flush().await.expect("flush");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let tones = |count: u64, tail: &str, ctcss: Option<f32>| TonesMessage {
        ctcss_hz: ctcss,
        dtmf_count: count,
        dtmf_tail: tail.into(),
        ..TonesMessage::empty(1)
    };
    let tones_subject = subjects::tones("itest", "1").expect("subject");
    for msg in [tones(1, "1", Some(88.5)), tones(2, "12", Some(88.5))] {
        nats.publish(tones_subject.clone(), msg.encode().into())
            .await
            .expect("publish");
        nats.flush().await.expect("flush");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let rds = RdsMessage {
        synced: true,
        pi: Some("D220".into()),
        ps: Some("RADIO X ".into()),
        rt: Some("Hello".into()),
        ..RdsMessage::empty(2)
    };
    nats.publish(
        subjects::rds("itest", "2").expect("subject"),
        rds.encode().into(),
    )
    .await
    .expect("publish");
    nats.flush().await.expect("flush");

    let dtmf = wait_for(&pool, "dtmf", "12").await;
    assert_eq!(dtmf.data.freq_hz, Some(433_500_000));
    assert_eq!(dtmf.data.mode.as_deref(), Some("nfm"));
    assert_eq!(dtmf.data.vfo, Some(1));
    let ctcss = wait_for(&pool, "ctcss", "88.5 Hz").await;
    assert_eq!(ctcss.data.meta["edge"], "start");
    let ps = wait_for(&pool, "rds_ps", "RADIO X").await;
    assert_eq!(ps.data.freq_hz, Some(99_200_000));
    assert_eq!(ps.data.meta["pi"], "D220");
    wait_for(&pool, "rds_rt", "Hello").await;
    wait_for(&pool, "rds_pi", "D220").await;

    // Every stored event was announced for live clients.
    let mut seen = Vec::new();
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(700), announced.next()).await
    {
        let v: serde_json::Value = serde_json::from_slice(&msg.payload).expect("json");
        seen.push(v["kind"].as_str().unwrap_or("").to_owned());
    }
    for kind in ["dtmf", "ctcss", "rds_ps", "rds_rt", "rds_pi"] {
        assert!(
            seen.iter().any(|k| k == kind),
            "{kind} not announced: {seen:?}"
        );
    }

    drop(_worker);
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE {name} WITH (FORCE)"
    )))
    .execute(&admin)
    .await
    .expect("drop db");
}
