//! Live state of the API process: guest sessions, open WebSocket connections per session and
//! the kill switch behind "kick" (ADR-0011/0012). Everything here is in memory; a restart of
//! the API drops guests and connections, accounts stay logged in (their sessions are in the
//! database).

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use superhet_bus::audit::Outcome;
use tokio::sync::watch;

use crate::audit::{Audit, Who};
use crate::settings::GuestPolicy;

/// Why a guest session could not be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestRefusal {
    /// `max_guests` reached.
    Full,
    /// `max_guests_per_ip` reached for this address.
    TooManyFromAddress,
}

/// An anonymous guest session.
#[derive(Debug, Clone)]
pub struct GuestSession {
    /// Public id (shown in audit rows and owner strings), 12 hex characters of the key hash.
    pub id: String,
    /// Client address at start (resolved behind a trusted proxy).
    pub ip: Option<IpAddr>,
    /// TCP peer address at start.
    pub peer: Option<IpAddr>,
    /// Truncated user agent.
    pub user_agent: Option<String>,
    /// Start, Unix milliseconds.
    pub created_ms: i64,
    last_active: Instant,
}

struct SessionConns {
    count: usize,
    since: Instant,
    who: Who,
    kill: watch::Sender<bool>,
}

struct Conn {
    session_key: String,
    kind: &'static str,
    device: String,
    since_ms: i64,
}

#[derive(Default)]
struct Inner {
    guests: HashMap<[u8; 32], GuestSession>,
    sessions: HashMap<String, SessionConns>,
    conns: HashMap<u64, Conn>,
    next_conn: u64,
    devices: BTreeSet<String>,
    /// Owner strings (`g:<id>`) of ended guests whose VFOs still have to be deleted.
    cleanups: Vec<String>,
}

impl Inner {
    fn schedule_cleanup(&mut self, guest_id: &str) {
        self.cleanups.push(format!("g:{guest_id}"));
    }

    /// Session keys of guests that hold at least one open WebSocket. Such a guest is not idle: a
    /// visitor who just listens keeps the sockets open and sends no requests.
    fn connected_guests(&self) -> std::collections::HashSet<String> {
        self.sessions
            .keys()
            .filter(|k| k.starts_with("g-"))
            .cloned()
            .collect()
    }
}

struct Shared {
    inner: Mutex<Inner>,
    audit: Audit,
    /// Serialises "count my VFOs, then create one" so concurrent requests cannot exceed a limit.
    vfo_gate: tokio::sync::Mutex<()>,
}

/// Handle to the registry (cheap to clone).
#[derive(Clone)]
pub struct Registry(Arc<Shared>);

/// Milliseconds since the Unix epoch.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// One open connection as shown to admins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnInfo {
    /// `spectrum`, `audio`, `state` or `events`.
    pub kind: &'static str,
    /// Device id.
    pub device: String,
    /// Opened, Unix milliseconds.
    pub since_ms: i64,
}

/// An open connection; dropping it unregisters it (and, for the last one of a session, writes
/// the `session.disconnect` event). `killed()` resolves when an admin kicks the session.
pub struct ConnGuard {
    id: u64,
    registry: Registry,
    kill: watch::Receiver<bool>,
}

impl ConnGuard {
    /// Resolves once the session was kicked or ended. Cancel-safe.
    pub async fn killed(&mut self) {
        // A dropped sender means the session is gone as well.
        let _ = self.kill.wait_for(|killed| *killed).await;
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.registry.disconnect(self.id);
    }
}

