//! Reverse DNS (PTR) lookups for the admin views: asynchronous, cached, time-limited.
//!
//! A tiny stub resolver over UDP against the nameservers of `/etc/resolv.conf` (no dependency, no
//! `unsafe`). Lookups never block a request for long: a batch waits at most [`BATCH_WAIT`] and
//! whatever is still running finishes in the background and is served from the cache on the next
//! refresh. Failures are cached briefly so a dead resolver is not hammered.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::Semaphore;

/// How long one query waits for an answer.
const QUERY_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long a list request waits for lookups it started.
const BATCH_WAIT: Duration = Duration::from_millis(600);
/// Cache lifetime of a name.
const TTL_FOUND: Duration = Duration::from_secs(3600);
/// Cache lifetime of "no PTR record".
const TTL_NONE: Duration = Duration::from_secs(900);
/// Cache lifetime of a failed lookup (timeout, unreachable resolver).
const TTL_FAILED: Duration = Duration::from_secs(60);
/// Most cached addresses.
const CACHE_MAX: usize = 4096;
/// Concurrent lookups.
const MAX_INFLIGHT: usize = 8;

#[derive(Debug, Clone)]
enum Entry {
    Name(String),
    None,
    Failed,
}

struct Cached {
    at: Instant,
    entry: Entry,
}

struct Inner {
    servers: Vec<SocketAddr>,
    cache: Mutex<HashMap<IpAddr, Cached>>,
    pending: Mutex<std::collections::HashSet<IpAddr>>,
    slots: Semaphore,
}

/// Cached reverse resolver (cheap to clone).
#[derive(Clone)]
pub struct Rdns(Arc<Inner>);

/// Nameservers listed in a `resolv.conf` text (port 53, at most three).
fn nameservers(text: &str) -> Vec<SocketAddr> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            (it.next()? == "nameserver").then_some(())?;
            let ip: IpAddr = it.next()?.split('%').next()?.parse().ok()?;
            Some(SocketAddr::new(ip, 53))
        })
        .take(3)
        .collect()
}

/// `4.3.2.1.in-addr.arpa` / `...ip6.arpa` name of an address.
fn ptr_name(ip: IpAddr) -> String {
    match crate::auth::bans::canonical_ip(ip) {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.{}.in-addr.arpa", o[3], o[2], o[1], o[0])
        }
        IpAddr::V6(v6) => {
            let mut out = String::new();
            for byte in v6.octets().iter().rev() {
                let _ = write!(out, "{:x}.{:x}.", byte & 0xf, byte >> 4);
            }
            out.push_str("ip6.arpa");
            out
        }
    }
}

fn build_query(id: u16, name: &str) -> Option<Vec<u8>> {
    let mut q = Vec::with_capacity(32 + name.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00]); // recursion desired
    q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // one question
    for label in name.split('.') {
        let len = u8::try_from(label.len())
            .ok()
            .filter(|l| *l > 0 && *l < 64)?;
        q.push(len);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&[0, 12, 0, 1]); // PTR, IN
    Some(q)
}

/// Read a (possibly compressed) name at `pos`; returns the name and the offset after it in the
/// original position (not following pointers).
fn read_name(buf: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *buf.get(pos)?;
        if len == 0 {
            return Some((out, end.unwrap_or(pos + 1)));
        }
        if len & 0xc0 == 0xc0 {
            let low = *buf.get(pos + 1)?;
            end.get_or_insert(pos + 2);
            pos = (usize::from(len & 0x3f) << 8) | usize::from(low);
            jumps += 1;
            if jumps > 8 {
                return None;
            }
            continue;
        }
        if len & 0xc0 != 0 {
            return None;
        }
        let label = buf.get(pos + 1..pos + 1 + usize::from(len))?;
        if !out.is_empty() {
            out.push('.');
        }
        out.push_str(std::str::from_utf8(label).ok()?);
        pos += 1 + usize::from(len);
        if out.len() > 255 {
            return None;
        }
    }
}

/// Outcome of parsing a DNS response.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Name(String),
    NoRecord,
    Invalid,
}

fn parse_response(id: u16, buf: &[u8]) -> Answer {
    parse_response_inner(id, buf).unwrap_or(Answer::Invalid)
}

