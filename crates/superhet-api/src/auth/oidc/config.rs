//! OIDC configuration: the admin-editable settings document, its validation, the
//! `SUPERHET_OIDC_*` environment form, and the pure mapping rules (claims to role and username).
//! Design: docs/adr/0019-oidc-login.md.

use std::fmt;

use openidconnect::url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use superhet_db::users::Role;

/// Settings key of the OIDC document.
pub const KEY_OIDC: &str = "oidc";

/// Path (below the public URL) the provider redirects back to.
pub const CALLBACK_PATH: &str = "/api/v1/auth/oidc/callback";
/// Path that starts a login.
pub const LOGIN_PATH: &str = "/api/v1/auth/oidc/login";

/// Environment variable: issuer URL. When set, the whole OIDC configuration comes from the
/// environment and the admin page shows it read-only.
pub const ENV_ISSUER: &str = "SUPERHET_OIDC_ISSUER";
/// Environment variable: client secret (kept out of the database on purpose).
pub const ENV_CLIENT_SECRET: &str = "SUPERHET_OIDC_CLIENT_SECRET";
/// Environment variable: file holding the client secret (Docker/Kubernetes secrets).
pub const ENV_CLIENT_SECRET_FILE: &str = "SUPERHET_OIDC_CLIENT_SECRET_FILE";
/// Environment variable: allow plain-http providers on non-loopback hosts (insecure).
pub const ENV_ALLOW_HTTP: &str = "SUPERHET_OIDC_ALLOW_HTTP";
/// Environment variable: keep password logins available whatever the settings say (recovery).
pub const ENV_KEEP_LOCAL_LOGIN: &str = "SUPERHET_OIDC_KEEP_LOCAL_LOGIN";
/// Environment variable: external base URL of this instance (`https://sdr.example.org`).
pub const ENV_PUBLIC_URL: &str = "SUPERHET_PUBLIC_URL";

/// Longest accepted value of the free-text fields, in characters.
const TEXT_MAX: usize = 200;
/// Most groups per role and most scopes.
const LIST_MAX: usize = 100;
/// Longest login name derived from a claim.
const USERNAME_MAX: usize = 64;

/// The admin-editable OIDC configuration (without the client secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OidcSettings {
    /// OIDC login is offered.
    pub enabled: bool,
    /// Label of the button: "Sign in with <name>".
    pub name: String,
    /// Issuer URL; `/.well-known/openid-configuration` is fetched from it.
    pub issuer: String,
    /// OAuth client id registered at the provider.
    pub client_id: String,
    /// External base URL of this instance; the redirect URI is derived from it.
    pub public_url: String,
    /// Scopes requested besides `openid`.
    pub scopes: Vec<String>,
    /// ID token claim listing the user's groups (dotted path for nested claims).
    pub groups_claim: String,
    /// ID token claim proposed as login name for new accounts.
    pub username_claim: String,
    /// Groups that grant the `admin` role.
    pub admin_groups: Vec<String>,
    /// Groups that grant the `operator` role.
    pub operator_groups: Vec<String>,
    /// Groups that grant the `listener` role.
    pub listener_groups: Vec<String>,
    /// Role when no group matches (`admin`, `operator`, `listener`); empty: sign-in is refused.
    pub default_role: String,
    /// Create an account on first login.
    pub auto_provision: bool,
    /// Link a first login to the existing account with the same verified email address.
    pub link_by_email: bool,
    /// Apply the mapped role to linked accounts at every login.
    pub sync_roles: bool,
    /// Password logins stay available. Can be switched off only while an enabled admin account
    /// is linked to an OIDC identity.
    pub local_login: bool,
}

impl Default for OidcSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            name: "Single sign-on".to_owned(),
            issuer: String::new(),
            client_id: String::new(),
            public_url: String::new(),
            scopes: vec!["email".to_owned(), "profile".to_owned()],
            groups_claim: "groups".to_owned(),
            username_claim: "preferred_username".to_owned(),
            admin_groups: Vec::new(),
            operator_groups: Vec::new(),
            listener_groups: Vec::new(),
            default_role: String::new(),
            auto_provision: false,
            link_by_email: false,
            sync_roles: true,
            local_login: true,
        }
    }
}

