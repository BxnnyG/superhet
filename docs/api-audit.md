# Audit log (v1)

Operator's trail of who did what (ADR-0012). Table `superhet.audit_events` in PostgreSQL, written
by `superhet-api` (accounts and guests) and `superhet-worker` (system events from the bus), read
and exported through the admin API. Not tamper-proof evidence: see "Append-only" below.

## Event

| Column | Meaning |
|---|---|
| `id` | bigint, monotonic |
| `ts` | time of the event (`ts_ms` in the API: Unix milliseconds) |
| `actor_kind` | `user`, `guest` or `system` |
| `actor_id` | user id, guest id or (system) the publishing component |
| `actor_name` | login name, `guest`, or the component; survives user deletion (replaced by `deleted-user-<id>`) |
| `role` | `admin`, `operator`, `listener` or null |
| `ip` | client address after the IP mode (below), null if not stored |
| `peer_addr` | TCP peer address after the IP mode (migration 0010; null on older rows) |
| `ip_info`, `peer_info` | computed on read from the stored value: `{class, label, family[, rdns, country, country_name, city, asn, org]}` (`class`: `loopback`, `private`, `docker_gateway`, `cgnat`, `link_local`, `multicast`, `reserved`, `public`); truncated addresses get no `rdns` and no `city` |
| `ua_info` | parsed `user_agent`: `{browser, browser_version, os, os_version, device}` (`device`: `desktop`, `mobile`, `tablet`, `bot`, `tool`, `unknown`) |
| `user_agent` | browser string, cut to 256 characters |
| `action` | dotted name, see the list |
| `target` | what was acted on (user, device, `<dev>/<vfo id>`, invite, route ...) |
| `outcome` | `ok`, `denied` or `error` |
| `details` | small JSON object; never passwords, tokens, IQ or audio |

### Actions

| Action | Written when | Notable `details` |
|---|---|---|
| `auth.setup` | the first admin is created | |
| `auth.login` / `auth.login_failed` | login ok / refused (also rate limited, disabled) | `reason`: `bad_credentials`, `disabled`, `rate_limited` |
| `auth.logout` | logout | |
| `auth.password_change` | own password change | `revoked_sessions` |
| `user.create`, `user.delete` | admin creates / deletes an account | `role` |
| `user.role_change` | role changed (also refused with `outcome:"denied"`, `reason:"last_admin"`) | `from`, `to` |
| `user.disable`, `user.enable` | account disabled / enabled | |
| `user.password_reset` | admin resets a password | `generated` |
| `invite.create`, `invite.revoke`, `invite.accept` | invite lifecycle (`accept`: the actor is the new account) | `role` |
| `session.kick` | admin kicks a session | `kind` |
| `ban.create`, `ban.lift` | admin bans or un-bans an address (`target` = network) | `reason`, `expires_at_ms`, `kicked`, `session` (create) / `ban` (lift) |
| `session.revoke` | user revokes an own session | |
| `session.connect`, `session.disconnect` | first WebSocket of a session opens / last one closes | `kind`, `duration_secs` |
| `guest.session_start`, `guest.session_end` | guest session starts / ends (idle, guest mode off) | `reason` |
| `guest.policy_change` | guest policy saved | `changed` (keys), `enabled` |
| `settings.change` | audit settings or VFO limits saved (`target`: `audit` / `limits`) | `changed` |
| `device.tune` | centre/rate/gain/ppm/bias request (coalesced) | the request fields |
| `device.lock`, `device.unlock` | tuning lock taken / released | `force` |
| `vfo.create`, `vfo.update`, `vfo.delete` | VFO changes (`vfo.update` coalesced) | `freq_hz`, `mode`, the changed fields |
| `profile.load` | profile applied to a device | `profile` |
| `access.banned` | a request from a banned address was refused before authentication (`outcome:"denied"`, no actor, coalesced per network) | `target` = the banned network, `path` |
| `access.denied` | a signed-in caller was refused by the access table (`outcome:"denied"`, coalesced) | `target` = `METHOD route-pattern` |
| `audit.export` | an export was downloaded | `format`, `rows`, `filters` |
| system events | published by services on `sh.v1.audit.<source>` | whatever the source sends |

Refused or failed device actions (locked tuning, quota, not the owner, bus errors) are written
with the same action and `outcome:"denied"` or `"error"`, with `details.status`.

### Coalescing

A tuning drag or a VFO move sends many requests. Events of one actor, action and target within
1.5 s of the first one are merged into **one row** written when the window ends: the details are
the union of the fields with the latest value winning, and `details.merged` counts the requests.
A restart of the API inside that window loses the pending rows (at most 1.5 s of events).
Spectrum and audio frames are never logged; `session.connect`/`session.disconnect` carry the
duration of a session instead.

## Privacy

Guests' addresses and user agents are personal data; the defaults are conservative.

`GET|PUT /api/v1/admin/audit-settings`:
`{"audit_settings":{"retention_user_days":90,"retention_guest_days":7,"ip_mode_user":"full",
"ip_mode_guest":"truncated"}}` (`GET` also returns `"stats":{"count","oldest_ms","bytes"}`).
`PUT` replaces the document (omitted fields take the defaults, unknown fields are `400`), logs
`settings.change`.

- **Retention** in days, `0` keeps that class forever (discouraged for guests): accounts and
  system events 90, guests 7. The worker applies it hourly (`audit::prune`), reading the stored
  values on every run. Deleting a user pseudonymises their rows, it does not remove them.
