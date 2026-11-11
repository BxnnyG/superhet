//! Outbound request guard (SSRF). Webhook and push URLs are chosen by people; without a guard
//! they could aim the server at its own internal network (databases, the message bus, cloud
//! metadata). Every address a request would connect to is checked, hostnames after resolving
//! them (the checked addresses are the ones used, so DNS rebinding cannot swap them), IP literals
//! up front, and redirects are followed by hand with the same checks.
//!
//! Public addresses are always fine. Private ranges (loopback, RFC 1918, link-local, unique
//! local, CGNAT) are blocked unless the admin allows them for that target explicitly; some
//! ranges stay blocked regardless (unspecified, multicast, broadcast, reserved, the cloud
//! metadata addresses).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::{Host, Url};

/// Why an address is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// Loopback, private, link-local, unique local or CGNAT: allowed only on request.
    Private,
    /// Never allowed: unspecified, multicast, broadcast, reserved or a metadata service.
    Reserved,
}

impl Blocked {
    /// Text for the delivery log.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::Private => {
                "target address is in a private range (allow it explicitly on the target if intended)"
            }
            Self::Reserved => "target address is in a reserved range and is never allowed",
        }
    }
}

fn v4_class(ip: Ipv4Addr) -> Option<Blocked> {
    let [a, b, c, _] = ip.octets();
    if ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || a >= 240
        || (a == 192 && b == 0 && c == 0)
        || ip == Ipv4Addr::new(169, 254, 169, 254)
        || ip == Ipv4Addr::new(100, 100, 100, 200)
    {
        return Some(Blocked::Reserved);
    }
    let private = ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || (a == 100 && (64..128).contains(&b)) // CGNAT 100.64.0.0/10
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113); // documentation
    private.then_some(Blocked::Private)
}

fn v6_class(ip: Ipv6Addr) -> Option<Blocked> {
    if ip.is_unspecified() || ip.is_multicast() {
        return Some(Blocked::Reserved);
    }
    let seg = ip.segments();
    // IPv4 inside IPv6: mapped, compatible, NAT64 and 6to4 take the class of the embedded address.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return v4_class(v4);
    }
    let embedded = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    if seg[..6] == [0, 0, 0, 0, 0, 0] && !ip.is_loopback() {
        return v4_class(embedded(seg[6], seg[7])).or(Some(Blocked::Reserved));
    }
    if seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
        return v4_class(embedded(seg[6], seg[7])).or(Some(Blocked::Reserved));
    }
    if seg[0] == 0x2002 {
        return v4_class(embedded(seg[1], seg[2])).or(Some(Blocked::Reserved));
    }
    if ip
        == "fd00:ec2::254"
            .parse::<Ipv6Addr>()
            .unwrap_or(Ipv6Addr::UNSPECIFIED)
    {
        return Some(Blocked::Reserved);
    }
    let private = ip.is_loopback()
        || (seg[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
        || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        || (seg[0] & 0xffc0) == 0xfec0 // site-local (deprecated) fec0::/10
        || (seg[0] == 0x2001 && seg[1] == 0x0db8); // documentation
    private.then_some(Blocked::Private)
}

/// Check one address.
///
/// # Errors
/// [`Blocked`] if requests to it are refused (`allow_private` lifts only [`Blocked::Private`]).
pub fn check_ip(ip: IpAddr, allow_private: bool) -> Result<(), Blocked> {
    let class = match ip {
        IpAddr::V4(v4) => v4_class(v4),
        IpAddr::V6(v6) => v6_class(v6),
    };
    match class {
        None => Ok(()),
        Some(Blocked::Private) if allow_private => Ok(()),
        Some(other) => Err(other),
    }
}

/// The shape of a request URL: `http` or `https`, a host, no credentials, and if the host is an
/// IP address, an allowed one.
///
/// # Errors
/// A message for the log / the admin.
pub fn check_url(url: &Url, allow_private: bool) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https URLs are allowed".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the URL must not contain a user name or password".to_owned());
    }
    match url.host() {
        None => Err("the URL has no host".to_owned()),
        Some(Host::Ipv4(ip)) => {
            check_ip(IpAddr::V4(ip), allow_private).map_err(|b| b.message().to_owned())
        }
        Some(Host::Ipv6(ip)) => {
            check_ip(IpAddr::V6(ip), allow_private).map_err(|b| b.message().to_owned())
        }
        Some(Host::Domain(_)) => Ok(()),
    }
}

