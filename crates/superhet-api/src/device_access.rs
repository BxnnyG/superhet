//! Who may use which device (ADR-0023).
//!
//! The role table in `access` says what a caller may do at all; this module narrows it per
//! device. A device without a stored policy is open (today's behaviour). A policy has two role
//! floors, `view_role` (see, listen) and `control_role` (tune, lock, VFOs, profiles, plugins),
//! a switch for guests and an optional list of accounts. Admins always pass.
//!
//! Enforcement is fail closed in the same spirit as [`crate::access::RULES`]: a route whose path
//! names a device (`{dev}`) or that is a WebSocket upgrade must have an entry in
//! [`DEVICE_RULES`]; one without is refused and a test fails. The middleware in `auth` calls
//! [`gate`] after the role check. Handlers that learn the device only from a body or a stored
//! record (plugin instances, decoder history) call [`check`] themselves.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::http::{Method, StatusCode, Uri};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::json;
use superhet_bus::audit::Outcome;
use superhet_db::{PgPool, device_policies};

use crate::access::{Actor, Principal, Role};
use crate::audit::{ActorKind, Who};
use crate::auth::AuthState;
use crate::devices::failure;

/// How long a read policy table is reused. Writes through this process refresh at once.
const CACHE_TTL: Duration = Duration::from_secs(5);
/// Most accounts one policy may name.
pub const MAX_ALLOWED_USERS: usize = 200;

/// What a request needs from a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// See and listen: state, spectrum, audio, decoder output.
    View,
    /// Change something: tune, lock, VFOs, profiles, plugin instances.
    Control,
}

impl Need {
    fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Control => "control",
        }
    }
}

/// The access policy of one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DevicePolicy {
    /// Lowest role that may see and listen.
    #[serde(with = "role_name")]
    pub view_role: Role,
    /// Lowest role that may change anything on the device (on top of what the route needs).
    #[serde(with = "role_name")]
    pub control_role: Role,
    /// Guests may see the device.
    pub guests_visible: bool,
    /// When not empty: only these accounts (and admins) may use the device; no guests.
    pub allowed_users: Vec<i64>,
}

impl Default for DevicePolicy {
    /// Today's behaviour: open to everybody who is signed in.
    fn default() -> Self {
        Self {
            view_role: Role::Listener,
            control_role: Role::Listener,
            guests_visible: true,
            allowed_users: Vec::new(),
        }
    }
}

mod role_name {
    use super::Role;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(role: &Role, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(role.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Role, D::Error> {
        let name = String::deserialize(d)?;
        Role::parse(&name).ok_or_else(|| serde::de::Error::custom("unknown role"))
    }
}

/// The part of a caller that policies look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subject {
    /// An anonymous guest session.
    pub guest: bool,
    /// Account id (accounts only).
    pub user_id: Option<i64>,
    /// Role; guests count as listeners.
    pub role: Role,
}

impl From<&Principal> for Subject {
    fn from(p: &Principal) -> Self {
        Self {
            guest: p.is_guest(),
            user_id: match p.actor {
                Actor::User(id) => Some(id),
                Actor::Guest(_) => None,
            },
            role: p.role,
        }
    }
}

impl Subject {
    /// The subject of an audit actor (live connections only remember a [`Who`]).
    #[must_use]
    pub fn from_who(who: &Who) -> Self {
        let guest = who.kind == ActorKind::Guest;
        Self {
            guest,
            user_id: if guest {
                None
            } else {
                who.id.as_deref().and_then(|id| id.parse().ok())
            },
            // Guests are listeners whatever the actor record says.
            role: if guest {
                Role::Listener
            } else {
                who.role.unwrap_or(Role::Listener)
            },
        }
    }
}

impl DevicePolicy {
    /// Whether `subject` may do `need` on a device with this policy.
    #[must_use]
    pub fn allows(&self, subject: &Subject, need: Need) -> bool {
        if !subject.guest && subject.role == Role::Admin {
            return true;
        }
        if subject.guest {
            if !self.guests_visible || !self.allowed_users.is_empty() {
                return false;
            }
        } else if !self.allowed_users.is_empty()
            && !subject
                .user_id
                .is_some_and(|id| self.allowed_users.contains(&id))
        {
            return false;
        }
        if subject.role < self.view_role {
            return false;
        }
        match need {
            Need::View => true,
            Need::Control => subject.role >= self.control_role,
        }
    }