fn check_text(label: &str, value: &str) -> Result<(), String> {
    if value.chars().count() > TEXT_MAX || value.chars().any(char::is_control) {
        return Err(format!(
            "{label} must be at most {TEXT_MAX} characters without control characters"
        ));
    }
    Ok(())
}

fn check_list(label: &str, values: &[String]) -> Result<(), String> {
    if values.len() > LIST_MAX {
        return Err(format!("{label} has more than {LIST_MAX} entries"));
    }
    for v in values {
        if v.trim().is_empty() {
            return Err(format!("{label} must not contain empty entries"));
        }
        check_text(label, v)?;
    }
    Ok(())
}

/// Whether `url` is a well-formed `https://` URL (or `http://` for loopback hosts, or any host
/// when `allow_http`), without credentials, query or fragment.
fn check_base_url(label: &str, raw: &str, allow_http: bool) -> Result<url::Url, String> {
    let url = url::Url::parse(raw).map_err(|_| format!("{label} is not a valid URL"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{label} must not contain credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!("{label} must not contain a query or fragment"));
    }
    let Some(host) = url.host_str() else {
        return Err(format!("{label} needs a host"));
    };
    match url.scheme() {
        "https" => {}
        "http" if allow_http || is_loopback(host) => {}
        "http" => {
            return Err(format!(
                "{label} must be https:// (plain http is accepted for localhost only)"
            ));
        }
        _ => return Err(format!("{label} must be an https:// URL")),
    }
    Ok(url)
}

pub(crate) fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl OidcSettings {
    /// Check the document. Fields that matter only while OIDC is on (issuer, client id, public
    /// URL) are checked when `enabled`, or when they are set.
    ///
    /// # Errors
    /// A message naming the first problem.
    pub fn validate(&self, allow_http: bool) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
            return Err("name must be 1-40 characters".to_owned());
        }
        if self.enabled || !self.issuer.is_empty() {
            check_base_url("issuer", self.issuer.trim(), allow_http)?;
        }
        if self.enabled || !self.public_url.is_empty() {
            check_base_url("public_url", self.public_url.trim(), allow_http)?;
        }
        if self.enabled && self.client_id.trim().is_empty() {
            return Err("client_id is required".to_owned());
        }
        check_text("client_id", &self.client_id)?;
        check_list("scopes", &self.scopes)?;
        if self.scopes.iter().any(|s| s.contains(char::is_whitespace)) {
            return Err("a scope must not contain whitespace".to_owned());
        }
        check_text("groups_claim", &self.groups_claim)?;
        check_text("username_claim", &self.username_claim)?;
        check_list("admin_groups", &self.admin_groups)?;
        check_list("operator_groups", &self.operator_groups)?;
        check_list("listener_groups", &self.listener_groups)?;
        if !self.default_role.is_empty() && Role::parse(&self.default_role).is_none() {
            return Err("default_role must be admin, operator, listener or empty".to_owned());
        }
        if self.enabled
            && self.default_role.is_empty()
            && self.admin_groups.is_empty()
            && self.operator_groups.is_empty()
            && self.listener_groups.is_empty()
        {
            return Err(
                "map at least one group to a role or set a default_role, otherwise nobody can sign in"
                    .to_owned(),
            );
        }
        Ok(())
    }

    /// The URL the provider sends the browser back to (to be registered at the provider).
    ///
    /// # Errors
    /// When `public_url` is not set.
    pub fn redirect_url(&self) -> Result<String, String> {
        let base = self.public_url.trim().trim_end_matches('/');
        if base.is_empty() {
            return Err("public_url is not set".to_owned());
        }
        Ok(format!("{base}{CALLBACK_PATH}"))
    }

    /// Scopes sent to the provider: `openid` first, then the configured ones, without duplicates.
    #[must_use]
    pub fn request_scopes(&self) -> Vec<String> {
        let mut out = vec!["openid".to_owned()];
        for s in &self.scopes {
            let s = s.trim();
            if !s.is_empty() && !out.iter().any(|o| o == s) {
                out.push(s.to_owned());
            }
        }
        out
    }

    /// The role for users no group matches.
    #[must_use]
    pub fn default_role(&self) -> Option<Role> {
        Role::parse(&self.default_role)
    }

    /// The highest role any of `groups` grants, else the default role, else `None` (no access).
    #[must_use]
    pub fn map_role(&self, groups: &[String]) -> Option<Role> {
        let has = |wanted: &[String]| wanted.iter().any(|w| groups.iter().any(|g| g == w));
        if has(&self.admin_groups) {
            Some(Role::Admin)
        } else if has(&self.operator_groups) {
            Some(Role::Operator)
        } else if has(&self.listener_groups) {
            Some(Role::Listener)
        } else {
            self.default_role()
        }
    }

    /// Cache key: changes whenever the provider connection has to be rebuilt.
    #[must_use]
    pub fn connection_key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.issuer.trim(),
            self.client_id.trim(),
            self.public_url.trim()
        )
    }
}

