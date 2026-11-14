//! Scheduled logical backups (ADR-0030, `docs/backup.md`).
//!
//! `SUPERHET_BACKUP_DIR` switches the job on; `SUPERHET_BACKUP_SCHEDULE` (default `daily 03:30`,
//! UTC), `SUPERHET_BACKUP_KEEP` (default 7), `SUPERHET_BACKUP_PASSPHRASE` (seal the archives),
//! `SUPERHET_BACKUP_CREDENTIALS` and `SUPERHET_BACKUP_HISTORY` (`true` to include password hashes
//! and secrets / the audit log and decoder history). The state is kept in the settings document
//! `backup_status`, which the admin page reads.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use superhet_backup::schedule::{self, RunOptions, Schedule};
use superhet_db::PgPool;
use tokio::sync::watch;

const STATUS_KEY: &str = "backup_status";
const DEFAULT_KEEP: usize = 7;

/// The job's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub dir: PathBuf,
    pub schedule: Schedule,
    pub schedule_text: String,
    pub keep: usize,
    pub passphrase: Option<String>,
    pub credentials: bool,
    pub history: bool,
}

fn flag(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

impl Config {
    /// Read the environment values; `Ok(None)` when no directory is configured.
    ///
    /// # Errors
    /// A message for an unusable schedule, keep count or passphrase.
    pub fn parse(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let Some(dir) = get("SUPERHET_BACKUP_DIR").filter(|d| !d.trim().is_empty()) else {
            return Ok(None);
        };
        let schedule_text = get("SUPERHET_BACKUP_SCHEDULE")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "daily 03:30".to_owned());
        let schedule = schedule::parse(&schedule_text)
            .map_err(|e| format!("SUPERHET_BACKUP_SCHEDULE: {e}"))?;
        let keep = match get("SUPERHET_BACKUP_KEEP").filter(|s| !s.trim().is_empty()) {
            None => DEFAULT_KEEP,
            Some(k) => k
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|k| (1..=1000).contains(k))
                .ok_or("SUPERHET_BACKUP_KEEP must be 1..=1000")?,
        };
        let passphrase = get("SUPERHET_BACKUP_PASSPHRASE").filter(|p| !p.is_empty());
        if let Some(p) = &passphrase {
            superhet_backup::crypto::check_passphrase(p)
                .map_err(|e| format!("SUPERHET_BACKUP_PASSPHRASE: {e}"))?;
        }
        Ok(Some(Self {
            dir: PathBuf::from(dir.trim()),
            schedule,
            schedule_text,
            keep,
            passphrase,
            credentials: flag(get("SUPERHET_BACKUP_CREDENTIALS").as_deref()),
            history: flag(get("SUPERHET_BACKUP_HISTORY").as_deref()),
        }))
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

async fn publish(pool: &PgPool, status: &Value) {
    if let Err(err) = superhet_db::settings::put(pool, STATUS_KEY, status, "worker").await {
        tracing::warn!(%err, "could not store the backup status");
    }
}

/// Run until `stop` flips to `true`.
pub async fn run(pool: PgPool, mut stop: watch::Receiver<bool>) {
    let config = match Config::parse(|k| std::env::var(k).ok()) {
        Ok(Some(c)) => c,
        Ok(None) => {
            publish(&pool, &json!({"enabled": false})).await;
            return;
        }
        Err(err) => {
            tracing::error!(%err, "scheduled backups disabled");
            publish(&pool, &json!({"enabled": false, "error": err})).await;
            return;
        }
    };
    let mut last: Option<Value> = None;
    loop {
        let next = config.schedule.next_after(now());
        let mut status = json!({
            "enabled": true,
            "dir": config.dir,
            "schedule": config.schedule_text,
            "keep": config.keep,
            "encrypted": config.passphrase.is_some(),
            "credentials": config.credentials,
            "history": config.history,
            "next_run": next,
            "last": last,
            "files": schedule::list(&config.dir).unwrap_or_default(),
        });
        if let Some(obj) = status.as_object_mut() {
            obj.insert("updated_at".into(), json!(now()));
        }
        publish(&pool, &status).await;
        let Some(next) = next else {
            // Schedule "off": keep the status fresh, nothing to run.
            let _ = stop.wait_for(|s| *s).await;
            return;
        };
        let wait = Duration::from_secs(u64::try_from(next - now()).unwrap_or(0));
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
        let options = RunOptions {
            dir: config.dir.clone(),
            keep: config.keep,
            passphrase: config.passphrase.clone(),
            include_credentials: config.credentials,
            include_history: config.history,
        };
        let started = now();
        last = Some(match schedule::run_once(&pool, started, &options).await {
            Ok(file) => {
                tracing::info!(file = %file.name, bytes = file.bytes, "scheduled backup written");
                json!({"at": started, "ok": true, "file": file})
            }
            Err(err) => {
                tracing::error!(%err, "scheduled backup failed");
                json!({"at": started, "ok": false, "error": err.to_string()})
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| map.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn off_without_a_directory() {
        assert_eq!(Config::parse(env(&[])).expect("ok"), None);
        assert_eq!(
            Config::parse(env(&[("SUPERHET_BACKUP_SCHEDULE", "hourly")])).expect("ok"),
            None
        );
    }

    #[test]
    fn defaults_and_overrides() {
        let c = Config::parse(env(&[("SUPERHET_BACKUP_DIR", "/data/backups")]))
            .expect("ok")
            .expect("on");
        assert_eq!((c.keep, c.schedule), (7, Schedule::Daily { minute: 210 }));
        assert!(!c.credentials && c.passphrase.is_none());
        let c = Config::parse(env(&[
            ("SUPERHET_BACKUP_DIR", "/b"),
            ("SUPERHET_BACKUP_SCHEDULE", "weekly sun 01:00"),
            ("SUPERHET_BACKUP_KEEP", "3"),
            ("SUPERHET_BACKUP_PASSPHRASE", "long enough"),
            ("SUPERHET_BACKUP_CREDENTIALS", "true"),
        ]))
        .expect("ok")
        .expect("on");
        assert_eq!((c.keep, c.credentials), (3, true));
        assert!(c.passphrase.is_some());
    }

    #[test]
    fn bad_values_are_named() {
        for (k, v) in [
            ("SUPERHET_BACKUP_SCHEDULE", "sometimes"),
            ("SUPERHET_BACKUP_KEEP", "0"),
            ("SUPERHET_BACKUP_PASSPHRASE", "short"),
        ] {
            let err =
                Config::parse(env(&[("SUPERHET_BACKUP_DIR", "/b"), (k, v)])).expect_err("refused");
            assert!(err.contains(k), "{err}");
        }
    }
}
