//! Address bans: networks that may not reach `/api/` at all.
//!
//! The gate runs before everything else (login, guest entry, invite preview, setup, the
//! WebSocket upgrades), so a banned address cannot even probe credentials. The active bans are
//! cached in memory (reloaded after each change and at start); a request costs one scan over a
//! short list and no database access. The client address follows `SUPERHET_TRUST_PROXY` like
//! everywhere else. The static web UI stays reachable so a banned visitor sees an explanation
//! instead of a blank page; the health probes are not gated.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{PoisonError, RwLock};

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use serde_json::json;
use superhet_bus::audit::Outcome;
use superhet_db::PgPool;

use super::{AuthState, ClientInfo, json_reply};
use crate::audit::{ActorKind, Who};
use crate::registry::now_ms;

/// Shortest accepted prefix: a ban cannot silence a whole /7 by accident.
pub const MIN_PREFIX_V4: u8 = 8;
/// Shortest accepted IPv6 prefix.
pub const MIN_PREFIX_V6: u8 = 32;
/// Most active bans kept (the scan is linear).
pub const MAX_ACTIVE: usize = 1000;

/// An IP network, normalised: host bits cleared, IPv4-mapped IPv6 addresses folded to IPv4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

/// Fold `::ffff:a.b.c.d` to the IPv4 address (dual-stack sockets report clients that way).
#[must_use]
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

fn mask_v4(addr: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    Ipv4Addr::from(u32::from(addr) & mask)
}

fn mask_v6(addr: Ipv6Addr, prefix: u8) -> Ipv6Addr {
    let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
    Ipv6Addr::from(u128::from(addr) & mask)
}

impl Cidr {
    /// Parse `a.b.c.d`, `a.b.c.d/len`, an IPv6 address or `addr/len`. A bare address is a host
    /// (/32 or /128). Host bits are cleared (`10.1.2.3/8` is `10.0.0.0/8`).
    ///
    /// # Errors
    /// A message for the admin when the text is not an address or the prefix is out of range.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (addr_text, prefix_text) = match text.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (text, None),
        };
        let addr: IpAddr = addr_text
            .parse()
            .map_err(|_| format!("{text:?} is not an IP address or network"))?;
        let mapped = matches!(addr, IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some());
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix: u8 = match prefix_text {
            None => max,
            Some(p) => p
                .parse()
                .map_err(|_| format!("{p:?} is not a prefix length"))?,
        };
        if prefix > max {
            return Err(format!("prefix /{prefix} is out of range"));
        }
        // `::ffff:a.b.c.d/96+n` is the IPv4 network a.b.c.d/n.
        let (addr, prefix) = match canonical_ip(addr) {
            IpAddr::V4(v4) if mapped => {
                if prefix < 96 {
                    return Err("IPv4-mapped networks need a prefix of at least /96".to_owned());
                }
                (IpAddr::V4(v4), prefix - 96)
            }
            other => (other, prefix),
        };
        let min = if addr.is_ipv4() {
            MIN_PREFIX_V4
        } else {
            MIN_PREFIX_V6
        };
        if prefix < min {
            return Err(format!(
                "too broad: networks need a prefix of at least /{min} ({})",
                if addr.is_ipv4() { "IPv4" } else { "IPv6" }
            ));
        }
        let addr = match addr {
            IpAddr::V4(v4) => IpAddr::V4(mask_v4(v4, prefix)),
            IpAddr::V6(v6) => IpAddr::V6(mask_v6(v6, prefix)),
        };
        Ok(Self { addr, prefix })
    }

    /// Whether `ip` lies inside the network.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, canonical_ip(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => mask_v4(ip, self.prefix) == net,
            (IpAddr::V6(net), IpAddr::V6(ip)) => mask_v6(ip, self.prefix) == net,
            _ => false,
        }
    }
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

#[derive(Debug, Clone)]
struct Entry {
    net: Cidr,
    expires_ms: Option<i64>,
}

/// The active bans, shared by the gate and the admin routes.
#[derive(Debug, Default)]
pub struct Bans {
    entries: RwLock<Vec<Entry>>,
}

/// Why a request was turned away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// The matching network.
    pub net: Cidr,
    /// Its deadline, Unix milliseconds.
    pub expires_ms: Option<i64>,
}

impl Bans {
    /// Replace the cache with the bans in force in the database.
    ///
    /// # Errors
    /// Fails if the query fails.
    pub async fn reload(&self, db: &PgPool) -> Result<usize, superhet_db::Error> {
        let bans = superhet_db::bans::list(db, true).await?;
        let mut entries = Vec::with_capacity(bans.len());
        for ban in bans {
            match Cidr::parse_stored(&ban.network) {
                Some(net) => entries.push(Entry {
                    net,
                    expires_ms: ban.expires_at_ms,
                }),
                None => tracing::error!(network = ban.network, "unusable ban row skipped"),
            }
        }
        let n = entries.len();
        *self.entries.write().unwrap_or_else(PoisonError::into_inner) = entries;
        Ok(n)
    }

    /// The first unexpired ban covering `ip`.
    #[must_use]
    pub fn check(&self, ip: IpAddr, now_ms: i64) -> Option<Hit> {
        self.entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|e| e.expires_ms.is_none_or(|t| t > now_ms) && e.net.contains(ip))
            .map(|e| Hit {
                net: e.net,
                expires_ms: e.expires_ms,
            })
    }

    /// Number of bans in the cache (expired ones included until the next reload).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// No bans cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    fn set(&self, entries: &[(&str, Option<i64>)]) {
        *self.entries.write().unwrap_or_else(PoisonError::into_inner) = entries
            .iter()
            .map(|(net, expires_ms)| Entry {
                net: Cidr::parse(net).expect("test network"),
                expires_ms: *expires_ms,
            })
            .collect();
    }
}

