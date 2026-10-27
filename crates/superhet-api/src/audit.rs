//! Writing the audit log (ADR-0012, docs/api-audit.md).
//!
//! The API writes the events of accounts and guests itself, straight into `audit_events`
//! (system events arrive over NATS and are written by the worker). Failures to write are logged
//! loudly but never fail the request that caused them. Bursts of the same action by the same
//! actor (a tuning drag) are coalesced into one row carrying the final values.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Map, Value, json};
use superhet_bus::audit::Outcome;
use superhet_db::{PgPool, audit::NewEvent, users::Role};

use crate::settings::{IpMode, SettingsStore};

/// How long a burst of one action is held back to be merged into a single row.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(1500);

/// Kind of actor, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorKind {
    /// A logged-in account (or somebody trying to log in).
    User,
    /// An anonymous guest session.
    Guest,
}

impl ActorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Guest => "guest",
        }
    }
}

/// Who did it, as far as it is known.
#[derive(Debug, Clone)]
pub struct Who {
    /// Account or guest.
    pub kind: ActorKind,
    /// User id or guest id; `None` for a failed login of an unknown name.
    pub id: Option<String>,
    /// Login name (or the name that was tried).
    pub name: Option<String>,
    /// Role at the time.
    pub role: Option<Role>,
    /// Client address as the API resolved it (truncated before it is stored).
    pub ip: Option<IpAddr>,
    /// TCP peer address (truncated before it is stored).
    pub peer: Option<IpAddr>,
    /// Forwarded chain of a trusted proxy (kept in the details, truncated like the address).
    pub via: Vec<IpAddr>,
    /// Truncated user agent.
    pub user_agent: Option<String>,
}

/// Apply the address mode: full, IPv4 /24 and IPv6 /48 (IPv4-mapped IPv6 counts as IPv4), or
/// nothing.
#[must_use]
pub fn store_ip(ip: Option<IpAddr>, mode: IpMode) -> Option<String> {
    let ip = ip?;
    if mode == IpMode::None {
        return None;
    }
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    if mode == IpMode::Full {
        return Some(ip.to_string());
    }
    Some(match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.0", o[0], o[1], o[2])
        }
        IpAddr::V6(v6) => {
            let mut o = v6.octets();
            o[6..].fill(0);
            std::net::Ipv6Addr::from(o).to_string()
        }
    })
}

struct Pending {
    who: Who,
    action: String,
    target: Option<String>,
    outcome: Outcome,
    details: Map<String, Value>,
    count: u32,
}

struct Inner {
    db: Option<PgPool>,
    settings: Arc<SettingsStore>,
    pending: Mutex<HashMap<String, Pending>>,
}

/// Handle to the audit writer (cheap to clone).
#[derive(Clone)]
pub struct Audit(Arc<Inner>);

fn object(details: Value) -> Map<String, Value> {
    match details {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        other => {
            let mut map = Map::new();
            map.insert("value".into(), other);
            map
        }
    }
}

impl Audit {
    /// A writer over `db`; without one events are only logged.
    #[must_use]
    pub fn new(db: Option<PgPool>, settings: Arc<SettingsStore>) -> Self {
        Self(Arc::new(Inner {
            db,
            settings,
            pending: Mutex::new(HashMap::new()),
        }))
    }

    /// Record an event now.
    pub async fn log(
        &self,
        who: &Who,
        action: &str,
        target: Option<&str>,
        outcome: Outcome,
        details: Value,
    ) {
        write(
            &self.0,
            who,
            action,
            target.map(str::to_owned),
            outcome,
            object(details),
        )
        .await;
    }