/// DNS resolver for the HTTP client that drops every blocked address; a name with only blocked
/// addresses fails to resolve at all.
#[derive(Debug, Clone, Copy)]
pub struct GuardedResolver {
    allow_private: bool,
}

impl GuardedResolver {
    /// A resolver for targets with the given policy.
    #[must_use]
    pub fn new(allow_private: bool) -> Arc<Self> {
        Arc::new(Self { allow_private })
    }
}

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow = self.allow_private;
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let found: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();
            let first_block = found.iter().find_map(|a| check_ip(a.ip(), allow).err());
            let allowed: Vec<SocketAddr> = found
                .into_iter()
                .filter(|a| check_ip(a.ip(), allow).is_ok())
                .collect();
            if allowed.is_empty() {
                let reason = first_block.map_or("the name does not resolve", Blocked::message);
                return Err(
                    Box::new(BlockedHost(reason)) as Box<dyn std::error::Error + Send + Sync>
                );
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

/// Error raised through the HTTP client when the resolver refused a host.
#[derive(Debug)]
pub struct BlockedHost(pub &'static str);

impl std::fmt::Display for BlockedHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for BlockedHost {}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("ip")
    }

    #[test]
    fn public_addresses_pass() {
        for a in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
            "2a00:1450:4001::200e",
        ] {
            assert_eq!(check_ip(ip(a), false), Ok(()), "{a}");
        }
    }

    #[test]
    fn private_ranges_are_blocked_unless_allowed() {
        for a in [
            "127.0.0.1",
            "127.8.9.10",
            "10.0.0.5",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "::1",
            "fe80::1",
            "fd12:3456::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "2002:7f00:1::1",
            "64:ff9b::a00:1",
        ] {
            assert_eq!(check_ip(ip(a), false), Err(Blocked::Private), "{a}");
            assert_eq!(check_ip(ip(a), true), Ok(()), "{a} when allowed");
        }
        // Neighbours of the private ranges are public.
        for a in [
            "172.15.255.255",
            "172.32.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "11.0.0.1",
            "169.253.0.1",
        ] {
            assert_eq!(check_ip(ip(a), false), Ok(()), "{a}");
        }
    }

    #[test]
    fn some_addresses_stay_blocked_even_when_private_is_allowed() {
        for a in [
            "0.0.0.0",
            "0.1.2.3",
            "255.255.255.255",
            "224.0.0.1",
            "240.0.0.1",
            "169.254.169.254",
            "100.100.100.200",
            "::",
            "ff02::1",
            "fd00:ec2::254",
            "::ffff:169.254.169.254",
            "::ffff:0.0.0.0",
        ] {
            assert_eq!(check_ip(ip(a), true), Err(Blocked::Reserved), "{a}");
            assert!(check_ip(ip(a), false).is_err(), "{a}");
        }
    }

    #[test]
    fn urls_are_checked_for_scheme_credentials_and_literal_hosts() {
        let url = |s: &str| Url::parse(s).expect("url");
        assert!(check_url(&url("https://hooks.example.com/x?y=1"), false).is_ok());
        assert!(check_url(&url("http://93.184.216.34:8080/"), false).is_ok());
        assert!(check_url(&url("ftp://example.com/"), false).is_err());
        assert!(check_url(&url("file:///etc/passwd"), false).is_err());
        assert!(check_url(&url("https://user:pw@example.com/"), false).is_err());
        assert!(check_url(&url("http://127.0.0.1:9000/"), false).is_err());
        assert!(check_url(&url("http://[::1]/"), false).is_err());
        assert!(check_url(&url("http://169.254.169.254/latest/meta-data"), true).is_err());
        assert!(check_url(&url("http://192.168.1.10/hook"), true).is_ok());
        // Tricks that parse to a blocked literal.
        assert!(
            check_url(&url("http://2130706433/"), false).is_err(),
            "decimal loopback"
        );
        assert!(
            check_url(&url("http://0x7f.1/"), false).is_err(),
            "hex loopback"
        );
        assert!(
            check_url(&url("http://127.1/"), false).is_err(),
            "short loopback"
        );
    }

    #[tokio::test]
    async fn the_resolver_refuses_names_that_point_inside() {
        use reqwest::dns::Resolve as _;
        let name = |s: &str| s.parse::<Name>().expect("name");
        let blocked = GuardedResolver::new(false).resolve(name("localhost")).await;
        let message = blocked.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(message.contains("private range"), "{message}");
        let allowed = GuardedResolver::new(true)
            .resolve(name("localhost"))
            .await
            .map(|a| a.count())
            .unwrap_or(0);
        assert!(allowed > 0);
    }
}
