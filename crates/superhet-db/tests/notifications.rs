//! Notification storage against a real PostgreSQL (`DATABASE_URL`).

mod common;

use common::TestDb;
use serde_json::json;
use superhet_db::notifications::{
    self as n, NewDelivery, Quiet, RuleInput, SecretChange, Subscribed, Updated, Vapid,
};
use superhet_db::users::{self, Created, Role};

macro_rules! db {
    ($tag:expr) => {{
        let Some(db) = TestDb::create($tag, true).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        db
    }};
}

fn rule_input(name: &str, targets: Vec<i64>) -> RuleInput {
    RuleInput {
        name: name.into(),
        enabled: true,
        source: Some("org.superhet.decoder.pocsag".into()),
        kind: None,
        conditions: json!([{"field": "value", "op": "contains", "value": "fire"}]),
        cooldown_secs: 60,
        max_per_hour: Some(10),
        quiet: Some(Quiet {
            start_min: 1320,
            end_min: 360,
            tz: "Europe/Berlin".into(),
        }),
        title_template: "{{rule}}".into(),
        body_template: String::new(),
        target_ids: targets,
    }
}

async fn user(pool: &superhet_db::PgPool, name: &str) -> i64 {
    match users::create(pool, name, "hash", Role::Listener, false)
        .await
        .expect("create")
    {
        Created::Id(id) => id,
        Created::UsernameTaken => panic!("taken"),
    }
}

fn delivery(rule: i64, target: i64, event: Option<i64>) -> NewDelivery {
    NewDelivery {
        rule_id: Some(rule),
        rule_name: "r".into(),
        target_id: target,
        target_name: "t".into(),
        event_id: event,
        message: json!({"title": "t", "body": "b"}),
    }
}

#[tokio::test]
async fn targets_keep_their_secret_apart() {
    let db = db!("notify_targets");
    let cfg = json!({"url": "https://example.com/hook"});
    let t = n::insert_target(&db.pool, "ops", "webhook", &cfg, Some("s3cret"))
        .await
        .expect("insert")
        .expect("created");
    assert_eq!(t.secret.as_deref(), Some("s3cret"));
    assert!(
        !format!("{t:?}").contains("s3cret"),
        "Debug must hide the secret"
    );
    assert!(
        n::insert_target(&db.pool, "ops", "email", &json!({}), None)
            .await
            .expect("insert")
            .is_none(),
        "names are unique"
    );
    // Keep leaves it, Set replaces it, Clear removes it.
    let kept = n::update_target(&db.pool, t.id, "ops2", &cfg, &SecretChange::Keep)
        .await
        .expect("update");
    assert!(
        matches!(&kept, Updated::Ok(k) if k.secret.as_deref() == Some("s3cret") && k.name == "ops2")
    );
    let set = n::update_target(
        &db.pool,
        t.id,
        "ops2",
        &cfg,
        &SecretChange::Set("new".into()),
    )
    .await
    .expect("update");
    assert!(matches!(&set, Updated::Ok(k) if k.secret.as_deref() == Some("new")));
    let cleared = n::update_target(&db.pool, t.id, "ops2", &cfg, &SecretChange::Clear)
        .await
        .expect("update");
    assert!(matches!(&cleared, Updated::Ok(k) if k.secret.is_none()));
    assert_eq!(
        n::update_target(&db.pool, 9999, "x", &cfg, &SecretChange::Keep)
            .await
            .expect("update"),
        Updated::Missing
    );
    db.drop().await;
}

