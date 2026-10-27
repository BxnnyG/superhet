//! Client address resolution and classification.
//!
//! Two addresses are kept apart: the TCP *peer* (whoever opened the connection, often a Docker
//! bridge gateway or a reverse proxy) and the *client* (the visitor behind it). The client address
//! is only taken from `X-Forwarded-For`, `Forwarded` or `X-Real-IP` when the peer is a trusted
//! proxy; from anybody else those headers are attacker-controlled and ignored.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::http::HeaderMap;

use crate::auth::bans::{Cidr, canonical_ip};

/// Longest forwarded chain that is looked at (a longer header is cut from the left).
const MAX_CHAIN: usize = 16;

/// Which peers may speak for a client.
#[derive(Debug, Clone, Default)]
pub struct ProxyTrust {
    /// `SUPERHET_TRUST_PROXY=true`: every peer is a proxy (only safe when the port is reachable
    /// from the proxy alone).
    pub all: bool,
    /// `SUPERHET_TRUSTED_PROXIES`: networks of trusted proxies. Hops from these networks are also
    /// skipped when walking `X-Forwarded-For` from the right.
    pub nets: Vec<Cidr>,
}

impl ProxyTrust {
    /// Parse a comma or space separated list of addresses and networks.
    ///
    /// # Errors
    /// A message naming the first entry that is not an address or network.
    pub fn parse_list(text: &str) -> Result<Vec<Cidr>, String> {
        text.split([',', ' ', '\t', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Cidr::parse_min(s, 0, 0))
            .collect()
    }

    /// Whether headers sent by `peer` may be believed.
    #[must_use]
    pub fn trusts(&self, peer: Option<IpAddr>) -> bool {
        self.all || peer.is_some_and(|p| self.nets.iter().any(|n| n.contains(p)))
    }

    fn is_proxy_hop(&self, ip: IpAddr) -> bool {
        self.nets.iter().any(|n| n.contains(ip))
    }
}

/// Result of [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolved {
    /// The TCP peer.
    pub peer: Option<IpAddr>,
    /// The address attributed to the visitor.
    pub client: Option<IpAddr>,
    /// Forwarded addresses as the trusted proxy reported them, client first (empty when the
    /// headers were absent or not trusted).
    pub chain: Vec<IpAddr>,
}

/// Parse one node of a forwarding header: `1.2.3.4`, `1.2.3.4:80`, `[::1]`, `[::1]:80`, `::1`.
fn parse_node(text: &str) -> Option<IpAddr> {
    let t = text.trim().trim_matches('"');
    if let Ok(ip) = t.parse::<IpAddr>() {
        return Some(canonical_ip(ip));
    }
    if let Some(rest) = t.strip_prefix('[') {
        let (inner, _) = rest.split_once(']')?;
        return inner.parse::<IpAddr>().ok().map(canonical_ip);
    }
    let (host, port) = t.rsplit_once(':')?;
    port.parse::<u16>().ok()?;
    host.parse::<Ipv4Addr>()
        .ok()
        .map(|v4| canonical_ip(IpAddr::V4(v4)))
}

/// `X-Forwarded-For` entries, left to right. `None` entries are tokens that are no address
/// (`unknown`, an obfuscated `_hidden`): the chain cannot be followed through them.
fn forwarded_for(headers: &HeaderMap) -> Vec<Option<IpAddr>> {
    let mut out = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        let Ok(text) = value.to_str() else {
            out.push(None);
            continue;
        };
        out.extend(text.split(',').map(parse_node));
    }
    out
}

/// `for=` nodes of the RFC 7239 `Forwarded` header, left to right.
fn forwarded_rfc7239(headers: &HeaderMap) -> Vec<Option<IpAddr>> {
    let mut out = Vec::new();
    for value in headers.get_all("forwarded") {
        let Ok(text) = value.to_str() else {
            out.push(None);
            continue;
        };
        for element in text.split(',') {
            for pair in element.split(';') {
                if let Some((key, val)) = pair.split_once('=')
                    && key.trim().eq_ignore_ascii_case("for")
                {
                    out.push(parse_node(val));
                }
            }
        }
    }
    out
}

fn real_ip(headers: &HeaderMap) -> Vec<Option<IpAddr>> {
    headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(|t| vec![parse_node(t)])
        .unwrap_or_default()
}

