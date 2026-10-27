//! What the admin views know about a client beyond the raw address: where the connection came
//! from (peer vs. resolved client address), address type, parsed user agent, reverse DNS and
//! optional offline GeoIP. Everything here is derived from the *stored* (possibly truncated)
//! values at read time, so the address privacy setting also limits what is shown: a truncated
//! address gets no reverse DNS and no city.

pub mod addr;
pub mod geo;
pub mod rdns;
pub mod ua;

use std::collections::HashSet;
use std::net::IpAddr;

use serde_json::{Map, Value, json};

use self::addr::{AddrClass, classify, family};
use self::geo::GeoDb;
use self::rdns::Rdns;

/// Environment variable: `false` turns reverse DNS off (default on).
pub const ENV_REVERSE_DNS: &str = "SUPERHET_REVERSE_DNS";

/// One client to describe.
#[derive(Debug, Clone, Default)]
pub struct Subject {
    /// The stored client address (as shown to the admin).
    pub ip: Option<IpAddr>,
    /// The stored TCP peer address.
    pub peer: Option<IpAddr>,
    /// The stored user agent.
    pub user_agent: Option<String>,
    /// The address is complete. `false` for truncated addresses: no reverse DNS, no city.
    pub precise: bool,
}

/// Enrichment sources.
#[derive(Default)]
pub struct Enricher {
    geo: Option<GeoDb>,
    rdns: Option<Rdns>,
}

fn base_info(ip: IpAddr) -> Map<String, Value> {
    let class = classify(ip);
    let mut m = Map::new();
    m.insert("class".into(), json!(class.as_str()));
    m.insert("label".into(), json!(class.label()));
    m.insert("family".into(), json!(family(ip)));
    m
}

fn wants_rdns(ip: IpAddr) -> bool {
    !matches!(classify(ip), AddrClass::Loopback | AddrClass::Reserved)
}

impl Enricher {
    /// Nothing enabled (tests).
    #[cfg(test)]
    #[must_use]
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Build from explicit parts (tests).
    #[cfg(test)]
    #[must_use]
    pub fn from_parts(geo: Option<GeoDb>, rdns: Option<Rdns>) -> Self {
        Self { geo, rdns }
    }

    /// Enable what the environment asks for: GeoIP databases from `SUPERHET_GEOIP_DB`, reverse DNS
    /// unless `SUPERHET_REVERSE_DNS=false`.
    #[must_use]
    pub fn from_env() -> Self {
        let rdns_on = !matches!(
            std::env::var(ENV_REVERSE_DNS)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "false" | "0" | "no" | "off"
        );
        Self {
            geo: geo::from_env(),
            rdns: if rdns_on { Rdns::from_system() } else { None },
        }
    }

    /// Describe `subjects`: one JSON object per subject with `ip_info`, `peer_info` and `ua_info`
    /// where there is something to say.
    pub async fn describe(&self, subjects: &[Subject]) -> Vec<Map<String, Value>> {
        let names = match &self.rdns {
            Some(rdns) => {
                let mut seen = HashSet::new();
                let ips: Vec<IpAddr> = subjects
                    .iter()
                    .filter(|s| s.precise)
                    .filter_map(|s| s.ip)
                    .filter(|ip| wants_rdns(*ip) && seen.insert(*ip))
                    .collect();
                if ips.is_empty() {
                    Default::default()
                } else {
                    rdns.lookup_many(&ips).await
                }
            }
            None => Default::default(),
        };
        subjects
            .iter()
            .map(|s| {
                let mut out = Map::new();
                if let Some(ip) = s.ip {
                    let mut info = base_info(ip);
                    if s.precise
                        && let Some(name) = names.get(&ip)
                    {
                        info.insert("rdns".into(), json!(name));
                    }
                    if let Some(db) = &self.geo {
                        let found = db.lookup(ip);
                        let found = if s.precise { found } else { found.coarse() };
                        if let Ok(Value::Object(geo)) = serde_json::to_value(found) {
                            for (k, v) in geo {
                                if !v.is_null() {
                                    info.insert(k, v);
                                }
                            }
                        }
                    }
                    out.insert("ip_info".into(), Value::Object(info));
                }
                if let Some(peer) = s.peer {
                    out.insert("peer_info".into(), Value::Object(base_info(peer)));
                }
                if let Some(agent) = &s.user_agent
                    && let Ok(info) = serde_json::to_value(ua::parse(agent))
                {
                    out.insert("ua_info".into(), info);
                }
                out
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(ip: &str, ua: Option<&str>) -> Subject {
        Subject {
            ip: ip.parse().ok(),
            peer: "172.22.0.1".parse().ok(),
            user_agent: ua.map(str::to_owned),
            precise: true,
        }
    }

    #[tokio::test]
    async fn describes_address_peer_and_agent() {
        let e = Enricher::disabled();
        let out = e.describe(&[subject("8.8.8.8", Some("curl/8.5.0"))]).await;
        let m = &out[0];
        assert_eq!(m["ip_info"]["class"], "public");
        assert_eq!(m["ip_info"]["family"], "ipv4");
        assert_eq!(m["peer_info"]["class"], "docker_gateway");
        assert_eq!(m["ua_info"]["browser"], "curl");
        assert_eq!(m["ua_info"]["device"], "tool");
        assert!(m["ip_info"].get("rdns").is_none());
    }

    #[tokio::test]
    async fn missing_parts_are_omitted() {
        let e = Enricher::disabled();
        let out = e
            .describe(&[Subject {
                precise: true,
                ..Subject::default()
            }])
            .await;
        assert!(out[0].is_empty());
    }
}