#[tokio::test]
async fn rules_round_trip_with_their_targets_and_quiet_hours() {
    let db = db!("notify_rules");
    let a = n::insert_target(&db.pool, "a", "push", &json!({}), None)
        .await
        .expect("t")
        .expect("created");
    let b = n::insert_target(&db.pool, "b", "push", &json!({}), None)
        .await
        .expect("t")
        .expect("created");
    let rule = n::insert_rule(&db.pool, &rule_input("fire", vec![b.id, a.id, 4242]))
        .await
        .expect("insert")
        .expect("created");
    assert_eq!(
        rule.data.target_ids,
        [a.id, b.id],
        "unknown targets are ignored"
    );
    assert_eq!(
        rule.data.quiet.as_ref().map(|q| q.tz.as_str()),
        Some("Europe/Berlin")
    );
    assert_eq!(rule.data.conditions[0]["op"], "contains");
    assert!(
        n::insert_rule(&db.pool, &rule_input("fire", vec![]))
            .await
            .expect("insert")
            .is_none()
    );

    let mut changed = rule_input("fire", vec![a.id]);
    changed.quiet = None;
    changed.enabled = false;
    let updated = n::update_rule(&db.pool, rule.id, &changed)
        .await
        .expect("update");
    let Updated::Ok(updated) = updated else {
        panic!("{updated:?}")
    };
    assert_eq!(updated.data.target_ids, [a.id]);
    assert!(updated.data.quiet.is_none() && !updated.data.enabled);
    assert!(
        n::list_rules(&db.pool, true)
            .await
            .expect("list")
            .is_empty()
    );
    assert_eq!(n::list_rules(&db.pool, false).await.expect("list").len(), 1);

    // A target deleted under a rule just disappears from it.
    n::delete_target(&db.pool, a.id).await.expect("delete");
    assert!(
        n::get_rule(&db.pool, rule.id)
            .await
            .expect("get")
            .expect("rule")
            .data
            .target_ids
            .is_empty()
    );

    // The check constraint refuses half-specified quiet hours.
    let mut bad = rule_input("bad", vec![]);
    bad.quiet = Some(Quiet {
        start_min: 2000,
        end_min: 1,
        tz: "UTC".into(),
    });
    assert!(n::insert_rule(&db.pool, &bad).await.is_err());

    let offsets = n::zone_offsets(&db.pool, &["UTC".into(), "Nowhere/Land".into()])
        .await
        .expect("zones");
    assert_eq!(offsets.get("UTC"), Some(&0));
    assert!(!offsets.contains_key("Nowhere/Land"));
    db.drop().await;
}

#[tokio::test]
async fn an_event_is_queued_once_per_rule_and_target() {
    let db = db!("notify_dedupe");
    let t = n::insert_target(&db.pool, "t", "push", &json!({}), None)
        .await
        .expect("t")
        .expect("c");
    let u = n::insert_target(&db.pool, "u", "push", &json!({}), None)
        .await
        .expect("t")
        .expect("c");
    let r = n::insert_rule(&db.pool, &rule_input("r", vec![t.id, u.id]))
        .await
        .expect("r")
        .expect("c");
    assert!(
        n::enqueue(&db.pool, &delivery(r.id, t.id, Some(100)))
            .await
            .expect("q")
            .is_some()
    );
    // The same event again (a redelivered message) queues nothing.
    assert!(
        n::enqueue(&db.pool, &delivery(r.id, t.id, Some(100)))
            .await
            .expect("q")
            .is_none()
    );
    // Another target, another event, and test sends without an event id are separate.
    assert!(
        n::enqueue(&db.pool, &delivery(r.id, u.id, Some(100)))
            .await
            .expect("q")
            .is_some()
    );
    assert!(
        n::enqueue(&db.pool, &delivery(r.id, t.id, Some(101)))
            .await
            .expect("q")
            .is_some()
    );
    assert!(
        n::enqueue(&db.pool, &delivery(r.id, t.id, None))
            .await
            .expect("q")
            .is_some()
    );
    assert!(
        n::enqueue(&db.pool, &delivery(r.id, t.id, None))
            .await
            .expect("q")
            .is_some()
    );
    assert_eq!(
        n::list_deliveries(&db.pool, 50, None, None, None)
            .await
            .expect("l")
            .len(),
        5
    );
    db.drop().await;
}

