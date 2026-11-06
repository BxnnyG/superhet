# ADR-0019: Optional OpenID Connect login

**Status:** accepted (2026-11-06)

## Context

ADR-0008/0011 built local accounts, roles and server-side sessions. SCOPE and ROADMAP (P5) list generic OIDC single sign-on as an optional addition: an operator who already runs an identity provider (Zitadel, Keycloak, Authentik, Entra ID, ...) should be able to sign in with it, with groups mapped to the three roles. Superhet stays a single-instance, self-hosted app; the browser is the only first-class client. Local login must keep working: a broken provider must never lock the operator out.

## Decision

1. **Authorization Code flow with PKCE (S256), confidential or public client.** Endpoints come from issuer discovery (`/.well-known/openid-configuration`); discovery documents and key sets are cached for 15 minutes and refreshed (at most every 30 s) after a failed verification so key rotation works. The ID token is verified (signature, issuer, audience, expiry, nonce) by the `openidconnect` crate (new dependency in `superhet-api`, no default features). The userinfo endpoint is not used; claims are read from the ID token. Alternative considered: hand-rolled JOSE/JWT verification; rejected as security-critical code better left to a maintained library.
2. **State, nonce and PKCE verifier live in memory, bound to the browser.** `GET /api/v1/auth/oidc/login` creates an attempt (random state, nonce, verifier; 10 minutes; bounded store), sets a short-lived `HttpOnly; SameSite=Lax` binding cookie and redirects to the provider. The callback must present a state that exists, was not used (single use), and the matching binding cookie (login CSRF / code injection); otherwise it fails and the state is burned. A restart drops pending attempts; the user just starts again. No new table.
3. **Configuration: database document or environment.** The admin console (`/api/v1/admin/oidc`) edits an `oidc` settings document (issuer, client id, public URL, name, scopes, claim names, group lists, default role, switches). If `SUPERHET_OIDC_ISSUER` is set the whole configuration comes from `SUPERHET_OIDC_*` and the page is read-only. **The client secret is environment-only** (`SUPERHET_OIDC_CLIENT_SECRET` or `..._FILE`): the database holds no key material to encrypt it with, and a secret encrypted with a key next to it is theatre. The UI only shows "secret set". The redirect URI is derived from the configured public URL (`SUPERHET_PUBLIC_URL` or the page field), never from request headers.
4. **Claims to roles.** The highest role whose group list matches the token's groups claim wins (admin > operator > listener); no match falls back to the default role; no default role means no access. With `sync_roles` (default) the role is re-evaluated at every login; a role change is written to the audit log, and the last enabled admin is never demoted by a login. A new account gets the mapped or default role, `admin` only when a group maps to it explicitly.
5. **Provisioning and linking are opt-in.** Identities are stored by `(issuer, subject)`; `sub` is the identity, name and email are never. An unknown identity is refused unless (a) `auto_provision` is on and a role applies (a passwordless account is created, login name from the username claim, made unique) or (b) `link_by_email` is on, the token says `email_verified: true` and exactly one local account carries that address (`users.email`, set by an admin) that has no identity at this issuer yet. Unverified addresses never link.
6. **Same sessions.** A successful login creates the usual server-side session cookie (ADR-0008). Disabled accounts are refused.
7. **Local login stays.** `local_login=false` is honoured only while at least one enabled admin is linked to an OIDC identity and OIDC is usable; otherwise password logins stay on (fail open). `SUPERHET_OIDC_KEEP_LOCAL_LOGIN=true` forces them on for recovery. The setup wizard and the admin password flows are unaffected.
8. **Audit.** `auth.oidc_login`, `auth.oidc_login_failed` (with a stable `reason`), `auth.oidc_link`, `auth.oidc_provision`, `user.role_change` (`source: oidc`) and `settings.change` for the document. Provider error text is logged, never shown to the browser; failures redirect to `/?oidc_error=<code>`.
9. **Transport hygiene.** Only `https://` providers (plain `http://` for loopback, or `SUPERHET_OIDC_ALLOW_HTTP` for lab setups); the HTTP client follows no redirects and has short timeouts and a 1 MiB response cap.

## Consequences

- `rsa`/`crypto-bigint` enter the tree with `openidconnect`; two timing advisories concern private-key operations Superhet never does, so `deny.toml` ignores them with a reason.
- Logout is local only; there is no RP-initiated logout or back-channel logout. Sessions end by logout, expiry or revocation as before.
- Group changes at the provider apply at the next login, not instantly.
- Pending logins do not survive an api restart or span several api replicas (single instance by design).
- Tested against a mock provider in-process (`tests/oidc_live.rs`); real-provider quirks (e.g. groups in non-standard claims) are covered by the claim path setting only.