    /// Check the value ranges and put the account list in order.
    ///
    /// # Errors
    /// A message naming the problem.
    pub fn normalise(&mut self) -> Result<(), String> {
        if self.allowed_users.iter().any(|id| *id <= 0) {
            return Err("allowed_users: account ids are positive numbers".into());
        }
        self.allowed_users.sort_unstable();
        self.allowed_users.dedup();
        if self.allowed_users.len() > MAX_ALLOWED_USERS {
            return Err(format!(
                "allowed_users: at most {MAX_ALLOWED_USERS} accounts"
            ));
        }
        Ok(())
    }

    fn from_record(r: &device_policies::Record) -> Self {
        Self {
            // A role the table should never hold is read as the strictest one.
            view_role: Role::parse(&r.view_role).unwrap_or(Role::Admin),
            control_role: Role::parse(&r.control_role).unwrap_or(Role::Admin),
            guests_visible: r.guests_visible,
            allowed_users: r.allowed_users.clone(),
        }
    }

    fn to_record(&self, device: &str) -> device_policies::Record {
        device_policies::Record {
            device: device.to_owned(),
            view_role: self.view_role.as_str().to_owned(),
            control_role: self.control_role.as_str().to_owned(),
            guests_visible: self.guests_visible,
            allowed_users: self.allowed_users.clone(),
        }
    }
}

/// The policy table could not be read; the request is refused.
#[derive(Debug)]
pub struct Unavailable;

type Table = Arc<HashMap<String, DevicePolicy>>;

struct Cached {
    at: Instant,
    table: Table,
}

/// The stored device policies behind a short-lived cache.
pub struct DevicePolicies {
    db: Option<PgPool>,
    cache: Mutex<Option<Cached>>,
}

impl DevicePolicies {
    /// Create the store; without a database every device is open (and nobody can sign in).
    #[must_use]
    pub fn new(db: Option<PgPool>) -> Self {
        Self {
            db,
            cache: Mutex::new(None),
        }
    }

    fn cached(&self) -> Option<Table> {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .as_ref()
            .filter(|c| c.at.elapsed() < CACHE_TTL)
            .map(|c| Arc::clone(&c.table))
    }

    fn store(&self, table: Option<Table>) {
        *self.cache.lock().unwrap_or_else(PoisonError::into_inner) = table.map(|table| Cached {
            at: Instant::now(),
            table,
        });
    }

    /// Every stored policy.
    ///
    /// # Errors
    /// [`Unavailable`] if the table cannot be read (callers refuse the request).
    pub async fn table(&self) -> Result<Table, Unavailable> {
        let Some(db) = &self.db else {
            return Ok(Arc::new(HashMap::new()));
        };
        if let Some(table) = self.cached() {
            return Ok(table);
        }
        let rows = device_policies::list(db).await.map_err(|err| {
            tracing::warn!(%err, "device policies unreadable: request refused");
            Unavailable
        })?;
        let table: Table = Arc::new(
            rows.iter()
                .map(|r| (r.device.clone(), DevicePolicy::from_record(r)))
                .collect(),
        );
        self.store(Some(Arc::clone(&table)));
        Ok(table)
    }

    /// The policy of one device (the open default when none is stored).
    ///
    /// # Errors
    /// [`Unavailable`] if the table cannot be read.
    pub async fn policy(&self, device: &str) -> Result<DevicePolicy, Unavailable> {
        Ok(self.table().await?.get(device).cloned().unwrap_or_default())
    }

    /// The devices with a stored policy that refuse `subject` for `need`. Devices without a
    /// policy are open and not listed.
    ///
    /// # Errors
    /// [`Unavailable`] if the table cannot be read.
    pub async fn denied(&self, subject: &Subject, need: Need) -> Result<Vec<String>, Unavailable> {
        let mut out: Vec<String> = self
            .table()
            .await?
            .iter()
            .filter(|(_, p)| !p.allows(subject, need))
            .map(|(device, _)| device.clone())
            .collect();
        out.sort_unstable();
        Ok(out)
    }

    /// Store a policy.
    ///
    /// # Errors
    /// The database error.
    pub async fn put(
        &self,
        device: &str,
        policy: &DevicePolicy,
        by: &str,
    ) -> Result<(), superhet_db::Error> {
        if let Some(db) = &self.db {
            device_policies::put(db, &policy.to_record(device), by).await?;
        }
        self.store(None);
        Ok(())
    }