impl Registry {
    /// An empty registry writing connect/disconnect events through `audit`.
    #[must_use]
    pub fn new(audit: Audit) -> Self {
        Self(Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
            audit,
            vfo_gate: tokio::sync::Mutex::new(()),
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The gate that serialises VFO creation (held across state read and create).
    pub async fn vfo_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.0.vfo_gate.lock().await
    }

    /// Remember a device id that clients talked to (for VFO cleanup and session views).
    pub fn note_device(&self, device: &str) {
        self.lock().devices.insert(device.to_owned());
    }

    /// Device ids seen so far.
    #[must_use]
    pub fn devices(&self) -> Vec<String> {
        self.lock().devices.iter().cloned().collect()
    }

    // ---- guests ------------------------------------------------------------------------------

    /// Start a guest session under `key` (the hash of the cookie token). Expired guests are
    /// dropped first so they do not count against the limits.
    ///
    /// # Errors
    /// [`GuestRefusal`] when a limit is reached.
    pub fn guest_start(
        &self,
        key: [u8; 32],
        ip: Option<IpAddr>,
        peer: Option<IpAddr>,
        user_agent: Option<String>,
        policy: &GuestPolicy,
    ) -> Result<GuestSession, GuestRefusal> {
        let idle = Duration::from_secs(policy.idle_timeout_secs);
        let mut inner = self.lock();
        let mut gone = Vec::new();
        let connected = inner.connected_guests();
        inner.guests.retain(|_, g| {
            if connected.contains(&guest_key(&g.id)) {
                g.last_active = Instant::now();
            }
            let keep = g.last_active.elapsed() < idle;
            if !keep {
                gone.push(g.id.clone());
            }
            keep
        });
        for id in &gone {
            inner.schedule_cleanup(id);
        }
        if inner.guests.len() >= policy.max_guests as usize {
            return Err(GuestRefusal::Full);
        }
        let same_address = inner.guests.values().filter(|g| g.ip == ip).count();
        if same_address >= policy.max_guests_per_ip as usize {
            return Err(GuestRefusal::TooManyFromAddress);
        }
        let session = GuestSession {
            id: key[..6].iter().map(|b| format!("{b:02x}")).collect(),
            ip,
            peer,
            user_agent,
            created_ms: now_ms(),
            last_active: Instant::now(),
        };
        inner.guests.insert(key, session.clone());
        Ok(session)
    }

    /// The guest behind `key`, refreshing its activity; `None` (and the session dropped) when it
    /// is unknown or idle for longer than `idle`.
    pub fn guest_touch(&self, key: &[u8; 32], idle: Duration) -> Option<GuestSession> {
        let mut inner = self.lock();
        let connected = inner.connected_guests();
        if let Some(g) = inner.guests.get_mut(key)
            && connected.contains(&guest_key(&g.id))
        {
            g.last_active = Instant::now();
        }
        let expired = inner
            .guests
            .get(key)
            .is_some_and(|g| g.last_active.elapsed() >= idle);
        if expired {
            let gone = inner.guests.remove(key);
            if let Some(g) = &gone {
                inner.schedule_cleanup(&g.id);
            }
            drop(inner);
            if let Some(g) = gone {
                self.kick(&guest_key(&g.id));
            }
            return None;
        }
        let guest = inner.guests.get_mut(key)?;
        guest.last_active = Instant::now();
        Some(guest.clone())
    }

    /// End one guest session and close its sockets.
    pub fn guest_remove(&self, key: &[u8; 32]) -> Option<GuestSession> {
        let gone = {
            let mut inner = self.lock();
            let gone = inner.guests.remove(key);
            if let Some(g) = &gone {
                inner.schedule_cleanup(&g.id);
            }
            gone
        };
        if let Some(g) = &gone {
            self.kick(&guest_key(&g.id));
        }
        gone
    }

    /// End a guest session by its public id.
    pub fn guest_remove_by_id(&self, id: &str) -> Option<GuestSession> {
        let key = self
            .lock()
            .guests
            .iter()
            .find(|(_, g)| g.id == id)
            .map(|(k, _)| *k)?;
        self.guest_remove(&key)
    }

    /// End every guest session (guest mode switched off).
    pub fn guests_clear(&self) -> Vec<GuestSession> {
        let all: Vec<GuestSession> = {
            let mut inner = self.lock();
            let all: Vec<GuestSession> = inner.guests.drain().map(|(_, g)| g).collect();
            for g in &all {
                inner.schedule_cleanup(&g.id);
            }
            all
        };
        for g in &all {
            self.kick(&guest_key(&g.id));
        }
        all
    }

    /// Drop and return the guests idle for longer than `idle`.
    pub fn guests_expire(&self, idle: Duration) -> Vec<GuestSession> {
        let mut gone = Vec::new();
        {
            let mut inner = self.lock();
            let connected = inner.connected_guests();
            inner.guests.retain(|_, g| {
                if connected.contains(&guest_key(&g.id)) {
                    g.last_active = Instant::now();
                }
                if g.last_active.elapsed() >= idle {
                    gone.push(g.clone());
                    false
                } else {
                    true
                }
            });
            for g in &gone {
                inner.schedule_cleanup(&g.id);
            }
        }
        for g in &gone {
            self.kick(&guest_key(&g.id));
        }
        gone
    }

    /// Owners (`g:<id>`) of guests that ended since the last call; their VFOs must be deleted.
    pub fn take_cleanups(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().cleanups)
    }

