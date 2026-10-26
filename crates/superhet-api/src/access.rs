//! Who is calling and what they may do (ADR-0011).
//!
//! Every protected route has an entry in [`RULES`] naming the weakest access that may call it;
//! the middleware in `auth` looks the matched route up there. A route without an entry is
//! refused (fail closed) and a test fails, so a new endpoint cannot ship unguarded. Handlers
//! get the resolved [`Principal`] as a request extension for ownership and policy checks.

use std::net::IpAddr;

use axum::http::Method;
use serde_json::Value;
pub use superhet_db::users::Role;

use crate::audit::{ActorKind, Who};

/// The caller of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// A logged-in account (user id).
    User(i64),
    /// An anonymous guest session (public guest id).
    Guest(String),
}

/// An authenticated caller: an account or a guest.
#[derive(Debug, Clone)]
pub struct Principal {
    /// Account or guest.
    pub actor: Actor,
    /// Login name; `guest` for guests.
    pub name: String,
    /// Role; guests have the listener role (further limited by the guest policy).
    pub role: Role,
    /// Key of the session (hex token hash, or `g-<id>`), used for kicks and connection lists.
    pub session_key: String,
    /// Client address as seen by the API.
    pub ip: Option<IpAddr>,
    /// Truncated user agent.
    pub user_agent: Option<String>,
    /// The account must change its password before anything else.
    pub must_change_password: bool,
}

impl Principal {
    /// Whether this is a guest.
    #[must_use]
    pub fn is_guest(&self) -> bool {
        matches!(self.actor, Actor::Guest(_))
    }

    /// Owner string attached to VFOs and lock holders: `u:<id>` or `g:<id>`.
    #[must_use]
    pub fn owner_id(&self) -> String {
        match &self.actor {
            Actor::User(id) => format!("u:{id}"),
            Actor::Guest(id) => format!("g:{id}"),
        }
    }

    /// May modify and hear every VFO, not just its own.
    #[must_use]
    pub fn sees_all_vfos(&self) -> bool {
        !self.is_guest() && self.role >= Role::Operator
    }

    /// The actor for audit rows.
    #[must_use]
    pub fn who(&self) -> Who {
        let (kind, id) = match &self.actor {
            Actor::User(id) => (ActorKind::User, id.to_string()),
            Actor::Guest(id) => (ActorKind::Guest, id.clone()),
        };
        Who {
            kind,
            id: Some(id),
            name: Some(self.name.clone()),
            role: Some(self.role),
            ip: self.ip,
            user_agent: self.user_agent.clone(),
        }
    }

    /// A principal for tests of single handlers.
    #[cfg(test)]
    #[must_use]
    pub fn test(role: Role) -> Self {
        Self {
            actor: Actor::User(1),
            name: "tester".into(),
            role,
            session_key: "test-session".into(),
            ip: None,
            user_agent: None,
            must_change_password: false,
        }
    }
}

/// Wrap a router so its handlers see an admin principal (unit tests of single routes).
#[cfg(test)]
pub fn as_test_principal(router: axum::Router) -> axum::Router {
    router.layer(axum::Extension(Principal::test(Role::Admin)))
}

/// The weakest caller a route accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Any signed-in caller, guests included.
    Guest,
    /// Guests only if the guest policy shows decoder events; every account.
    GuestDecoder,
    /// Any account (listener or better), no guests.
    Listener,
    /// Operators and admins.
    Operator,
    /// Admins only.
    Admin,
}

impl Access {
    /// Whether `principal` may pass, given whether guests may see decoder events.
    #[must_use]
    pub fn allows(self, principal: &Principal, guests_see_decoder_events: bool) -> bool {
        if principal.is_guest() {
            return match self {
                Self::Guest => true,
                Self::GuestDecoder => guests_see_decoder_events,
                _ => false,
            };
        }
        let needed = match self {
            Self::Guest | Self::GuestDecoder | Self::Listener => Role::Listener,
            Self::Operator => Role::Operator,
            Self::Admin => Role::Admin,
        };
        principal.role >= needed
    }
}

/// One protected route.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// HTTP method (HEAD is matched as GET).
    pub method: &'static str,
    /// Route pattern exactly as registered with axum.
    pub path: &'static str,
    /// Weakest access that may call it.
    pub access: Access,
}

const fn rule(method: &'static str, path: &'static str, access: Access) -> Rule {
    Rule {
        method,
        path,
        access,
    }
}

use Access::{Admin, Guest, GuestDecoder, Listener, Operator};