    /// Remove a policy (the device is open again). Returns whether one was stored.
    ///
    /// # Errors
    /// The database error.
    pub async fn delete(&self, device: &str) -> Result<bool, superhet_db::Error> {
        let existed = match &self.db {
            Some(db) => device_policies::delete(db, device).await?,
            None => false,
        };
        self.store(None);
        Ok(existed)
    }
}

/// Where a device id sits in a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// The `{dev}` segment of the path.
    Path,
    /// The `dev` query parameter (WebSocket upgrades).
    Query,
}

/// One device-scoped route.
#[derive(Debug, Clone, Copy)]
pub struct DeviceRule {
    /// HTTP method.
    pub method: &'static str,
    /// Route pattern exactly as in [`crate::access::RULES`].
    pub path: &'static str,
    /// What the route needs from the device.
    pub need: Need,
}

const fn dev(method: &'static str, path: &'static str, need: Need) -> DeviceRule {
    DeviceRule { method, path, need }
}

use Need::{Control, View};

/// Every route that acts on one device and is checked by [`gate`].
pub const DEVICE_RULES: &[DeviceRule] = &[
    dev("GET", "/api/v1/devices/{dev}/state", View),
    dev("POST", "/api/v1/devices/{dev}/tune", Control),
    dev("POST", "/api/v1/devices/{dev}/lock", Control),
    dev("DELETE", "/api/v1/devices/{dev}/lock", Control),
    dev("POST", "/api/v1/devices/{dev}/scanner/{action}", Control),
    dev("GET", "/api/v1/devices/{dev}/vfos", View),
    dev("POST", "/api/v1/devices/{dev}/vfos", Control),
    dev("PATCH", "/api/v1/devices/{dev}/vfos/{id}", Control),
    dev("DELETE", "/api/v1/devices/{dev}/vfos/{id}", Control),
    dev("POST", "/api/v1/devices/{dev}/profile/save", Control),
    dev("POST", "/api/v1/devices/{dev}/profile/load", Control),
    dev("GET", "/api/v1/ws/spectrum", View),
    dev("GET", "/api/v1/ws/audio", View),
    dev("GET", "/api/v1/ws/state", View),
    dev("GET", "/api/v1/ws/events", View),
];

/// The device rule of a matched route and method (HEAD counts as GET).
#[must_use]
pub fn find_device_rule(method: &Method, path: &str) -> Option<&'static DeviceRule> {
    let method = if method == Method::HEAD {
        "GET"
    } else {
        method.as_str()
    };
    DEVICE_RULES
        .iter()
        .find(|r| r.method == method && r.path == path)
}

/// Whether a route pattern acts on a device and therefore needs a [`DeviceRule`].
#[must_use]
pub fn is_device_route(pattern: &str) -> bool {
    pattern.contains("{dev}") || pattern.starts_with("/api/v1/ws/")
}

fn place_of(pattern: &str) -> Place {
    if pattern.contains("{dev}") {
        Place::Path
    } else {
        Place::Query
    }
}

/// A device id as it appears in routes: one bus token, nothing that needs decoding.
fn plain_id(raw: &str) -> Option<&str> {
    (superhet_bus::subjects::state_key(raw).is_ok() && raw.len() <= 64).then_some(raw)
}

/// The device id a request names. Percent-encoded or repeated ids are refused rather than
/// decoded, so the id checked here is byte for byte the one the handler sees.
fn device_id<'a>(pattern: &str, uri: &'a Uri) -> Option<&'a str> {
    match place_of(pattern) {
        Place::Path => {
            let mut pattern_parts = pattern.split('/');
            let mut path_parts = uri.path().split('/');
            loop {
                let (p, a) = (pattern_parts.next()?, path_parts.next()?);
                if p == "{dev}" {
                    return plain_id(a);
                }
            }
        }
        Place::Query => {
            let mut found = None;
            for pair in uri.query().unwrap_or("").split('&') {
                if let Some(value) = pair.strip_prefix("dev=") {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(value);
                }
            }
            plain_id(found?)
        }
    }
}

fn forbidden_view() -> Response {
    failure(
        StatusCode::FORBIDDEN,
        "device_forbidden",
        "this device is not open to your account",
    )
}

fn forbidden_control() -> Response {
    failure(
        StatusCode::FORBIDDEN,
        "device_read_only",
        "this device is read-only for your account",
    )
}

fn unavailable() -> Response {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "device policy unavailable",
    )
}

async fn deny(
    auth: &AuthState,
    principal: &Principal,
    device: &str,
    need: Need,
    action: &str,
) -> Response {
    auth.audit().log_coalesced(
        &principal.who(),
        "access.denied",
        Some(action),
        Outcome::Denied,
        json!({"device": device, "need": need.as_str()}),
    );
    match need {
        Need::View => forbidden_view(),
        Need::Control => forbidden_control(),
    }
}

