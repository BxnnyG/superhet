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
