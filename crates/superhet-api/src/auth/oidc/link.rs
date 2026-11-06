//! The account decision after a verified login: sign in, link to a local account, provision a
//! new one, or refuse. Pure: the caller looks the facts up and applies the outcome.

use superhet_db::oidc::LinkedUser;
use superhet_db::users::Role;

use super::config::{Identity, OidcSettings};

/// Why a login is refused (also the `reason` of the audit event and the `oidc_error` code the
/// login page receives).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// The identity is not linked to an account and neither linking nor provisioning applies.
    NoAccount,
    /// Neither a group nor the default role grants access.
    NoRole,
    /// The account is disabled.
    Disabled,
    /// The matching local account already has another identity at this provider.
    AlreadyLinked,
}

impl Denial {
    /// Stable code for audit and the login page.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::NoAccount => "no_account",
            Self::NoRole => "no_role",
            Self::Disabled => "disabled",
            Self::AlreadyLinked => "already_linked",
        }
    }
}

/// What to do with a verified login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The identity is linked already: sign in.
    SignIn(LinkedUser),
    /// First login of a local account that matches by verified email: link, then sign in.
    Link(LinkedUser),
    /// Create an account with this role.
    Provision(Role),
    /// Refuse.
    Deny(Denial),
}

/// What the database knows about the login.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// The account linked to `(issuer, subject)`.
    pub linked: Option<LinkedUser>,
    /// The account whose email equals the token's address (only looked up when needed).
    pub by_email: Option<LinkedUser>,
    /// `by_email` already has an identity at this issuer.
    pub by_email_has_identity: bool,
}

/// Whether the facts call for an email lookup at all (saves the query otherwise).
#[must_use]
pub fn wants_email_lookup(settings: &OidcSettings, identity: &Identity, linked: bool) -> bool {
    !linked && settings.link_by_email && identity.email_verified && identity.email.is_some()
}