/// Where the active configuration comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The `SUPERHET_OIDC_*` environment (read-only on the admin page).
    Env,
    /// The settings table (editable on the admin page).
    Database,
}

/// A secret that does not show up in logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a secret.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// The secret itself.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// The OIDC part of the process environment.
#[derive(Debug, Clone, Default)]
pub struct OidcEnv {
    /// A complete configuration given through the environment, if `SUPERHET_OIDC_ISSUER` is set.
    pub settings: Option<OidcSettings>,
    /// Client secret (applies to environment and database configuration alike).
    pub client_secret: Option<Secret>,
    /// Accept plain-http providers on any host.
    pub allow_http: bool,
    /// Never lock out password logins.
    pub keep_local_login: bool,
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn split_list(value: &str, whitespace_too: bool) -> Vec<String> {
    value
        .split(|c: char| c == ',' || (whitespace_too && c.is_whitespace()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

impl OidcEnv {
    /// Read the `SUPERHET_OIDC_*` and `SUPERHET_PUBLIC_URL` variables.
    ///
    /// # Errors
    /// A message when a value is invalid or the secret file cannot be read.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(
            |name| std::env::var(name).ok(),
            |path| std::fs::read_to_string(path).map_err(|e| e.to_string()),
        )
    }

    /// [`OidcEnv::from_env`] with injectable sources (tests).
    ///
    /// # Errors
    /// A message when a value is invalid or the secret file cannot be read.
    pub fn from_lookup(
        get: impl Fn(&str) -> Option<String>,
        read_file: impl Fn(&str) -> Result<String, String>,
    ) -> Result<Self, String> {
        let var = |name: &str| {
            get(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let flag = |name: &str| get(name).is_some_and(|v| truthy(&v));
        let client_secret = match (var(ENV_CLIENT_SECRET), var(ENV_CLIENT_SECRET_FILE)) {
            (Some(secret), _) => Some(Secret::new(secret)),
            (None, Some(path)) => {
                let text = read_file(&path)
                    .map_err(|e| format!("{ENV_CLIENT_SECRET_FILE}: cannot read {path}: {e}"))?;
                let text = text.trim().to_owned();
                (!text.is_empty()).then(|| Secret::new(text))
            }
            (None, None) => None,
        };
        let allow_http = flag(ENV_ALLOW_HTTP);
        let settings = if let Some(issuer) = var(ENV_ISSUER) {
            let mut s = OidcSettings {
                enabled: true,
                issuer,
                ..OidcSettings::default()
            };
            if let Some(v) = var("SUPERHET_OIDC_NAME") {
                s.name = v;
            }
            if let Some(v) = var("SUPERHET_OIDC_CLIENT_ID") {
                s.client_id = v;
            }
            if let Some(v) = var(ENV_PUBLIC_URL) {
                s.public_url = v;
            }
            if let Some(v) = var("SUPERHET_OIDC_SCOPES") {
                // `openid` is always sent; listing it is harmless.
                s.scopes = split_list(&v, true)
                    .into_iter()
                    .filter(|x| x != "openid")
                    .collect();
            }
            if let Some(v) = var("SUPERHET_OIDC_GROUPS_CLAIM") {
                s.groups_claim = v;
            }
            if let Some(v) = var("SUPERHET_OIDC_USERNAME_CLAIM") {
                s.username_claim = v;
            }
            if let Some(v) = var("SUPERHET_OIDC_ADMIN_GROUPS") {
                s.admin_groups = split_list(&v, false);
            }
            if let Some(v) = var("SUPERHET_OIDC_OPERATOR_GROUPS") {
                s.operator_groups = split_list(&v, false);
            }
            if let Some(v) = var("SUPERHET_OIDC_LISTENER_GROUPS") {
                s.listener_groups = split_list(&v, false);
            }
            if let Some(v) = var("SUPERHET_OIDC_DEFAULT_ROLE") {
                s.default_role = v.to_ascii_lowercase();
            }
            s.auto_provision = flag("SUPERHET_OIDC_AUTO_PROVISION");
            s.link_by_email = flag("SUPERHET_OIDC_LINK_BY_EMAIL");
            if let Some(v) = var("SUPERHET_OIDC_SYNC_ROLES") {
                s.sync_roles = truthy(&v);
            }
            if let Some(v) = var("SUPERHET_OIDC_LOCAL_LOGIN") {
                s.local_login = truthy(&v);
            }
            s.validate(allow_http)
                .map_err(|e| format!("SUPERHET_OIDC_*: {e}"))?;
            Some(s)
        } else {
            None
        };
        Ok(Self {
            settings,
            client_secret,
            allow_http,
            keep_local_login: flag(ENV_KEEP_LOCAL_LOGIN),
        })
    }
}

/// Read a claim from an ID token payload: the key itself, else a dotted path into nested
/// objects (`realm_access.roles`).
#[must_use]
pub fn claim<'a>(payload: &'a Value, name: &str) -> Option<&'a Value> {
    if name.is_empty() {
        return None;
    }
    if let Some(v) = payload.get(name) {
        return Some(v);
    }
    let mut cur = payload;
    for part in name.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

/// A claim as a list of strings: an array of strings, or one string.
#[must_use]
pub fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// Some providers send `email_verified` as the string "true".
#[must_use]
pub fn is_true(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A login name from a claim value: characters outside `[A-Za-z0-9._-]` become `-`.
#[must_use]
pub fn sanitize_username(raw: &str) -> String {
    let mapped: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(USERNAME_MAX)
        .collect();
    mapped.trim_matches(['-', '.']).to_owned()
}

/// What the ID token says about the user (verified token, configured claim names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Issuer the token was verified against.
    pub issuer: String,
    /// The provider's stable user id.
    pub subject: String,
    /// Email address, if the token has one.
    pub email: Option<String>,
    /// The provider vouches for the address.
    pub email_verified: bool,
    /// Group names from the configured claim.
    pub groups: Vec<String>,
    /// Proposed login name for a new account.
    pub username: String,
}

impl Identity {
    /// Read the identity out of an already verified ID token payload.
    #[must_use]
    pub fn from_payload(
        issuer: &str,
        subject: &str,
        payload: &Value,
        settings: &OidcSettings,
    ) -> Self {
        let email = payload
            .get("email")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|e| (3..=254).contains(&e.len()) && e.contains('@'))
            .map(str::to_owned);
        let from_claim = claim(payload, &settings.username_claim)
            .and_then(Value::as_str)
            .map(sanitize_username)
            .filter(|n| !n.is_empty());
        let from_email = email
            .as_deref()
            .and_then(|e| e.split('@').next())
            .map(sanitize_username)
            .filter(|n| !n.is_empty());
        let username = from_claim
            .or(from_email)
            .unwrap_or_else(|| "user".to_owned());
        Self {
            issuer: issuer.to_owned(),
            subject: subject.to_owned(),
            email_verified: email.is_some() && is_true(payload.get("email_verified")),
            email,
            groups: string_list(claim(payload, &settings.groups_claim)),
            username,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mapping() -> OidcSettings {
        OidcSettings {
            admin_groups: vec!["sh-admins".into()],
            operator_groups: vec!["sh-ops".into(), "radio".into()],
            listener_groups: vec!["sh-users".into()],
            ..OidcSettings::default()
        }
    }

    fn g(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_highest_matching_group_wins() {
        let s = mapping();
        assert_eq!(
            s.map_role(&g(&["sh-users", "sh-admins"])),
            Some(Role::Admin)
        );
        assert_eq!(s.map_role(&g(&["sh-users", "radio"])), Some(Role::Operator));
        assert_eq!(s.map_role(&g(&["sh-users"])), Some(Role::Listener));
    }

    #[test]
    fn unmatched_users_get_the_default_role_or_nothing() {
        let mut s = mapping();
        assert_eq!(s.map_role(&g(&["other"])), None);
        assert_eq!(s.map_role(&[]), None);
        s.default_role = "listener".into();
        assert_eq!(s.map_role(&g(&["other"])), Some(Role::Listener));
        // Group names are compared exactly.
        assert_eq!(s.map_role(&g(&["SH-ADMINS"])), Some(Role::Listener));
    }

    #[test]
    fn claims_resolve_plain_and_dotted_names() {
        let p = json!({
            "groups": ["a", "b"],
            "realm_access": {"roles": ["r1"]},
            "https://x.example/roles": "solo",
        });
        assert_eq!(string_list(claim(&p, "groups")), g(&["a", "b"]));
        assert_eq!(string_list(claim(&p, "realm_access.roles")), g(&["r1"]));
        assert_eq!(
            string_list(claim(&p, "https://x.example/roles")),
            g(&["solo"])
        );
        assert!(string_list(claim(&p, "missing")).is_empty());
        assert!(claim(&p, "").is_none());
        assert!(string_list(Some(&json!(7))).is_empty());
    }

    #[test]
    fn email_verified_accepts_bool_and_string() {
        assert!(is_true(Some(&json!(true))));
        assert!(is_true(Some(&json!("true"))));
        assert!(!is_true(Some(&json!(false))));
        assert!(!is_true(Some(&json!("yes"))));
        assert!(!is_true(None));
    }

    #[test]
    fn usernames_are_sanitised() {
        assert_eq!(sanitize_username("Anna Müller"), "Anna-M-ller");
        assert_eq!(sanitize_username("  bob@example.org "), "bob-example.org");
        assert_eq!(sanitize_username("--x--"), "x");
        assert_eq!(sanitize_username("???"), "");
        assert_eq!(sanitize_username(&"a".repeat(100)).len(), 64);
    }

    #[test]
    fn identity_prefers_the_claim_then_the_email_then_a_fallback() {
        let s = OidcSettings::default();
        let p = json!({"preferred_username": "anna", "email": "a.b@x.org", "email_verified": true,
                       "groups": ["g"]});
        let id = Identity::from_payload("https://i", "s1", &p, &s);
        assert_eq!(id.username, "anna");
        assert_eq!(id.email.as_deref(), Some("a.b@x.org"));
        assert!(id.email_verified);
        assert_eq!(id.groups, g(&["g"]));
        let id = Identity::from_payload("https://i", "s1", &json!({"email": "a.b@x.org"}), &s);
        assert_eq!(id.username, "a.b");
        assert!(
            !id.email_verified,
            "no email_verified claim means unverified"
        );
        let id = Identity::from_payload("https://i", "s1", &json!({}), &s);
        assert_eq!(id.username, "user");
        assert_eq!(id.email, None);
        // A verified flag without an address means nothing.
        let id = Identity::from_payload("https://i", "s1", &json!({"email_verified": true}), &s);
        assert!(!id.email_verified);
        let id = Identity::from_payload("https://i", "s1", &json!({"email": "no-at-sign"}), &s);
        assert_eq!(id.email, None);
    }

    fn enabled() -> OidcSettings {
        OidcSettings {
            enabled: true,
            issuer: "https://idp.example".into(),
            client_id: "superhet".into(),
            public_url: "https://sdr.example".into(),
            default_role: "listener".into(),
            ..OidcSettings::default()
        }
    }

    #[test]
    fn validation_catches_unusable_documents() {
        assert!(OidcSettings::default().validate(false).is_ok());
        assert!(enabled().validate(false).is_ok());
        let mut s = enabled();
        s.issuer = "http://idp.example".into();
        assert!(s.validate(false).is_err(), "plain http to a remote host");
        assert!(s.validate(true).is_ok(), "unless the operator opted in");
        s.issuer = "http://127.0.0.1:9000".into();
        assert!(s.validate(false).is_ok(), "loopback is fine");
        s.issuer = "https://user:pw@idp.example".into();
        assert!(s.validate(false).is_err());
        s.issuer = "https://idp.example/?x=1".into();
        assert!(s.validate(false).is_err());
        let mut s = enabled();
        s.client_id = " ".into();
        assert!(s.validate(false).is_err());
        let mut s = enabled();
        s.public_url = String::new();
        assert!(s.validate(false).is_err());
        let mut s = enabled();
        s.default_role = "root".into();
        assert!(s.validate(false).is_err());
        let mut s = enabled();
        s.default_role = String::new();
        assert!(s.validate(false).is_err(), "nobody could sign in");
        s.admin_groups = g(&["a"]);
        assert!(s.validate(false).is_ok());
        let mut s = enabled();
        s.scopes = g(&["a b"]);
        assert!(s.validate(false).is_err());
    }

    #[test]
    fn redirect_url_and_scopes_are_derived() {
        let mut s = enabled();
        s.public_url = "https://sdr.example/".into();
        assert_eq!(
            s.redirect_url().as_deref(),
            Ok("https://sdr.example/api/v1/auth/oidc/callback")
        );
        s.scopes = g(&["email", "openid", "email", "groups"]);
        assert_eq!(s.request_scopes(), g(&["openid", "email", "groups"]));
        assert!(OidcSettings::default().redirect_url().is_err());
    }

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn no_files(_: &str) -> Result<String, String> {
        Err("no files in this test".into())
    }

    #[test]
    fn the_environment_form_builds_a_complete_configuration() {
        let env = OidcEnv::from_lookup(
            lookup(&[
                ("SUPERHET_OIDC_ISSUER", "https://idp.example"),
                ("SUPERHET_OIDC_CLIENT_ID", "superhet"),
                ("SUPERHET_OIDC_CLIENT_SECRET", " s3cret "),
                ("SUPERHET_PUBLIC_URL", "https://sdr.example"),
                ("SUPERHET_OIDC_NAME", "Authentik"),
                ("SUPERHET_OIDC_SCOPES", "openid email groups"),
                ("SUPERHET_OIDC_ADMIN_GROUPS", "Superhet Admins, root"),
                ("SUPERHET_OIDC_DEFAULT_ROLE", "Listener"),
                ("SUPERHET_OIDC_AUTO_PROVISION", "true"),
                ("SUPERHET_OIDC_LOCAL_LOGIN", "false"),
            ]),
            no_files,
        )
        .expect("valid");
        let s = env.settings.expect("settings");
        assert!(s.enabled && s.auto_provision && !s.local_login && !s.link_by_email);
        assert_eq!(s.name, "Authentik");
        assert_eq!(s.scopes, g(&["email", "groups"]));
        assert_eq!(s.admin_groups, g(&["Superhet Admins", "root"]));
        assert_eq!(s.default_role(), Some(Role::Listener));
        assert_eq!(env.client_secret.expect("secret").expose(), "s3cret");
        assert!(!env.allow_http && !env.keep_local_login);
    }

    #[test]
    fn without_an_issuer_the_environment_configures_nothing() {
        let env = OidcEnv::from_lookup(
            lookup(&[
                ("SUPERHET_OIDC_CLIENT_SECRET", "x"),
                ("SUPERHET_OIDC_KEEP_LOCAL_LOGIN", "1"),
            ]),
            no_files,
        )
        .expect("ok");
        assert!(env.settings.is_none());
        assert!(env.keep_local_login);
        assert!(
            env.client_secret.is_some(),
            "the secret also serves a database configuration"
        );
    }

    #[test]
    fn an_invalid_environment_configuration_is_an_error() {
        let err = OidcEnv::from_lookup(
            lookup(&[("SUPERHET_OIDC_ISSUER", "https://idp.example")]),
            no_files,
        )
        .expect_err("client id and public url missing");
        assert!(err.starts_with("SUPERHET_OIDC_*"), "{err}");
    }

    #[test]
    fn the_secret_can_come_from_a_file_and_never_prints() {
        let env = OidcEnv::from_lookup(
            lookup(&[("SUPERHET_OIDC_CLIENT_SECRET_FILE", "/run/secrets/oidc")]),
            |path| {
                assert_eq!(path, "/run/secrets/oidc");
                Ok("from-file\n".to_owned())
            },
        )
        .expect("ok");
        let secret = env.client_secret.clone().expect("secret");
        assert_eq!(secret.expose(), "from-file");
        assert!(!format!("{env:?}").contains("from-file"));
        assert!(
            OidcEnv::from_lookup(
                lookup(&[("SUPERHET_OIDC_CLIENT_SECRET_FILE", "/nope")]),
                no_files
            )
            .is_err()
        );
    }
}
