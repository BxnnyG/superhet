//! Accounts, roles, the last-admin rule and invites against a real PostgreSQL (`DATABASE_URL`).

mod common;

use std::borrow::Cow;
use std::time::Duration;

use common::TestDb;
use superhet_db::{
    audit,
    auth::{self, Lifetimes},
    users::{self, Accepted, Created, Guarded, InviteState, Role},
};

const LIFETIMES: Lifetimes = Lifetimes {
    idle: Duration::from_secs(7 * 24 * 3600),
    absolute: Duration::from_secs(30 * 24 * 3600),
    touch_interval: Duration::from_secs(60),
};

macro_rules! db {
    ($tag:expr) => {{
        let Some(db) = TestDb::create($tag, true).await else {
            eprintln!("DATABASE_URL not set; skipping");
            return;
        };
        db
    }};
}

#[tokio::test]
async fn an_existing_single_user_database_migrates_to_an_admin() {
    let Some(db) = TestDb::create("legacy", false).await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    // The schema of the last single-user release: migrations 1..=6 only.
    let mut old = sqlx::migrate!("./migrations");
    let all = old.migrations.to_vec();
    old.migrations = Cow::Owned(all.into_iter().filter(|m| m.version <= 6).collect());
    old.create_schema("superhet");
    old.dangerous_set_table_name("superhet._sqlx_migrations");
    old.run(&db.pool).await.expect("old schema");
    sqlx::query("INSERT INTO superhet.users (username, password_hash) VALUES ('benny', 'h')")
        .execute(&db.pool)
        .await
        .expect("legacy user");
    let session_id = [7u8; 32];
    sqlx::query("INSERT INTO superhet.sessions (id, user_id) SELECT $1, id FROM superhet.users")
        .bind(&session_id[..])
        .execute(&db.pool)
        .await
        .expect("legacy session");

    superhet_db::migrate(&db.pool).await.expect("upgrade");

    let user = auth::find_user_by_name(&db.pool, "benny")
        .await
        .expect("find")
        .expect("kept");
    assert_eq!(user.role, Role::Admin);
    assert!(!user.disabled && !user.must_change_password);
    let session = auth::find_active_session(&db.pool, &session_id, LIFETIMES)
        .await
        .expect("session")
        .expect("still valid");
    assert_eq!(
        (session.username.as_str(), session.role),
        ("benny", Role::Admin)
    );
    // Guest mode is off: nothing is stored, the default applies.
    assert!(
        superhet_db::settings::get(&db.pool, "guest_policy")
            .await
            .expect("settings")
            .is_none()
    );
    // The single-row restriction is gone.
    assert!(matches!(
        users::create(&db.pool, "second", "h", Role::Listener, false).await,
        Ok(Created::Id(_))
    ));
    db.drop().await;
}

#[tokio::test]
async fn usernames_are_unique_and_setup_is_first_run_only() {
    let db = db!("names");
    let admin = auth::create_admin(&db.pool, "Admin", "h")
        .await
        .expect("setup")
        .expect("first");
    assert!(
        auth::create_admin(&db.pool, "x", "h")
            .await
            .expect("setup")
            .is_none()
    );
    assert_eq!(
        users::create(&db.pool, "ADMIN", "h", Role::Operator, false)
            .await
            .expect("create"),
        Created::UsernameTaken
    );
    let Created::Id(op) = users::create(&db.pool, "op", "h", Role::Operator, true)
        .await
        .expect("create")
    else {
        panic!("created");
    };
    let all = users::list(&db.pool).await.expect("list");
    assert_eq!(
        all.iter().map(|u| (u.id, u.role)).collect::<Vec<_>>(),
        [(admin, Role::Admin), (op, Role::Operator)]
    );
    assert!(all[1].must_change_password);
    users::mark_login(&db.pool, op).await.expect("login");
    assert!(
        users::get(&db.pool, op)
            .await
            .expect("get")
            .expect("some")
            .last_login_ms
            .is_some()
    );
    db.drop().await;
}