    /// Record an event, merging it into a pending one of the same actor, action and target
    /// (later values win). The row is written [`COALESCE_WINDOW`] after the first event.
    pub fn log_coalesced(
        &self,
        who: &Who,
        action: &str,
        target: Option<&str>,
        outcome: Outcome,
        details: Value,
    ) {
        let key = format!(
            "{}|{}|{action}|{}",
            who.kind.as_str(),
            who.id.as_deref().unwrap_or("-"),
            target.unwrap_or("-")
        );
        let details = object(details);
        let first = {
            let mut pending = self
                .0
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(p) = pending.get_mut(&key) {
                p.details.extend(details);
                p.outcome = outcome;
                p.count += 1;
                false
            } else {
                pending.insert(
                    key.clone(),
                    Pending {
                        who: who.clone(),
                        action: action.to_owned(),
                        target: target.map(str::to_owned),
                        outcome,
                        details,
                        count: 1,
                    },
                );
                true
            }
        };
        if !first {
            return;
        }
        let inner = Arc::clone(&self.0);
        let spawned = tokio::runtime::Handle::try_current().map(|handle| {
            handle.spawn(async move {
                tokio::time::sleep(COALESCE_WINDOW).await;
                let taken = inner
                    .pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&key);
                if let Some(mut p) = taken {
                    if p.count > 1 {
                        p.details.insert("merged".into(), json!(p.count));
                    }
                    write(&inner, &p.who, &p.action, p.target, p.outcome, p.details).await;
                }
            })
        });
        if spawned.is_err() {
            tracing::warn!(action, "audit event dropped: no async runtime");
        }
    }

    /// Record an event from a context that cannot await (a drop handler).
    pub fn log_detached(
        &self,
        who: Who,
        action: &'static str,
        target: Option<String>,
        outcome: Outcome,
        details: Value,
    ) {
        let inner = Arc::clone(&self.0);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                write(&inner, &who, action, target, outcome, object(details)).await;
            });
        }
    }
}

async fn write(
    inner: &Inner,
    who: &Who,
    action: &str,
    target: Option<String>,
    outcome: Outcome,
    details: Map<String, Value>,
) {
    let settings = inner.settings.audit().await;
    let mode = match who.kind {
        ActorKind::User => settings.ip_mode_user,
        ActorKind::Guest => settings.ip_mode_guest,
    };
    let mut details = details;
    if !who.via.is_empty() && mode != IpMode::None && !details.contains_key("forwarded") {
        let chain: Vec<String> = who
            .via
            .iter()
            .filter_map(|ip| store_ip(Some(*ip), mode))
            .collect();
        details.insert("forwarded".into(), json!(chain));
    }
    let event = NewEvent {
        ts_ms: None,
        actor_kind: who.kind.as_str().to_owned(),
        actor_id: who.id.clone(),
        actor_name: who.name.clone(),
        role: who.role.map(|r| r.as_str().to_owned()),
        ip: store_ip(who.ip, mode),
        peer_ip: store_ip(who.peer, mode),
        user_agent: who.user_agent.clone(),
        action: action.to_owned(),
        target: target.map(|t| t.chars().take(256).collect()),
        outcome: outcome.as_str().to_owned(),
        details: Value::Object(details),
    };
    tracing::debug!(action, outcome = event.outcome, "audit");
    let Some(db) = &inner.db else {
        return;
    };
    if let Err(err) = superhet_db::audit::insert(db, &event).await {
        tracing::error!(%err, action, "could not write the audit event");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Option<IpAddr> {
        s.parse().ok()
    }

    #[test]
    fn ip_modes() {
        assert_eq!(
            store_ip(ip("203.0.113.77"), IpMode::Full).as_deref(),
            Some("203.0.113.77")
        );
        assert_eq!(
            store_ip(ip("203.0.113.77"), IpMode::Truncated).as_deref(),
            Some("203.0.113.0")
        );
        assert_eq!(store_ip(ip("203.0.113.77"), IpMode::None), None);
        assert_eq!(store_ip(None, IpMode::Full), None);
        assert_eq!(
            store_ip(
                ip("2001:db8:abcd:1234:5678:9abc:def0:1234"),
                IpMode::Truncated
            )
            .as_deref(),
            Some("2001:db8:abcd::")
        );
        assert_eq!(
            store_ip(ip("2001:db8:abcd:1234::1"), IpMode::Full).as_deref(),
            Some("2001:db8:abcd:1234::1")
        );
        // An IPv4 client behind a dual-stack socket is truncated as IPv4.
        assert_eq!(
            store_ip(ip("::ffff:198.51.100.9"), IpMode::Truncated).as_deref(),
            Some("198.51.100.0")
        );
    }

    #[test]
    fn details_become_objects() {
        assert_eq!(object(json!({"a": 1})).len(), 1);
        assert!(object(Value::Null).is_empty());
        assert_eq!(object(json!(5))["value"], 5);
    }
}
