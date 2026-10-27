//! Audit log queries and exports (`/api/v1/admin/audit`, `/api/v1/admin/audit/export`).

use axum::{
    Extension,
    body::Body,
    extract::{Query, State},
    http::{StatusCode, header},
    response::Response,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use superhet_bus::audit::Outcome;
use superhet_db::audit::{Event, Filter, MAX_PAGE};

use super::{AdminState, bad};
use crate::access::Principal;
use crate::auth::{Reply, db_failed, flatten, json_reply};
use crate::clientinfo::Subject;
use crate::decoder_events::{csv_field, iso};
use crate::settings::IpMode;

/// Default page size.
const DEFAULT_LIMIT: i64 = 100;
/// Most rows one export contains.
pub const EXPORT_MAX_ROWS: usize = 100_000;

/// Filters shared by the list and the export.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuditQuery {
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    actor_kind: Option<String>,
    actor: Option<String>,
    role: Option<String>,
    action: Option<String>,
    outcome: Option<String>,
    ip: Option<String>,
    target: Option<String>,
    before_id: Option<i64>,
    limit: Option<i64>,
    format: Option<String>,
}

fn one_of(value: &Option<String>, allowed: &[&str], name: &str) -> Result<(), Response> {
    match value {
        Some(v) if !allowed.contains(&v.as_str()) => {
            Err(bad(&format!("{name} must be one of {allowed:?}")))
        }
        _ => Ok(()),
    }
}

/// A host address or a network (`198.51.100.0/24`).
fn valid_ip_filter(value: &str) -> bool {
    match value.split_once('/') {
        Some((addr, prefix)) => {
            let max = if addr.contains(':') { 128 } else { 32 };
            addr.parse::<std::net::IpAddr>().is_ok()
                && prefix.parse::<u8>().is_ok_and(|p| u32::from(p) <= max)
        }
        None => value.parse::<std::net::IpAddr>().is_ok(),
    }
}

impl AuditQuery {
    fn filter(&self) -> Result<Filter, Response> {
        one_of(&self.actor_kind, &["user", "guest", "system"], "actor_kind")?;
        one_of(&self.role, &["admin", "operator", "listener"], "role")?;
        one_of(&self.outcome, &["ok", "denied", "error"], "outcome")?;
        if let Some(ip) = &self.ip
            && !valid_ip_filter(ip)
        {
            return Err(bad(
                "ip must be an address or a network such as 198.51.100.0/24",
            ));
        }
        for text in [&self.actor, &self.action, &self.target]
            .into_iter()
            .flatten()
        {
            if text.chars().count() > 128 {
                return Err(bad("filter text is limited to 128 characters"));
            }
        }
        Ok(Filter {
            from_ms: self.from_ms,
            to_ms: self.to_ms,
            actor_kind: self.actor_kind.clone(),
            actor: self.actor.clone(),
            role: self.role.clone(),
            action: self.action.clone(),
            outcome: self.outcome.clone(),
            ip: self.ip.clone(),
            target: self.target.clone(),
            before_id: self.before_id,
        })
    }

    /// Names of the filters that are set (for the audit row of an export).
    fn used(&self) -> Vec<&'static str> {
        [
            ("from_ms", self.from_ms.is_some()),
            ("to_ms", self.to_ms.is_some()),
            ("actor_kind", self.actor_kind.is_some()),
            ("actor", self.actor.is_some()),
            ("role", self.role.is_some()),
            ("action", self.action.is_some()),
            ("outcome", self.outcome.is_some()),
            ("ip", self.ip.is_some()),
            ("target", self.target.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, set)| set.then_some(name))
        .collect()
    }
}

pub(super) async fn query(
    State(state): State<AdminState>,
    Query(q): Query<AuditQuery>,
) -> Response {
    flatten(query_inner(&state, &q).await)
}

