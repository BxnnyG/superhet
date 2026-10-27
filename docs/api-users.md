# Users, roles, invites, guests and live sessions (v1)

Backend contract of ADR-0011 (users, roles, guest access) and the admin part of ADR-0012. The audit
log has its own document, `docs/api-audit.md`; login, logout and the cookie are in
`docs/api-auth.md`. Conventions as there: JSON bodies (`Content-Type: application/json`, at most
16 KiB), `Cache-Control: no-store`, errors are `{"error": "<code>"[, "message": "..."]}`, mutating
routes run the `Origin` check (`403 forbidden`).

## Roles

| Role | May |
|---|---|
| `admin` | everything, including `/api/v1/admin/*` and installing / removing plugins (`docs/api-plugins.md`) |
| `operator` | listener rights plus: centre/rate/gain/ppm/bias (`/tune`), the tuning lock, profiles, bookmark edits, clearing the decoder history, modifying and hearing any VFO, listing plugins and starting / stopping / restarting plugin instances |
| `listener` | own VFOs (create, move, change, delete) and their audio, shared VFOs (listen only), spectrum, state, decoder history, reading bookmarks |
| guest | like `listener`, but no account: no bookmarks, no own sessions page, decoder history only if the policy shows it, and every limit of the guest policy |

The access table is `crates/superhet-api/src/access.rs` (`RULES`): one entry per protected route
(method, route pattern, weakest access). The middleware refuses a route without an entry (`403`)
and a unit test fails if a registered route has none, so a new endpoint cannot ship unguarded. An
integration test (`multiuser_live.rs`) drives every entry as anonymous, guest, listener, operator
and admin.

| Access | Meaning |
|---|---|
| `Guest` | any signed-in caller, guests included |
| `GuestDecoder` | accounts always, guests only while the policy sets `show_decoder_events` |
| `Listener` | any account |
| `Operator` | operator, admin |
| `Admin` | admin |

Status codes: `401 unauthenticated` (no/invalid session, or a guest cookie while guest mode is off),
`403 forbidden` (role too low; the refusal is audited as `access.denied`), `403
password_change_required` (the account still has a one-time password: only
`POST /api/v1/auth/password` and logout work).

Every protected request re-reads the role of the account, so a role change, disable or delete
applies to the very next request of an existing session. WebSockets keep their role until they
reconnect, except that disabling, deleting, a password reset and a kick close them at once.

## Auth additions (public, `docs/api-auth.md` has the rest)

`GET /api/v1/auth/status` now returns
`{"setup_required","authenticated","username","user_id","role","kind":"user"|"guest"|null,
"must_change_password","guest":{"enabled","privacy_text"}}`. For a guest `username` and `user_id`
are `null`, `role` is `listener`. `guest.privacy_text` is only set while guest mode is on.

`POST /api/v1/auth/guest` (no body) starts a guest session. `404` while guest mode is off (the
endpoint does not exist then). Otherwise `200 {"guest":{"id"},"limits":{"idle_timeout_secs",
"max_vfos","allowed_modes","freq_min_hz","freq_max_hz","show_decoder_events"}}` and
`Set-Cookie: superhet_guest=<token>; Path=/; HttpOnly; SameSite=Strict[; Secure]` (a browser-session
cookie; the server side idle timeout ends the session). A browser that already holds a live guest
cookie gets `200` without a new cookie. Refusals: `503 guests_full` (`max_guests` reached), `429
too_many_guests` (`max_guests_per_ip` reached for this address). Writes `guest.session_start`.
`POST /api/v1/auth/logout` also ends a guest session and deletes the guest's VFOs.

A guest session ends after `idle_timeout_secs` without any request **and** without an open
WebSocket (a visitor who just listens keeps sockets open and stays; closing the tab starts the
clock), when an admin kicks it, or when guest mode is switched off. The API then deletes the guest's VFOs on the
devices (within about 2 s).

`GET /api/v1/auth/invites/{token}` previews an open invite: `200 {"role","expires_at_ms",
"invited_by"}` or `404 invalid_invite` (unknown, used, revoked or expired; counts as a failed
attempt for the rate limiter).

`POST /api/v1/auth/invites/accept` with `{"token","username","password"}` creates the account with
the role fixed at invite time and logs in: `204` and `Set-Cookie`; `404 invalid_invite`, `409
username_taken` (the invite stays usable), `400 invalid` (username/password policy), `429`.
The username/password rules are those of the setup (`docs/api-auth.md`).

Login additions: a disabled account fails exactly like an unknown user (`401 invalid_credentials`,
including the Argon2 verification time); besides the per-address limit (5 failures / 15 min) there
is a per-name limit (10 failures / 15 min for a user name, from any address, unknown names count
too), both answered `429 rate_limited` with `Retry-After`.

## Admin API (`/api/v1/admin`, role `admin`)