/// The access table of every route behind the session check.
pub const RULES: &[Rule] = &[
    // Device state, tuning and VFOs.
    rule("GET", "/api/v1/devices/{dev}/state", Guest),
    rule("POST", "/api/v1/devices/{dev}/tune", Operator),
    rule("POST", "/api/v1/devices/{dev}/lock", Operator),
    rule("DELETE", "/api/v1/devices/{dev}/lock", Operator),
    rule("GET", "/api/v1/devices/{dev}/vfos", Guest),
    rule("POST", "/api/v1/devices/{dev}/vfos", Guest),
    rule("PATCH", "/api/v1/devices/{dev}/vfos/{id}", Guest),
    rule("DELETE", "/api/v1/devices/{dev}/vfos/{id}", Guest),
    // WebSocket upgrades.
    rule("GET", "/api/v1/ws/spectrum", Guest),
    rule("GET", "/api/v1/ws/audio", Guest),
    rule("GET", "/api/v1/ws/state", Guest),
    rule("GET", "/api/v1/ws/events", GuestDecoder),
    // Decoder history.
    rule("GET", "/api/v1/decoder-events", GuestDecoder),
    rule("GET", "/api/v1/decoder-events/export.csv", GuestDecoder),
    rule("DELETE", "/api/v1/decoder-events", Operator),
    // Profiles.
    rule("GET", "/api/v1/profiles", Operator),
    rule("POST", "/api/v1/profiles", Operator),
    rule("GET", "/api/v1/profiles/{id}", Operator),
    rule("PUT", "/api/v1/profiles/{id}", Operator),
    rule("DELETE", "/api/v1/profiles/{id}", Operator),
    rule("POST", "/api/v1/devices/{dev}/profile/save", Operator),
    rule("POST", "/api/v1/devices/{dev}/profile/load", Operator),
    // Bookmarks: everybody with an account reads, operators edit.
    rule("GET", "/api/v1/bookmarks", Listener),
    rule("POST", "/api/v1/bookmarks", Operator),
    rule("POST", "/api/v1/bookmarks/bulk", Operator),
    rule("PATCH", "/api/v1/bookmarks/{id}", Operator),
    rule("DELETE", "/api/v1/bookmarks/{id}", Operator),
    rule("GET", "/api/v1/bookmark-groups", Listener),
    rule("POST", "/api/v1/bookmark-groups", Operator),
    rule("PUT", "/api/v1/bookmark-groups/order", Operator),
    rule("PATCH", "/api/v1/bookmark-groups/{id}", Operator),
    rule("DELETE", "/api/v1/bookmark-groups/{id}", Operator),
    // Own sessions.
    rule("GET", "/api/v1/me/sessions", Listener),
    rule("DELETE", "/api/v1/me/sessions/{sid}", Listener),
    // Administration.
    rule("GET", "/api/v1/admin/users", Admin),
    rule("POST", "/api/v1/admin/users", Admin),
    rule("GET", "/api/v1/admin/users/{id}", Admin),
    rule("PATCH", "/api/v1/admin/users/{id}", Admin),
    rule("DELETE", "/api/v1/admin/users/{id}", Admin),
    rule("POST", "/api/v1/admin/users/{id}/password", Admin),
    rule("GET", "/api/v1/admin/invites", Admin),
    rule("POST", "/api/v1/admin/invites", Admin),
    rule("DELETE", "/api/v1/admin/invites/{id}", Admin),
    rule("GET", "/api/v1/admin/sessions", Admin),
    rule("DELETE", "/api/v1/admin/sessions/{sid}", Admin),
    rule("GET", "/api/v1/admin/audit", Admin),
    rule("GET", "/api/v1/admin/audit/export", Admin),
    rule("GET", "/api/v1/admin/guest-policy", Admin),
    rule("PUT", "/api/v1/admin/guest-policy", Admin),
    rule("GET", "/api/v1/admin/audit-settings", Admin),
    rule("PUT", "/api/v1/admin/audit-settings", Admin),
    rule("GET", "/api/v1/admin/limits", Admin),
    rule("PUT", "/api/v1/admin/limits", Admin),
];

/// The rule for a matched route and method (HEAD counts as GET).
#[must_use]
pub fn find_rule(method: &Method, path: &str) -> Option<&'static Rule> {
    let method = if method == Method::HEAD {
        "GET"
    } else {
        method.as_str()
    };
    RULES.iter().find(|r| r.method == method && r.path == path)
}