fn parse_response_inner(id: u16, buf: &[u8]) -> Option<Answer> {
    if buf.len() < 12 || u16::from_be_bytes([buf[0], buf[1]]) != id || buf[2] & 0x80 == 0 {
        return None;
    }
    match buf[3] & 0x0f {
        0 => {}
        3 => return Some(Answer::NoRecord),
        _ => return None,
    }
    let questions = u16::from_be_bytes([buf[4], buf[5]]);
    let answers = u16::from_be_bytes([buf[6], buf[7]]);
    let mut pos = 12;
    for _ in 0..questions {
        pos = read_name(buf, pos)?.1 + 4;
    }
    for _ in 0..answers {
        let (_, after) = read_name(buf, pos)?;
        let header = buf.get(after..after + 10)?;
        let rtype = u16::from_be_bytes([header[0], header[1]]);
        let rdlen = usize::from(u16::from_be_bytes([header[8], header[9]]));
        let rdata = after + 10;
        if rtype == 12 {
            let (name, _) = read_name(buf, rdata)?;
            return Some(Answer::Name(name));
        }
        pos = rdata + rdlen;
    }
    Some(Answer::NoRecord)
}

async fn ask(server: SocketAddr, ip: IpAddr) -> Option<Answer> {
    let bind: SocketAddr = if server.is_ipv4() {
        ([0, 0, 0, 0], 0).into()
    } else {
        ([0u16; 8], 0).into()
    };
    let socket = UdpSocket::bind(bind).await.ok()?;
    socket.connect(server).await.ok()?;
    // Not cryptographic: the answer is only display text for an admin.
    let id = u16::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos())
            % 65_536,
    )
    .unwrap_or(0);
    let query = build_query(id, &ptr_name(ip))?;
    socket.send(&query).await.ok()?;
    let mut buf = [0u8; 1500];
    let n = tokio::time::timeout(QUERY_TIMEOUT, socket.recv(&mut buf))
        .await
        .ok()?
        .ok()?;
    Some(parse_response(id, &buf[..n]))
}

impl Rdns {
    /// A resolver over the system nameservers; `None` when there are none.
    #[must_use]
    pub fn from_system() -> Option<Self> {
        let text = std::fs::read_to_string(Path::new("/etc/resolv.conf")).ok()?;
        Self::with_servers(nameservers(&text))
    }

    /// A resolver over given nameservers.
    #[must_use]
    pub fn with_servers(servers: Vec<SocketAddr>) -> Option<Self> {
        if servers.is_empty() {
            return None;
        }
        Some(Self(Arc::new(Inner {
            servers,
            cache: Mutex::new(HashMap::new()),
            pending: Mutex::new(std::collections::HashSet::new()),
            slots: Semaphore::new(MAX_INFLIGHT),
        })))
    }

    fn cached(&self, ip: IpAddr) -> Option<Option<String>> {
        let cache = self.0.cache.lock().unwrap_or_else(PoisonError::into_inner);
        let c = cache.get(&ip)?;
        let (ttl, name) = match &c.entry {
            Entry::Name(n) => (TTL_FOUND, Some(n.clone())),
            Entry::None => (TTL_NONE, None),
            Entry::Failed => (TTL_FAILED, None),
        };
        (c.at.elapsed() < ttl).then_some(name)
    }

    fn store(&self, ip: IpAddr, entry: Entry) {
        let mut cache = self.0.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.len() >= CACHE_MAX {
            cache.retain(|_, c| c.at.elapsed() < TTL_FOUND);
            if cache.len() >= CACHE_MAX {
                cache.clear();
            }
        }
        cache.insert(
            ip,
            Cached {
                at: Instant::now(),
                entry,
            },
        );
    }

