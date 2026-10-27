# ADR-0008: Local admin authentication

**Status:** accepted (2026-10-20), partially superseded by [ADR-0011](0011-users-roles-and-guest-access.md) (single admin, no roles, no anonymous mode)

## Context

Superhet is a single-user, self-hosted web SDR that is reachable over the network (published port 8073, usually behind the user's reverse proxy). Whoever can reach the API can retune the receiver, listen in and, later, install plugins that run code. There must be no anonymous mode (SCOPE section 7). OIDC is planned for P5; until then a local account has to carry everything. The browser is the only first-class client: it needs a cookie session that JavaScript cannot read, that is not sent cross-site, and that can be revoked server-side. The API and the web UI are served from the same origin.

## Options

- **Stateless JWT in a cookie**: no database lookup, but no revocation (logout, password change) without a denylist, which is state again.
- **HTTP Basic / API tokens only**: no logout, credentials in every request, poor fit for WebSockets in browsers.
- **Server-side sessions in PostgreSQL**: one indexed lookup per request, trivially revocable, survives restarts, PostgreSQL is already a hard dependency (ADR-0006).

## Decision

**One local admin account, Argon2id password hashes, server-side sessions in PostgreSQL, opaque random cookie.** Implemented in `superhet-api` (`auth` module) and `superhet-db` (`auth` queries, migration `0002_auth.sql`). The wire contract is `docs/api-auth.md`.

- **Single admin.** `superhet.users` has a unique index on a constant expression, so a second row is impossible and concurrent first-run setups cannot both win. Usernames are `[A-Za-z0-9._-]`, 1-64 characters, case-insensitive. Passwords are 12-1024 characters, no composition rules.
- **First-run setup token.** The setup endpoint is reachable by anyone who can reach the port, so a fresh install must not let the first visitor become admin. While no user exists, api generates a random token (192 bit) at startup and logs it at WARN level with instructions (`docker compose logs superhet-api`). `SUPERHET_SETUP_TOKEN` (at least 16 characters) pins a value instead, e.g. for unattended installs; in that case the value is not logged. The token is compared in constant time, lives only in memory, is replaced on every restart until the account exists and is dead afterwards (`409 already_setup`). Wrong tokens count as failed attempts for the rate limiter.
- **Password hashing.** Argon2id, OWASP parameters (m = 19456 KiB, t = 2, p = 1), random salt, PHC string. Hashing runs on the blocking pool, at most four at a time, so a flood of logins cannot exhaust memory. Login always performs a verification, against a dummy hash for unknown users, so response time does not reveal valid usernames.
- **Sessions.** 32 random bytes from the OS CSPRNG, base64url, sent as cookie `superhet_session`. The database stores only the SHA-256 of the token (a leaked dump yields no usable cookies), plus created_at, last_seen_at, a truncated user agent and the client address for a future "active sessions" view. Idle timeout 7 days (sliding; `last_seen_at` is refreshed at most once a minute), absolute lifetime 30 days. Expired sessions are rejected on access (and the row deleted) and pruned hourly by a background task. Changing the password revokes all other sessions; logout deletes the session and clears the cookie.
- **Cookie.** `Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000`. `Secure` follows `SUPERHET_COOKIE_SECURE`: `true` always, `false` never, `auto` (default) only if the request arrived over https. api itself speaks plain HTTP, so `auto` behind a TLS-terminating proxy needs `SUPERHET_TRUST_PROXY=true`, which makes api honour `X-Forwarded-Proto` (and `X-Forwarded-For`, rightmost entry, for rate limiting). Without it these headers are ignored because clients could forge them. Browsers accept `Secure` cookies on `http://localhost`, but a plain-http LAN deployment needs `false`.
- **CSRF.** `SameSite=Strict` plus the existing `Origin` check (`SUPERHET_ALLOWED_ORIGINS`, default same-origin) on all state-changing routes including the auth endpoints (login CSRF), and a JSON content-type requirement. WebSocket upgrades need the session and pass the `Origin` check; the session check runs first and refuses with `401` before the upgrade.
- **Deny by default.** The session check wraps every route except `/healthz`, `/readyz` and `/api/v1/auth/*`, and also the fallback, so unknown paths answer `401` too and nothing leaks about which routes exist. A route added later is protected automatically. Without a database configured nothing can authenticate: protected routes answer `401`, auth endpoints `503`.
- **Rate limiting.** In-memory sliding window of failed attempts (login, setup token, password change): 5 failures per client address per 15 minutes, 50 globally (IPv6 addresses count per /64). A blocked client gets `429 {"error":"rate_limited"}` with `Retry-After`, even with the correct password; a success clears that client's counter. The global limit stops distributed guessing at the price that an attacker can lock everybody out for up to 15 minutes by spending 50 failures; with a single user and an operator who can restart api that is the better side of the trade. State is lost on restart (no persistence by design).
- **Migrations.** api applies the embedded migrations at startup (sqlx takes a PostgreSQL advisory lock, so concurrent services are safe) and refuses to start if that fails. This replaces the earlier idea that only the worker migrates: api needs the auth tables before it can serve anything and must not depend on start order.

## Consequences

- One DB lookup per authenticated request and per WebSocket upgrade (not per frame). Acceptable for a single user.
- Losing the database loses the admin account; recovery is "wipe the volume, run setup again". A CLI `reset-password` can follow if needed.
- A forgotten setup token is recoverable from the container log; operators who ship logs elsewhere should treat that log line as sensitive until setup is done.
- There is no multi-user model, roles or API tokens. OIDC (P5) adds identities in a separate table that map to the local user; the local admin stays as fallback and can then be disabled. Per-device or per-feature authorisation is out of scope until there is more than one user.
- Rate limiting by address is only as good as the address api sees (`SUPERHET_TRUSTED_PROXIES` now limits header trust to listed proxies and a peer address is kept next to the client address, `docs/client-addresses.md`); behind a proxy without `SUPERHET_TRUST_PROXY=true` all clients share the proxy's address.