### Users

- `GET /api/v1/admin/users` -> `{"users":[User]}` with
  `User = {"id","username","role","disabled","must_change_password","created_at_ms",
  "last_login_ms","sessions"}` (`sessions` = number of live sessions).
- `GET /api/v1/admin/users/{id}` -> `{"user":User}`; `404 not_found`.
- `POST /api/v1/admin/users` with `{"username","role","password"?}` -> `201 {"user":User,
  "password"?}`. Without `password` the API generates one (20 characters) and returns it **once**.
  Any password set by an admin is a one-time password: the account has `must_change_password:true`
  until its owner changes it. `409 username_taken`, `400 invalid`.
- `PATCH /api/v1/admin/users/{id}` with `{"role"?,"disabled"?}` (at least one) -> `200
  {"user":User}`. Disabling revokes all sessions and closes their sockets. `409 last_admin` when it
  would leave no active admin (demote, disable). Role changes keep sessions valid.
- `DELETE /api/v1/admin/users/{id}` -> `204`; sessions go with the user, audit rows stay with
  `actor_name` replaced by `deleted-user-<id>`. `409 last_admin`, `404`.
- `POST /api/v1/admin/users/{id}/password` with `{"password"?}` (body optional) -> `200
  {"password"}` when generated, `204` otherwise. Revokes all sessions, closes their sockets, sets
  `must_change_password`.

The last-admin rule is enforced in the database transaction (the active admin rows are locked), so
two concurrent requests cannot both succeed.

### Invites

- `POST /api/v1/admin/invites` with `{"role","expires_in_hours"?(1..720, default 72),"note"?}` ->
  `201 {"invite":Invite,"token","path":"/invite/<token>"}`. The token is random (256 bit), shown
  once and stored only as SHA-256. `Invite = {"id","role","note","created_by","created_at_ms",
  "expires_at_ms","state":"open"|"used"|"expired"|"revoked","used_by"}`.
- `GET /api/v1/admin/invites` -> `{"invites":[Invite]}`, newest first.
- `DELETE /api/v1/admin/invites/{id}` -> `204`; `404` when it is not open any more.

The web UI serves the link path (`/invite/<token>`); it calls the two public invite endpoints above.

### Live sessions and kick

`GET /api/v1/admin/sessions` -> `{"sessions":[...]}`: every logged-in account session and every
guest.

`{"sid":"u.<hex>","kind":"user","user_id","username","role","ip","user_agent","created_at_ms",
"last_seen_ms","connections":[{"kind":"spectrum|audio|state|events","device","since_ms"}],
"vfos":[{"device","id","freq_hz","mode","status"}]}` for accounts and
`{"sid":"g.<id>","kind":"guest","role":"listener","ip","user_agent","created_at_ms","idle_secs",
"connections":[...],"vfos":[...]}` for guests, each with `peer_addr`, `ip_info`, `peer_info` and `ua_info` (see
`docs/api-audit.md`, `docs/client-addresses.md`). `ip` follows the audit IP mode of the actor class
(`docs/api-audit.md`), so guests show up truncated by default. `vfos` are the channels the
session's owner has on the devices the API has seen.

Both kinds also carry `client_report` (`null` when the browser reported nothing): the device
details the browser claims, see "Client report" below. They are **unverified**; the public address
inside is shown in the audit IP mode of the actor class (`ip_mode_user` / `ip_mode_guest`).

`DELETE /api/v1/admin/sessions/{sid}` kicks: the session is revoked (account) or ended (guest) and
all its WebSockets close (within 2 s in the integration test; they are signalled at once). A kicked
guest's VFOs are deleted. `204`, `404` unknown, `400` malformed `sid`. Audited as `session.kick`.

### Address bans

A ban blocks a host or a network from the whole API (`/api/`): the check runs **before**
authentication, so a banned address cannot log in, enter as guest, preview an invite, run the
setup or open a WebSocket, even with a valid session cookie. Such a request gets `403
{"error":"banned","message","expires_at_ms"}` (the deadline, `null` for permanent; the reason is
never shown). `/healthz`, `/readyz` and the static web UI are not gated, so a banned visitor sees
the explanation instead of a blank page. The client address is the one described in
`docs/client-addresses.md`: the socket peer, or the address a trusted proxy forwarded
(`SUPERHET_TRUST_PROXY`, `SUPERHET_TRUSTED_PROXIES`; IPv4-mapped IPv6 is folded to IPv4).
The active bans are cached in memory and rebuilt from the database at start; an expiry applies
to the next request.

- `GET /api/v1/admin/bans[?state=active|all]` -> `{"bans":[Ban]}`, newest first (`all` adds lifted
  and expired bans of the last 30 days, at most 500). `Ban = {"id","network","reason","state":
  "active"|"expired"|"lifted","created_at_ms","created_by","expires_at_ms","lifted_at_ms",
  "lifted_by"}`; `network` is CIDR text (`203.0.113.7/32`).