async fn query_inner(state: &AdminState, q: &AuditQuery) -> Reply {
    let db = state.auth.db()?;
    let filter = q.filter()?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err(bad(&format!("limit must be 1..={MAX_PAGE}")));
    }
    let events = superhet_db::audit::query(db, &filter, limit)
        .await
        .map_err(db_failed)?;
    let next = events
        .last()
        .filter(|_| i64::try_from(events.len()).is_ok_and(|n| n == limit))
        .map(|e| e.id);
    let info = describe(state, &events).await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({
            "events": events.iter().zip(info).map(|(e, i)| event_json(e, i)).collect::<Vec<_>>(),
            "next_before_id": next,
        }),
    ))
}

/// Enrichment (address type, reverse DNS, GeoIP, parsed user agent) of stored events. Computed
/// from the stored values, so a truncated address never yields a name or a city.
async fn describe(state: &AdminState, events: &[Event]) -> Vec<Map<String, Value>> {
    let settings = state.auth.settings().audit().await;
    let subjects: Vec<Subject> = events
        .iter()
        .map(|e| {
            let d = &e.data;
            let mode = match d.actor_kind.as_str() {
                "guest" => settings.ip_mode_guest,
                "user" => settings.ip_mode_user,
                _ => IpMode::Full,
            };
            Subject {
                ip: d.ip.as_deref().and_then(|v| v.parse().ok()),
                peer: d.peer_ip.as_deref().and_then(|v| v.parse().ok()),
                user_agent: d.user_agent.clone(),
                precise: mode == IpMode::Full,
            }
        })
        .collect();
    state.auth.enricher().describe(&subjects).await
}

fn event_json(event: &Event, info: Map<String, Value>) -> Value {
    let mut value = event.to_json();
    if let Value::Object(map) = &mut value {
        map.extend(info);
    }
    value
}

/// The enrichment columns come last so the positions of the original ones stay put.
const CSV_HEADER: &str = "id,time,actor_kind,actor_id,actor_name,role,ip,user_agent,action,target,outcome,details,peer_addr,address_type,reverse_dns,country,city,asn,as_org,browser,os,device\r\n";

fn csv_row(e: &Event, info: &Map<String, Value>) -> String {
    let d = &e.data;
    let opt = |v: &Option<String>| csv_field(v.as_deref().unwrap_or(""));
    let ip_info = |key: &str| {
        info.get("ip_info")
            .and_then(|i| i.get(key))
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
            .unwrap_or_default()
    };
    let ua_info = |key: &str| {
        let text = info
            .get("ua_info")
            .and_then(|i| i.get(key))
            .and_then(Value::as_str)
            .unwrap_or_default();
        text.to_owned()
    };
    let browser = format!("{} {}", ua_info("browser"), ua_info("browser_version"))
        .trim()
        .to_owned();
    let os = format!("{} {}", ua_info("os"), ua_info("os_version"))
        .trim()
        .to_owned();
    let fields = [
        e.id.to_string(),
        iso(e.ts_ms),
        csv_field(&d.actor_kind),
        opt(&d.actor_id),
        opt(&d.actor_name),
        opt(&d.role),
        opt(&d.ip),
        opt(&d.user_agent),
        csv_field(&d.action),
        opt(&d.target),
        csv_field(&d.outcome),
        csv_field(&d.details.to_string()),
        opt(&d.peer_ip),
        csv_field(&ip_info("class")),
        csv_field(&ip_info("rdns")),
        csv_field(&ip_info("country")),
        csv_field(&ip_info("city")),
        csv_field(&ip_info("asn")),
        csv_field(&ip_info("org")),
        csv_field(&browser),
        csv_field(&os),
        csv_field(&ua_info("device")),
    ];
    format!("{}\r\n", fields.join(","))
}

pub(super) async fn export(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Query(q): Query<AuditQuery>,
) -> Response {
    flatten(export_inner(&state, &me, &q).await)
}