/// Check `principal` against the policy of `device` for `need` (handlers that learn the device
/// from a body or a stored record).
///
/// # Errors
/// The response to send: 403 `device_forbidden` / `device_read_only`, 503 if the policy table
/// cannot be read.
pub async fn check(
    auth: &AuthState,
    principal: &Principal,
    device: &str,
    need: Need,
    action: &str,
) -> Result<(), Box<Response>> {
    let Ok(policy) = auth.device_policies().policy(device).await else {
        return Err(Box::new(unavailable()));
    };
    let subject = Subject::from(principal);
    if policy.allows(&subject, need) {
        return Ok(());
    }
    // Someone who cannot even see the device is told that, not that it is read-only.
    let shown = if policy.allows(&subject, Need::View) {
        need
    } else {
        Need::View
    };
    Err(Box::new(deny(auth, principal, device, shown, action).await))
}

/// The device check of the access middleware, run after the role check. Routes that name no
/// device pass; a device route without an entry in [`DEVICE_RULES`] is refused.
///
/// # Errors
/// The response to send instead of calling the handler.
pub async fn gate(
    auth: &AuthState,
    principal: &Principal,
    method: &Method,
    pattern: &str,
    uri: &Uri,
) -> Result<(), Box<Response>> {
    let Some(rule) = find_device_rule(method, pattern) else {
        if is_device_route(pattern) {
            tracing::error!(%method, path = pattern, "device route has no device rule: refused");
            return Err(Box::new(failure(
                StatusCode::FORBIDDEN,
                "forbidden",
                "forbidden",
            )));
        }
        return Ok(());
    };
    let Some(device) = device_id(pattern, uri) else {
        return Err(Box::new(failure(
            StatusCode::BAD_REQUEST,
            "invalid",
            "invalid device id",
        )));
    };
    check(
        auth,
        principal,
        device,
        rule.need,
        &format!("{} {}", rule.method, rule.path),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: i64, role: Role) -> Subject {
        Subject {
            guest: false,
            user_id: Some(id),
            role,
        }
    }

    const GUEST: Subject = Subject {
        guest: true,
        user_id: None,
        role: Role::Listener,
    };

    fn policy(view: Role, control: Role, guests: bool, users: &[i64]) -> DevicePolicy {
        DevicePolicy {
            view_role: view,
            control_role: control,
            guests_visible: guests,
            allowed_users: users.to_vec(),
        }
    }

    #[test]
    fn the_default_is_open_to_everybody_for_everything() {
        let open = DevicePolicy::default();
        for need in [Need::View, Need::Control] {
            assert!(open.allows(&GUEST, need));
            for role in [Role::Listener, Role::Operator, Role::Admin] {
                assert!(open.allows(&user(5, role), need), "{role:?}");
            }
        }
    }

    #[test]
    fn role_floors_apply_per_need() {
        let p = policy(Role::Operator, Role::Admin, true, &[]);
        assert!(!p.allows(&user(1, Role::Listener), Need::View));
        assert!(!p.allows(&GUEST, Need::View), "a guest is a listener");
        assert!(p.allows(&user(2, Role::Operator), Need::View));
        assert!(!p.allows(&user(2, Role::Operator), Need::Control));
        assert!(p.allows(&user(3, Role::Admin), Need::Control));

        let read_only = policy(Role::Listener, Role::Operator, true, &[]);
        assert!(read_only.allows(&user(1, Role::Listener), Need::View));
        assert!(!read_only.allows(&user(1, Role::Listener), Need::Control));
        assert!(read_only.allows(&GUEST, Need::View));
        assert!(!read_only.allows(&GUEST, Need::Control));
        assert!(read_only.allows(&user(2, Role::Operator), Need::Control));
    }

    #[test]
    fn guests_can_be_switched_off() {
        let p = policy(Role::Listener, Role::Listener, false, &[]);
        assert!(!p.allows(&GUEST, Need::View));
        assert!(p.allows(&user(1, Role::Listener), Need::View));
    }

    #[test]
    fn an_allow_list_restricts_to_those_accounts_and_admins() {
        let p = policy(Role::Listener, Role::Operator, true, &[7, 9]);
        assert!(p.allows(&user(7, Role::Listener), Need::View));
        assert!(!p.allows(&user(7, Role::Listener), Need::Control));
        assert!(p.allows(&user(9, Role::Operator), Need::Control));
        assert!(
            !p.allows(&user(8, Role::Operator), Need::View),
            "not listed"
        );
        assert!(
            !p.allows(&GUEST, Need::View),
            "no guests on a private device"
        );
        assert!(
            p.allows(&user(1, Role::Admin), Need::Control),
            "admins pass"
        );
    }

    #[test]
    fn a_demoted_listed_account_follows_its_role() {
        let p = policy(Role::Operator, Role::Operator, true, &[7]);
        assert!(!p.allows(&user(7, Role::Listener), Need::View));
    }

    #[test]
    fn normalise_sorts_dedupes_and_bounds_the_list() {
        let mut p = policy(Role::Listener, Role::Listener, true, &[9, 3, 9]);
        p.normalise().expect("valid");
        assert_eq!(p.allowed_users, [3, 9]);
        let mut p = policy(Role::Listener, Role::Listener, true, &[0]);
        assert!(p.normalise().is_err());
        let mut p = policy(Role::Listener, Role::Listener, true, &[]);
        p.allowed_users = (1..=201).collect();
        assert!(p.normalise().is_err());
    }

    #[test]
    fn policies_speak_role_names_and_refuse_unknown_fields() {
        let p: DevicePolicy = serde_json::from_str(
            r#"{"view_role":"operator","control_role":"admin","guests_visible":false,"allowed_users":[2]}"#,
        )
        .expect("parse");
        assert_eq!(p, policy(Role::Operator, Role::Admin, false, &[2]));
        let json = serde_json::to_value(&p).expect("serialise");
        assert_eq!(json["view_role"], "operator");
        assert!(serde_json::from_str::<DevicePolicy>(r#"{"view_role":"root"}"#).is_err());
        assert!(serde_json::from_str::<DevicePolicy>(r#"{"bogus":1}"#).is_err());
        let partial: DevicePolicy = serde_json::from_str(r#"{"guests_visible":false}"#).expect("");
        assert_eq!(partial.view_role, Role::Listener);
    }

    #[test]
    fn the_device_id_comes_from_the_path_or_the_query_and_never_decoded() {
        let uri = |s: &str| s.parse::<Uri>().expect("uri");
        let path = "/api/v1/devices/{dev}/vfos/{id}";
        assert_eq!(
            device_id(path, &uri("/api/v1/devices/rtl1/vfos/3")),
            Some("rtl1")
        );
        assert_eq!(device_id(path, &uri("/api/v1/devices/dev%30/vfos/3")), None);
        assert_eq!(device_id(path, &uri("/api/v1/devices/a.b/vfos/3")), None);
        let ws = "/api/v1/ws/audio";
        assert_eq!(
            device_id(ws, &uri("/api/v1/ws/audio?dev=rtl1")),
            Some("rtl1")
        );
        assert_eq!(
            device_id(ws, &uri("/api/v1/ws/audio?x=1&dev=rtl1&y=2")),
            Some("rtl1")
        );
        assert_eq!(device_id(ws, &uri("/api/v1/ws/audio")), None);
        assert_eq!(device_id(ws, &uri("/api/v1/ws/audio?dev=")), None);
        assert_eq!(device_id(ws, &uri("/api/v1/ws/audio?dev=a&dev=b")), None);
        assert_eq!(device_id(ws, &uri("/api/v1/ws/audio?dev=rtl%31")), None);
    }

    #[test]
    fn who_round_trips_to_a_subject() {
        let mut who = Principal::test(Role::Operator).who();
        assert_eq!(Subject::from_who(&who), user(1, Role::Operator));
        who.kind = ActorKind::Guest;
        who.id = Some("ab12".into());
        assert_eq!(Subject::from_who(&who), GUEST);
    }

    #[test]
    fn every_device_route_has_a_rule_and_every_rule_a_route() {
        for route in crate::access::RULES {
            let has_rule = DEVICE_RULES
                .iter()
                .any(|r| r.method == route.method && r.path == route.path);
            assert_eq!(
                has_rule,
                is_device_route(route.path),
                "{} {}: device rule and device route disagree",
                route.method,
                route.path
            );
        }
        for rule in DEVICE_RULES {
            assert!(
                crate::access::RULES
                    .iter()
                    .any(|r| r.method == rule.method && r.path == rule.path),
                "device rule {} {} matches no route",
                rule.method,
                rule.path
            );
        }
        // Anything that changes a device needs control; reading needs view.
        for rule in DEVICE_RULES {
            let reads = rule.method == "GET";
            assert_eq!(rule.need == View, reads, "{} {}", rule.method, rule.path);
        }
    }
}