    /// The live guests, oldest first, with seconds since their last request.
    pub fn guests(&self) -> Vec<(GuestSession, u64)> {
        let mut all: Vec<(GuestSession, u64)> = self
            .lock()
            .guests
            .values()
            .map(|g| (g.clone(), g.last_active.elapsed().as_secs()))
            .collect();
        all.sort_by_key(|(g, _)| g.created_ms);
        all
    }

    // ---- connections -------------------------------------------------------------------------

    /// Register an open WebSocket of the session `session_key`. The first connection of a
    /// session writes `session.connect`.
    pub async fn connect(
        &self,
        session_key: &str,
        who: &Who,
        kind: &'static str,
        device: &str,
    ) -> ConnGuard {
        let (id, kill, first) = {
            let mut inner = self.lock();
            inner.next_conn += 1;
            let id = inner.next_conn;
            inner.devices.insert(device.to_owned());
            inner.conns.insert(
                id,
                Conn {
                    session_key: session_key.to_owned(),
                    kind,
                    device: device.to_owned(),
                    since_ms: now_ms(),
                },
            );
            let entry = inner
                .sessions
                .entry(session_key.to_owned())
                .or_insert_with(|| SessionConns {
                    count: 0,
                    since: Instant::now(),
                    who: who.clone(),
                    kill: watch::channel(false).0,
                });
            entry.count += 1;
            (id, entry.kill.subscribe(), entry.count == 1)
        };
        if first {
            self.0
                .audit
                .log(
                    who,
                    "session.connect",
                    None,
                    Outcome::Ok,
                    json!({"kind": kind}),
                )
                .await;
        }
        ConnGuard {
            id,
            registry: self.clone(),
            kill,
        }
    }

    fn disconnect(&self, id: u64) {
        let ended = {
            let mut inner = self.lock();
            let Some(conn) = inner.conns.remove(&id) else {
                return;
            };
            let last = inner
                .sessions
                .get_mut(&conn.session_key)
                .map(|s| {
                    s.count = s.count.saturating_sub(1);
                    s.count == 0
                })
                .unwrap_or(false);
            if last {
                inner.sessions.remove(&conn.session_key)
            } else {
                None
            }
        };
        if let Some(session) = ended {
            self.0.audit.log_detached(
                session.who,
                "session.disconnect",
                None,
                Outcome::Ok,
                json!({"duration_secs": session.since.elapsed().as_secs()}),
            );
        }
    }

    /// Close every socket of a session (they notice through [`ConnGuard::killed`]).
    pub fn kick(&self, session_key: &str) {
        if let Some(session) = self.lock().sessions.get(session_key) {
            let _ = session.kill.send(true);
        }
    }

    /// The open connections of a session.
    #[must_use]
    pub fn connections_of(&self, session_key: &str) -> Vec<ConnInfo> {
        let inner = self.lock();
        let mut list: Vec<(u64, ConnInfo)> = inner
            .conns
            .iter()
            .filter(|(_, c)| c.session_key == session_key)
            .map(|(id, c)| {
                (
                    *id,
                    ConnInfo {
                        kind: c.kind,
                        device: c.device.clone(),
                        since_ms: c.since_ms,
                    },
                )
            })
            .collect();
        list.sort_by_key(|(id, _)| *id);
        list.into_iter().map(|(_, info)| info).collect()
    }
}