#[tokio::test]
async fn the_last_active_admin_cannot_be_demoted_disabled_or_deleted() {
    let db = db!("lastadmin");
    let a = auth::create_admin(&db.pool, "a", "h")
        .await
        .expect("a")
        .expect("id");
    assert_eq!(
        users::set_role(&db.pool, a, Role::Operator)
            .await
            .expect("r"),
        Guarded::LastAdmin
    );
    assert_eq!(
        users::set_disabled(&db.pool, a, true).await.expect("d"),
        Guarded::LastAdmin
    );
    assert_eq!(
        users::delete(&db.pool, a).await.expect("x"),
        Guarded::LastAdmin
    );

    let Created::Id(b) = users::create(&db.pool, "b", "h", Role::Admin, false)
        .await
        .expect("b")
    else {
        panic!("created");
    };
    // Two admins: one may go, the other then is the last.
    assert_eq!(
        users::set_role(&db.pool, a, Role::Listener)
            .await
            .expect("r"),
        Guarded::Done
    );
    assert_eq!(
        users::set_role(&db.pool, b, Role::Listener)
            .await
            .expect("r"),
        Guarded::LastAdmin
    );
    assert_eq!(
        users::set_disabled(&db.pool, b, true).await.expect("d"),
        Guarded::LastAdmin
    );
    assert_eq!(
        users::delete(&db.pool, b).await.expect("x"),
        Guarded::LastAdmin
    );
    // A disabled admin does not count as active.
    assert_eq!(
        users::set_role(&db.pool, a, Role::Admin).await.expect("r"),
        Guarded::Done
    );
    assert_eq!(
        users::set_disabled(&db.pool, a, true).await.expect("d"),
        Guarded::Done
    );
    assert_eq!(
        users::set_role(&db.pool, b, Role::Operator)
            .await
            .expect("r"),
        Guarded::LastAdmin
    );
    assert_eq!(
        users::set_role(&db.pool, 9999, Role::Listener)
            .await
            .expect("r"),
        Guarded::NotFound
    );
    db.drop().await;
}

#[tokio::test]
async fn concurrent_demotions_cannot_remove_the_last_admin() {
    let db = db!("race");
    let a = auth::create_admin(&db.pool, "a", "h")
        .await
        .expect("a")
        .expect("id");
    let Created::Id(b) = users::create(&db.pool, "b", "h", Role::Admin, false)
        .await
        .expect("b")
    else {
        panic!("created");
    };
    let (p1, p2) = (db.pool.clone(), db.pool.clone());
    let (r1, r2) = tokio::join!(
        users::set_role(&p1, a, Role::Listener),
        users::set_role(&p2, b, Role::Listener),
    );
    let outcomes = [r1.expect("r1"), r2.expect("r2")];
    assert_eq!(
        outcomes.iter().filter(|o| **o == Guarded::Done).count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == Guarded::LastAdmin)
            .count(),
        1,
        "{outcomes:?}"
    );
    db.drop().await;
}