/// Decide what happens with `identity`. `role` is the role the group mapping yields.
///
/// Rules:
/// - A linked identity signs in (unless the account is disabled). With `sync_roles` a login
///   without any role (no group, no default) is refused, so removing a user from all groups
///   at the provider locks them out.
/// - An unlinked identity is linked to a local account only if `link_by_email` is on, the
///   token's address is marked verified by the provider and exactly that account has it. An
///   unverified address never links.
/// - Otherwise a new account is created if `auto_provision` is on and a role is granted.
#[must_use]
pub fn decide(
    settings: &OidcSettings,
    identity: &Identity,
    role: Option<Role>,
    facts: &Facts,
) -> Decision {
    let role_ok = !settings.sync_roles || role.is_some();
    if let Some(user) = &facts.linked {
        if user.disabled {
            return Decision::Deny(Denial::Disabled);
        }
        if !role_ok {
            return Decision::Deny(Denial::NoRole);
        }
        return Decision::SignIn(user.clone());
    }
    if settings.link_by_email
        && identity.email_verified
        && let Some(user) = &facts.by_email
    {
        if user.disabled {
            return Decision::Deny(Denial::Disabled);
        }
        if facts.by_email_has_identity {
            return Decision::Deny(Denial::AlreadyLinked);
        }
        if !role_ok {
            return Decision::Deny(Denial::NoRole);
        }
        return Decision::Link(user.clone());
    }
    if settings.auto_provision {
        return match role {
            Some(role) => Decision::Provision(role),
            None => Decision::Deny(Denial::NoRole),
        };
    }
    Decision::Deny(Denial::NoAccount)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> OidcSettings {
        OidcSettings {
            enabled: true,
            default_role: "listener".into(),
            ..OidcSettings::default()
        }
    }

    fn identity(email: Option<&str>, verified: bool) -> Identity {
        Identity {
            issuer: "https://idp".into(),
            subject: "sub-1".into(),
            email: email.map(str::to_owned),
            email_verified: verified,
            groups: Vec::new(),
            username: "anna".into(),
        }
    }

    fn user(id: i64, disabled: bool) -> LinkedUser {
        LinkedUser {
            id,
            username: format!("u{id}"),
            role: Role::Operator,
            disabled,
        }
    }

    fn linked(u: LinkedUser) -> Facts {
        Facts {
            linked: Some(u),
            ..Facts::default()
        }
    }

    fn by_email(u: LinkedUser) -> Facts {
        Facts {
            by_email: Some(u),
            ..Facts::default()
        }
    }

    #[test]
    fn a_linked_identity_signs_in() {
        let d = decide(
            &settings(),
            &identity(None, false),
            Some(Role::Listener),
            &linked(user(1, false)),
        );
        assert_eq!(d, Decision::SignIn(user(1, false)));
    }

    #[test]
    fn a_linked_but_disabled_account_is_refused() {
        let d = decide(
            &settings(),
            &identity(None, false),
            Some(Role::Admin),
            &linked(user(1, true)),
        );
        assert_eq!(d, Decision::Deny(Denial::Disabled));
    }

    #[test]
    fn a_linked_user_without_any_role_is_refused_only_when_roles_sync() {
        let mut s = settings();
        let facts = linked(user(1, false));
        assert_eq!(
            decide(&s, &identity(None, false), None, &facts),
            Decision::Deny(Denial::NoRole)
        );
        s.sync_roles = false;
        assert_eq!(
            decide(&s, &identity(None, false), None, &facts),
            Decision::SignIn(user(1, false))
        );
    }

    #[test]
    fn linking_by_email_needs_the_switch_and_a_verified_address() {
        let facts = by_email(user(7, false));
        let verified = identity(Some("a@x.org"), true);
        let unverified = identity(Some("a@x.org"), false);
        let mut s = settings();
        // Off by default: the match is ignored (and no account is created either).
        assert_eq!(
            decide(&s, &verified, Some(Role::Listener), &facts),
            Decision::Deny(Denial::NoAccount)
        );
        s.link_by_email = true;
        assert_eq!(
            decide(&s, &verified, Some(Role::Listener), &facts),
            Decision::Link(user(7, false))
        );
        // The provider does not vouch for the address: never link.
        assert_eq!(
            decide(&s, &unverified, Some(Role::Listener), &facts),
            Decision::Deny(Denial::NoAccount)
        );
    }

    #[test]
    fn an_unverified_address_does_not_fall_through_to_a_takeover_via_provisioning() {
        let facts = by_email(user(7, false));
        let mut s = settings();
        s.link_by_email = true;
        s.auto_provision = true;
        // A separate account is created; the existing one stays untouched.
        assert_eq!(
            decide(
                &s,
                &identity(Some("a@x.org"), false),
                Some(Role::Listener),
                &facts
            ),
            Decision::Provision(Role::Listener)
        );
    }

    #[test]
    fn linking_never_touches_disabled_or_already_linked_accounts() {
        let mut s = settings();
        s.link_by_email = true;
        let verified = identity(Some("a@x.org"), true);
        assert_eq!(
            decide(
                &s,
                &verified,
                Some(Role::Listener),
                &by_email(user(7, true))
            ),
            Decision::Deny(Denial::Disabled)
        );
        let taken = Facts {
            by_email: Some(user(7, false)),
            by_email_has_identity: true,
            ..Facts::default()
        };
        assert_eq!(
            decide(&s, &verified, Some(Role::Listener), &taken),
            Decision::Deny(Denial::AlreadyLinked)
        );
    }

    #[test]
    fn linking_respects_the_role_requirement() {
        let mut s = settings();
        s.link_by_email = true;
        assert_eq!(
            decide(
                &s,
                &identity(Some("a@x.org"), true),
                None,
                &by_email(user(7, false))
            ),
            Decision::Deny(Denial::NoRole)
        );
    }

    #[test]
    fn provisioning_needs_the_switch_and_a_role() {
        let mut s = settings();
        let id = identity(Some("a@x.org"), true);
        assert_eq!(
            decide(&s, &id, Some(Role::Listener), &Facts::default()),
            Decision::Deny(Denial::NoAccount)
        );
        s.auto_provision = true;
        assert_eq!(
            decide(&s, &id, Some(Role::Operator), &Facts::default()),
            Decision::Provision(Role::Operator)
        );
        assert_eq!(
            decide(&s, &id, None, &Facts::default()),
            Decision::Deny(Denial::NoRole)
        );
    }

    #[test]
    fn the_email_lookup_is_skipped_when_it_cannot_matter() {
        let mut s = settings();
        let verified = identity(Some("a@x.org"), true);
        assert!(!wants_email_lookup(&s, &verified, false), "switch off");
        s.link_by_email = true;
        assert!(wants_email_lookup(&s, &verified, false));
        assert!(!wants_email_lookup(&s, &verified, true), "already linked");
        assert!(!wants_email_lookup(
            &s,
            &identity(Some("a@x.org"), false),
            false
        ));
        assert!(!wants_email_lookup(&s, &identity(None, false), false));
    }
}