    async fn lookup_uncached(self, ip: IpAddr) {
        let Ok(_slot) = self.0.slots.acquire().await else {
            return;
        };
        let mut entry = Entry::Failed;
        for server in &self.0.servers {
            match ask(*server, ip).await {
                Some(Answer::Name(n)) => {
                    entry = Entry::Name(n.trim_end_matches('.').to_owned());
                    break;
                }
                Some(Answer::NoRecord) => {
                    entry = Entry::None;
                    break;
                }
                Some(Answer::Invalid) | None => {}
            }
        }
        self.store(ip, entry);
        self.0
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&ip);
    }

    /// Names for `ips`: cached ones at once, missing ones are looked up and waited for up to
    /// [`BATCH_WAIT`]. Addresses without a (known) name are absent from the result.
    pub async fn lookup_many(&self, ips: &[IpAddr]) -> HashMap<IpAddr, String> {
        let mut found = HashMap::new();
        let mut waiting = Vec::new();
        for ip in ips {
            match self.cached(*ip) {
                Some(Some(name)) => {
                    found.insert(*ip, name);
                }
                Some(None) => {}
                None => {
                    let fresh = self
                        .0
                        .pending
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(*ip);
                    if fresh {
                        let me = self.clone();
                        let ip = *ip;
                        waiting.push(tokio::spawn(me.lookup_uncached(ip)));
                    }
                }
            }
        }
        if waiting.is_empty() {
            return found;
        }
        let _ = tokio::time::timeout(BATCH_WAIT, async {
            for handle in waiting {
                let _ = handle.await;
            }
        })
        .await;
        for ip in ips {
            if let Some(Some(name)) = self.cached(*ip) {
                found.insert(*ip, name);
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ptr_names() {
        assert_eq!(
            ptr_name("203.0.113.9".parse().expect("ip")),
            "9.113.0.203.in-addr.arpa"
        );
        assert_eq!(
            ptr_name("2001:db8::1".parse().expect("ip")),
            "1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa"
        );
    }

    #[test]
    fn resolv_conf_nameservers() {
        let text = "# comment\nsearch lan\nnameserver 127.0.0.11\nnameserver fe80::1%eth0\nnameserver nonsense\noptions ndots:0\n";
        let servers = nameservers(text);
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].to_string(), "127.0.0.11:53");
    }

    /// A response for `9.113.0.203.in-addr.arpa` with a compressed PTR answer.
    fn response(id: u16, rcode: u8, target: Option<&[&str]>) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&id.to_be_bytes());
        r.extend_from_slice(&[0x81, 0x80 | rcode]);
        r.extend_from_slice(&[0, 1, 0, u8::from(target.is_some()), 0, 0, 0, 0]);
        let q = build_query(id, "9.113.0.203.in-addr.arpa").expect("query");
        r.extend_from_slice(&q[12..]);
        if let Some(labels) = target {
            r.extend_from_slice(&[0xc0, 12, 0, 12, 0, 1, 0, 0, 0, 60]);
            let mut rdata = Vec::new();
            for l in labels {
                rdata.push(u8::try_from(l.len()).expect("short"));
                rdata.extend_from_slice(l.as_bytes());
            }
            rdata.push(0);
            r.extend_from_slice(&u16::try_from(rdata.len()).expect("short").to_be_bytes());
            r.extend_from_slice(&rdata);
        }
        r
    }

    #[test]
    fn parses_ptr_answers() {
        let r = response(7, 0, Some(&["host", "example", "net"]));
        assert_eq!(
            parse_response(7, &r),
            Answer::Name("host.example.net".into())
        );
        // wrong id (a stray or forged datagram)
        assert_eq!(parse_response(8, &r), Answer::Invalid);
        assert_eq!(parse_response(7, &response(7, 3, None)), Answer::NoRecord);
        assert_eq!(parse_response(7, &response(7, 2, None)), Answer::Invalid);
        assert_eq!(parse_response(7, &response(7, 0, None)), Answer::NoRecord);
        assert_eq!(parse_response(7, &r[..10]), Answer::Invalid);
        assert_eq!(parse_response(7, &[]), Answer::Invalid);
    }

    #[test]
    fn pointer_loops_are_rejected() {
        // a name that points at itself
        let mut r = response(7, 0, Some(&["a"]));
        let at = r.len() - 3;
        r[at] = 0xc0;
        r[at + 1] = u8::try_from(at).expect("short");
        assert_eq!(parse_response(7, &r), Answer::Invalid);
    }

    #[tokio::test]
    async fn resolves_through_a_local_udp_server_and_caches() {
        let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let addr = server.local_addr().expect("addr");
        let hits = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = Arc::clone(&hits);
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            while let Ok((n, from)) = server.recv_from(&mut buf).await {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let id = u16::from_be_bytes([buf[0], buf[1]]);
                let mut reply = response(id, 0, Some(&["radio", "lan"]));
                // echo the real question so the parser sees a matching one
                reply.truncate(12);
                reply.extend_from_slice(&buf[12..n]);
                reply.extend_from_slice(&[0xc0, 12, 0, 12, 0, 1, 0, 0, 0, 60, 0, 11]);
                reply.extend_from_slice(b"\x05radio\x03lan\x00");
                let _ = server.send_to(&reply, from).await;
            }
        });
        let rdns = Rdns::with_servers(vec![addr]).expect("resolver");
        let ip: IpAddr = "203.0.113.9".parse().expect("ip");
        let first = rdns.lookup_many(&[ip]).await;
        assert_eq!(first.get(&ip).map(String::as_str), Some("radio.lan"));
        let second = rdns.lookup_many(&[ip]).await;
        assert_eq!(second.get(&ip).map(String::as_str), Some("radio.lan"));
        assert_eq!(hits.load(std::sync::atomic::Ordering::Relaxed), 1, "cached");
    }

    #[tokio::test]
    async fn dead_resolver_gives_no_name_without_hanging() {
        // a bound but silent socket
        let silent = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let rdns = Rdns::with_servers(vec![silent.local_addr().expect("addr")]).expect("resolver");
        let ip: IpAddr = "203.0.113.10".parse().expect("ip");
        let started = Instant::now();
        let found = rdns.lookup_many(&[ip]).await;
        assert!(found.is_empty());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