async fn export_inner(state: &AdminState, me: &Principal, q: &AuditQuery) -> Reply {
    let db = state.auth.db()?;
    let filter = q.filter()?;
    let json_format = match q.format.as_deref() {
        None | Some("csv") => false,
        Some("json") => true,
        Some(_) => return Err(bad("format must be csv or json")),
    };
    let mut rows: Vec<Event> = Vec::new();
    let mut after = 0;
    let mut truncated = false;
    // Walk forward by id so the export is stable while new events arrive.
    while rows.len() < EXPORT_MAX_ROWS {
        let page = superhet_db::audit::export_page(db, &filter, after, MAX_PAGE)
            .await
            .map_err(db_failed)?;
        let Some(last) = page.last() else { break };
        after = last.id;
        let full = i64::try_from(page.len()).is_ok_and(|n| n == MAX_PAGE);
        rows.extend(page);
        if !full {
            break;
        }
        truncated = rows.len() >= EXPORT_MAX_ROWS;
    }
    rows.truncate(EXPORT_MAX_ROWS);
    let info = describe(state, &rows).await;
    let (content_type, name, body) = if json_format {
        let events: Vec<Value> = rows
            .iter()
            .zip(info)
            .map(|(e, i)| event_json(e, i))
            .collect();
        (
            "application/json",
            "superhet-audit.json",
            json!({"events": events, "truncated": truncated}).to_string(),
        )
    } else {
        let mut text = String::from(CSV_HEADER);
        for (event, i) in rows.iter().zip(&info) {
            text.push_str(&csv_row(event, i));
        }
        ("text/csv; charset=utf-8", "superhet-audit.csv", text)
    };
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "audit.export",
            None,
            Outcome::Ok,
            json!({"format": if json_format { "json" } else { "csv" }, "rows": rows.len(), "filters": q.used()}),
        )
        .await;
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    if let Ok(value) = header::HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    if truncated {
        headers.insert(
            "x-superhet-truncated",
            header::HeaderValue::from_static("true"),
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_filters_accept_hosts_and_networks_only() {
        for ok in [
            "203.0.113.5",
            "198.51.100.0/24",
            "2001:db8::1",
            "2001:db8::/48",
        ] {
            assert!(valid_ip_filter(ok), "{ok}");
        }
        for bad in ["", "x", "1.2.3.4/33", "1.2.3.4/", "::1/129", "1.2.3"] {
            assert!(!valid_ip_filter(bad), "{bad}");
        }
    }

    #[test]
    fn enumerated_filters_are_checked() {
        let q = AuditQuery {
            outcome: Some("maybe".into()),
            ..AuditQuery::default()
        };
        assert!(q.filter().is_err());
        let q = AuditQuery {
            outcome: Some("denied".into()),
            actor_kind: Some("guest".into()),
            ..AuditQuery::default()
        };
        assert_eq!(
            q.filter().map(|f| f.outcome).ok().flatten().as_deref(),
            Some("denied")
        );
        assert_eq!(q.used(), ["actor_kind", "outcome"]);
    }

    #[test]
    fn csv_rows_quote_and_defuse() {
        let event = Event {
            id: 7,
            ts_ms: 1_700_000_000_123,
            data: superhet_db::audit::NewEvent {
                ts_ms: Some(1_700_000_000_123),
                actor_kind: "user".into(),
                actor_id: Some("1".into()),
                actor_name: Some("=cmd|' /C calc'!A0".into()),
                role: Some("admin".into()),
                ip: None,
                peer_ip: None,
                user_agent: Some("a,b".into()),
                action: "auth.login".into(),
                target: None,
                outcome: "ok".into(),
                details: json!({"k": "v"}),
            },
        };
        let row = csv_row(&event, &Map::new());
        assert!(row.starts_with("7,2023-11-14T22:13:20.123Z,user,1,"));
        assert!(row.contains(",'=cmd"), "formula defused: {row}");
        assert!(row.contains("\"a,b\""));
        assert!(row.ends_with("\r\n"));
    }
}