/// Prepare a device state document for one viewer: guests see no names, only operators and
/// admins see who owns a VFO; everybody gets `mine` and `shared` per VFO.
pub fn view_state(doc: &mut Value, viewer: &Principal) {
    let me = viewer.owner_id();
    let privileged = viewer.sees_all_vfos();
    if let Some(vfos) = doc.get_mut("vfos").and_then(Value::as_array_mut) {
        for vfo in vfos {
            let Some(vfo) = vfo.as_object_mut() else {
                continue;
            };
            let mine = vfo.get("owner").and_then(Value::as_str) == Some(me.as_str());
            // Ownerless VFOs are shared: everybody may listen. Without this flag a listener
            // could not tell a shared VFO from somebody else's (the owner is hidden).
            let shared = vfo.get("owner").is_none_or(Value::is_null);
            vfo.insert("mine".into(), Value::Bool(mine));
            vfo.insert("shared".into(), Value::Bool(shared));
            if !privileged {
                vfo.remove("owner");
            }
        }
    }
    if viewer.is_guest()
        && let Some(locks) = doc.get_mut("locks").and_then(Value::as_array_mut)
    {
        for lock in locks {
            if let Some(lock) = lock.as_object_mut() {
                lock.remove("name");
                lock.remove("id");
            }
        }
    }
}

