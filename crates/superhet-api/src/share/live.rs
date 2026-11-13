//! Open connections of share links: the per-link, global and per-address caps, and the
//! session keys needed to end a link's connections when it is revoked or changed.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::links::{Kind, Link};

/// Why a connection was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The link's own cap is reached.
    LinkFull,
    /// The server-wide cap for share links is reached.
    ServerFull,
    /// This address holds too many connections.
    TooManyFromAddress,
}

impl Refusal {
    /// A short code for audit details and error bodies.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::LinkFull => "link_full",
            Self::ServerFull => "server_full",
            Self::TooManyFromAddress => "too_many_from_address",
        }
    }
}

/// IPv6 addresses count per /64 so a subscriber cannot rotate through its prefix.
fn addr_key(ip: Option<IpAddr>) -> IpAddr {
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

#[derive(Default)]
struct Inner {
    next: u64,
    global: usize,
    per_link: HashMap<(i64, Kind), usize>,
    per_addr: HashMap<IpAddr, usize>,
    keys: HashMap<i64, HashSet<String>>,
    /// Owners (`s:...`) of the VFOs that live connections hold.
    owners: HashSet<String>,
}

/// The table of open share connections.
pub struct Live {
    inner: Mutex<Inner>,
    global_cap: usize,
    addr_cap: usize,
}

impl Live {
    /// Caps: connections of all share links together, and per client address.
    #[must_use]
    pub fn new(global_cap: usize, addr_cap: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
            global_cap,
            addr_cap,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Admit a connection of `kind` to `link` from `ip`, or say why not. The permit frees the
    /// slot when dropped.
    ///
    /// # Errors
    /// [`Refusal`] naming the cap that is reached.
    pub fn acquire(
        self: &Arc<Self>,
        link: &Link,
        kind: Kind,
        ip: Option<IpAddr>,
    ) -> Result<Permit, Refusal> {
        let addr = addr_key(ip);
        let mut inner = self.lock();
        if inner.global >= self.global_cap {
            return Err(Refusal::ServerFull);
        }
        let in_link = inner.per_link.get(&(link.id, kind)).copied().unwrap_or(0);
        if in_link >= link.max_listeners as usize {
            return Err(Refusal::LinkFull);
        }
        if inner.per_addr.get(&addr).copied().unwrap_or(0) >= self.addr_cap {
            return Err(Refusal::TooManyFromAddress);
        }
        inner.next += 1;
        let key = format!("share:{}:{}", link.id, inner.next);
        inner.global += 1;
        *inner.per_link.entry((link.id, kind)).or_default() += 1;
        *inner.per_addr.entry(addr).or_default() += 1;
        inner.keys.entry(link.id).or_default().insert(key.clone());
        Ok(Permit {
            live: Arc::clone(self),
            link: link.id,
            kind,
            addr,
            key,
        })
    }

    /// Open connections of the given kind on a link (the listener count of the page).
    #[must_use]
    pub fn count(&self, link: i64, kind: Kind) -> usize {
        self.lock()
            .per_link
            .get(&(link, kind))
            .copied()
            .unwrap_or(0)
    }

    /// Session keys of every open connection of a link.
    #[must_use]
    pub fn keys_of(&self, link: i64) -> Vec<String> {
        self.lock()
            .keys
            .get(&link)
            .map(|k| k.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Ids of the links that have open connections.
    #[must_use]
    pub fn links(&self) -> Vec<i64> {
        self.lock().keys.keys().copied().collect()
    }

    /// Remember a VFO owner string while its connection lives.
    pub fn hold_owner(&self, owner: &str) {
        self.lock().owners.insert(owner.to_owned());
    }

    /// Forget a VFO owner string.
    pub fn release_owner(&self, owner: &str) {
        self.lock().owners.remove(owner);
    }

    /// Whether a connection still holds a VFO under this owner string.
    #[must_use]
    pub fn owner_is_live(&self, owner: &str) -> bool {
        self.lock().owners.contains(owner)
    }
}

/// One admitted connection; dropping it frees the slot.
pub struct Permit {
    live: Arc<Live>,
    link: i64,
    kind: Kind,
    addr: IpAddr,
    key: String,
}

impl Permit {
    /// The registry session key of this connection (`share:<link>:<n>`), also used to kick it.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut inner = self.live.lock();
        inner.global = inner.global.saturating_sub(1);
        if let Some(n) = inner.per_link.get_mut(&(self.link, self.kind)) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                inner.per_link.remove(&(self.link, self.kind));
            }
        }
        if let Some(n) = inner.per_addr.get_mut(&self.addr) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                inner.per_addr.remove(&self.addr);
            }
        }
        if let Some(keys) = inner.keys.get_mut(&self.link) {
            keys.remove(&self.key);
            if keys.is_empty() {
                inner.keys.remove(&self.link);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::links::test_link;
    use super::*;

    fn ip(last: u8) -> Option<IpAddr> {
        Some(IpAddr::from([192, 0, 2, last]))
    }

    #[test]
    fn a_link_admits_at_most_max_listeners_per_kind() {
        let live = Live::new(100, 100);
        let link = test_link(1); // max_listeners = 2
        let a = live.acquire(&link, Kind::Audio, ip(1)).expect("first");
        let _b = live.acquire(&link, Kind::Audio, ip(2)).expect("second");
        assert_eq!(
            live.acquire(&link, Kind::Audio, ip(3)).err(),
            Some(Refusal::LinkFull)
        );
        // Another kind has its own pool, another link its own too.
        assert!(live.acquire(&link, Kind::Spectrum, ip(3)).is_ok());
        assert!(live.acquire(&test_link(2), Kind::Audio, ip(3)).is_ok());
        assert_eq!(live.count(1, Kind::Audio), 2);
        drop(a);
        assert_eq!(live.count(1, Kind::Audio), 1);
        assert!(live.acquire(&link, Kind::Audio, ip(3)).is_ok());
    }

    #[test]
    fn the_global_and_address_caps_apply_across_links() {
        let live = Live::new(3, 2);
        let (l1, l2) = (test_link(1), test_link(2));
        let _a = live.acquire(&l1, Kind::Audio, ip(1)).expect("a");
        let _b = live.acquire(&l2, Kind::Audio, ip(1)).expect("b");
        assert_eq!(
            live.acquire(&l1, Kind::Spectrum, ip(1)).err(),
            Some(Refusal::TooManyFromAddress)
        );
        let _c = live.acquire(&l1, Kind::Spectrum, ip(2)).expect("c");
        assert_eq!(
            live.acquire(&l2, Kind::Spectrum, ip(3)).err(),
            Some(Refusal::ServerFull)
        );
    }

    #[test]
    fn ipv6_addresses_share_their_64_bit_prefix() {
        let live = Live::new(100, 1);
        let link = test_link(1);
        let a: IpAddr = "2001:db8::1".parse().expect("ip");
        let b: IpAddr = "2001:db8::2".parse().expect("ip");
        let _p = live.acquire(&link, Kind::Audio, Some(a)).expect("first");
        assert_eq!(
            live.acquire(&link, Kind::Spectrum, Some(b)).err(),
            Some(Refusal::TooManyFromAddress)
        );
    }

    #[test]
    fn session_keys_are_listed_until_the_permit_is_dropped() {
        let live = Live::new(100, 100);
        let link = test_link(4);
        let permit = live.acquire(&link, Kind::Audio, ip(1)).expect("permit");
        assert_eq!(live.keys_of(4), vec![permit.key().to_owned()]);
        assert_eq!(live.links(), vec![4]);
        drop(permit);
        assert!(live.keys_of(4).is_empty());
        assert!(live.links().is_empty());
    }
}
