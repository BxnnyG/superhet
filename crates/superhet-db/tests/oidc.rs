//! OIDC identities against a real PostgreSQL (`DATABASE_URL`).

mod common;

use common::TestDb;
use superhet_db::{
    oidc::{self, Linked},
    users::{self, Created, EmailSet, Role},
};

const ISS: &str = "https://idp.example";

macro_rules! db {
    ($tag:expr) => {{
        let Some(db) = TestDb::create($tag, true).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        db
    }};
}

async fn local_user(pool: &superhet_db::PgPool, name: &str, role: Role) -> i64 {
    match users::create(pool, name, "hash", role, false)
        .await
        .expect("create")
    {
        Created::Id(id) => id,
        Created::UsernameTaken => panic!("name taken"),
    }
}

#[tokio::test]
async fn identities_link_once_and_resolve_to_the_account() {
    let db = db!("oidc_link");
    let a = local_user(&db.pool, "anna", Role::Operator).await;
    let b = local_user(&db.pool, "bob", Role::Listener).await;

    assert!(
        oidc::find_by_identity(&db.pool, ISS, "sub-a")
            .await
            .expect("find")
            .is_none()
    );
    assert_eq!(
        oidc::link(&db.pool, a, ISS, "sub-a", Some("a@x.org"))
            .await
            .expect("link"),
        Linked::Done
    );
    let found = oidc::find_by_identity(&db.pool, ISS, "sub-a")
        .await
        .expect("find")
        .expect("linked");
    assert_eq!(
        (found.id, found.role, found.disabled),
        (a, Role::Operator, false)
    );
    // The same identity cannot belong to a second account, nor can an account hold two
    // identities at one issuer.
    assert_eq!(
        oidc::link(&db.pool, b, ISS, "sub-a", None)
            .await
            .expect("link"),
        Linked::Conflict
    );
    assert_eq!(
        oidc::link(&db.pool, a, ISS, "sub-other", None)
            .await
            .expect("link"),
        Linked::Conflict
    );
    assert!(oidc::has_identity_at(&db.pool, a, ISS).await.expect("has"));
    assert!(!oidc::has_identity_at(&db.pool, b, ISS).await.expect("has"));
    // Another issuer is a different namespace.
    assert_eq!(
        oidc::link(&db.pool, b, "https://other.example", "sub-a", None)
            .await
            .expect("link"),
        Linked::Done
    );
    // Deleting the account removes its identities.
    assert_eq!(
        users::delete(&db.pool, b).await.expect("delete"),
        users::Guarded::Done
    );
    assert!(
        oidc::find_by_identity(&db.pool, "https://other.example", "sub-a")
            .await
            .expect("find")
            .is_none()
    );
}

#[tokio::test]
async fn email_lookup_is_case_insensitive_and_unique() {
    let db = db!("oidc_email");
    let a = local_user(&db.pool, "anna", Role::Listener).await;
    let b = local_user(&db.pool, "bob", Role::Listener).await;
    assert_eq!(
        users::set_email(&db.pool, a, Some("Anna@Example.org"))
            .await
            .expect("set"),
        EmailSet::Done
    );
    assert_eq!(
        users::set_email(&db.pool, b, Some("anna@example.ORG"))
            .await
            .expect("set"),
        EmailSet::Taken
    );
    assert_eq!(
        users::set_email(&db.pool, 9999, Some("x@example.org"))
            .await
            .expect("set"),
        EmailSet::NotFound
    );
    let found = oidc::find_by_email(&db.pool, "ANNA@example.org")
        .await
        .expect("find")
        .expect("match");
    assert_eq!(found.id, a);
    assert!(
        oidc::find_by_email(&db.pool, "nobody@example.org")
            .await
            .expect("find")
            .is_none()
    );
    assert_eq!(
        users::set_email(&db.pool, a, None).await.expect("clear"),
        EmailSet::Done
    );
    assert!(
        oidc::find_by_email(&db.pool, "anna@example.org")
            .await
            .expect("find")
            .is_none()
    );
}

#[tokio::test]
async fn provisioning_picks_a_free_name_and_creates_a_passwordless_account() {
    let db = db!("oidc_provision");
    local_user(&db.pool, "anna", Role::Listener).await;
    let first = oidc::provision(&db.pool, "Anna", Role::Operator, ISS, "s1", Some("a@x.org"))
        .await
        .expect("provision")
        .expect("created");
    assert_eq!(first.username, "Anna-2");
    let second = oidc::provision(&db.pool, "anna", Role::Listener, ISS, "s2", Some("a@x.org"))
        .await
        .expect("provision")
        .expect("created");
    assert_eq!(second.username, "anna-3");
    // The address is stored once only.
    let info = users::get(&db.pool, second.id)
        .await
        .expect("get")
        .expect("user");
    assert_eq!(info.email, None);
    assert!(info.oidc);
    let info = users::get(&db.pool, first.id)
        .await
        .expect("get")
        .expect("user");
    assert_eq!(info.email.as_deref(), Some("a@x.org"));
    assert_eq!(info.role, Role::Operator);
    // No password verifies against the locked hash.
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM superhet.users WHERE id = $1")
        .bind(first.id)
        .fetch_one(&db.pool)
        .await
        .expect("hash");
    assert_eq!(hash, oidc::LOCKED_PASSWORD_HASH);
    // The same identity twice: the second call reports the race instead of failing.
    assert!(
        oidc::provision(&db.pool, "zed", Role::Listener, ISS, "s1", None)
            .await
            .expect("provision")
            .is_none()
    );
    assert!(
        users::list(&db.pool)
            .await
            .expect("list")
            .iter()
            .all(|u| u.username != "zed"),
        "the rolled back account must not remain"
    );
}

