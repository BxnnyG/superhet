//! Persistence of the started instances (table `superhet.plugin_instances`, migration 0008).
//!
//! The table is the host's *desired* state: a row exists from `start` until `stop`. After a
//! restart the host starts every row again with the same instance id. Without a database
//! (`SUPERHET_DATABASE_URL` unset) the store is inert and instances live only in memory, as
//! before.

use superhet_db::PgPool;
use superhet_db::plugin_instances::{self, Record};

use crate::supervisor::StartSpec;

/// Handle to the instance table; cheap to clone.
#[derive(Clone, Default)]
pub struct InstanceStore {
    pool: Option<PgPool>,
}

/// A stored instance.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    /// Instance id.
    pub id: String,
    /// Plugin id.
    pub plugin_id: String,
    /// How to start it.
    pub spec: StartSpec,
}

impl InstanceStore {
    /// A store over `pool`; `None` keeps nothing.
    #[must_use]
    pub fn new(pool: Option<PgPool>) -> Self {
        Self { pool }
    }

    /// Whether instances are persisted.
    #[must_use]
    pub fn is_persistent(&self) -> bool {
        self.pool.is_some()
    }

    /// Every stored instance, oldest first. Failures are logged and give an empty list.
    pub async fn load(&self) -> Vec<Stored> {
        let Some(pool) = &self.pool else {
            return Vec::new();
        };
        match plugin_instances::list(pool).await {
            Ok(records) => records.into_iter().map(stored).collect(),
            Err(err) => {
                tracing::error!(%err, "could not read the stored plugin instances");
                Vec::new()
            }
        }
    }

    /// Remember a started instance. A failure is logged: the instance runs, it just would not
    /// come back after a restart.
    pub async fn save(&self, id: &str, plugin_id: &str, spec: &StartSpec) {
        let Some(pool) = &self.pool else { return };
        let record = Record {
            id: id.to_owned(),
            plugin_id: plugin_id.to_owned(),
            device: spec.device.clone(),
            vfo: spec.vfo.and_then(|v| i32::try_from(v).ok()),
            freq_hz: spec.freq_hz.and_then(|f| i64::try_from(f).ok()),
            config: spec.config.clone(),
        };
        if let Err(err) = plugin_instances::upsert(pool, &record).await {
            tracing::error!(%err, instance = id, "could not store the plugin instance");
        }
    }

    /// Forget an instance.
    pub async fn forget(&self, id: &str) {
        let Some(pool) = &self.pool else { return };
        if let Err(err) = plugin_instances::delete(pool, id).await {
            tracing::error!(%err, instance = id, "could not forget the plugin instance");
        }
    }
}

fn stored(record: Record) -> Stored {
    Stored {
        id: record.id,
        plugin_id: record.plugin_id,
        spec: StartSpec {
            device: record.device,
            vfo: record.vfo.and_then(|v| u32::try_from(v).ok()),
            freq_hz: record.freq_hz.and_then(|f| u64::try_from(f).ok()),
            config: record.config,
        },
    }
}