/// Resolve peer and client for one request.
///
/// With an untrusted peer the headers are ignored entirely. With a trusted one the first non-empty
/// of `X-Forwarded-For`, `Forwarded` and `X-Real-IP` is used and walked from the right, skipping
/// hops that are configured proxies; the first other address is the client. With no proxy
/// networks configured (`all` only) the rightmost entry is the client: it is the one our single
/// proxy appended, everything left of it came from the visitor.
#[must_use]
pub fn resolve(peer: Option<IpAddr>, headers: &HeaderMap, trust: &ProxyTrust) -> Resolved {
    let peer = peer.map(canonical_ip);
    let mut resolved = Resolved {
        peer,
        client: peer,
        chain: Vec::new(),
    };
    if !trust.trusts(peer) {
        return resolved;
    }
    let mut entries = forwarded_for(headers);
    if entries.is_empty() {
        entries = forwarded_rfc7239(headers);
    }
    if entries.is_empty() {
        entries = real_ip(headers);
    }
    if entries.len() > MAX_CHAIN {
        entries.drain(..entries.len() - MAX_CHAIN);
    }
    let mut client = None;
    for entry in entries.iter().rev() {
        match entry {
            // Not an address: stop, whatever is left of it cannot be attributed.
            None => break,
            Some(ip) if trust.is_proxy_hop(*ip) => client = Some(*ip),
            Some(ip) => {
                client = Some(*ip);
                break;
            }
        }
    }
    // The chain is kept for display only when every entry is an address.
    if entries.iter().all(Option::is_some) {
        resolved.chain = entries.iter().flatten().copied().collect();
    }
    if let Some(ip) = client {
        resolved.client = Some(ip);
    }
    resolved
}

/// Coarse kind of an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrClass {
    /// `127.0.0.0/8`, `::1`.
    Loopback,
    /// RFC 1918, IPv6 unique local.
    Private,
    /// A private `172.16.0.0/12` address ending in `.1` (or `.0`, the truncated form): the
    /// gateway of a Docker bridge network, i.e. the stack's own NAT, not a visitor.
    DockerGateway,
    /// `100.64.0.0/10`, shared address space of carrier-grade NAT.
    Cgnat,
    /// `169.254.0.0/16`, `fe80::/10`.
    LinkLocal,
    /// Multicast.
    Multicast,
    /// Unspecified, documentation, benchmarking, reserved ranges.
    Reserved,
    /// Globally routable.
    Public,
}

impl AddrClass {
    /// Stable identifier used in the API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::DockerGateway => "docker_gateway",
            Self::Cgnat => "cgnat",
            Self::LinkLocal => "link_local",
            Self::Multicast => "multicast",
            Self::Reserved => "reserved",
            Self::Public => "public",
        }
    }

    /// Short label for badges.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Loopback => "Loopback",
            Self::Private => "Private LAN",
            Self::DockerGateway => "Docker gateway",
            Self::Cgnat => "CGNAT",
            Self::LinkLocal => "Link-local",
            Self::Multicast => "Multicast",
            Self::Reserved => "Reserved",
            Self::Public => "Public",
        }
    }
}

fn classify_v4(ip: Ipv4Addr) -> AddrClass {
    let o = ip.octets();
    if ip.is_loopback() {
        AddrClass::Loopback
    } else if o[0] == 172 && (16..=31).contains(&o[1]) {
        if o[3] <= 1 {
            AddrClass::DockerGateway
        } else {
            AddrClass::Private
        }
    } else if o[0] == 10 || (o[0] == 192 && o[1] == 168) {
        AddrClass::Private
    } else if o[0] == 100 && (64..=127).contains(&o[1]) {
        AddrClass::Cgnat
    } else if ip.is_link_local() {
        AddrClass::LinkLocal
    } else if ip.is_multicast() {
        AddrClass::Multicast
    } else if o[0] == 0
        || o[0] >= 240
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
    {
        AddrClass::Reserved
    } else {
        AddrClass::Public
    }
}

