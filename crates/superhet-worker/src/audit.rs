//! Audit log upkeep (ADR-0012): persists system events published on `sh.v1.audit.>` and prunes
//! old rows according to the retention stored in the settings (90 days for accounts and the
//! system, 7 days for guests by default; `0` keeps a class forever).

use std::time::Duration;

use async_nats::Client;
use futures_util::StreamExt;
use serde_json::Value;
use superhet_bus::{audit::SystemEvent, subjects};
use superhet_db::{PgPool, audit::NewEvent};

/// How often retention is applied.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// Default retention in days for accounts and system events.
pub const DEFAULT_USER_DAYS: u32 = 90;
/// Default retention in days for guest events.
pub const DEFAULT_GUEST_DAYS: u32 = 7;
/// Largest retention accepted from the settings document (matches the API's validation).
const MAX_DAYS: u64 = 36_500;

/// `(user_days, guest_days)` from the stored `audit` settings document (absent or unusable
/// fields fall back to the defaults).
#[must_use]
pub fn retention_from(doc: Option<&Value>) -> (u32, u32) {
    let days = |key: &str, default: u32| {
        doc.and_then(|d| d.get(key))
            .and_then(Value::as_u64)
            .filter(|d| *d <= MAX_DAYS)
            .and_then(|d| u32::try_from(d).ok())
            .unwrap_or(default)
    };
    (
        days("retention_user_days", DEFAULT_USER_DAYS),
        days("retention_guest_days", DEFAULT_GUEST_DAYS),
    )
}

/// The database row for a system event received from the bus.
#[must_use]
pub fn to_row(event: SystemEvent) -> NewEvent {
    NewEvent {
        ts_ms: event.ts_ms,
        actor_kind: "system".to_owned(),
        actor_id: Some(event.source.clone()),
        actor_name: Some(event.source),
        role: None,
        ip: None,
        user_agent: None,
        action: event.action,
        target: event.target,
        outcome: event.outcome.as_str().to_owned(),
        details: event
            .details
            .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
    }
}

async fn store(pool: &PgPool, payload: &[u8]) {
    let event = match serde_json::from_slice::<SystemEvent>(payload) {
        Ok(event) => event,
        Err(err) => {
            tracing::warn!(%err, "dropping a malformed audit event");
            crate::metrics::audit_event_dropped();
            return;
        }
    };
    if let Err(reason) = event.validate() {
        tracing::warn!(reason, "dropping an invalid audit event");
        crate::metrics::audit_event_dropped();
        return;
    }
    match superhet_db::audit::insert(pool, &to_row(event)).await {
        Ok(_) => crate::metrics::audit_event_stored(),
        Err(err) => {
            tracing::error!(%err, "could not store an audit event");
            crate::metrics::audit_event_dropped();
        }
    }
}

/// Apply the retention once. Returns the number of rows removed.
pub async fn prune_once(pool: &PgPool) -> Result<u64, superhet_db::Error> {
    let doc = superhet_db::settings::get(pool, "audit").await?;
    let (user_days, guest_days) = retention_from(doc.as_ref());
    superhet_db::audit::prune(pool, user_days, guest_days).await
}

