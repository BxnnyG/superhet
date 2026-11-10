//! Notifications (ADR-0026).
//!
//! Three loops share this module:
//! - the matcher is a durable JetStream consumer of the decoder-event announcements the recorder
//!   publishes after storing a row (`sh.v1.events.core.decoder.<dev>`, core detectors and plugins
//!   alike). Each event is checked against the enabled rules; a firing rule queues one delivery
//!   per target in PostgreSQL. The message is acknowledged only after the queueing succeeded; a
//!   redelivery queues nothing twice because (rule, target, event id) is unique;
//! - the sender works the queue off: webhook, e-mail or Web Push, with retries and backoff kept in
//!   the row, so a restart loses nothing;
//! - an hourly job prunes finished deliveries.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{
    self, AckKind,
    consumer::{AckPolicy, DeliverPolicy, pull},
};
use futures_util::StreamExt;
use serde_json::Value;
use superhet_bus::subjects;
use superhet_db::{
    PgPool,
    notifications::{self as db, Delivery, NewDelivery},
};
use superhet_notify::{
    Egress, Message, SendError,
    email::{self, SmtpConfig},
    push,
    retry::next_delay,
    rule::{self, CompiledRule, Limiter, QuietWindow, Verdict},
    target::{self, Config},
    webhook,
};

const CONSUMER: &str = "worker-notifications";
const MAX_DELIVER: i64 = 5;
const NAK_DELAY: Duration = Duration::from_secs(5);
/// How long the rule set is trusted before it is read again.
const RULES_TTL: Duration = Duration::from_secs(5);
/// Poll interval of the delivery queue.
const SEND_POLL: Duration = Duration::from_secs(2);
/// Deliveries taken per round and sent concurrently.
const BATCH: i64 = 16;
/// A taken delivery is invisible to others for this long (covers the slowest attempt).
const LEASE_SECS: i32 = 120;
const PRUNE_EVERY: Duration = Duration::from_secs(3600);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// What a rule decided for one event.
#[derive(Debug, PartialEq)]
pub struct Planned {
    pub rule_id: i64,
    pub rule_name: String,
    pub message: Message,
    pub target_ids: Vec<i64>,
}

/// Evaluate `event` (the announced history row) against `rules`. Rules that match but are
/// quiet, cooling down or rate limited produce nothing; they are counted in `suppressed`.
pub fn evaluate(
    event: &Value,
    rules: &[CompiledRule],
    limiter: &mut Limiter,
    now_ms: i64,
    suppressed: &mut u64,
) -> Vec<Planned> {
    let ts = event.get("ts_ms").and_then(Value::as_i64).unwrap_or(now_ms);
    if now_ms - ts > rule::MAX_EVENT_AGE_MS {
        return Vec::new();
    }
    let ctx = rule::context(event);
    let mut planned = Vec::new();
    for rule in rules {
        if !rule.matches(&ctx) {
            continue;
        }
        if limiter.check(rule, now_ms) != Verdict::Fire {
            *suppressed += 1;
            continue;
        }
        let (title, body) = rule.render(&ctx);
        planned.push(Planned {
            rule_id: rule.id,
            rule_name: rule.name.clone(),
            message: Message {
                title,
                body,
                event: event.clone(),
                rule: rule.name.clone(),
                rule_id: Some(rule.id),
            },
            target_ids: rule.target_ids.clone(),
        });
    }
    planned
}

struct RuleSet {
    rules: Vec<CompiledRule>,
    target_names: HashMap<i64, String>,
    loaded: Instant,
}

async fn load(pool: &PgPool) -> Result<RuleSet, superhet_db::Error> {
    let stored = db::list_rules(pool, true).await?;
    let targets = db::list_targets(pool).await?;
    let zones: Vec<String> = stored
        .iter()
        .filter_map(|r| r.data.quiet.as_ref().map(|q| q.tz.clone()))
        .collect();
    let offsets = if zones.is_empty() {
        HashMap::new()
    } else {
        db::zone_offsets(pool, &zones).await?
    };
    let mut rules = Vec::new();
    for r in stored {
        let quiet = r.data.quiet.as_ref().and_then(|q| {
            offsets.get(&q.tz).map(|offset| QuietWindow {
                start_min: q.start_min,
                end_min: q.end_min,
                offset_secs: *offset,
            })
        });
        match CompiledRule::new(
            r.id,
            &r.data.name,
            r.data.source.clone(),
            r.data.kind.clone(),
            &r.data.conditions,
            r.data.cooldown_secs,
            r.data.max_per_hour,
            quiet,
            &r.data.title_template,
            &r.data.body_template,
            r.data.target_ids.clone(),
        ) {
            Ok(rule) => rules.push(rule),
            Err(err) => tracing::warn!(rule = %r.data.name, %err, "notification rule skipped"),
        }
    }
    Ok(RuleSet {
        rules,
        target_names: targets.into_iter().map(|t| (t.id, t.name)).collect(),
        loaded: Instant::now(),
    })
}

