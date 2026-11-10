# OIDC login (superhet-api)

Optional single sign-on next to the local accounts; design in ADR-0019. Off by default.

## Setup

1. Register a client at the provider: type web application, grant `authorization_code`, redirect URI `<public URL>/api/v1/auth/oidc/callback` (shown on the admin page once the public URL is set). For logout also register the post-logout redirect URI `<public URL>/` and, for back-channel logout, the URL `<public URL>/api/v1/auth/oidc/backchannel-logout` (both shown on the admin page; see "Logout"). Hand-run recipes for Authentik and Keycloak: `docs/oidc-providers.md`.
2. Configure either in the admin console (Settings, "Single sign-on") or through the environment. The client secret is environment-only.
3. Use "Test connection" (`POST /api/v1/admin/oidc/test`) to run discovery, then enable.

## Environment

If `SUPERHET_OIDC_ISSUER` is set, the whole configuration comes from these variables and the admin page is read-only.

- `SUPERHET_OIDC_ISSUER`, `SUPERHET_OIDC_CLIENT_ID`, `SUPERHET_PUBLIC_URL`
- `SUPERHET_OIDC_CLIENT_SECRET` or `SUPERHET_OIDC_CLIENT_SECRET_FILE` (also in database mode; omit for a public client)
- `SUPERHET_OIDC_NAME` (button label, default "Single sign-on"), `SUPERHET_OIDC_SCOPES` (besides `openid`, default `email profile`, comma or space separated)
- `SUPERHET_OIDC_GROUPS_CLAIM` (default `groups`, dotted path for nested claims), `SUPERHET_OIDC_USERNAME_CLAIM` (default `preferred_username`)
- `SUPERHET_OIDC_ADMIN_GROUPS`, `..._OPERATOR_GROUPS`, `..._LISTENER_GROUPS` (comma separated), `SUPERHET_OIDC_DEFAULT_ROLE` (`listener`/`operator`/`admin`; empty = refuse users no group matches)
- `SUPERHET_OIDC_AUTO_PROVISION`, `SUPERHET_OIDC_LINK_BY_EMAIL`, `SUPERHET_OIDC_SYNC_ROLES` (default on), `SUPERHET_OIDC_LOCAL_LOGIN` (default on)
- `SUPERHET_OIDC_RP_LOGOUT`, `SUPERHET_OIDC_BACKCHANNEL_LOGOUT` (both default on; see "Logout")
- `SUPERHET_OIDC_KEEP_LOCAL_LOGIN` (recovery: password logins on whatever the settings say), `SUPERHET_OIDC_ALLOW_HTTP` (accept plain-http providers on non-loopback hosts; insecure)

## Rules

- Role: highest matching group list (admin > operator > listener), else default role, else refused. With `sync_roles` the role follows the provider at every login.
- Unknown identity: refused, unless `auto_provision` creates a passwordless account, or `link_by_email` finds exactly one local account with that address and the token has `email_verified: true`. Addresses are set per user by an admin (`PATCH /api/v1/admin/users/{id}` with `email`, `""` removes).
- Password logins can be disabled (`local_login: false`) only while an enabled admin is linked to an OIDC identity and OIDC is usable.

## Logout

Design in ADR-0024. Two independent switches, both default on (admin page, "Sign-out", or `SUPERHET_OIDC_RP_LOGOUT` / `SUPERHET_OIDC_BACKCHANNEL_LOGOUT` in environment mode).

- **Signing out of Superhet ends the provider session too** (`rp_logout`, OIDC RP-Initiated Logout 1.0). Only for a session that came from OIDC, and only when the provider's discovery document has an `end_session_endpoint`; otherwise logout stays local (204). `POST /api/v1/auth/logout` then answers 200 `{"end_session_url"}` and the web UI navigates there. The URL carries `id_token_hint` (the ID token of that login, kept with the session), `client_id`, `post_logout_redirect_uri` (`<public URL>/`, register it at the provider) and a random `state`. The local session is already gone when the browser leaves; the provider's answer is not needed. "Test connection" reports whether the provider advertises the endpoint.
- **The provider ends Superhet sessions** (`backchannel_logout`, OIDC Back-Channel Logout 1.0). The provider POSTs `application/x-www-form-urlencoded` `logout_token=<JWT>` to `<public URL>/api/v1/auth/oidc/backchannel-logout` when the user signs out there or an admin ends the session. The token is accepted only if its signature verifies against the provider's published keys (asymmetric algorithms only; refreshed once on an unknown key), `iss` is the provider, `aud` contains our client id, `iat` is recent (5 minutes, 1 minute skew), `exp` (if present) has not passed, `events` has the back-channel logout member, `sub` or `sid` is present, there is no `nonce`, and the `jti` was not seen before. With `sid` (the ID token's session id, stored per session at login) the sessions of that provider session end; with only `sub`, all OIDC sessions of the linked account at that issuer; with both, a session must match both. Password sessions of the same account are untouched. Answers: 200 (also when no session matched), 400 `{"error": "<reason>"}` for a refused token (`invalid_request` for a missing token or wrong content type, `not_enabled`), 429 when too many refused requests came from one address, 503 when the provider or the database is unreachable.
- A provider that does not put `sid` into the ID token can still log users out by `sub`. If it sends neither, back-channel logout cannot match anything.

## Endpoints

- `GET /api/v1/auth/oidc/login` (public): 302 to the provider; sets the binding cookie. When OIDC is off or busy it answers 302 to `/?oidc_error=not_enabled` (`rate_limited`, `unavailable`, `busy`).
- `GET /api/v1/auth/oidc/callback` (public): completes the login, sets the session cookie, 302 to `/`; on failure 302 to `/?oidc_error=<code>` (`invalid_state`, `expired`, `binding_mismatch`, `token_invalid`, `unavailable`, `no_account`, `no_role`, `disabled`, `already_linked`, `rate_limited`, ...). Never a JSON error.
- `POST /api/v1/auth/oidc/backchannel-logout` (public, called by the provider, not the browser): see "Logout".
- `GET /api/v1/auth/status` gains `"oidc": {"enabled": bool, "name": string|null, "login_url": string|null}`; the login page shows "Sign in with <name>".
- Admin (role admin, CSRF header as for other admin writes): `GET`/`PUT /api/v1/admin/oidc` return `{"oidc": {...settings...}, "source": "env"|"database", "redirect_url", "client_secret_set", "oidc_admin_exists", "local_login_active", "keep_local_login"}` (`PUT` answers 409 `managed_by_env` in env mode); `POST /api/v1/admin/oidc/test` returns `{"ok": bool, "issuer"?, "end_session"?, "error"?}`. The view also carries `post_logout_url` and `backchannel_url` (null until the public URL is set), the settings carry `rp_logout` and `backchannel_logout`.
- Users in `docs/api-users.md` responses carry `email` and `oidc` (linked identity).

Audit actions: `auth.oidc_backchannel_logout`, `auth.logout` (`provider_logout`), `auth.oidc_login`, `auth.oidc_login_failed`, `auth.oidc_link`, `auth.oidc_provision`, `user.role_change` (`source: oidc`), `settings.change`.
