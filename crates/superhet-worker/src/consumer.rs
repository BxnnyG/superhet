//! Decoder history writer: consumes the tone / RDS / state subjects of all devices, feeds the
//! [`Recorder`], stores the resulting events in PostgreSQL and announces each stored event on
//! `sh.v1.events.core.decoder.<dev>` for live UIs (via the API's event WebSocket).

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_nats::{Client, Message, jetstream};
use futures_util::StreamExt;
use superhet_bus::{rds::RdsMessage, subjects, tones::TonesMessage};
use superhet_db::{PgPool, decoder_events, decoder_events::NewEvent};

use crate::recorder::{Ctx, Recorder};

/// Environment variable with the retention in days (default 30, `0` keeps everything).
pub const ENV_RETENTION_DAYS: &str = "SUPERHET_EVENT_RETENTION_DAYS";
/// Default retention in days.
pub const DEFAULT_RETENTION_DAYS: u32 = 30;
/// Timer resolution of the recorder (burst gaps, tone confirmation).
const TICK: Duration = Duration::from_millis(250);
/// How often old events are pruned.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// Minimum time between two attempts to read a device's state from the KV bucket.
const KV_RETRY: Duration = Duration::from_secs(10);
/// Timeout of a KV lookup.
const KV_TIMEOUT: Duration = Duration::from_secs(3);
const MS_PER_DAY: i64 = 86_400_000;

/// Retention from an environment lookup.
///
/// # Errors
/// A message if the value is not a number of days (0..=36500).
pub fn retention_days(value: Option<&str>) -> Result<u32, String> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(DEFAULT_RETENTION_DAYS),
        Some(v) => v
            .parse::<u32>()
            .ok()
            .filter(|d| *d <= 36_500)
            .ok_or_else(|| format!("{ENV_RETENTION_DAYS} must be a number of days (0..=36500)")),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// `(device, vfo)` from `sh.v1.dev.<dev>.tones.<vfo>` / `.rds.<vfo>`.
fn dev_vfo(subject: &str) -> Option<(&str, u32)> {
    let mut it = subject.split('.');
    let (_, _, _, dev, _, vfo) = (
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
    );
    Some((dev, vfo.parse().ok()?))
}

/// Device id from `sh.v1.dev.<dev>.state`.
fn state_device(subject: &str) -> Option<&str> {
    subject.split('.').nth(3)
}

/// The VFOs of a state document: `vfos[].{id, freq_hz, mode}`.
fn parse_vfos(doc: &[u8]) -> Option<Vec<(u32, Ctx)>> {
    let value: serde_json::Value = serde_json::from_slice(doc).ok()?;
    let vfos = value.get("vfos")?.as_array()?;
    Some(
        vfos.iter()
            .filter_map(|v| {
                let id = u32::try_from(v.get("id")?.as_u64()?).ok()?;
                let freq = v.get("freq_hz")?.as_f64()?;
                let mode = v.get("mode")?.as_str()?.to_owned();
                (freq.is_finite() && freq >= 0.0).then_some((
                    id,
                    Ctx {
                        freq_hz: freq.round() as i64,
                        mode,
                    },
                ))
            })
            .collect(),
    )
}

async fn kv_state(client: &Client, device: &str) -> Option<Vec<(u32, Ctx)>> {
    let key = subjects::state_key(device).ok()?;
    let lookup = async {
        let store = jetstream::new(client.clone())
            .get_key_value(subjects::STATE_BUCKET)
            .await
            .ok()?;
        store.get(key).await.ok().flatten()
    };
    let doc = tokio::time::timeout(KV_TIMEOUT, lookup).await.ok()??;
    parse_vfos(&doc)
}

struct Consumer {
    pool: PgPool,
    client: Client,
    recorder: Recorder,
    /// Devices whose tuning is known, and the last KV attempt for those that are not.
    known: HashMap<String, Option<Instant>>,
}

