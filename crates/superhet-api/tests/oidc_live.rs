//! OIDC login against a mock provider (`common::idp`): the real `superhet-api` binary on a real
//! PostgreSQL (needs `DATABASE_URL`; skipped otherwise). The test plays the browser; no real
//! identity provider is involved.

mod common;

use common::idp::{CLIENT_SECRET, Flaw, MockIdp};
use common::*;
use openidconnect::url::Url;
use serde_json::{Value, json};

const ISS_SUB: &str = "sub-1";

struct Rig {
    _db: TestDb,
    _api: Proc,
    port: u16,
    idp: MockIdp,
    admin: String,
}

/// Where the provider sends the browser back to, as the API sees itself.
fn public_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn base_settings(rig_idp: &MockIdp, port: u16) -> Value {
    json!({
        "enabled": true,
        "name": "Mock ID",
        "issuer": rig_idp.base,
        "client_id": common::idp::CLIENT_ID,
        "public_url": public_url(port),
        "admin_groups": ["sh-admins"],
        "operator_groups": ["sh-ops"],
        "default_role": "listener",
        "auto_provision": true,
    })
}

async fn rig(tag: &str, extra_env: &[(&'static str, String)]) -> Option<Rig> {
    let db = TestDb::create(tag).await?;
    let idp = MockIdp::start().await;
    let extra: Vec<(&'static str, String)> = extra_env.to_vec();
    let (api, port) = spawn_api(&db.url, move |cmd| {
        cmd.env("SUPERHET_OIDC_CLIENT_SECRET", CLIENT_SECRET);
        for (k, v) in &extra {
            cmd.env(k, v);
        }
    });
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    Some(Rig {
        _db: db,
        _api: api,
        port,
        idp,
        admin,
    })
}

macro_rules! rig {
    ($tag:expr) => {
        match rig($tag, &[]).await {
            Some(r) => r,
            None => {
                eprintln!("DATABASE_URL not set; skipping");
                return;
            }
        }
    };
}

impl Rig {
    fn admin(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        req(self.port, method, path, Some(&self.admin), body)
    }

    fn configure(&self, settings: Value) {
        let reply = self.admin("PUT", "/api/v1/admin/oidc", Some(&settings));
        assert_eq!(reply.status, 200, "{reply:?}");
    }

    /// Start a login; returns the provider URL and the binding cookie.
    fn start(&self) -> (String, String) {
        let reply = http(self.port, "GET", "/api/v1/auth/oidc/login", &[], "");
        assert_eq!(reply.status, 302, "{reply:?}");
        let location = reply.header("location").expect("location").to_owned();
        let cookie = reply.set_cookie().expect("binding cookie");
        assert!(
            cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"),
            "{cookie}"
        );
        (location, cookie.split(';').next().expect("pair").to_owned())
    }

    fn callback(&self, query: &str, cookie: Option<&str>) -> Reply {
        let headers: Vec<(&str, &str)> = cookie.map(|c| ("Cookie", c)).into_iter().collect();
        http(
            self.port,
            "GET",
            &format!("/api/v1/auth/oidc/callback?{query}"),
            &headers,
            "",
        )
    }

    /// A complete login with the given token claims; returns the callback answer.
    fn login(&self, claims: Value, flaw: Flaw) -> Reply {
        let (location, cookie) = self.start();
        let state = state_of(&location);
        let code = self.idp.authorize(&location, claims, flaw);
        self.callback(&format!("code={code}&state={state}"), Some(&cookie))
    }

    fn status(&self, session: &str) -> Value {
        req(self.port, "GET", "/api/v1/auth/status", Some(session), None).json()
    }

    fn actions(&self) -> Vec<(String, String)> {
        let reply = self.admin("GET", "/api/v1/admin/audit?limit=200", None);
        assert_eq!(reply.status, 200, "{reply:?}");
        reply.json()["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(|e| {
                (
                    e["action"].as_str().unwrap_or_default().to_owned(),
                    e["details"]["reason"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                )
            })
            .collect()
    }
}

fn state_of(location: &str) -> String {
    Url::parse(location)
        .expect("url")
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .expect("state")
}

/// The session cookie (`name=value`) a callback answer sets, if it signed the user in.
fn session_of(reply: &Reply) -> Option<String> {
    reply
        .headers
        .iter()
        .filter(|(n, _)| n == "set-cookie")
        .map(|(_, v)| v.split(';').next().unwrap_or_default().to_owned())
        .find(|c| c.starts_with("superhet_session=") && c.len() > "superhet_session=".len())
}

fn failed_with(reply: &Reply, code: &str) {
    assert_eq!(reply.status, 302, "{reply:?}");
    assert_eq!(
        reply.header("location"),
        Some(format!("/?oidc_error={code}").as_str()),
        "{reply:?}"
    );
    assert!(
        session_of(reply).is_none(),
        "no session on failure: {reply:?}"
    );
}

fn claims(sub: &str, email: &str, verified: bool, groups: &[&str]) -> Value {
    json!({
        "sub": sub,
        "email": email,
        "email_verified": verified,
        "preferred_username": email.split('@').next().unwrap_or("user"),
        "groups": groups,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_user_is_provisioned_with_the_role_of_their_group() {
    let rig = rig!("provision");
    // Before configuration the button does not exist and the login redirects back.
    assert_eq!(rig.status(&rig.admin)["oidc"]["enabled"], false);
    rig.configure(base_settings(&rig.idp, rig.port));
    let status = http(rig.port, "GET", "/api/v1/auth/status", &[], "").json();
    assert_eq!(status["oidc"]["enabled"], true);
    assert_eq!(status["oidc"]["name"], "Mock ID");
    assert_eq!(status["oidc"]["login_url"], "/api/v1/auth/oidc/login");
    assert_eq!(status["local_login"], true);

    let reply = rig.login(
        claims(ISS_SUB, "Anna@example.org", true, &["sh-ops"]),
        Flaw::None,
    );
    assert_eq!(reply.status, 302, "{reply:?}");
    assert_eq!(reply.header("location"), Some("/"));
    let session = session_of(&reply).expect("session cookie");
    assert!(
        reply
            .headers
            .iter()
            .any(|(n, v)| n == "set-cookie" && v.starts_with("superhet_oidc=;")),
        "the binding cookie is cleared"
    );
    let me = rig.status(&session);
    assert_eq!(me["authenticated"], true);
    assert_eq!(me["username"], "Anna");
    assert_eq!(me["role"], "operator");

    // The same identity signs in again as the same account.
    let again = rig.login(
        claims(ISS_SUB, "Anna@example.org", true, &["sh-admins"]),
        Flaw::None,
    );
    let me = rig.status(&session_of(&again).expect("session"));
    assert_eq!(
        (me["username"].as_str(), me["role"].as_str()),
        (Some("Anna"), Some("admin")),
        "roles follow the provider's groups at every login"
    );

    let users = rig.admin("GET", "/api/v1/admin/users", None).json();
    let anna = users["users"]
        .as_array()
        .expect("users")
        .iter()
        .find(|u| u["username"] == "Anna")
        .expect("Anna");
    assert_eq!(anna["oidc"], true);
    assert_eq!(anna["email"], "Anna@example.org");
    // No local password: the account cannot sign in with one.
    let local = login(rig.port, "Anna", "anything at all, really");
    assert_eq!(local.status, 401);

    let actions = rig.actions();
    for expected in [
        "auth.oidc_provision",
        "auth.oidc_login",
        "user.role_change",
        "settings.change",
    ] {
        assert!(
            actions.iter().any(|(a, _)| a == expected),
            "{expected} in {actions:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn state_binding_and_single_use_are_enforced() {
    let rig = rig!("state");
    rig.configure(base_settings(&rig.idp, rig.port));
    let c = claims(ISS_SUB, "a@example.org", true, &[]);

    // A guessed state.
    let (location, cookie) = rig.start();
    let code = rig.idp.authorize(&location, c, Flaw::None);
    failed_with(
        &rig.callback(&format!("code={code}&state=guessed-state"), Some(&cookie)),
        "invalid_state",
    );
    // No state at all.
    failed_with(
        &rig.callback(&format!("code={code}"), Some(&cookie)),
        "invalid_state",
    );

    // A callback from another browser: the right state without the binding cookie.
    let state = state_of(&location);
    failed_with(
        &rig.callback(&format!("code={code}&state={state}"), None),
        "invalid_state",
    );
    failed_with(
        &rig.callback(
            &format!("code={code}&state={state}"),
            Some("superhet_oidc=someone-elses-cookie"),
        ),
        "invalid_state",
    );
    // That attempt is burnt even for the real browser: the state works once.
    failed_with(
        &rig.callback(&format!("code={code}&state={state}"), Some(&cookie)),
        "invalid_state",
    );
    assert_eq!(
        rig.idp.token_requests(),
        0,
        "refused callbacks never reach the token endpoint"
    );

    // Five refused callbacks from one address use up the shared sign-in allowance: no new
    // attempt can be started for a while.
    let blocked = http(rig.port, "GET", "/api/v1/auth/oidc/login", &[], "");
    assert_eq!(
        blocked.header("location"),
        Some("/?oidc_error=rate_limited"),
        "{blocked:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_valid_callback_completes_once_and_a_provider_refusal_is_reported() {
    let rig = rig!("complete");
    rig.configure(base_settings(&rig.idp, rig.port));
    let (location, cookie) = rig.start();
    let state = state_of(&location);
    let code = rig.idp.authorize(
        &location,
        claims(ISS_SUB, "a@example.org", true, &[]),
        Flaw::None,
    );
    let query = format!("code={code}&state={state}");
    assert!(session_of(&rig.callback(&query, Some(&cookie))).is_some());
    failed_with(&rig.callback(&query, Some(&cookie)), "invalid_state");

    // The provider says no.
    let (location, cookie) = rig.start();
    let state = state_of(&location);
    failed_with(
        &rig.callback(&format!("error=access_denied&state={state}"), Some(&cookie)),
        "provider_denied",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_id_tokens_and_pkce_mismatches_are_rejected() {
    let rig = rig!("tokens");
    rig.configure(base_settings(&rig.idp, rig.port));
    let c = claims(ISS_SUB, "a@example.org", true, &["sh-admins"]);
    for flaw in [
        Flaw::WrongNonce,
        Flaw::WrongAudience,
        Flaw::Expired,
        Flaw::WrongSignature,
        Flaw::WrongIssuer,
    ] {
        failed_with(&rig.login(c.clone(), flaw), "token_invalid");
    }
    // The provider refuses the exchange, or the verifier does not match the challenge.
    failed_with(&rig.login(c.clone(), Flaw::TokenError), "unavailable");
    failed_with(&rig.login(c.clone(), Flaw::WrongChallenge), "unavailable");
    assert!(
        rig.admin("GET", "/api/v1/admin/users", None).json()["users"]
            .as_array()
            .expect("users")
            .len()
            == 1,
        "no account was created by a rejected login"
    );
    // And a good token still works afterwards.
    assert!(session_of(&rig.login(c, Flaw::None)).is_some());
    let failures = rig
        .actions()
        .iter()
        .filter(|(a, _)| a == "auth.oidc_login_failed")
        .count();
    assert_eq!(failures, 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_matching_group_the_default_role_applies_or_the_login_is_refused() {
    let rig = rig!("roles");
    let mut settings = base_settings(&rig.idp, rig.port);
    settings["default_role"] = json!("");
    rig.configure(settings.clone());
    failed_with(
        &rig.login(
            claims("s1", "a@example.org", true, &["unrelated"]),
            Flaw::None,
        ),
        "no_role",
    );
    let ok = rig.login(claims("s1", "a@example.org", true, &["sh-ops"]), Flaw::None);
    assert_eq!(
        rig.status(&session_of(&ok).expect("session"))["role"],
        "operator"
    );
    // Removed from all groups at the provider: locked out at the next login.
    failed_with(
        &rig.login(claims("s1", "a@example.org", true, &[]), Flaw::None),
        "no_role",
    );

    settings["default_role"] = json!("listener");
    rig.configure(settings);
    let ok = rig.login(claims("s2", "b@example.org", true, &[]), Flaw::None);
    assert_eq!(
        rig.status(&session_of(&ok).expect("session"))["role"],
        "listener"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_users_are_refused_unless_provisioning_is_on() {
    let rig = rig!("noprov");
    let mut settings = base_settings(&rig.idp, rig.port);
    settings["auto_provision"] = json!(false);
    rig.configure(settings);
    failed_with(
        &rig.login(
            claims("s1", "a@example.org", true, &["sh-admins"]),
            Flaw::None,
        ),
        "no_account",
    );
    assert_eq!(
        rig.admin("GET", "/api/v1/admin/users", None).json()["users"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

fn set_email(rig: &Rig, user: &str, email: &str) -> i64 {
    let id = make_user_id(rig, user);
    let reply = rig.admin(
        "PATCH",
        &format!("/api/v1/admin/users/{id}"),
        Some(&json!({"email": email})),
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    id
}

fn make_user_id(rig: &Rig, name: &str) -> i64 {
    let reply = rig.admin(
        "POST",
        "/api/v1/admin/users",
        Some(&json!({"username": name, "role": "operator"})),
    );
    assert_eq!(reply.status, 201, "{reply:?}");
    reply.json()["user"]["id"].as_i64().expect("id")
}

#[tokio::test(flavor = "multi_thread")]
async fn linking_by_email_needs_the_switch_and_a_verified_address() {
    let rig = rig!("link");
    let mut settings = base_settings(&rig.idp, rig.port);
    settings["auto_provision"] = json!(false);
    settings["sync_roles"] = json!(false);
    rig.configure(settings.clone());
    let id = set_email(&rig, "carol", "carol@example.org");

    // Switch off: nothing links.
    failed_with(
        &rig.login(claims("c1", "carol@example.org", true, &[]), Flaw::None),
        "no_account",
    );

    settings["link_by_email"] = json!(true);
    rig.configure(settings);
    // The provider does not vouch for the address: nothing links.
    failed_with(
        &rig.login(claims("c1", "carol@example.org", false, &[]), Flaw::None),
        "no_account",
    );
    // Verified: linked to the existing account, which keeps its role (sync off).
    let reply = rig.login(claims("c1", "Carol@Example.org", true, &[]), Flaw::None);
    let me = rig.status(&session_of(&reply).expect("session"));
    assert_eq!(
        (me["username"].as_str(), me["role"].as_str()),
        (Some("carol"), Some("operator"))
    );
    assert!(rig.actions().iter().any(|(a, _)| a == "auth.oidc_link"));
    let user = rig
        .admin("GET", &format!("/api/v1/admin/users/{id}"), None)
        .json();
    assert_eq!(user["user"]["oidc"], true);

    // The link is by subject from now on: the same subject with another address still works,
    // and a different subject claiming Carol's address does not take the account over.
    let reply = rig.login(claims("c1", "other@example.org", true, &[]), Flaw::None);
    assert!(session_of(&reply).is_some());
    failed_with(
        &rig.login(
            claims("someone-else", "carol@example.org", true, &[]),
            Flaw::None,
        ),
        "already_linked",
    );

    // A disabled account is never linked.
    let id2 = set_email(&rig, "dave", "dave@example.org");
    let off = rig.admin(
        "PATCH",
        &format!("/api/v1/admin/users/{id2}"),
        Some(&json!({"disabled": true})),
    );
    assert_eq!(off.status, 200, "{off:?}");
    failed_with(
        &rig.login(claims("d1", "dave@example.org", true, &[]), Flaw::None),
        "disabled",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_admins_are_synced_but_the_last_admin_is_never_demoted() {
    let rig = rig!("sync");
    let mut settings = base_settings(&rig.idp, rig.port);
    settings["link_by_email"] = json!(true);
    rig.configure(settings);
    // The setup admin gets an address and links by it; the provider only grants "listener".
    let admin_id = rig.admin("GET", "/api/v1/admin/users", None).json()["users"][0]["id"]
        .as_i64()
        .expect("id");
    let reply = rig.admin(
        "PATCH",
        &format!("/api/v1/admin/users/{admin_id}"),
        Some(&json!({"email": "root@example.org"})),
    );
    assert_eq!(reply.status, 200, "{reply:?}");
    let reply = rig.login(claims("r1", "root@example.org", true, &[]), Flaw::None);
    let me = rig.status(&session_of(&reply).expect("session"));
    assert_eq!(me["role"], "admin", "the only admin stays admin");
    let denied = rig
        .actions()
        .into_iter()
        .any(|(a, reason)| a == "user.role_change" && reason == "last_admin");
    assert!(denied, "the refused demotion is audited");
}

#[tokio::test(flavor = "multi_thread")]
async fn password_logins_can_be_switched_off_only_with_an_oidc_admin() {
    let rig = rig!("locallogin");
    let mut settings = base_settings(&rig.idp, rig.port);
    settings["local_login"] = json!(false);
    // No OIDC admin yet: refused.
    let refused = rig.admin("PUT", "/api/v1/admin/oidc", Some(&settings));
    assert_eq!(refused.status, 409, "{refused:?}");
    assert_eq!(refused.json()["error"], "no_oidc_admin");

    // An admin signs in through OIDC (provisioned from the admin group) ...
    settings["local_login"] = json!(true);
    rig.configure(settings.clone());
    let reply = rig.login(
        claims("a1", "boss@example.org", true, &["sh-admins"]),
        Flaw::None,
    );
    let boss = session_of(&reply).expect("session");
    assert_eq!(rig.status(&boss)["role"], "admin");

    // ... now the switch is allowed.
    settings["local_login"] = json!(false);
    rig.configure(settings.clone());
    let status = http(rig.port, "GET", "/api/v1/auth/status", &[], "").json();
    assert_eq!(status["local_login"], false);
    let local = login(rig.port, ADMIN_USER, ADMIN_PASSWORD);
    assert_eq!(local.status, 403, "{local:?}");
    assert_eq!(local.json()["error"], "local_login_disabled");
    // OIDC keeps working.
    let again = rig.login(
        claims("a1", "boss@example.org", true, &["sh-admins"]),
        Flaw::None,
    );
    assert!(session_of(&again).is_some());

    // If the OIDC admin goes away the password form comes back instead of locking everybody out.
    let users = rig.admin("GET", "/api/v1/admin/users", None).json();
    let boss_id = users["users"]
        .as_array()
        .expect("users")
        .iter()
        .find(|u| u["oidc"] == true)
        .and_then(|u| u["id"].as_i64())
        .expect("oidc admin");
    let off = rig.admin(
        "PATCH",
        &format!("/api/v1/admin/users/{boss_id}"),
        Some(&json!({"disabled": true})),
    );
    assert_eq!(off.status, 200, "{off:?}");
    assert_eq!(login(rig.port, ADMIN_USER, ADMIN_PASSWORD).status, 204);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_admin_api_validates_tests_and_hides_the_secret() {
    let rig = rig!("adminapi");
    let view = rig.admin("GET", "/api/v1/admin/oidc", None).json();
    assert_eq!(view["oidc"]["enabled"], false);
    assert_eq!(view["source"], "database");
    assert_eq!(view["client_secret_set"], true);
    assert!(!view.to_string().contains(CLIENT_SECRET));

    let mut bad = base_settings(&rig.idp, rig.port);
    bad["issuer"] = json!("http://idp.example.org");
    assert_eq!(
        rig.admin("PUT", "/api/v1/admin/oidc", Some(&bad)).status,
        400
    );
    bad["issuer"] = json!(rig.idp.base);
    bad["default_role"] = json!("root");
    assert_eq!(
        rig.admin("PUT", "/api/v1/admin/oidc", Some(&bad)).status,
        400
    );
    bad["default_role"] = json!("listener");
    bad["surprise"] = json!(true);
    assert_eq!(
        rig.admin("PUT", "/api/v1/admin/oidc", Some(&bad)).status,
        400
    );

    rig.configure(base_settings(&rig.idp, rig.port));
    let view = rig.admin("GET", "/api/v1/admin/oidc", None).json();
    assert_eq!(
        view["redirect_url"],
        format!("{}/api/v1/auth/oidc/callback", public_url(rig.port))
    );
    let test = rig.admin("POST", "/api/v1/admin/oidc/test", None).json();
    assert_eq!(test["ok"], true, "{test}");
    assert_eq!(test["issuer"], rig.idp.base);

    // A provider that is not there is an answer, not a server error.
    let mut dead = base_settings(&rig.idp, rig.port);
    dead["issuer"] = json!("http://127.0.0.1:9");
    rig.configure(dead);
    let test = rig.admin("POST", "/api/v1/admin/oidc/test", None).json();
    assert_eq!(test["ok"], false);
    // A login with an unreachable provider returns to the login page with a code.
    let reply = http(rig.port, "GET", "/api/v1/auth/oidc/login", &[], "");
    assert_eq!(reply.header("location"), Some("/?oidc_error=unavailable"));

    // Admin only.
    let listener = make_user(rig.port, &rig.admin, "lisa", "listener");
    assert_eq!(
        req(rig.port, "GET", "/api/v1/admin/oidc", Some(&listener), None).status,
        403
    );
    assert_eq!(
        req(rig.port, "GET", "/api/v1/admin/oidc", None, None).status,
        401
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_configuration_wins_and_is_read_only() {
    let Some(db) = TestDb::create("envcfg").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let idp = MockIdp::start().await;
    let port = free_port();
    let (proc, port) = spawn_api_on(&db.url, port, &idp.base);
    wait_ready(port, "api").await;
    let admin = setup_admin(port);
    let view = req(port, "GET", "/api/v1/admin/oidc", Some(&admin), None).json();
    assert_eq!(view["source"], "env");
    assert_eq!(view["oidc"]["enabled"], true);
    assert_eq!(view["oidc"]["name"], "Env IdP");
    let put = req(
        port,
        "PUT",
        "/api/v1/admin/oidc",
        Some(&admin),
        Some(&json!({"enabled": false})),
    );
    assert_eq!(put.status, 409, "{put:?}");
    assert_eq!(put.json()["error"], "managed_by_env");
    let status = http(port, "GET", "/api/v1/auth/status", &[], "").json();
    assert_eq!(status["oidc"]["name"], "Env IdP");

    // A full login through the environment configuration.
    let reply = http(port, "GET", "/api/v1/auth/oidc/login", &[], "");
    let location = reply.header("location").expect("location").to_owned();
    let cookie = reply
        .set_cookie()
        .expect("cookie")
        .split(';')
        .next()
        .expect("pair")
        .to_owned();
    let state = state_of(&location);
    let code = idp.authorize(
        &location,
        claims("e1", "env@example.org", true, &["env-admins"]),
        Flaw::None,
    );
    let done = http(
        port,
        "GET",
        &format!("/api/v1/auth/oidc/callback?code={code}&state={state}"),
        &[("Cookie", &cookie)],
        "",
    );
    let session = session_of(&done).expect("session");
    assert_eq!(
        req(port, "GET", "/api/v1/auth/status", Some(&session), None).json()["role"],
        "admin"
    );
    drop(proc);
}

const BACKCHANNEL: &str = "/api/v1/auth/oidc/backchannel-logout";

fn with_sid(mut claims: Value, sid: &str) -> Value {
    claims["sid"] = json!(sid);
    claims
}

impl Rig {
    /// Sign in through the provider and return the session cookie.
    fn session(&self, sub: &str, name: &str, sid: &str) -> String {
        let claims = with_sid(claims(sub, &format!("{name}@example.org"), true, &[]), sid);
        session_of(&self.login(claims, Flaw::None)).expect("session")
    }

    fn signed_in(&self, session: &str) -> bool {
        self.status(session)["authenticated"] == true
    }

    fn logout(&self, session: &str) -> Reply {
        req(
            self.port,
            "POST",
            "/api/v1/auth/logout",
            Some(session),
            None,
        )
    }

    fn backchannel(&self, token: &str) -> Reply {
        http(
            self.port,
            "POST",
            BACKCHANNEL,
            &[("Content-Type", "application/x-www-form-urlencoded")],
            &format!("logout_token={token}"),
        )
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_of_an_oidc_session_sends_the_browser_to_the_provider() {
    let rig = rig!("rplogout");
    rig.configure(base_settings(&rig.idp, rig.port));
    let view = rig.admin("GET", "/api/v1/admin/oidc", None).json();
    assert_eq!(view["oidc"]["rp_logout"], true);
    assert_eq!(view["oidc"]["backchannel_logout"], true);
    assert_eq!(
        view["post_logout_url"],
        format!("{}/", public_url(rig.port))
    );
    assert_eq!(
        view["backchannel_url"],
        format!("{}{BACKCHANNEL}", public_url(rig.port))
    );
    let test = rig.admin("POST", "/api/v1/admin/oidc/test", None).json();
    assert_eq!(test["end_session"], true, "{test}");

    let session = rig.session(ISS_SUB, "anna", "sid-1");
    assert!(rig.signed_in(&session));
    let out = rig.logout(&session);
    assert_eq!(out.status, 200, "{out:?}");
    let url = out.json()["end_session_url"]
        .as_str()
        .expect("end_session_url")
        .to_owned();
    assert!(url.starts_with(&rig.idp.end_session_endpoint()), "{url}");
    let q: std::collections::HashMap<String, String> = Url::parse(&url)
        .expect("url")
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(
        Some(&q["id_token_hint"]),
        rig.idp.last_id_token().as_ref(),
        "the ID token of the login comes back as hint"
    );
    assert_eq!(q["client_id"], common::idp::CLIENT_ID);
    assert_eq!(
        q["post_logout_redirect_uri"],
        format!("{}/", public_url(rig.port))
    );
    assert!(q["state"].len() >= 16, "{q:?}");
    assert!(
        out.headers
            .iter()
            .any(|(n, v)| n == "set-cookie" && v.starts_with("superhet_session=;")),
        "the session cookie is cleared: {out:?}"
    );
    assert!(!rig.signed_in(&session), "the local session is gone");
    let event = audit(rig.port, &rig.admin, "action=auth.logout");
    assert_eq!(event[0]["details"]["provider_logout"], true, "{event:?}");

    // A password session has no provider to tell.
    let local = login(rig.port, ADMIN_USER, ADMIN_PASSWORD);
    let admin = local.session_cookie().expect("admin session");
    assert_eq!(rig.logout(&admin).status, 204);
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_logout_follows_the_switch_and_the_providers_abilities() {
    let rig = rig!("rpswitch");
    let mut settings = base_settings(&rig.idp, rig.port);
    settings["rp_logout"] = json!(false);
    rig.configure(settings);
    let session = rig.session(ISS_SUB, "anna", "sid-1");
    let out = rig.logout(&session);
    assert_eq!(
        out.status, 204,
        "switched off: local sign-out only: {out:?}"
    );
    assert!(!rig.signed_in(&session));

    // On again, but the provider advertises no end session endpoint.
    rig.idp.set_end_session(false);
    rig.configure(base_settings(&rig.idp, rig.port));
    let test = rig.admin("POST", "/api/v1/admin/oidc/test", None).json();
    assert_eq!(test["end_session"], false);
    let session = rig.session(ISS_SUB, "anna", "sid-2");
    assert_eq!(rig.logout(&session).status, 204);

    // And once it does, the next sign-out redirects.
    rig.idp.set_end_session(true);
    rig.configure(base_settings(&rig.idp, rig.port));
    let session = rig.session(ISS_SUB, "anna", "sid-3");
    assert_eq!(rig.logout(&session).status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backchannel_logout_ends_the_matching_sessions_once() {
    let rig = rig!("bclogout");
    rig.configure(base_settings(&rig.idp, rig.port));
    let anna1 = rig.session("sub-anna", "anna", "sid-a1");
    let anna2 = rig.session("sub-anna", "anna", "sid-a2");
    let bob = rig.session("sub-bob", "bob", "sid-b1");
    assert!(rig.signed_in(&anna1) && rig.signed_in(&anna2) && rig.signed_in(&bob));

    // One provider session (sid) ends one Superhet session.
    let token = rig.idp.logout_token(&json!({"sid": "sid-a1"}), false);
    let reply = rig.backchannel(&token);
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert!(!rig.signed_in(&anna1));
    assert!(rig.signed_in(&anna2) && rig.signed_in(&bob));
    assert!(
        rig.signed_in(&rig.admin),
        "password sessions are not touched"
    );

    // The same token again is a replay, and changes nothing.
    let again = rig.backchannel(&token);
    assert_eq!(again.status, 400, "{again:?}");
    assert_eq!(again.json()["error"], "replay");

    // A subject without a session id ends all of that user's sessions, not other users'.
    let token = rig.idp.logout_token(&json!({"sub": "sub-anna"}), false);
    assert_eq!(rig.backchannel(&token).status, 200);
    assert!(!rig.signed_in(&anna2));
    assert!(rig.signed_in(&bob) && rig.signed_in(&rig.admin));

    // A token for a session nobody knows is accepted and ends nothing.
    let token = rig.idp.logout_token(&json!({"sid": "sid-unknown"}), false);
    assert_eq!(rig.backchannel(&token).status, 200);
    assert!(rig.signed_in(&bob));

    let events = audit(rig.port, &rig.admin, "action=auth.oidc_backchannel_logout");
    let ended: Vec<u64> = events
        .iter()
        .filter(|e| e["outcome"] == "ok")
        .map(|e| e["details"]["sessions"].as_u64().unwrap_or(99))
        .collect();
    assert_eq!(ended.len(), 3, "{events:?}");
    assert_eq!(ended.iter().sum::<u64>(), 2, "{events:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_logout_tokens_are_refused_and_end_nothing() {
    let rig = rig!("bcbad");
    rig.configure(base_settings(&rig.idp, rig.port));
    let victim = rig.session("sub-anna", "anna", "sid-a1");

    let idp = &rig.idp;
    let sid = json!({"sid": "sid-a1"});
    let with = |extra: Value| {
        let mut claims = sid.clone();
        claims
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        claims
    };
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "signed with a key the provider does not publish",
            idp.logout_token(&sid, true),
            "signature",
        ),
        (
            "wrong audience",
            idp.logout_token(&with(json!({"aud": "someone-else"})), false),
            "audience",
        ),
        (
            "wrong issuer",
            idp.logout_token(&with(json!({"iss": "https://evil.example"})), false),
            "issuer",
        ),
        (
            "not a logout event",
            idp.logout_token(&with(json!({"events": {"other": {}}})), false),
            "event",
        ),
        (
            "an ID token style nonce",
            idp.logout_token(&with(json!({"nonce": "abc"})), false),
            "nonce",
        ),
        (
            "issued long ago",
            idp.logout_token(&with(json!({"iat": 1_000_000})), false),
            "issued_at",
        ),
        (
            "no iat",
            idp.logout_token(&with(json!({"iat": null})), false),
            "issued_at",
        ),
        (
            "no jti",
            idp.logout_token(&with(json!({"jti": null})), false),
            "no_jti",
        ),
        (
            "neither sub nor sid",
            idp.logout_token(&json!({}), false),
            "no_subject",
        ),
        ("not a JWT", "garbage".to_owned(), "malformed"),
    ];
    // The limiter for failed requests is generous but finite: stay below it.
    for (what, token, code) in cases {
        let reply = rig.backchannel(&token);
        assert_eq!(reply.status, 400, "{what}: {reply:?}");
        assert_eq!(reply.json()["error"], code, "{what}");
        assert!(rig.signed_in(&victim), "{what}: the session survived");
    }

    // Not a form post, or no token in it.
    let token = idp.logout_token(&sid, false);
    let json_post = req(
        rig.port,
        "POST",
        BACKCHANNEL,
        None,
        Some(&json!({"logout_token": token})),
    );
    assert_eq!(json_post.status, 400, "{json_post:?}");
    let empty = http(
        rig.port,
        "POST",
        BACKCHANNEL,
        &[("Content-Type", "application/x-www-form-urlencoded")],
        "other=1",
    );
    assert_eq!(empty.status, 400, "{empty:?}");
    assert!(rig.signed_in(&victim));

    // The valid token still works afterwards (failures did not poison anything).
    assert_eq!(rig.backchannel(&token).status, 200);
    assert!(!rig.signed_in(&victim));

    // Refusals are audited (coalesced, so wait for the row).
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    let denied = audit(rig.port, &rig.admin, "action=auth.oidc_backchannel_logout");
    assert!(
        denied.iter().any(|e| e["outcome"] == "denied"),
        "a denied event: {denied:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn backchannel_logout_can_be_switched_off() {
    let rig = rig!("bcoff");
    let mut settings = base_settings(&rig.idp, rig.port);
    settings["backchannel_logout"] = json!(false);
    rig.configure(settings);
    let session = rig.session("sub-anna", "anna", "sid-a1");
    let token = rig.idp.logout_token(&json!({"sid": "sid-a1"}), false);
    let reply = rig.backchannel(&token);
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.json()["error"], "not_enabled");
    assert!(rig.signed_in(&session));

    // Switched on, the very same token (never consumed) works.
    rig.configure(base_settings(&rig.idp, rig.port));
    assert_eq!(rig.backchannel(&token).status, 200);
    assert!(!rig.signed_in(&session));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_provider_makes_the_backchannel_unavailable() {
    let rig = rig!("bcdead");
    let mut dead = base_settings(&rig.idp, rig.port);
    dead["issuer"] = json!("http://127.0.0.1:9");
    rig.configure(dead);
    let token = rig.idp.logout_token(&json!({"sid": "sid-a1"}), false);
    let reply = rig.backchannel(&token);
    assert_eq!(reply.status, 503, "{reply:?}");
}

fn spawn_api_on(db_url: &str, port: u16, issuer: &str) -> (Proc, u16) {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_superhet-api"));
    command
        .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
        .env("SUPERHET_DATABASE_URL", db_url)
        .env("SUPERHET_SETUP_TOKEN", SETUP_TOKEN)
        .env("SUPERHET_OIDC_ISSUER", issuer)
        .env("SUPERHET_OIDC_CLIENT_ID", common::idp::CLIENT_ID)
        .env("SUPERHET_OIDC_CLIENT_SECRET", CLIENT_SECRET)
        .env("SUPERHET_PUBLIC_URL", public_url(port))
        .env("SUPERHET_OIDC_NAME", "Env IdP")
        .env("SUPERHET_OIDC_ADMIN_GROUPS", "env-admins")
        .env("SUPERHET_OIDC_DEFAULT_ROLE", "listener")
        .env("SUPERHET_OIDC_AUTO_PROVISION", "true")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    (Proc(command.spawn().expect("spawn api")), port)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_environment_configuration_stops_the_api() {
    let Some(db) = TestDb::create("envbad").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let port = free_port();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_superhet-api"))
        .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
        .env("SUPERHET_DATABASE_URL", &db.url)
        .env("SUPERHET_OIDC_ISSUER", "https://idp.example.org")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    // client id and public URL are missing: refuse to start instead of running half-configured.
    let status = tokio::task::spawn_blocking(move || child.wait().expect("wait"))
        .await
        .expect("join");
    assert!(!status.success());
    let _ = &db;
}