fn classify_v6(ip: Ipv6Addr) -> AddrClass {
    let s = ip.segments();
    if ip.is_loopback() {
        AddrClass::Loopback
    } else if ip.is_unspecified() || (s[0] == 0x2001 && s[1] == 0x0db8) {
        AddrClass::Reserved
    } else if s[0] & 0xfe00 == 0xfc00 {
        AddrClass::Private
    } else if s[0] & 0xffc0 == 0xfe80 {
        AddrClass::LinkLocal
    } else if s[0] & 0xff00 == 0xff00 {
        AddrClass::Multicast
    } else {
        AddrClass::Public
    }
}

/// Classify an address (IPv4-mapped IPv6 counts as IPv4).
#[must_use]
pub fn classify(ip: IpAddr) -> AddrClass {
    match canonical_ip(ip) {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

/// `ipv4` or `ipv6` (IPv4-mapped IPv6 counts as IPv4).
#[must_use]
pub fn family(ip: IpAddr) -> &'static str {
    if canonical_ip(ip).is_ipv4() {
        "ipv4"
    } else {
        "ipv6"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("ip")
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_static(v));
        }
        h
    }

    fn trust_nets(list: &str) -> ProxyTrust {
        ProxyTrust {
            all: false,
            nets: ProxyTrust::parse_list(list).expect("nets"),
        }
    }

    #[test]
    fn untrusted_peer_cannot_spoof_the_client() {
        let h = headers(&[
            ("x-forwarded-for", "6.6.6.6"),
            ("x-real-ip", "6.6.6.6"),
            ("forwarded", "for=6.6.6.6"),
        ]);
        let r = resolve(Some(ip("172.22.0.1")), &h, &ProxyTrust::default());
        assert_eq!(r.client, Some(ip("172.22.0.1")));
        assert_eq!(r.peer, Some(ip("172.22.0.1")));
        assert!(r.chain.is_empty());
        // a peer outside the configured proxy networks is just as untrusted
        let r = resolve(Some(ip("198.51.100.7")), &h, &trust_nets("172.16.0.0/12"));
        assert_eq!(r.client, Some(ip("198.51.100.7")));
    }

    #[test]
    fn trusted_peer_by_network_yields_the_forwarded_client() {
        let h = headers(&[("x-forwarded-for", "8.8.4.4")]);
        let r = resolve(Some(ip("172.22.0.1")), &h, &trust_nets("172.16.0.0/12"));
        assert_eq!(r.client, Some(ip("8.8.4.4")));
        assert_eq!(r.peer, Some(ip("172.22.0.1")));
        assert_eq!(r.chain, vec![ip("8.8.4.4")]);
    }

    #[test]
    fn trust_all_takes_the_rightmost_entry() {
        let h = headers(&[("x-forwarded-for", "6.6.6.6, 8.8.4.4")]);
        let trust = ProxyTrust {
            all: true,
            nets: vec![],
        };
        let r = resolve(Some(ip("10.0.0.2")), &h, &trust);
        assert_eq!(r.client, Some(ip("8.8.4.4")));
        assert_eq!(r.chain, vec![ip("6.6.6.6"), ip("8.8.4.4")]);
    }

    #[test]
    fn proxy_hops_are_skipped_from_the_right() {
        // visitor -> edge proxy 10.0.0.5 -> app proxy 10.0.0.2 (peer)
        let h = headers(&[("x-forwarded-for", "6.6.6.6, 8.8.4.4, 10.0.0.5")]);
        let r = resolve(Some(ip("10.0.0.2")), &h, &trust_nets("10.0.0.0/8"));
        assert_eq!(r.client, Some(ip("8.8.4.4")));
        assert_eq!(r.chain.len(), 3);
        // a spoofed left part never wins over the first untrusted hop
        let h = headers(&[("x-forwarded-for", "1.1.1.1, 8.8.4.4")]);
        let r = resolve(Some(ip("10.0.0.2")), &h, &trust_nets("10.0.0.0/8"));
        assert_eq!(r.client, Some(ip("8.8.4.4")));
    }

    #[test]
    fn all_hops_trusted_falls_back_to_the_leftmost() {
        let h = headers(&[("x-forwarded-for", "10.0.0.9, 10.0.0.5")]);
        let r = resolve(Some(ip("10.0.0.2")), &h, &trust_nets("10.0.0.0/8"));
        assert_eq!(r.client, Some(ip("10.0.0.9")));
    }

    #[test]
    fn falls_back_to_forwarded_then_real_ip() {
        let trust = trust_nets("10.0.0.0/8");
        let h = headers(&[(
            "forwarded",
            "for=192.0.2.60;proto=http, for=\"[2001:db8::7]:4711\"",
        )]);
        let r = resolve(Some(ip("10.0.0.2")), &h, &trust);
        assert_eq!(r.client, Some(ip("2001:db8::7")));
        let h = headers(&[("x-real-ip", "8.8.4.4")]);
        let r = resolve(Some(ip("10.0.0.2")), &h, &trust);
        assert_eq!(r.client, Some(ip("8.8.4.4")));
        assert_eq!(r.chain, vec![ip("8.8.4.4")]);
    }

    #[test]
    fn garbage_headers_fall_back_to_the_peer() {
        let trust = trust_nets("10.0.0.0/8");
        for value in ["", "unknown", "_hidden", "not an ip, also not"] {
            let h = headers(&[("x-forwarded-for", value)]);
            let r = resolve(Some(ip("10.0.0.2")), &h, &trust);
            assert_eq!(r.client, Some(ip("10.0.0.2")), "{value:?}");
            assert!(r.chain.is_empty(), "{value:?}");
        }
        // no address after the last valid entry: keep what could be attributed
        let h = headers(&[("x-forwarded-for", "8.8.4.4, unknown")]);
        let r = resolve(Some(ip("10.0.0.2")), &h, &trust);
        assert_eq!(r.client, Some(ip("10.0.0.2")));
    }

    #[test]
    fn ports_brackets_and_mapped_addresses_are_understood() {
        assert_eq!(parse_node("8.8.4.4:5000"), Some(ip("8.8.4.4")));
        assert_eq!(parse_node("[2001:db8::1]:80"), Some(ip("2001:db8::1")));
        assert_eq!(parse_node("[2001:db8::1]"), Some(ip("2001:db8::1")));
        assert_eq!(parse_node("::ffff:8.8.4.4"), Some(ip("8.8.4.4")));
        assert_eq!(parse_node("8.8.4.4:http"), None);
    }

    #[test]
    fn no_peer_means_no_client() {
        let r = resolve(None, &HeaderMap::new(), &ProxyTrust::default());
        assert_eq!(r, Resolved::default());
    }

    #[test]
    fn trusted_proxy_list_parses() {
        let nets = ProxyTrust::parse_list("172.16.0.0/12, fc00::/7 10.1.2.3").expect("ok");
        assert_eq!(nets.len(), 3);
        assert!(ProxyTrust::parse_list("172.16.0.0/12,nope").is_err());
        assert!(ProxyTrust::parse_list("").expect("empty").is_empty());
    }

    #[test]
    fn classification() {
        let cases = [
            ("127.0.0.1", AddrClass::Loopback),
            ("::1", AddrClass::Loopback),
            ("192.168.1.20", AddrClass::Private),
            ("10.1.2.3", AddrClass::Private),
            ("172.20.5.9", AddrClass::Private),
            ("172.22.0.1", AddrClass::DockerGateway),
            ("172.17.0.1", AddrClass::DockerGateway),
            ("172.22.0.0", AddrClass::DockerGateway),
            ("172.32.0.1", AddrClass::Public),
            ("100.64.3.4", AddrClass::Cgnat),
            ("100.128.0.1", AddrClass::Public),
            ("169.254.1.2", AddrClass::LinkLocal),
            ("fe80::1", AddrClass::LinkLocal),
            ("fd12:3456::1", AddrClass::Private),
            ("224.0.0.1", AddrClass::Multicast),
            ("ff02::1", AddrClass::Multicast),
            ("0.0.0.0", AddrClass::Reserved),
            ("203.0.113.9", AddrClass::Reserved),
            ("2001:db8::1", AddrClass::Reserved),
            ("8.8.8.8", AddrClass::Public),
            ("2606:4700:4700::1111", AddrClass::Public),
            ("::ffff:192.168.0.5", AddrClass::Private),
            ("::ffff:8.8.8.8", AddrClass::Public),
        ];
        for (text, want) in cases {
            assert_eq!(classify(ip(text)), want, "{text}");
        }
        assert_eq!(family(ip("::ffff:8.8.8.8")), "ipv4");
        assert_eq!(family(ip("2606:4700::1")), "ipv6");
    }
}