- `POST /api/v1/admin/bans` with exactly one of `"network"` (`203.0.113.7`, `203.0.113.0/24`,
  `2001:db8::/48`; host bits are cleared) or `"session"` (a live session id `g.<id>` / `u.<hex>`:
  bans that session's exact client address, whatever the audit IP mode shows), plus optional
  `"reason"` (200 printable characters), `"expires_in_minutes"` (1..525600, absent = permanent) and
  `"kick"` (default `true`) -> `201 {"ban":Ban,"kicked":n}`. With `kick` every guest session and
  every account session opened from the network ends at once (sockets close, guest VFOs go);
  accounts can log in again from elsewhere. Refusals: `400 invalid` (not an address, prefix
  shorter than /8 IPv4 or /32 IPv6, both or neither of `network`/`session`), `404 not_found`
  (unknown session), `409 would_ban_self` (the network contains the caller's own address),
  `409 already_banned` (identical active ban), `409 no_address` (the session has no known
  address), `409 too_many_bans` (1000 active). Audited as `ban.create`.
- `DELETE /api/v1/admin/bans/{id}` lifts a ban: `204`, `404` when it is not in force. Audited as
  `ban.lift`. Refused requests are audited as `access.banned` (coalesced per network).

The web console offers "Kick & ban" on the live sessions and on audit rows (the address of an
audit row is the stored one: with the default guest mode `truncated` it is the /24 network
address, so the console asks which network to ban).

### Self-service

Role `listener` and up (not guests):

- `GET /api/v1/me/sessions` -> `{"sessions":[{"sid","ip","peer_addr","ip_info","ua_info","user_agent","created_at_ms",
  "last_seen_ms","current"}]}`.
- `DELETE /api/v1/me/sessions/{sid}` -> `204`, `404` (not yours). Revoking the current session logs
  out. Audited as `session.revoke`.

### Guest policy

`GET|PUT /api/v1/admin/guest-policy` -> `{"guest_policy":{...}}`. `PUT` replaces the document
(omitted fields take their defaults, unknown fields are `400`):

| Field | Default | Range / meaning |
|---|---|---|
| `enabled` | `false` | guest mode on/off; switching off ends all guest sessions |
| `max_guests` | 5 | 0..1000 concurrent guests |
| `max_guests_per_ip` | 2 | 1..100 per client address (`SUPERHET_TRUST_PROXY` applies) |
| `max_vfos_per_guest` | 1 | 1..16 |
| `allowed_modes` | `["wfm","nfm","am"]` | subset of `wfm nfm am usb lsb cw` |
| `freq_min_hz`, `freq_max_hz` | `null` | optional frequency window for guest VFOs |
| `idle_timeout_secs` | 600 | 30..86400 |
| `only_without_lock` | `false` | guests cannot create VFOs while any tuning lock is held |
| `show_decoder_events` | `false` | decoder history and its WebSocket for guests |
| `privacy_text` | built-in notice | up to 4000 characters, shown on the guest page |

### Client report

Optional, off by default, controlled by the admin. A browser can tell the server what it is; the
server stores it with the session and shows it in Live sessions and in the audit log, always as
**client-reported and unverified**. It never feeds bans, rate limits or access decisions.

`GET|PUT /api/v1/admin/client-report-policy` (admin) -> `{"client_report":{...}}`. `PUT` replaces
the document (omitted fields take the defaults, unknown fields are `400`), audited as
`settings.change` with `target:"client_report"`:

| Field | Default | Meaning |
|---|---|---|
| `collect` | `"off"` | `off`, `users` (accounts only) or `all` (accounts and guests): whose browsers are asked |
| `ask_public_ip` | `false` | also make the browser look up its public address; only for the browsers `collect` covers |
| `public_ip_service` | `https://api.ipify.org?format=json` | `https://` URL answering `{"ip":"..."}` or the bare address; no credentials, at most 200 characters |

`GET /api/v1/auth/status` tells the browser what is asked: `client_report` is `null` or
`{"public_ip_service": <url|null>}` for a signed-in caller, and `guest.client_report` (same shape)
tells the guest page up front while guest mode is on.

`POST /api/v1/auth/client-report` (any signed-in caller, guests included; same-origin check; body
up to 8 KiB, `Content-Type: application/json`) stores the report for the caller's own session and
answers `204`. Every field is optional, unknown fields are `400`:

`platform`, `platform_version`, `model`, `architecture`, `bitness` (strings, at most 64
characters, no control characters), `mobile` (bool), `brands` (up to 12 `{brand,version}` from
`fullVersionList`), `screen` `{width,height,dpr}`, `languages` (up to 12), `timezone`,
`touch_points`, `cpu_cores`, `memory_gb`, `connection` `{kind,effective_type,downlink_mbps,save_data}`,
`public_ip` (a literal address; dropped unless `ask_public_ip` and the caller's group is asked) and
`public_ip_status` (`ok`, `blocked`, `failed`). `409 disabled` when the policy does not ask this
caller, `400 invalid` for a malformed report. Stored on the session (`sessions.client_report`,
migration 0012) for accounts; guests keep it in memory until the guest session ends. Each accepted
report is also audited as `session.client_report` with the report in `details.client` (the public
address in the audit IP mode of the actor class), so it stays readable for as long as audit
retention allows. In the admin API the stored object comes back as `client_report` with
`reported_at_ms`, and `public_ip` plus `public_ip_info` (`class`, `label`, `family`; no reverse
DNS or GeoIP is ever done for a claimed address).

The page's content security policy allows exactly one extra `connect-src` origin, the host of
`public_ip_service`, while `ask_public_ip` is on and somebody is asked; an open tab needs a reload
after the admin turns it on. Privacy: `docs/client-addresses.md`, "Client-reported information".

### VFO limits

`GET|PUT /api/v1/admin/limits` -> `{"limits":{"listener_max_vfos":4,"operator_max_vfos":0}}`
(0..64; `0` = only the daemon's cap). Admins have no per-user limit. The daemon cap itself is
`SUPERHET_MAX_VFOS` (sdrd, default 8, at most 64).

### Audit

`GET /api/v1/admin/audit`, `GET /api/v1/admin/audit/export` and `GET|PUT
/api/v1/admin/audit-settings`: `docs/api-audit.md`.

## VFO ownership and quotas (devices API)

Every VFO created through the API carries an `owner` (`u:<user id>` or `g:<guest id>`) that is set
by the API from the session; a client-supplied `owner` is discarded. VFOs created by the daemon
(the default VFO) or by loading a profile have no owner: they are **shared**.

- Listeners and guests may modify and delete only their own VFOs (`403 not_owner` otherwise, `404`
  for an unknown id); operators and admins may change any VFO.
- They hear their own and shared VFOs on the audio WebSocket; subscribing to somebody else's, or to
  an id the daemon does not know, answers `{"v":1,"type":"error","error":"forbidden"}` and keeps the
  previous subscription. Operators and admins hear all.
- Creating a VFO is refused with `403` and `code` `vfo_limit` (quota reached, per user: listener and
  operator limits above, guests `max_vfos_per_guest`), `mode_not_allowed`, `frequency_not_allowed`
  (guest policy) or `locked` (`only_without_lock`). Counting and creating happen under one gate, so
  concurrent requests cannot exceed a quota.
- The state document (`GET .../state`, `GET .../vfos`, the state WebSocket) is prepared per viewer:
  every VFO has `"mine":true|false` and `"shared":true|false` (no owner: everybody may listen;
  without it a listener could not tell a shared VFO from somebody else's); the raw `owner` is only
  visible to operators and admins;
  guests also lose lock holder names and ids.
- `POST|DELETE /api/v1/devices/{dev}/lock` need `operator`. The holder is the account (`kind:
  "user"`, `id:"u:<id>"`, `name:<login>`), each account releases only its own lock;
  `DELETE ...?force=true` (admin only) releases every user lock. A tune refused by a lock names
  the holders (`409`, `holders[].name`). The lock of a daemon that ran before accounts existed
  has the anonymous holder `id:"user"`; an admin releases it with `force`.
- `POST /api/v1/devices/{dev}/profile/load` replaces all VFOs of the device with the profile's
  shared VFOs (including those of listeners); only operators and admins can do that.

## Notes for the web UI

- Gate the app on `GET /api/v1/auth/status`: `setup_required` -> setup wizard; not `authenticated`
  -> login page (plus "continue as guest" when `guest.enabled`, with `guest.privacy_text`);
  `must_change_password` -> password-change page (every other route answers `403
  password_change_required`). Invite links open `/invite/<token>` and use the two public invite
  endpoints.
- Hide what the role cannot do (tune, gain, lock, profiles, bookmark edits for listeners; guests
  also lose bookmarks and the sessions page); the server answers `403 forbidden` anyway.
- A closed WebSocket may mean a kick or a disabled account: re-read `auth/status` and go to the
  login page on `401`.
- VFOs: use `mine` and `shared` to tell own channels from shared and foreign ones; only own ones
  are editable for listeners and guests, and only own/shared ones can be listened to (subscribe
  the audio socket to those only: one foreign id makes the whole subscription `forbidden`). Do not assume 8 VFOs
  per device (`SUPERHET_MAX_VFOS`) or per user (`/admin/limits`, `403 vfo_limit`).
- The lock is per account: show the holder's `name` from `locks[]`, unlock releases only the
  caller's own lock (`force=true` for admins).