#[tokio::test]
async fn an_oidc_admin_counts_only_while_enabled() {
    let db = db!("oidc_admin");
    let admin = local_user(&db.pool, "root", Role::Admin).await;
    assert!(!oidc::has_active_oidc_admin(&db.pool).await.expect("q"));
    oidc::link(&db.pool, admin, ISS, "root-sub", None)
        .await
        .expect("link");
    assert!(oidc::has_active_oidc_admin(&db.pool).await.expect("q"));
    // A second admin keeps the disable allowed; the linked one no longer counts when disabled.
    local_user(&db.pool, "other", Role::Admin).await;
    users::set_disabled(&db.pool, admin, true)
        .await
        .expect("disable");
    assert!(!oidc::has_active_oidc_admin(&db.pool).await.expect("q"));
    users::set_disabled(&db.pool, admin, false)
        .await
        .expect("enable");
    users::set_role(&db.pool, admin, Role::Operator)
        .await
        .expect("demote");
    assert!(!oidc::has_active_oidc_admin(&db.pool).await.expect("q"));
}

async fn session(pool: &superhet_db::PgPool, tag: &str, user: i64, oidc_sid: Option<(&str, &str)>) {
    match oidc_sid {
        Some((issuer, sid)) => oidc::create_session(
            pool,
            &key(tag),
            user,
            (None, None, None),
            &oidc::SessionOidc {
                issuer,
                sid: Some(sid),
                id_token: Some("header.payload.sig"),
            },
        )
        .await
        .expect("oidc session"),
        None => superhet_db::auth::create_session(pool, &key(tag), user, None, None)
            .await
            .expect("password session"),
    }
}

/// Session ids are 32 bytes.
fn key(tag: &str) -> Vec<u8> {
    let mut id = tag.as_bytes().to_vec();
    id.resize(32, 0);
    id
}

async fn alive(pool: &superhet_db::PgPool, tag: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM superhet.sessions WHERE id = $1)")
        .bind(key(tag))
        .fetch_one(pool)
        .await
        .expect("query")
}

#[tokio::test]
async fn logout_ends_exactly_the_matching_oidc_sessions() {
    let db = db!("oidc_logout");
    let p = &db.pool;
    let anna = local_user(p, "anna", Role::Operator).await;
    let bob = local_user(p, "bob", Role::Listener).await;
    oidc::link(p, anna, ISS, "sub-a", None).await.expect("link");
    oidc::link(p, bob, ISS, "sub-b", None).await.expect("link");
    session(p, "a-sid1", anna, Some((ISS, "sid-1"))).await;
    session(p, "a-sid2", anna, Some((ISS, "sid-2"))).await;
    session(p, "a-pw", anna, None).await;
    session(p, "b-sid1", bob, Some((ISS, "sid-1"))).await;
    session(p, "b-other", bob, Some(("https://other.example", "sid-1"))).await;

    // Nothing named: nothing ends.
    let none = oidc::end_sessions_for_logout(p, ISS, None, None).await;
    assert_eq!(none.expect("q").len(), 0);
    // A sid ends that provider session for whoever holds it, at this issuer only.
    let by_sid = oidc::end_sessions_for_logout(p, ISS, Some("sid-1"), None).await;
    assert_eq!(by_sid.expect("q").len(), 2);
    assert!(!alive(p, "a-sid1").await && !alive(p, "b-sid1").await);
    assert!(alive(p, "a-sid2").await && alive(p, "b-other").await);
    // sub and sid together must both match.
    let mismatch = oidc::end_sessions_for_logout(p, ISS, Some("sid-2"), Some("sub-b")).await;
    assert_eq!(mismatch.expect("q").len(), 0);
    // A subject alone ends the user's OIDC sessions at that issuer, not the password session.
    let by_sub = oidc::end_sessions_for_logout(p, ISS, None, Some("sub-a")).await;
    assert_eq!(by_sub.expect("q").len(), 1);
    assert!(!alive(p, "a-sid2").await && alive(p, "a-pw").await);
    let unknown = oidc::end_sessions_for_logout(p, ISS, None, Some("unknown")).await;
    assert_eq!(unknown.expect("q").len(), 0);
}

#[tokio::test]
async fn ending_a_session_reports_its_oidc_login() {
    let db = db!("oidc_end");
    let p = &db.pool;
    let anna = local_user(p, "anna", Role::Operator).await;
    session(p, "oidc", anna, Some((ISS, "sid-1"))).await;
    session(p, "pw", anna, None).await;
    let ended = oidc::end_session(p, &key("oidc")).await.expect("end");
    assert_eq!(
        ended,
        Some(oidc::EndedSession {
            issuer: Some(ISS.to_owned()),
            id_token: Some("header.payload.sig".to_owned()),
        })
    );
    let ended = oidc::end_session(p, &key("pw")).await.expect("end");
    assert_eq!(
        ended,
        Some(oidc::EndedSession {
            issuer: None,
            id_token: None
        })
    );
    assert_eq!(oidc::end_session(p, &key("pw")).await.expect("again"), None);
}