#[tokio::test]
async fn disabling_revokes_sessions_and_blocks_them() {
    let db = db!("disable");
    auth::create_admin(&db.pool, "root", "h")
        .await
        .expect("a")
        .expect("id");
    let Created::Id(u) = users::create(&db.pool, "bob", "h", Role::Listener, false)
        .await
        .expect("u")
    else {
        panic!("created");
    };
    auth::create_session(&db.pool, &[1u8; 32], u, Some("ua"), Some("203.0.113.5"))
        .await
        .expect("s");
    auth::create_session(&db.pool, &[2u8; 32], u, None, None)
        .await
        .expect("s");
    assert_eq!(
        users::list_sessions(&db.pool, Some(u))
            .await
            .expect("list")
            .len(),
        2
    );
    // A client report is stored on the owner's own session only and comes back in the list.
    let report = serde_json::json!({"platform": "Linux", "screen": {"width": 1920}});
    assert!(
        users::set_session_client_report(&db.pool, &[1u8; 32], u, &report)
            .await
            .expect("report")
    );
    assert!(
        !users::set_session_client_report(&db.pool, &[1u8; 32], u + 1, &report)
            .await
            .expect("report")
    );
    let listed = users::list_sessions(&db.pool, Some(u)).await.expect("list");
    assert_eq!(
        listed
            .iter()
            .find(|s| s.id == [1u8; 32])
            .and_then(|s| s.client_report.clone()),
        Some(report)
    );
    assert!(
        listed
            .iter()
            .find(|s| s.id == [2u8; 32])
            .is_some_and(|s| s.client_report.is_none())
    );
    assert!(
        users::revoke_session(&db.pool, &[2u8; 32], Some(u))
            .await
            .expect("revoke")
    );
    // Not the owner's session: untouched.
    assert!(
        !users::revoke_session(&db.pool, &[1u8; 32], Some(u + 1))
            .await
            .expect("revoke")
    );
    assert_eq!(
        users::set_disabled(&db.pool, u, true).await.expect("d"),
        Guarded::Done
    );
    assert!(
        users::list_sessions(&db.pool, None)
            .await
            .expect("list")
            .is_empty()
    );
    assert!(
        auth::find_user_by_name(&db.pool, "bob")
            .await
            .expect("f")
            .expect("u")
            .disabled
    );
    // A session that somehow survives is still refused for a disabled user.
    auth::create_session(&db.pool, &[3u8; 32], u, None, None)
        .await
        .expect("s");
    assert!(
        auth::find_active_session(&db.pool, &[3u8; 32], LIFETIMES)
            .await
            .expect("s")
            .is_none()
    );
    assert_eq!(
        users::set_disabled(&db.pool, u, false).await.expect("e"),
        Guarded::Done
    );
    assert!(
        auth::find_active_session(&db.pool, &[3u8; 32], LIFETIMES)
            .await
            .expect("s")
            .is_some()
    );
    db.drop().await;
}

#[tokio::test]
async fn password_reset_revokes_sessions_and_change_clears_the_flag() {
    let db = db!("reset");
    let admin = auth::create_admin(&db.pool, "root", "h")
        .await
        .expect("a")
        .expect("id");
    auth::create_session(&db.pool, &[1u8; 32], admin, None, None)
        .await
        .expect("s");
    assert!(
        users::reset_password(&db.pool, admin, "new", true)
            .await
            .expect("reset")
    );
    assert!(
        users::list_sessions(&db.pool, None)
            .await
            .expect("list")
            .is_empty()
    );
    let user = auth::find_user_by_id(&db.pool, admin)
        .await
        .expect("f")
        .expect("u");
    assert!(user.must_change_password);
    auth::change_password(&db.pool, admin, "mine", &[9u8; 32])
        .await
        .expect("change");
    let user = auth::find_user_by_id(&db.pool, admin)
        .await
        .expect("f")
        .expect("u");
    assert!(!user.must_change_password);
    assert!(
        !users::reset_password(&db.pool, 4242, "x", false)
            .await
            .expect("reset")
    );
    db.drop().await;
}

#[tokio::test]
async fn deleting_a_user_pseudonymises_the_audit_trail_but_keeps_it() {
    let db = db!("delete");
    auth::create_admin(&db.pool, "root", "h")
        .await
        .expect("a")
        .expect("id");
    let Created::Id(u) = users::create(&db.pool, "carol", "h", Role::Operator, false)
        .await
        .expect("u")
    else {
        panic!("created");
    };
    auth::create_session(&db.pool, &[1u8; 32], u, None, None)
        .await
        .expect("s");
    audit::insert(
        &db.pool,
        &event("user", Some(u.to_string()), Some("carol"), "auth.login"),
    )
    .await
    .expect("audit");
    assert_eq!(
        users::delete(&db.pool, u).await.expect("del"),
        Guarded::Done
    );
    assert_eq!(
        users::delete(&db.pool, u).await.expect("del"),
        Guarded::NotFound
    );
    assert!(
        users::list_sessions(&db.pool, None)
            .await
            .expect("list")
            .is_empty()
    );
    let events = audit::query(&db.pool, &audit::Filter::default(), 10)
        .await
        .expect("q");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].data.actor_name.as_deref(),
        Some(format!("deleted-user-{u}").as_str())
    );
    assert_eq!(
        events[0].data.actor_id.as_deref(),
        Some(u.to_string().as_str())
    );
    db.drop().await;
}