- **IP mode**: `full`, `truncated` (IPv4 /24 -> `a.b.c.0`, IPv6 /48; IPv4-mapped IPv6 counts as
  IPv4) or `none`. It is applied **before** the row is written, per actor class (`ip_mode_user`
  for accounts, `ip_mode_guest` for guests). The rate limiter keeps the full address in memory
  only. The admin session list shows addresses in the same mode.
- `ip` is the **client address**: the TCP peer, or the address a *trusted* proxy forwarded
  (`SUPERHET_TRUST_PROXY=true` for every peer, or `SUPERHET_TRUSTED_PROXIES` for listed networks;
  `X-Forwarded-For` / `Forwarded` / `X-Real-IP`, see `docs/client-addresses.md`). Headers from
  untrusted peers are ignored, because clients could forge them. `peer_addr` is always the TCP
  peer (behind Docker port forwarding often the bridge gateway), stored with the same IP mode. A
  forwarded chain goes into `details.forwarded`.
- The guest page shows `guest_policy.privacy_text` (admin-editable).

## Query: `GET /api/v1/admin/audit`

Newest first. Query parameters (all optional, unknown ones are `400`):

| Parameter | Meaning |
|---|---|
| `from_ms`, `to_ms` | time range, Unix ms (`from` inclusive, `to` exclusive) |
| `actor_kind` | `user`, `guest`, `system` |
| `actor` | exact actor id, or part of the actor name (case-insensitive) |
| `role` | `admin`, `operator`, `listener` |
| `action` | exact action, or a prefix with a trailing `*` (`user.*`) |
| `outcome` | `ok`, `denied`, `error` |
| `ip` | an address or a network (`198.51.100.0/24`, `2001:db8::/48`) containing the stored address |
| `target` | part of the target (case-insensitive) |
| `limit` | 1..1000, default 100 |
| `before_id` | paging cursor |

Reply: `{"events":[Event],"next_before_id":<id>|null}` where `Event` has the columns above
(`ts_ms`, `actor_kind`, `actor_id`, `actor_name`, `role`, `ip`, `user_agent`, `action`, `target`,
`outcome`, `details`, plus `peer_addr`, `ip_info`, `peer_info`, `ua_info`). Pass `next_before_id` as `before_id` for the next page; it is `null` on
the last page. `400` for malformed filters (`outcome=maybe`, `ip=banana`, `limit=0`).

## Export: `GET /api/v1/admin/audit/export`

Same filters (without `limit`/`before_id` paging), plus `format=csv` (default) or `json`. Rows are
oldest first, at most 100 000 (a truncated export sets `X-Superhet-Truncated: true`, JSON also
`"truncated":true`). CSV columns: `id,time,actor_kind,actor_id,actor_name,role,ip,user_agent,
action,target,outcome,details,peer_addr,address_type,reverse_dns,country,city,asn,as_org,browser,
os,device` (the enrichment columns last, empty when unknown) with `time` in ISO 8601 UTC, `details` as a JSON string, CRLF
rows and a leading apostrophe on text that a spreadsheet would run as a formula. JSON:
`{"events":[Event],"truncated":false}`. The download is an attachment and is itself audited
(`audit.export`).

## System events on the bus

Services without a database publish `sh.v1.audit.<source>` (core NATS), the worker stores them
(`actor_kind:"system"`, `actor_id`/`actor_name` = the source). Payload (`superhet-bus::audit`):

`{"v":1,"action":"plugin.crash","source":"pluginhost","target":"direwolf","outcome":"error",
"details":{...},"ts_ms":1760000000000}`; `ts_ms`, `target` and `details` are optional. `action`
and `source` are 1..128 characters of `[A-Za-z0-9._:-]`, `details` an object of at most 4096
bytes; invalid messages are dropped and counted
(`superhet_worker_audit_events_dropped_total`). Core NATS means a message published while the
worker is down is lost; a durable stream is a follow-up. Nothing publishes system events yet; the
subject, permissions in `deploy/nats/nats.conf` and the worker side are in place.

## Append-only

- A trigger refuses `UPDATE`, `DELETE` and `TRUNCATE` on `audit_events` unless the transaction
  sets `superhet.audit_maintenance` to `on`. Only the retention job and the pseudonymisation of a
  deleted user do that. This protects against bugs.
- Against a compromised API, run it under a database role with `INSERT` and `SELECT` only on the
  table (the integration test `a_restricted_api_role_...` proves the table works with such a
  role):

  ```sql
  CREATE ROLE superhet_api LOGIN PASSWORD '...';
  GRANT USAGE ON SCHEMA superhet TO superhet_api;
  -- every other table: the usual SELECT/INSERT/UPDATE/DELETE; audit_events only:
  GRANT INSERT, SELECT ON superhet.audit_events TO superhet_api;
  ```

  The worker keeps the owning role (it prunes). The compose stack uses one database user; the
  role split is an operator hardening step. A database superuser can always change the table.

## Configuration summary

| Setting | Where | Default |
|---|---|---|
| retention and IP modes | `PUT /api/v1/admin/audit-settings` | 90 d / 7 d, `full` / `truncated` |
| trusted proxy header | `SUPERHET_TRUST_PROXY` (api) | off |
| trusted proxy networks | `SUPERHET_TRUSTED_PROXIES` (api) | none |
| reverse DNS / GeoIP | `SUPERHET_REVERSE_DNS`, `SUPERHET_GEOIP_DB` (api) | on / off |
| daemon VFO cap | `SUPERHET_MAX_VFOS` (sdrd) | 8 |
