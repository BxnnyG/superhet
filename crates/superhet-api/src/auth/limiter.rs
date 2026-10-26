//! In-memory limiter for failed authentication attempts (per client address and global).
//!
//! State is lost on restart, which is acceptable: an attacker gains at most one extra window
//! of attempts per restart of a service they cannot trigger.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Limiter thresholds.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Sliding window in which failures are counted.
    pub window: Duration,
    /// Failures per client address within the window before it is blocked.
    pub per_ip: usize,
    /// Failures from all clients within the window before everyone is blocked.
    pub global: usize,
    /// Failed logins for one user name (from any address) within the window before that name
    /// is locked for logins; unknown names count too, so this does not reveal which exist.
    pub per_name: usize,
    /// Upper bound on tracked client addresses (memory cap).
    pub max_tracked_ips: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(15 * 60),
            per_ip: 5,
            global: 50,
            per_name: 10,
            max_tracked_ips: 10_000,
        }
    }
}

#[derive(Default)]
struct State {
    per_ip: HashMap<IpAddr, VecDeque<Instant>>,
    global: VecDeque<Instant>,
}

/// Sliding-window failure limiter.
pub struct Limiter {
    limits: Limits,
    state: Mutex<State>,
}

/// Collapse IPv6 addresses to their /64 so one subscriber cannot rotate through its prefix.
fn key(ip: Option<IpAddr>) -> IpAddr {
    match ip {
        Some(IpAddr::V6(v6)) => {
            let mut octets = v6.octets();
            octets[8..].fill(0);
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        Some(v4) => v4,
        None => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    }
}

fn expire(queue: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while queue
        .front()
        .is_some_and(|t| now.saturating_duration_since(*t) >= window)
    {
        queue.pop_front();
    }
}

fn retry_after(queue: &VecDeque<Instant>, now: Instant, window: Duration) -> Duration {
    queue.front().map_or(Duration::ZERO, |oldest| {
        (*oldest + window).saturating_duration_since(now)
    })
}

impl Limiter {
    /// Limiter with the given thresholds.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `Ok` if an attempt may proceed, otherwise the time until it may be retried.
    pub fn check(&self, ip: Option<IpAddr>, now: Instant) -> Result<(), Duration> {
        let window = self.limits.window;
        let mut state = self.lock();
        let mut wait = Duration::ZERO;
        expire(&mut state.global, now, window);
        if state.global.len() >= self.limits.global {
            wait = wait.max(retry_after(&state.global, now, window));
        }
        let k = key(ip);
        if let Some(queue) = state.per_ip.get_mut(&k) {
            expire(queue, now, window);
            if queue.len() >= self.limits.per_ip {
                wait = wait.max(retry_after(queue, now, window));
            }
            if queue.is_empty() {
                state.per_ip.remove(&k);
            }
        }
        if wait.is_zero() { Ok(()) } else { Err(wait) }
    }

    /// Count a failed attempt.
    pub fn record_failure(&self, ip: Option<IpAddr>, now: Instant) {
        let window = self.limits.window;
        let mut state = self.lock();
        expire(&mut state.global, now, window);
        state.global.push_back(now);
        let k = key(ip);
        if !state.per_ip.contains_key(&k) && state.per_ip.len() >= self.limits.max_tracked_ips {
            state.per_ip.retain(|_, queue| {
                expire(queue, now, window);
                !queue.is_empty()
            });
            if state.per_ip.len() >= self.limits.max_tracked_ips {
                return; // table full of live entries: only the global counter applies
            }
        }
        state.per_ip.entry(k).or_default().push_back(now);
    }

    /// Forget the failures of one client after a successful authentication.
    pub fn record_success(&self, ip: Option<IpAddr>) {
        self.lock().per_ip.remove(&key(ip));
    }
}

/// Failure counter per (case-insensitive) user name. Slows down guessing against one account
/// from many addresses, which the per-address limiter cannot see.
pub struct NameLimiter {
    window: Duration,
    max: usize,
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
}

/// Upper bound on tracked names (memory cap against random-name floods).
const MAX_TRACKED_NAMES: usize = 10_000;

impl NameLimiter {
    /// A limiter allowing `max` failures per name within `window`.
    #[must_use]
    pub fn new(window: Duration, max: usize) -> Self {
        Self {
            window,
            max,
            failures: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, VecDeque<Instant>>> {
        self.failures.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `Ok` if a login for `name` may proceed, otherwise the time until it may.
    pub fn check(&self, name: &str, now: Instant) -> Result<(), Duration> {
        let mut map = self.lock();
        let key = name.to_lowercase();
        let Some(queue) = map.get_mut(&key) else {
            return Ok(());
        };
        expire(queue, now, self.window);
        let wait = if queue.len() >= self.max {
            retry_after(queue, now, self.window)
        } else {
            Duration::ZERO
        };
        if queue.is_empty() {
            map.remove(&key);
        }
        if wait.is_zero() { Ok(()) } else { Err(wait) }
    }

    /// Count a failed login for `name`.
    pub fn record_failure(&self, name: &str, now: Instant) {
        let mut map = self.lock();
        if map.len() >= MAX_TRACKED_NAMES && !map.contains_key(&name.to_lowercase()) {
            let window = self.window;
            map.retain(|_, q| {
                expire(q, now, window);
                !q.is_empty()
            });
            if map.len() >= MAX_TRACKED_NAMES {
                return;
            }
        }
        map.entry(name.to_lowercase()).or_default().push_back(now);
    }

    /// Forget the failures of `name` after a successful login.
    pub fn record_success(&self, name: &str) {
        self.lock().remove(&name.to_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_throttled_across_addresses_and_case() {
        let l = NameLimiter::new(Duration::from_secs(60), 3);
        let now = Instant::now();
        assert!(l.check("Alice", now).is_ok());
        for _ in 0..3 {
            l.record_failure("alice", now);
        }
        assert!(l.check("ALICE", now).is_err());
        assert!(l.check("bob", now).is_ok());
        assert!(
            l.check("alice", now + Duration::from_secs(61)).is_ok(),
            "window passed"
        );
        l.record_success("alice");
        assert!(l.check("alice", now).is_ok());
    }

    fn small() -> Limiter {
        Limiter::new(Limits {
            window: Duration::from_secs(60),
            per_ip: 3,
            global: 5,
            per_name: 10,
            max_tracked_ips: 2,
        })
    }

    fn ip(last: u8) -> Option<IpAddr> {
        Some(IpAddr::from([10, 0, 0, last]))
    }

    #[test]
    fn blocks_after_threshold_and_reports_retry_after() {
        let l = small();
        let t0 = Instant::now();
        for i in 0..3 {
            assert!(l.check(ip(1), t0).is_ok());
            l.record_failure(ip(1), t0 + Duration::from_secs(i));
        }
        let wait = l
            .check(ip(1), t0 + Duration::from_secs(10))
            .expect_err("blocked");
        assert_eq!(wait, Duration::from_secs(50)); // oldest failure at t0 leaves at +60
        assert!(l.check(ip(2), t0 + Duration::from_secs(10)).is_ok());
    }

    #[test]
    fn window_slides() {
        let l = small();
        let t0 = Instant::now();
        for _ in 0..3 {
            l.record_failure(ip(1), t0);
        }
        assert!(l.check(ip(1), t0 + Duration::from_secs(59)).is_err());
        assert!(l.check(ip(1), t0 + Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn success_clears_the_client() {
        let l = small();
        let t0 = Instant::now();
        for _ in 0..3 {
            l.record_failure(ip(1), t0);
        }
        l.record_success(ip(1));
        assert!(l.check(ip(1), t0).is_ok());
    }

    #[test]
    fn global_limit_blocks_everyone() {
        let l = Limiter::new(Limits {
            max_tracked_ips: 100,
            ..small().limits
        });
        let t0 = Instant::now();
        for i in 0..5 {
            l.record_failure(ip(i), t0);
        }
        assert!(l.check(ip(200), t0).is_err());
        assert!(l.check(ip(200), t0 + Duration::from_secs(61)).is_ok());
    }

    #[test]
    fn ipv6_is_tracked_per_64_prefix() {
        let l = small();
        let t0 = Instant::now();
        let a: IpAddr = "2001:db8:1:2::1".parse().expect("ip");
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().expect("ip");
        let other: IpAddr = "2001:db8:1:3::1".parse().expect("ip");
        for _ in 0..3 {
            l.record_failure(Some(a), t0);
        }
        assert!(l.check(Some(b), t0).is_err());
        assert!(l.check(Some(other), t0).is_ok());
    }

    #[test]
    fn memory_is_capped() {
        let l = small(); // max 2 tracked addresses
        let t0 = Instant::now();
        for i in 0..10 {
            l.record_failure(ip(i), t0);
        }
        assert!(l.lock().per_ip.len() <= 2);
        assert_eq!(l.lock().global.len(), 10);
    }
}
