# OIDC login (superhet-api)

Optional single sign-on next to the local accounts; design in ADR-0019. Off by default.

## Setup

1. Register a client at the provider: type web application, grant `authorization_code`, redirect URI `<public URL>/api/v1/auth/oidc/callback` (shown on the admin page once the public URL is set).
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
- `SUPERHET_OIDC_KEEP_LOCAL_LOGIN` (recovery: password logins on whatever the settings say), `SUPERHET_OIDC_ALLOW_HTTP` (accept plain-http providers on non-loopback hosts; insecure)

## Rules

- Role: highest matching group list (admin > operator > listener), else default role, else refused. With `sync_roles` the role follows the provider at every login.
- Unknown identity: refused, unless `auto_provision` creates a passwordless account, or `link_by_email` finds exactly one local account with that address and the token has `email_verified: true`. Addresses are set per user by an admin (`PATCH /api/v1/admin/users/{id}` with `email`, `""` removes).
- Password logins can be disabled (`local_login: false`) only while an enabled admin is linked to an OIDC identity and OIDC is usable.

## Endpoints

- `GET /api/v1/auth/oidc/login` (public): 302 to the provider; sets the binding cookie. When OIDC is off or busy it answers 302 to `/?oidc_error=not_enabled` (`rate_limited`, `unavailable`, `busy`).
- `GET /api/v1/auth/oidc/callback` (public): completes the login, sets the session cookie, 302 to `/`; on failure 302 to `/?oidc_error=<code>` (`invalid_state`, `expired`, `binding_mismatch`, `token_invalid`, `unavailable`, `no_account`, `no_role`, `disabled`, `already_linked`, `rate_limited`, ...). Never a JSON error.
- `GET /api/v1/auth/status` gains `"oidc": {"enabled": bool, "name": string|null, "login_url": string|null}`; the login page shows "Sign in with <name>".
- Admin (role admin, CSRF header as for other admin writes): `GET`/`PUT /api/v1/admin/oidc` return `{"oidc": {...settings...}, "source": "env"|"database", "redirect_url", "client_secret_set", "oidc_admin_exists", "local_login_active", "keep_local_login"}` (`PUT` answers 409 `managed_by_env` in env mode); `POST /api/v1/admin/oidc/test` returns `{"ok": bool, "issuer"?, "error"?}`.
- Users in `docs/api-users.md` responses carry `email` and `oidc` (linked identity).

Audit actions: `auth.oidc_login`, `auth.oidc_login_failed`, `auth.oidc_link`, `auth.oidc_provision`, `user.role_change` (`source: oidc`), `settings.change`.