/// Queue the deliveries of one announced event. `Err` means "try this message again".
async fn queue_event(
    pool: &PgPool,
    set: &RuleSet,
    limiter: &mut Limiter,
    payload: &[u8],
) -> Result<usize, superhet_db::Error> {
    let Ok(event) = serde_json::from_slice::<Value>(payload) else {
        return Ok(0);
    };
    let Some(event_id) = event.get("id").and_then(Value::as_i64) else {
        return Ok(0);
    };
    let mut suppressed = 0;
    let planned = evaluate(&event, &set.rules, limiter, now_ms(), &mut suppressed);
    crate::metrics::notifications_suppressed(suppressed);
    let mut queued = 0;
    for plan in planned {
        for target_id in &plan.target_ids {
            let Some(target_name) = set.target_names.get(target_id) else {
                continue;
            };
            let new = NewDelivery {
                rule_id: Some(plan.rule_id),
                rule_name: plan.rule_name.clone(),
                target_id: *target_id,
                target_name: target_name.clone(),
                event_id: Some(event_id),
                message: serde_json::to_value(&plan.message).unwrap_or(Value::Null),
            };
            if db::enqueue(pool, &new).await?.is_some() {
                queued += 1;
            }
        }
    }
    Ok(queued)
}

async fn matcher(
    pool: PgPool,
    client: async_nats::Client,
    shutdown: impl std::future::Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    let js = jetstream::new(client);
    let mut messages = loop {
        let setup = async {
            let stream = js
                .get_or_create_stream(superhet_bus::events_stream_config())
                .await
                .map_err(|e| e.to_string())?;
            let consumer = stream
                .get_or_create_consumer(
                    CONSUMER,
                    pull::Config {
                        durable_name: Some(CONSUMER.to_owned()),
                        ack_policy: AckPolicy::Explicit,
                        deliver_policy: DeliverPolicy::New,
                        filter_subject: subjects::ALL_DECODER_EVENTS.to_owned(),
                        max_deliver: MAX_DELIVER,
                        ack_wait: Duration::from_secs(30),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            consumer.messages().await.map_err(|e| e.to_string())
        };
        tokio::select! {
            () = &mut shutdown => return,
            result = tokio::time::timeout(Duration::from_secs(10), setup) => match result {
                Ok(Ok(messages)) => break messages,
                Ok(Err(err)) => tracing::warn!(%err, "notification matcher not ready; retrying"),
                Err(_) => tracing::warn!("notification matcher setup timed out; retrying"),
            },
        }
        tokio::select! {
            () = &mut shutdown => return,
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    };
    tracing::info!("notification matcher running");
    let mut set = RuleSet {
        rules: Vec::new(),
        target_names: HashMap::new(),
        loaded: Instant::now()
            .checked_sub(RULES_TTL * 2)
            .unwrap_or_else(Instant::now),
    };
    let mut limiter = Limiter::default();
    loop {
        let message = tokio::select! {
            () = &mut shutdown => return,
            next = messages.next() => match next {
                Some(Ok(message)) => message,
                Some(Err(err)) => {
                    tracing::warn!(%err, "notification stream error");
                    continue;
                }
                None => return,
            },
        };
        if set.loaded.elapsed() >= RULES_TTL {
            match load(&pool).await {
                Ok(fresh) => {
                    let ids: Vec<i64> = fresh.rules.iter().map(|r| r.id).collect();
                    limiter.retain(&ids);
                    set = fresh;
                }
                Err(err) => {
                    tracing::warn!(%err, "loading notification rules failed");
                    let _ = message.ack_with(AckKind::Nak(Some(NAK_DELAY))).await;
                    continue;
                }
            }
        }
        let ack = match queue_event(&pool, &set, &mut limiter, &message.message.payload).await {
            Ok(_) => message.ack().await,
            Err(err) => {
                tracing::warn!(%err, "queueing notifications failed");
                message.ack_with(AckKind::Nak(Some(NAK_DELAY))).await
            }
        };
        if let Err(err) = ack {
            tracing::warn!(%err, "acknowledging a notification event failed");
        }
    }
}

/// Everything the sender needs to know about its environment.
pub struct Sender {
    pool: PgPool,
    egress: Egress,
    smtp: Option<SmtpConfig>,
    vapid_subject: String,
}

/// Result of one attempt.
enum Outcome {
    Sent(Option<String>),
    Failed(SendError, u32),
}

impl Sender {
    /// Read the SMTP and VAPID settings from the environment.
    ///
    /// # Errors
    /// A message for an invalid `SUPERHET_SMTP_*` combination or a TLS setup failure.
    pub fn from_env(pool: PgPool) -> Result<Self, String> {
        let smtp = SmtpConfig::from_env(|k| std::env::var(k).ok())?;
        let subject = std::env::var("SUPERHET_VAPID_SUBJECT")
            .ok()
            .filter(|s| s.starts_with("mailto:") || s.starts_with("https://"))
            .or_else(|| {
                std::env::var("SUPERHET_PUBLIC_URL")
                    .ok()
                    .filter(|s| s.starts_with("https://"))
            })
            .unwrap_or_else(|| "mailto:noreply@superhet.invalid".to_owned());
        Ok(Self {
            pool,
            egress: Egress::new()?,
            smtp,
            vapid_subject: subject,
        })
    }

    async fn vapid(&self) -> Result<db::Vapid, SendError> {
        let transient = |_| SendError::transient("database unavailable");
        if let Some(found) = db::get_vapid(&self.pool).await.map_err(transient)? {
            return Ok(found);
        }
        let (private_pkcs8, public_key) = push::generate_vapid().map_err(SendError::permanent)?;
        db::ensure_vapid(
            &self.pool,
            &db::Vapid {
                private_pkcs8,
                public_key,
            },
        )
        .await
        .map_err(transient)
    }

    async fn attempt(&self, d: &Delivery) -> Outcome {
        let fail = |err: SendError, retries: u32| Outcome::Failed(err, retries.min(8));
        let Ok(message) = serde_json::from_value::<Message>(d.message.clone()) else {
            return fail(SendError::permanent("stored message is damaged"), 0);
        };
        let Some(target_id) = d.target_id else {
            return fail(SendError::permanent("the target was deleted"), 0);
        };
        let stored = match db::get_target(&self.pool, target_id).await {
            Ok(Some(t)) => t,
            Ok(None) => return fail(SendError::permanent("the target was deleted"), 0),
            Err(_) => return fail(SendError::transient("database unavailable"), 8),
        };
        let config = match target::parse(&stored.kind, &stored.config) {
            Ok(c) => c,
            Err(err) => {
                return fail(
                    SendError::permanent(format!("target settings invalid: {err}")),
                    0,
                );
            }
        };
        let retries = config.retries();
        let result = match &config {
            Config::Webhook(cfg) => webhook::send(
                &self.egress,
                cfg,
                stored.secret.as_deref(),
                d.id,
                &message,
                now_ms(),
            )
            .await
            .map(|()| None),
            Config::Email(cfg) => match &self.smtp {
                None => Err(SendError::permanent(
                    "SMTP is not configured (SUPERHET_SMTP_HOST on the worker)",
                )),
                Some(smtp) => email::send(smtp, &cfg.to, &message.title, &message.body)
                    .await
                    .map(|()| None),
            },
            Config::Push(cfg) => self.push(cfg, &message).await,
        };
        match result {
            Ok(note) => Outcome::Sent(note),
            Err(err) => fail(err, retries),
        }
    }

    async fn push(
        &self,
        cfg: &target::PushDoc,
        message: &Message,
    ) -> Result<Option<String>, SendError> {
        let vapid = self.vapid().await?;
        let subs = db::list_subscriptions_for(&self.pool, &cfg.user_ids)
            .await
            .map_err(|_| SendError::transient("database unavailable"))?;
        if subs.is_empty() {
            return Err(SendError::permanent(
                "nobody has a push subscription for this target",
            ));
        }
        let identity = push::Identity {
            private_pkcs8: &vapid.private_pkcs8,
            public_key: &vapid.public_key,
            subject: &self.vapid_subject,
        };
        let (mut sent, mut failed, mut gone) = (0usize, 0usize, 0usize);
        let mut last: Option<SendError> = None;
        for sub in &subs {
            let view = push::Subscription {
                endpoint: &sub.endpoint,
                p256dh: &sub.p256dh,
                auth: &sub.auth,
            };
            match push::send(&self.egress, identity, &view, cfg, message, now_ms()).await {
                Ok(()) => sent += 1,
                Err(err) if err.gone => {
                    gone += 1;
                    let _ = db::delete_subscription_by_id(&self.pool, sub.id).await;
                }
                Err(err) => {
                    failed += 1;
                    if last.as_ref().is_none_or(|l| !l.retryable) {
                        last = Some(err);
                    }
                }
            }
        }
        if sent > 0 {
            let note = (failed + gone > 0).then(|| {
                format!(
                    "{sent} of {} browsers reached ({gone} expired and removed)",
                    subs.len()
                )
            });
            return Ok(note);
        }
        Err(last.unwrap_or_else(|| {
            SendError::permanent("all push subscriptions had expired and were removed")
        }))
    }

    async fn finish(&self, d: &Delivery, outcome: Outcome) {
        let stored = match outcome {
            Outcome::Sent(note) => {
                crate::metrics::notification_result(true);
                db::mark_sent(&self.pool, d.id, note.as_deref()).await
            }
            Outcome::Failed(err, retries) => {
                let failed_attempts = u32::try_from(d.attempts).unwrap_or(0) + 1;
                let delay = next_delay(failed_attempts, retries, &err);
                if delay.is_none() {
                    crate::metrics::notification_result(false);
                }
                tracing::info!(delivery = d.id, target = %d.target_name, error = %err, retry = delay.is_some(), "notification attempt failed");
                db::mark_failed(&self.pool, d.id, &err.message, delay).await
            }
        };
        if let Err(err) = stored {
            tracing::warn!(%err, delivery = d.id, "recording a delivery result failed");
        }
    }
}

async fn send_loop(sender: Arc<Sender>, shutdown: impl std::future::Future<Output = ()>) {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => return,
            () = tokio::time::sleep(SEND_POLL) => {}
        }
        let due = match db::claim_due(&sender.pool, BATCH, LEASE_SECS).await {
            Ok(due) => due,
            Err(err) => {
                tracing::warn!(%err, "reading the notification queue failed");
                continue;
            }
        };
        let mut tasks = tokio::task::JoinSet::new();
        for delivery in due {
            let sender = sender.clone();
            tasks.spawn(async move {
                let outcome = sender.attempt(&delivery).await;
                sender.finish(&delivery, outcome).await;
            });
        }
        while tasks.join_next().await.is_some() {}
    }
}

async fn prune_loop(pool: PgPool, days: u32, shutdown: impl std::future::Future<Output = ()>) {
    tokio::pin!(shutdown);
    loop {
        match db::prune_deliveries(&pool, days).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "old notification deliveries pruned"),
            Err(err) => tracing::warn!(%err, "pruning notification deliveries failed"),
        }
        tokio::select! {
            () = &mut shutdown => return,
            () = tokio::time::sleep(PRUNE_EVERY) => {}
        }
    }
}