/// Prepare a daemon reply for one viewer. `/tune`, lock, VFO and profile replies carry the new
/// state document and the changed VFO; both get the same treatment as the state feed (`mine`,
/// `shared`, no foreign owners for listeners and guests, no lock holder names for guests).
pub fn view_reply(reply: &mut Value, viewer: &Principal) {
    if let Some(state) = reply.get_mut("state") {
        view_state(state, viewer);
    }
    if let Some(vfo) = reply.get_mut("vfo") {
        let mut doc = serde_json::json!({ "vfos": [vfo.take()] });
        view_state(&mut doc, viewer);
        *vfo = doc["vfos"][0].take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guest() -> Principal {
        Principal {
            actor: Actor::Guest("ab12".into()),
            name: "guest".into(),
            role: Role::Listener,
            session_key: "g-ab12".into(),
            ip: None,
            user_agent: None,
            must_change_password: false,
        }
    }

    #[test]
    fn access_matrix() {
        let p = |role| Principal::test(role);
        for (access, listener, operator, admin, g, g_dec) in [
            (Access::Guest, true, true, true, true, true),
            (Access::GuestDecoder, true, true, true, false, true),
            (Access::Listener, true, true, true, false, false),
            (Access::Operator, false, true, true, false, false),
            (Access::Admin, false, false, true, false, false),
        ] {
            assert_eq!(
                access.allows(&p(Role::Listener), false),
                listener,
                "{access:?} listener"
            );
            assert_eq!(
                access.allows(&p(Role::Operator), false),
                operator,
                "{access:?} operator"
            );
            assert_eq!(
                access.allows(&p(Role::Admin), false),
                admin,
                "{access:?} admin"
            );
            assert_eq!(access.allows(&guest(), false), g, "{access:?} guest");
            assert_eq!(
                access.allows(&guest(), true),
                g_dec,
                "{access:?} guest+decoder"
            );
        }
    }

    #[test]
    fn rules_are_unique_and_well_formed() {
        for (i, a) in RULES.iter().enumerate() {
            assert!(a.path.starts_with("/api/v1/"), "{}", a.path);
            assert!(["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&a.method));
            for b in &RULES[i + 1..] {
                assert!(
                    !(a.method == b.method && a.path == b.path),
                    "duplicate {} {}",
                    a.method,
                    a.path
                );
            }
        }
        assert!(find_rule(&Method::HEAD, "/api/v1/bookmarks").is_some());
        assert!(find_rule(&Method::POST, "/api/v1/nothing").is_none());
        // Everything under /admin is admin-only, whatever the method.
        assert!(
            RULES
                .iter()
                .filter(|r| r.path.contains("/admin/"))
                .all(|r| r.access == Access::Admin)
        );
    }

    /// Every route registered in the protected routers, read from the sources (test modules
    /// cut off). A new `.route(...)` without an entry in [`RULES`] fails here.
    fn registered_paths() -> Vec<String> {
        let sources = [
            include_str!("devices.rs"),
            include_str!("ws.rs"),
            include_str!("audio_ws.rs"),
            include_str!("state_ws.rs"),
            include_str!("decoder_events.rs"),
            include_str!("profiles.rs"),
            include_str!("bookmarks.rs"),
            include_str!("admin/mod.rs"),
        ];
        let mut paths = Vec::new();
        for source in sources {
            let source = source
                .split("#[cfg(test)]\nmod tests")
                .next()
                .unwrap_or(source);
            for chunk in source.split(".route(").skip(1) {
                let first = chunk.split(',').next().unwrap_or("").trim();
                let path = if let Some(literal) = first.strip_prefix('"') {
                    literal.trim_end_matches('"').to_owned()
                } else {
                    // A constant such as `PATH` defined in the same file.
                    let marker = format!("const {first}: &str = \"");
                    let rest = source
                        .split(&marker)
                        .nth(1)
                        .unwrap_or_else(|| panic!("cannot resolve route {first}"));
                    rest.split('"').next().unwrap_or("").to_owned()
                };
                paths.push(path);
            }
        }
        paths
    }

    #[test]
    fn every_registered_route_has_an_access_rule_and_every_rule_a_route() {
        let paths = registered_paths();
        assert!(paths.len() >= 30, "found only {} routes", paths.len());
        for path in &paths {
            assert!(
                RULES.iter().any(|r| r.path == path),
                "route {path} has no entry in access::RULES"
            );
        }
        for rule in RULES {
            assert!(
                paths.iter().any(|p| p == rule.path),
                "rule {} {} matches no registered route",
                rule.method,
                rule.path
            );
        }
    }

    #[test]
    fn owner_ids_and_visibility() {
        assert_eq!(Principal::test(Role::Listener).owner_id(), "u:1");
        assert_eq!(guest().owner_id(), "g:ab12");
        assert!(Principal::test(Role::Operator).sees_all_vfos());
        assert!(!Principal::test(Role::Listener).sees_all_vfos());
        assert!(!guest().sees_all_vfos());
    }

    #[test]
    fn replies_are_prepared_for_the_viewer_too() {
        let reply = json!({
            "ok": true,
            "vfo": {"id": 2, "owner": "g:ab12"},
            "state": {
                "vfos": [{"id": 1, "owner": "u:1"}, {"id": 2, "owner": "g:ab12"}, {"id": 3}],
                "locks": [{"kind": "user", "id": "u:1", "name": "tester", "since_ms": 5}],
            },
        });
        let mut g = reply.clone();
        view_reply(&mut g, &guest());
        assert_eq!(g["vfo"]["mine"], true);
        assert_eq!(g["vfo"]["shared"], false);
        assert!(g["vfo"].get("owner").is_none());
        assert_eq!(g["state"]["vfos"][0]["mine"], false);
        assert_eq!(g["state"]["vfos"][2]["shared"], true);
        assert!(g["state"]["vfos"][1].get("owner").is_none());
        assert!(g["state"]["locks"][0].get("name").is_none());
        assert_eq!(g["ok"], true);

        let mut op = reply;
        view_reply(&mut op, &Principal::test(Role::Operator));
        assert_eq!(op["vfo"]["owner"], "g:ab12");
        assert_eq!(op["state"]["vfos"][1]["mine"], false);
        // A reply without state or vfo (an error body) is left alone.
        let mut err = json!({"ok": false, "code": "locked"});
        view_reply(&mut err, &guest());
        assert_eq!(err, json!({"ok": false, "code": "locked"}));
    }

    #[test]
    fn state_views_hide_owners_and_names() {
        let doc = json!({
            "vfos": [{"id": 1, "owner": "u:1"}, {"id": 2, "owner": "g:ab12"}, {"id": 3}],
            "locks": [{"kind": "user", "id": "u:1", "name": "tester", "since_ms": 5}],
        });
        let mut listener = doc.clone();
        view_state(&mut listener, &Principal::test(Role::Listener));
        assert_eq!(listener["vfos"][0]["mine"], true);
        assert_eq!(listener["vfos"][1]["mine"], false);
        assert!(listener["vfos"][0].get("owner").is_none());
        assert_eq!(listener["vfos"][1]["shared"], false);
        assert_eq!(listener["vfos"][2]["shared"], true);
        assert_eq!(listener["vfos"][2]["mine"], false);
        assert_eq!(listener["locks"][0]["name"], "tester");

        let mut operator = doc.clone();
        view_state(&mut operator, &Principal::test(Role::Operator));
        assert_eq!(operator["vfos"][1]["owner"], "g:ab12");

        let mut g = doc;
        view_state(&mut g, &guest());
        assert_eq!(g["vfos"][1]["mine"], true);
        assert!(g["vfos"][1].get("owner").is_none());
        assert!(g["locks"][0].get("name").is_none() && g["locks"][0].get("id").is_none());
        assert_eq!(g["locks"][0]["kind"], "user");
    }
}