impl Consumer {
    async fn store(&self, events: Vec<NewEvent>) {
        for new in events {
            match decoder_events::insert(&self.pool, &new).await {
                Ok(stored) => {
                    if let Ok(subject) = subjects::decoder_events(&stored.data.device) {
                        let payload = stored.to_json().to_string();
                        if let Err(err) = self.client.publish(subject, payload.into()).await {
                            tracing::debug!(%err, "decoder event announce failed");
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(%err, kind = %new.kind, "storing a decoder event failed");
                }
            }
        }
    }

    async fn apply_state(&mut self, device: &str, vfos: &[(u32, Ctx)]) {
        self.known.insert(device.to_owned(), None);
        let events = self.recorder.set_state(device, now_ms(), vfos);
        self.store(events).await;
    }

    /// Learn a device's tuning from the KV bucket the first time a VFO message shows up.
    async fn ensure_state(&mut self, device: &str) {
        match self.known.get(device) {
            Some(None) => return,
            Some(Some(at)) if at.elapsed() < KV_RETRY => return,
            _ => {}
        }
        self.known.insert(device.to_owned(), Some(Instant::now()));
        if let Some(vfos) = kv_state(&self.client, device).await {
            self.apply_state(device, &vfos).await;
        }
    }

    async fn on_message(&mut self, msg: &Message, kind: Kind) {
        let subject = msg.subject.as_str();
        match kind {
            Kind::State => {
                if let (Some(dev), Some(vfos)) = (state_device(subject), parse_vfos(&msg.payload)) {
                    self.apply_state(dev, &vfos).await;
                }
            }
            Kind::Tones => {
                let Some((dev, vfo)) = dev_vfo(subject) else {
                    return;
                };
                let Ok(parsed) = TonesMessage::parse(&msg.payload) else {
                    return;
                };
                if parsed.vfo != vfo {
                    return;
                }
                self.ensure_state(dev).await;
                let events = self.recorder.on_tones(dev, now_ms(), &parsed);
                self.store(events).await;
            }
            Kind::Rds => {
                let Some((dev, vfo)) = dev_vfo(subject) else {
                    return;
                };
                let Ok(parsed) = RdsMessage::parse(&msg.payload) else {
                    return;
                };
                if parsed.vfo != vfo {
                    return;
                }
                self.ensure_state(dev).await;
                let events = self.recorder.on_rds(dev, now_ms(), &parsed);
                self.store(events).await;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    State,
    Tones,
    Rds,
}

/// Run until `shutdown` resolves. Returns early (with an error) if the subscriptions fail.
///
/// # Errors
/// A message if the bus subscriptions cannot be created.
pub async fn run(
    pool: PgPool,
    client: Client,
    retention_days: u32,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let sub = |subject: &'static str| {
        let client = client.clone();
        async move {
            client
                .subscribe(subject)
                .await
                .map_err(|e| format!("subscribe {subject}: {e}"))
        }
    };
    let mut state = sub(subjects::ALL_STATE).await?;
    let mut tones = sub(subjects::ALL_TONES).await?;
    let mut rds = sub(subjects::ALL_RDS).await?;
    let mut me = Consumer {
        pool,
        client,
        recorder: Recorder::new(),
        known: HashMap::new(),
    };
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut prune = tokio::time::interval(PRUNE_EVERY);
    tokio::pin!(shutdown);
    tracing::info!(retention_days, "decoder history recorder running");
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            Some(m) = state.next() => me.on_message(&m, Kind::State).await,
            Some(m) = tones.next() => me.on_message(&m, Kind::Tones).await,
            Some(m) = rds.next() => me.on_message(&m, Kind::Rds).await,
            _ = tick.tick() => {
                let events = me.recorder.tick(now_ms());
                me.store(events).await;
            }
            _ = prune.tick() => {
                let cutoff = if retention_days == 0 {
                    0
                } else {
                    now_ms() - i64::from(retention_days) * MS_PER_DAY
                };
                match decoder_events::prune(&me.pool, cutoff).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(removed = n, "pruned old decoder events"),
                    Err(err) => tracing::warn!(%err, "pruning decoder events failed"),
                }
            }
        }
    }
    let events = me.recorder.flush_all(now_ms());
    me.store(events).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_parsing() {
        assert_eq!(retention_days(None), Ok(30));
        assert_eq!(retention_days(Some("  ")), Ok(30));
        assert_eq!(retention_days(Some("7")), Ok(7));
        assert_eq!(retention_days(Some("0")), Ok(0));
        for bad in ["-1", "abc", "36501", "1.5"] {
            assert!(retention_days(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn subject_parsing() {
        assert_eq!(dev_vfo("sh.v1.dev.rtl0.tones.3"), Some(("rtl0", 3)));
        assert_eq!(dev_vfo("sh.v1.dev.rtl0.rds.x"), None);
        assert_eq!(dev_vfo("sh.v1.dev.rtl0.rds"), None);
        assert_eq!(state_device("sh.v1.dev.rtl0.state"), Some("rtl0"));
    }

    #[test]
    fn state_documents_yield_vfo_tuning() {
        let doc = br#"{"v":1,"vfos":[{"id":1,"freq_hz":433500000,"mode":"nfm"},
            {"id":2,"freq_hz":99200000.0,"mode":"wfm","status":"paused"},{"id":"x"}]}"#;
        let vfos = parse_vfos(doc).expect("vfos");
        assert_eq!(vfos.len(), 2);
        assert_eq!(vfos[0].0, 1);
        assert_eq!(vfos[0].1.freq_hz, 433_500_000);
        assert_eq!(vfos[1].1.mode, "wfm");
        assert!(parse_vfos(b"{}").is_none());
        assert!(parse_vfos(b"nope").is_none());
    }
}