/// Persist system events (when a bus client exists) and prune hourly until `shutdown`.
pub async fn run(
    pool: PgPool,
    client: Option<Client>,
    shutdown: impl std::future::Future<Output = ()>,
) {
    let mut events = match client {
        Some(client) => match client.subscribe(subjects::ALL_AUDIT).await {
            Ok(sub) => Some(sub),
            Err(err) => {
                tracing::error!(%err, "cannot subscribe to system audit events");
                None
            }
        },
        None => None,
    };
    let mut prune = tokio::time::interval(PRUNE_EVERY);
    tokio::pin!(shutdown);
    tracing::info!("audit upkeep running");
    loop {
        let next = async {
            match &mut events {
                Some(sub) => sub.next().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = &mut shutdown => break,
            Some(message) = next => store(&pool, &message.payload).await,
            _ = prune.tick() => match prune_once(&pool).await {
                Ok(0) => {}
                Ok(removed) => {
                    crate::metrics::audit_pruned(removed);
                    tracing::info!(removed, "pruned old audit events");
                }
                Err(err) => tracing::warn!(%err, "pruning audit events failed"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use superhet_bus::audit::Outcome;

    #[test]
    fn retention_defaults_and_overrides() {
        assert_eq!(retention_from(None), (90, 7));
        assert_eq!(retention_from(Some(&json!({}))), (90, 7));
        assert_eq!(
            retention_from(Some(
                &json!({"retention_user_days": 30, "retention_guest_days": 0})
            )),
            (30, 0)
        );
        // Garbage falls back instead of deleting everything or nothing by accident.
        assert_eq!(
            retention_from(Some(
                &json!({"retention_user_days": -1, "retention_guest_days": 99999})
            )),
            (90, 7)
        );
    }

    #[test]
    fn system_events_become_system_rows() {
        let row = to_row(SystemEvent {
            v: 1,
            ts_ms: Some(1_700_000_000_000),
            action: "device.error".into(),
            source: "sdrd".into(),
            target: Some("dev0".into()),
            outcome: Outcome::Error,
            details: None,
        });
        assert_eq!(
            (row.actor_kind.as_str(), row.actor_id.as_deref()),
            ("system", Some("sdrd"))
        );
        assert_eq!(row.outcome, "error");
        assert_eq!(row.details, json!({}));
        assert_eq!(row.ts_ms, Some(1_700_000_000_000));
        assert!(row.ip.is_none() && row.role.is_none());
    }

    /// Needs `DATABASE_URL` (a PostgreSQL where databases may be created); skipped otherwise.
    async fn scratch_db(tag: &str) -> Option<(PgPool, PgPool, String)> {
        let url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|u| !u.is_empty())?;
        let admin = superhet_db::connect(&url).await.expect("admin");
        let name = format!("sh_worker_{tag}_{}", std::process::id());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("create db");
        let (base, _) = url.rsplit_once('/').expect("url with database");
        let pool = superhet_db::connect(&format!("{base}/{name}"))
            .await
            .expect("pool");
        superhet_db::migrate(&pool).await.expect("migrate");
        Some((admin, pool, name))
    }

    #[tokio::test]
    async fn system_events_are_stored_and_retention_follows_the_settings() {
        let Some((admin, pool, name)) = scratch_db("audit").await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        store(&pool, br#"{"v":1,"action":"plugin.crash","source":"pluginhost","target":"direwolf","outcome":"error","details":{"code":1}}"#).await;
        store(&pool, b"not json").await;
        store(
            &pool,
            br#"{"v":1,"action":"bad action","source":"x","outcome":"ok"}"#,
        )
        .await;
        let rows = superhet_db::audit::query(&pool, &superhet_db::audit::Filter::default(), 10)
            .await
            .expect("query");
        assert_eq!(rows.len(), 1, "only the valid event is stored");
        assert_eq!(rows[0].data.actor_kind, "system");
        assert_eq!(rows[0].data.target.as_deref(), Some("direwolf"));
        assert_eq!(rows[0].data.details, json!({"code": 1}));

        // Age it by 100 days: the default retention (90 days for the system) removes it ...
        let mut tx = pool.begin().await.expect("tx");
        sqlx::query("SELECT set_config('superhet.audit_maintenance', 'on', true)")
            .execute(&mut *tx)
            .await
            .expect("maintenance");
        sqlx::query("UPDATE superhet.audit_events SET ts = now() - interval '100 days'")
            .execute(&mut *tx)
            .await
            .expect("age");
        tx.commit().await.expect("commit");
        // ... unless the admin raised it in the settings.
        superhet_db::settings::put(&pool, "audit", &json!({"retention_user_days": 365}), "test")
            .await
            .expect("settings");
        assert_eq!(prune_once(&pool).await.expect("prune"), 0);
        superhet_db::settings::put(&pool, "audit", &json!({"retention_user_days": 30}), "test")
            .await
            .expect("settings");
        assert_eq!(prune_once(&pool).await.expect("prune"), 1);

        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop db");
    }
}
