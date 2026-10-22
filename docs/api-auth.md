# Auth API (superhet-api)

Local admin authentication, see ADR-0008 for the design. All bodies are JSON
(`Content-Type: application/json`, at most 8 KiB); all responses carry `Cache-Control: no-store`.
Errors are `{"error": "<code>"}`, optionally with `"message"` (human readable).

Public routes: `/healthz`, `/readyz`, `/api/v1/auth/*` and the static web UI (every other `GET`/`HEAD`
path outside `/api`, served from `SUPERHET_WEB_DIR`, default `/usr/share/superhet/web`; unknown
paths get `index.html` for client-side routing, missing `/assets/*` and `*.ext` files a plain 404;
without an `index.html` there it is a plain-text 404 hint and a one-time startup warning). **Every
other route**, including unknown `/api/*` paths and WebSocket upgrades, answers `401 {"error":"unauthenticated"}` without a valid session
cookie. WebSocket upgrades are refused with `401` before the upgrade.

POST routes also run the `Origin` check (`403 {"error":"forbidden"}`, see `SUPERHET_ALLOWED_ORIGINS`)
and require `Content-Type: application/json` (`415`) when they carry a body. Without a configured
database the auth routes answer `503 {"error":"unavailable"}`.

## Session cookie

`superhet_session=<43 chars base64url>; Path=/; Max-Age=2592000; HttpOnly; SameSite=Strict[; Secure]`.
Opaque; send it back unchanged (browsers do so automatically, same-origin `fetch` needs no extra
options). A session expires after 7 days without use or 30 days after login. `Secure` is set per
`SUPERHET_COOKIE_SECURE` (`auto`/`true`/`false`, default `auto`; `auto` needs
`SUPERHET_TRUST_PROXY=true` behind a TLS proxy).

## Endpoints

`GET /api/v1/auth/status` returns 200
`{"setup_required": bool, "authenticated": bool, "username": string|null}`. Works with or without a
cookie. The UI shows the setup wizard if `setup_required`, the login page if not `authenticated`.

`POST /api/v1/auth/setup` with `{"username", "password", "setup_token"}` creates the admin account
and logs in.
- 204 and `Set-Cookie`
- 409 `already_setup`
- 403 `invalid_setup_token`
- 400 `invalid` with `message` (username must be 1-64 characters of `[A-Za-z0-9._-]`, password 12-1024 characters)
- 429 `rate_limited`

The setup token is printed by api at startup while no account exists (WARN log line "first run: ...
setup token: ..."; `docker compose logs superhet-api`) or pinned via `SUPERHET_SETUP_TOKEN`.

`POST /api/v1/auth/login` with `{"username", "password"}`.
- 204 and `Set-Cookie`
- 401 `invalid_credentials` (same answer and similar timing for unknown user and wrong password)
- 429 `rate_limited` with `Retry-After: <seconds>` (5 failures per client address and 50 globally per 15 minutes; also blocks correct passwords while active)

`POST /api/v1/auth/logout` (no body) always answers 204, deletes the session if there is one and
clears the cookie.

`POST /api/v1/auth/password` with `{"current_password", "new_password"}` (session required).
- 204; all other sessions of the user are revoked, the calling one stays
- 401 `unauthenticated` (no session)
- 401 `invalid_credentials` (wrong current password; counts towards the rate limit)
- 400 `invalid` (new password policy)
- 429 `rate_limited`

## Client notes

- Treat any `401 unauthenticated` from a data route as "session ended": go back to `status`.
- WebSocket: the browser sends the cookie on the upgrade automatically (same origin). Non-browser
  clients must send a `Cookie: superhet_session=...` header obtained from login.
- Dev server on another origin (vite on :5173): proxy `/api` through vite so requests stay
  same-origin, or list the origin in `SUPERHET_ALLOWED_ORIGINS`; `SameSite=Strict` cookies are not sent to a different site.