/// Run the three loops until `shutdown` resolves.
pub async fn run(
    pool: PgPool,
    client: Option<async_nats::Client>,
    retention_days: u32,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let sender = match Sender::from_env(pool.clone()) {
        Ok(s) => Arc::new(s),
        Err(err) => {
            tracing::error!(%err, "notifications disabled");
            return;
        }
    };
    let stop = |mut rx: tokio::sync::watch::Receiver<bool>| async move {
        let _ = rx.wait_for(|stopped| *stopped).await;
    };
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(send_loop(sender, stop(shutdown.clone())));
    tasks.spawn(prune_loop(
        pool.clone(),
        if retention_days == 0 {
            365
        } else {
            retention_days
        },
        stop(shutdown.clone()),
    ));
    if let Some(client) = client {
        tasks.spawn(matcher(pool, client, stop(shutdown)));
    } else {
        tracing::warn!(
            "notification rules are not evaluated without SUPERHET_NATS_URL; queued sends still go out"
        );
    }
    while tasks.join_next().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(id: i64, ts_ms: i64, text: &str, address: i64) -> Value {
        json!({
            "id": id, "ts_ms": ts_ms, "device": "rtl0", "vfo": 1, "freq_hz": 153_350_000,
            "kind": "pocsag.message", "source": "org.superhet.decoder.pocsag", "value": text,
            "meta": {"data": {"address": address, "text": text}},
        })
    }

    fn rules() -> Vec<CompiledRule> {
        let make = |id, name: &str, conditions: Value, cooldown| {
            CompiledRule::new(
                id,
                name,
                Some("org.superhet.decoder.pocsag".into()),
                None,
                &conditions,
                cooldown,
                None,
                None,
                "",
                "RIC {{data.address}}: {{data.text}}",
                vec![10, 11],
            )
            .expect("rule")
        };
        vec![
            make(
                1,
                "fire",
                json!([{"field": "value", "op": "contains", "value": "fire"}]),
                60,
            ),
            make(
                2,
                "ric",
                json!([{"field": "data.address", "op": "equals", "value": 1234}]),
                0,
            ),
        ]
    }

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn matching_rules_plan_a_message_for_every_target() {
        let mut limiter = Limiter::default();
        let mut suppressed = 0;
        let planned = evaluate(
            &event(5, NOW, "FIRE alarm", 1234),
            &rules(),
            &mut limiter,
            NOW,
            &mut suppressed,
        );
        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].rule_name, "fire");
        assert_eq!(planned[0].target_ids, [10, 11]);
        assert_eq!(planned[0].message.body, "RIC 1234: FIRE alarm");
        assert_eq!(planned[0].message.rule_id, Some(1));
        assert_eq!(planned[0].message.event["id"], 5);
        let none = evaluate(
            &event(6, NOW, "all quiet", 7),
            &rules(),
            &mut limiter,
            NOW,
            &mut suppressed,
        );
        assert!(none.is_empty());
    }

    #[test]
    fn cooldown_suppresses_and_is_counted() {
        let mut limiter = Limiter::default();
        let mut suppressed = 0;
        let rules = rules();
        assert_eq!(
            evaluate(
                &event(1, NOW, "fire", 1),
                &rules,
                &mut limiter,
                NOW,
                &mut suppressed
            )
            .len(),
            1
        );
        assert_eq!(
            evaluate(
                &event(2, NOW + 1000, "fire", 1),
                &rules,
                &mut limiter,
                NOW + 1000,
                &mut suppressed
            )
            .len(),
            0
        );
        assert_eq!(suppressed, 1);
        assert_eq!(
            evaluate(
                &event(3, NOW + 61_000, "fire", 1),
                &rules,
                &mut limiter,
                NOW + 61_000,
                &mut suppressed
            )
            .len(),
            1
        );
    }

    #[test]
    fn stale_events_after_an_outage_are_not_notified() {
        let mut limiter = Limiter::default();
        let mut suppressed = 0;
        let old = event(1, NOW - rule::MAX_EVENT_AGE_MS - 1, "fire", 1234);
        assert!(evaluate(&old, &rules(), &mut limiter, NOW, &mut suppressed).is_empty());
        let fresh = event(2, NOW - 60_000, "fire", 1234);
        assert!(!evaluate(&fresh, &rules(), &mut limiter, NOW, &mut suppressed).is_empty());
    }

    #[test]
    fn the_same_event_plans_identical_deliveries_so_the_unique_index_can_drop_repeats() {
        // Redelivery of one announcement (same id) produces the same (rule, event) keys; with a
        // fresh limiter, as after a restart, the plan is identical.
        let ev = event(9, NOW, "fire", 1234);
        let plan = |limiter: &mut Limiter| {
            let mut s = 0;
            evaluate(&ev, &rules(), limiter, NOW, &mut s)
                .into_iter()
                .map(|p| (p.rule_id, p.message.event["id"].clone(), p.target_ids))
                .collect::<Vec<_>>()
        };
        assert_eq!(plan(&mut Limiter::default()), plan(&mut Limiter::default()));
    }

    // ---- queue and sender against a real PostgreSQL (`DATABASE_URL`) and a mock webhook ----

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Answers each connection with the next status of `statuses`; counts requests.
    async fn mock_hook(statuses: Vec<u16>) -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            for status in statuses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf).await;
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (port, hits)
    }

    async fn test_pool() -> Option<(PgPool, PgPool, String)> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let admin = superhet_db::connect(&url).await.expect("admin");
        let name = format!("sh_worker_notify_{}", std::process::id());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("create");
        let (base, _) = url.rsplit_once('/').expect("url");
        let pool = superhet_db::connect(&format!("{base}/{name}"))
            .await
            .expect("connect");
        superhet_db::migrate(&pool).await.expect("migrate");
        Some((admin, pool, name))
    }

    #[tokio::test]
    async fn events_are_queued_once_and_sent_with_retries() {
        let Some((admin, pool, name)) = test_pool().await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        let (port, hits) = mock_hook(vec![503, 200]).await;
        let config = target::normalize(
            "webhook",
            &json!({"url": format!("http://127.0.0.1:{port}/hook"), "allow_private": true, "retries": 3}),
            None,
        )
        .expect("config");
        let t = db::insert_target(&pool, "hook", "webhook", &config, None)
            .await
            .expect("t")
            .expect("created");
        let input = db::RuleInput {
            name: "fire".into(),
            enabled: true,
            source: None,
            kind: None,
            conditions: json!([{"field": "value", "op": "contains", "value": "fire"}]),
            cooldown_secs: 0,
            max_per_hour: None,
            quiet: None,
            title_template: String::new(),
            body_template: String::new(),
            target_ids: vec![t.id],
        };
        db::insert_rule(&pool, &input)
            .await
            .expect("r")
            .expect("created");
        let set = load(&pool).await.expect("load");
        assert_eq!(set.rules.len(), 1);

        // The same announcement delivered twice queues one delivery.
        let payload = event(77, now_ms(), "FIRE at the depot", 1)
            .to_string()
            .into_bytes();
        let mut limiter = Limiter::default();
        assert_eq!(
            queue_event(&pool, &set, &mut limiter, &payload)
                .await
                .expect("queue"),
            1
        );
        assert_eq!(
            queue_event(&pool, &set, &mut Limiter::default(), &payload)
                .await
                .expect("queue"),
            0
        );
        // A non-matching event queues nothing.
        let quiet = event(78, now_ms(), "all quiet", 1).to_string().into_bytes();
        assert_eq!(
            queue_event(&pool, &set, &mut limiter, &quiet)
                .await
                .expect("queue"),
            0
        );

        // First attempt hits a 503: rescheduled with the backoff and the error recorded.
        let sender = Sender::from_env(pool.clone()).expect("sender");
        let due = db::claim_due(&pool, 10, 120).await.expect("claim");
        assert_eq!(due.len(), 1);
        let outcome = sender.attempt(&due[0]).await;
        sender.finish(&due[0], outcome).await;
        let after = db::get_delivery(&pool, due[0].id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(
            (
                after.status.as_str(),
                after.attempts,
                after.last_error.as_deref()
            ),
            ("pending", 1, Some("HTTP 503 Service Unavailable"))
        );
        assert!(
            db::claim_due(&pool, 10, 120)
                .await
                .expect("claim")
                .is_empty(),
            "backoff holds it back"
        );

        // Make it due again: the second attempt succeeds.
        sqlx::query("UPDATE superhet.notification_deliveries SET next_attempt_at = now()")
            .execute(&pool)
            .await
            .expect("due");
        let due = db::claim_due(&pool, 10, 120).await.expect("claim");
        let outcome = sender.attempt(&due[0]).await;
        sender.finish(&due[0], outcome).await;
        let done = db::get_delivery(&pool, due[0].id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!((done.status.as_str(), done.attempts), ("sent", 2));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);

        // A deleted target cannot be queued for any more.
        db::delete_target(&pool, t.id).await.expect("delete");
        let orphan = db::NewDelivery {
            rule_id: None,
            rule_name: "(test)".into(),
            target_id: t.id,
            target_name: "hook".into(),
            event_id: None,
            message: json!({"title": "t", "body": "b", "event": {}, "rule": "r"}),
        };
        assert!(
            db::enqueue(&pool, &orphan).await.is_err(),
            "foreign key refuses a vanished target"
        );
        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop");
    }
}