impl Cidr {
    /// Parse PostgreSQL's `cidr` text without the minimum-prefix policy (old rows stay usable).
    fn parse_stored(text: &str) -> Option<Self> {
        let (addr, prefix) = text.split_once('/')?;
        let addr: IpAddr = addr.parse().ok()?;
        let prefix: u8 = prefix.parse().ok()?;
        let addr = canonical_ip(addr);
        Some(match addr {
            IpAddr::V4(v4) if prefix <= 32 => Self {
                addr: IpAddr::V4(mask_v4(v4, prefix)),
                prefix,
            },
            IpAddr::V6(v6) if prefix <= 128 => Self {
                addr: IpAddr::V6(mask_v6(v6, prefix)),
                prefix,
            },
            _ => return None,
        })
    }
}

/// Middleware: refuse `/api/` requests from banned addresses before any authentication.
pub(super) async fn gate(State(auth): State<AuthState>, request: Request, next: Next) -> Response {
    if !request.uri().path().starts_with("/api/") || auth.0.bans.is_empty() {
        return next.run(request).await;
    }
    let client = ClientInfo::from_request(
        &auth.0.config,
        request.extensions(),
        request.headers(),
        request.uri(),
    );
    let Some(ip) = client.ip else {
        return next.run(request).await;
    };
    let Some(hit) = auth.0.bans.check(ip, now_ms()) else {
        return next.run(request).await;
    };
    // Bursts from one network collapse into one audit row.
    auth.0.audit.log_coalesced(
        &Who {
            kind: ActorKind::User,
            id: None,
            name: None,
            role: None,
            ip: Some(ip),
            user_agent: client.user_agent,
        },
        "access.banned",
        Some(&hit.net.to_string()),
        Outcome::Denied,
        json!({"path": request.uri().path()}),
    );
    banned_response(hit.expires_ms)
}

fn banned_response(expires_ms: Option<i64>) -> Response {
    json_reply(
        StatusCode::FORBIDDEN,
        &json!({
            "error": "banned",
            "message": "This address is blocked by the administrator.",
            "expires_at_ms": expires_ms,
        }),
    )
}

impl AuthState {
    /// Load the bans in force from the database (call at start and after every change).
    ///
    /// # Errors
    /// Fails if the query fails.
    pub async fn reload_bans(&self) -> Result<usize, superhet_db::Error> {
        match &self.0.db {
            Some(db) => self.0.bans.reload(db).await,
            None => Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("ip")
    }

    #[test]
    fn parses_hosts_and_networks() {
        assert_eq!(
            Cidr::parse("203.0.113.5").expect("host").to_string(),
            "203.0.113.5/32"
        );
        assert_eq!(
            Cidr::parse("203.0.113.77/24").expect("net").to_string(),
            "203.0.113.0/24"
        );
        assert_eq!(
            Cidr::parse("2001:db8:1:2::9/48").expect("v6").to_string(),
            "2001:db8:1::/48"
        );
        assert_eq!(
            Cidr::parse("::ffff:198.51.100.9")
                .expect("mapped")
                .to_string(),
            "198.51.100.9/32"
        );
    }

    #[test]
    fn rejects_junk_and_overbroad_networks() {
        for bad in [
            "",
            "banana",
            "1.2.3.4/33",
            "1.2.3.4/x",
            "0.0.0.0/0",
            "10.0.0.0/7",
            "::/0",
            "2001:db8::/31",
        ] {
            assert!(Cidr::parse(bad).is_err(), "{bad}");
        }
        assert!(Cidr::parse("10.0.0.0/8").is_ok());
    }

    #[test]
    fn containment_folds_mapped_addresses() {
        let net = Cidr::parse("203.0.113.0/24").expect("net");
        assert!(net.contains(ip("203.0.113.200")));
        assert!(net.contains(ip("::ffff:203.0.113.9")));
        assert!(!net.contains(ip("203.0.114.1")));
        assert!(!net.contains(ip("2001:db8::1")));
        let v6 = Cidr::parse("2001:db8:1::/48").expect("v6");
        assert!(v6.contains(ip("2001:db8:1:ffff::1")));
        assert!(!v6.contains(ip("2001:db8:2::1")));
    }

    #[test]
    fn expired_bans_do_not_match() {
        let bans = Bans::default();
        bans.set(&[("203.0.113.5", Some(1_000)), ("198.51.100.0/24", None)]);
        assert!(bans.check(ip("203.0.113.5"), 999).is_some());
        assert!(bans.check(ip("203.0.113.5"), 1_000).is_none());
        assert!(bans.check(ip("198.51.100.77"), i64::MAX).is_some());
        assert!(bans.check(ip("192.0.2.1"), 0).is_none());
    }

    #[test]
    fn stored_text_round_trips() {
        let net = Cidr::parse_stored("203.0.113.0/24").expect("stored");
        assert_eq!(net.to_string(), "203.0.113.0/24");
        assert!(Cidr::parse_stored("garbage").is_none());
        assert!(Cidr::parse_stored("1.2.3.4/40").is_none());
    }
}