#[tokio::test]
async fn deliveries_are_claimed_leased_retried_and_finished() {
    let db = db!("notify_queue");
    let t = n::insert_target(&db.pool, "t", "push", &json!({}), None)
        .await
        .expect("t")
        .expect("c");
    let r = n::insert_rule(&db.pool, &rule_input("r", vec![t.id]))
        .await
        .expect("r")
        .expect("c");
    for e in 1..=3 {
        n::enqueue(&db.pool, &delivery(r.id, t.id, Some(e)))
            .await
            .expect("q");
    }
    let first = n::claim_due(&db.pool, 2, 120).await.expect("claim");
    assert_eq!(first.len(), 2);
    // Leased rows are not handed out again, the third is.
    let second = n::claim_due(&db.pool, 5, 120).await.expect("claim");
    assert_eq!(second.len(), 1);
    assert!(
        n::claim_due(&db.pool, 5, 120)
            .await
            .expect("claim")
            .is_empty()
    );

    n::mark_sent(&db.pool, first[0].id, None)
        .await
        .expect("sent");
    // A retry reschedules and records the error; a final failure closes the row.
    n::mark_failed(&db.pool, first[1].id, "HTTP 503", Some(3600))
        .await
        .expect("retry");
    n::mark_failed(&db.pool, second[0].id, "HTTP 400", None)
        .await
        .expect("fail");
    let sent = n::get_delivery(&db.pool, first[0].id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!((sent.status.as_str(), sent.attempts), ("sent", 1));
    assert!(sent.finished_at_ms.is_some());
    let retry = n::get_delivery(&db.pool, first[1].id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(
        (
            retry.status.as_str(),
            retry.attempts,
            retry.last_error.as_deref()
        ),
        ("pending", 1, Some("HTTP 503"))
    );
    let failed = n::get_delivery(&db.pool, second[0].id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(
        (failed.status.as_str(), failed.last_error.as_deref()),
        ("failed", Some("HTTP 400"))
    );
    assert!(
        n::claim_due(&db.pool, 5, 120)
            .await
            .expect("claim")
            .is_empty(),
        "retry is an hour away"
    );

    let failures = n::list_deliveries(&db.pool, 10, None, Some("failed"), None)
        .await
        .expect("list");
    assert_eq!(failures.len(), 1);
    // Deleting the rule keeps the log rows with their names.
    n::delete_rule(&db.pool, r.id).await.expect("delete");
    let kept = n::get_delivery(&db.pool, sent.id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!((kept.rule_id, kept.rule_name.as_str()), (None, "r"));
    // Pruning removes finished rows only.
    sqlx_age(&db.pool).await;
    assert_eq!(n::prune_deliveries(&db.pool, 30).await.expect("prune"), 2);
    assert_eq!(
        n::list_deliveries(&db.pool, 10, None, None, None)
            .await
            .expect("list")
            .len(),
        1
    );
    db.drop().await;
}

async fn sqlx_age(pool: &superhet_db::PgPool) {
    sqlx::query(
        "UPDATE superhet.notification_deliveries SET created_at = now() - interval '40 days'",
    )
    .execute(pool)
    .await
    .expect("age");
}

#[tokio::test]
async fn the_vapid_key_is_created_once() {
    let db = db!("notify_vapid");
    assert!(n::get_vapid(&db.pool).await.expect("get").is_none());
    let first = Vapid {
        private_pkcs8: "priv1".into(),
        public_key: "pub1".into(),
    };
    let second = Vapid {
        private_pkcs8: "priv2".into(),
        public_key: "pub2".into(),
    };
    assert_eq!(
        n::ensure_vapid(&db.pool, &first).await.expect("ensure"),
        first
    );
    assert_eq!(
        n::ensure_vapid(&db.pool, &second).await.expect("ensure"),
        first,
        "first one wins"
    );
    assert!(!format!("{first:?}").contains("priv1"));
    db.drop().await;
}

#[tokio::test]
async fn push_subscriptions_belong_to_one_account() {
    let db = db!("notify_push");
    let anna = user(&db.pool, "anna").await;
    let bob = user(&db.pool, "bob").await;
    let Subscribed::Ok(sub) = n::upsert_subscription(
        &db.pool,
        anna,
        "https://push.example/a",
        "k1",
        "a1",
        Some("Phone"),
    )
    .await
    .expect("sub") else {
        panic!("expected a subscription");
    };
    // The same browser re-subscribing refreshes its keys; another account cannot take the endpoint.
    let Subscribed::Ok(again) =
        n::upsert_subscription(&db.pool, anna, "https://push.example/a", "k2", "a2", None)
            .await
            .expect("sub")
    else {
        panic!("expected a subscription");
    };
    assert_eq!(
        (again.id, again.p256dh.as_str(), again.label.as_deref()),
        (sub.id, "k2", Some("Phone"))
    );
    assert_eq!(
        n::upsert_subscription(&db.pool, bob, "https://push.example/a", "k", "a", None)
            .await
            .expect("sub"),
        Subscribed::OwnedByOther
    );
    for i in 1..n::MAX_SUBSCRIPTIONS_PER_USER {
        n::upsert_subscription(
            &db.pool,
            anna,
            &format!("https://push.example/{i}"),
            "k",
            "a",
            None,
        )
        .await
        .expect("sub");
    }
    assert_eq!(
        n::upsert_subscription(&db.pool, anna, "https://push.example/over", "k", "a", None)
            .await
            .expect("sub"),
        Subscribed::TooMany
    );
    n::upsert_subscription(&db.pool, bob, "https://push.example/b", "k", "a", None)
        .await
        .expect("sub");

    assert_eq!(
        n::list_subscriptions_for(&db.pool, &[])
            .await
            .expect("all")
            .len(),
        11
    );
    assert_eq!(
        n::list_subscriptions_for(&db.pool, &[bob])
            .await
            .expect("bob")
            .len(),
        1
    );
    // Accounts can only delete their own.
    assert!(
        !n::delete_subscription(&db.pool, bob, sub.id)
            .await
            .expect("del")
    );
    assert!(
        n::delete_subscription(&db.pool, anna, sub.id)
            .await
            .expect("del")
    );
    // A disabled account stops receiving.
    users::set_disabled(&db.pool, bob, true)
        .await
        .expect("disable");
    assert!(
        n::list_subscriptions_for(&db.pool, &[bob])
            .await
            .expect("bob")
            .is_empty()
    );
    db.drop().await;
}