fn event(kind: &str, id: Option<String>, name: Option<&str>, action: &str) -> audit::NewEvent {
    audit::NewEvent {
        ts_ms: None,
        actor_kind: kind.into(),
        actor_id: id,
        actor_name: name.map(Into::into),
        role: None,
        ip: None,
        peer_ip: None,
        user_agent: None,
        action: action.into(),
        target: None,
        outcome: "ok".into(),
        details: serde_json::json!({}),
    }
}

#[tokio::test]
async fn invites_work_once_expire_and_can_be_revoked() {
    let db = db!("invites");
    let admin = auth::create_admin(&db.pool, "root", "h")
        .await
        .expect("a")
        .expect("id");
    let hour = Duration::from_secs(3600);
    let hash = |n: u8| [n; 32];

    let inv = users::create_invite(
        &db.pool,
        &hash(1),
        Role::Operator,
        Some("for dave"),
        admin,
        "root",
        hour,
    )
    .await
    .expect("invite");
    assert_eq!((inv.role, inv.state), (Role::Operator, InviteState::Open));
    // Only the hash is stored: the table has no token column and the bytes are what we passed.
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT token_hash FROM superhet.invites WHERE id = $1")
            .bind(inv.id)
            .fetch_one(&db.pool)
            .await
            .expect("hash");
    assert_eq!(stored, hash(1));
    assert!(
        users::find_invite(&db.pool, &hash(1))
            .await
            .expect("f")
            .is_some()
    );
    assert!(
        users::find_invite(&db.pool, &hash(2))
            .await
            .expect("f")
            .is_none()
    );

    // A taken username does not burn the invite.
    assert_eq!(
        users::accept_invite(&db.pool, &hash(1), "ROOT", "h")
            .await
            .expect("a"),
        Accepted::UsernameTaken
    );
    let Accepted::Created { user_id, role } = users::accept_invite(&db.pool, &hash(1), "dave", "h")
        .await
        .expect("a")
    else {
        panic!("accepted");
    };
    assert_eq!(role, Role::Operator);
    assert_eq!(
        users::get(&db.pool, user_id)
            .await
            .expect("g")
            .expect("u")
            .role,
        Role::Operator
    );
    // Single use.
    assert_eq!(
        users::accept_invite(&db.pool, &hash(1), "erin", "h")
            .await
            .expect("a"),
        Accepted::InvalidToken
    );
    let listed = users::list_invites(&db.pool).await.expect("list");
    assert_eq!(
        (listed[0].state, listed[0].used_by_name.as_deref()),
        (InviteState::Used, Some("dave"))
    );
    assert!(
        !users::revoke_invite(&db.pool, inv.id)
            .await
            .expect("revoke used")
    );

    // Revoked.
    let inv2 = users::create_invite(
        &db.pool,
        &hash(3),
        Role::Listener,
        None,
        admin,
        "root",
        hour,
    )
    .await
    .expect("i");
    assert!(
        users::revoke_invite(&db.pool, inv2.id)
            .await
            .expect("revoke")
    );
    assert_eq!(
        users::accept_invite(&db.pool, &hash(3), "frank", "h")
            .await
            .expect("a"),
        Accepted::InvalidToken
    );
    assert_eq!(
        users::find_invite(&db.pool, &hash(3))
            .await
            .expect("f")
            .expect("i")
            .state,
        InviteState::Revoked
    );

    // Expired.
    let inv3 = users::create_invite(
        &db.pool,
        &hash(4),
        Role::Listener,
        None,
        admin,
        "root",
        hour,
    )
    .await
    .expect("i");
    sqlx::query(
        "UPDATE superhet.invites SET expires_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(inv3.id)
    .execute(&db.pool)
    .await
    .expect("backdate");
    assert_eq!(
        users::accept_invite(&db.pool, &hash(4), "gina", "h")
            .await
            .expect("a"),
        Accepted::InvalidToken
    );
    assert_eq!(
        users::find_invite(&db.pool, &hash(4))
            .await
            .expect("f")
            .expect("i")
            .state,
        InviteState::Expired
    );
    db.drop().await;
}