/// The session key of a guest.
#[must_use]
pub fn guest_key(id: &str) -> String {
    format!("g-{id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingsStore;

    fn registry() -> Registry {
        let settings = Arc::new(SettingsStore::new(None));
        Registry::new(Audit::new(None, settings))
    }

    fn policy(max: u32, per_ip: u32) -> GuestPolicy {
        GuestPolicy {
            enabled: true,
            max_guests: max,
            max_guests_per_ip: per_ip,
            ..GuestPolicy::default()
        }
    }

    fn who() -> Who {
        Who {
            kind: crate::audit::ActorKind::Guest,
            id: Some("abc".into()),
            name: None,
            role: None,
            ip: None,
            peer: None,
            via: Vec::new(),
            user_agent: None,
        }
    }

    #[test]
    fn guest_limits_total_and_per_address() {
        let r = registry();
        let a: Option<IpAddr> = "203.0.113.1".parse().ok();
        let b: Option<IpAddr> = "203.0.113.2".parse().ok();
        let p = policy(3, 2);
        r.guest_start([1; 32], a, None, None, &p).expect("first");
        r.guest_start([2; 32], a, None, None, &p).expect("second");
        assert_eq!(
            r.guest_start([3; 32], a, None, None, &p).err(),
            Some(GuestRefusal::TooManyFromAddress)
        );
        r.guest_start([4; 32], b, None, None, &p)
            .expect("other address");
        assert_eq!(
            r.guest_start([5; 32], b, None, None, &p).err(),
            Some(GuestRefusal::Full)
        );
        assert_eq!(r.guests().len(), 3);
        assert!(r.guest_remove(&[1; 32]).is_some());
        r.guest_start([6; 32], b, None, None, &p)
            .expect("room again");
    }

    #[test]
    fn idle_guests_expire() {
        let r = registry();
        r.guest_start([1; 32], None, None, None, &policy(5, 5))
            .expect("start");
        assert!(r.guest_touch(&[1; 32], Duration::from_secs(60)).is_some());
        assert!(r.guest_touch(&[9; 32], Duration::from_secs(60)).is_none());
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(r.guests_expire(Duration::from_secs(60)).len(), 0);
        assert!(
            r.guest_touch(&[1; 32], Duration::from_millis(10)).is_none(),
            "idle too long"
        );
        assert!(r.guests().is_empty());
        r.guest_start([2; 32], None, None, None, &policy(5, 5))
            .expect("start");
        r.guest_start([3; 32], None, None, None, &policy(5, 5))
            .expect("start");
        assert_eq!(r.guests_clear().len(), 2);
        let owners = r.take_cleanups();
        assert_eq!(
            owners.len(),
            3,
            "the idle one and the two cleared ones: {owners:?}"
        );
        assert!(owners.iter().all(|o| o.starts_with("g:")));
        assert!(r.take_cleanups().is_empty());
    }

    #[tokio::test]
    async fn a_guest_with_an_open_socket_is_not_idle() {
        let r = registry();
        let guest = r
            .guest_start([1; 32], None, None, None, &policy(5, 5))
            .expect("start");
        let key = guest_key(&guest.id);
        let conn = r.connect(&key, &who(), "audio", "dev0").await;
        std::thread::sleep(Duration::from_millis(30));
        assert!(r.guests_expire(Duration::from_millis(10)).is_empty());
        assert!(r.guest_touch(&[1; 32], Duration::from_millis(10)).is_some());
        drop(conn);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            r.guests_expire(Duration::from_millis(10)).len(),
            1,
            "idle clock runs once the socket is gone"
        );
    }

    #[tokio::test]
    async fn kick_reaches_every_socket_of_the_session_only() {
        let r = registry();
        let mut a1 = r.connect("s-a", &who(), "spectrum", "dev0").await;
        let mut a2 = r.connect("s-a", &who(), "audio", "dev0").await;
        let mut b = r.connect("s-b", &who(), "state", "dev0").await;
        assert_eq!(
            r.connections_of("s-a")
                .iter()
                .map(|c| c.kind)
                .collect::<Vec<_>>(),
            ["spectrum", "audio"]
        );
        r.kick("s-a");
        tokio::time::timeout(Duration::from_secs(1), a1.killed())
            .await
            .expect("a1 killed");
        tokio::time::timeout(Duration::from_secs(1), a2.killed())
            .await
            .expect("a2 killed");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), b.killed())
                .await
                .is_err()
        );
        drop((a1, a2));
        assert!(r.connections_of("s-a").is_empty());
        // A new connection after the kick is not born dead.
        let mut again = r.connect("s-a", &who(), "audio", "dev0").await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), again.killed())
                .await
                .is_err()
        );
        assert_eq!(r.devices(), ["dev0"]);
    }
}
