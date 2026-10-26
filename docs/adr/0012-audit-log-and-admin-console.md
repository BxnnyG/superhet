# ADR-0012: Audit log and admin console

**Status:** accepted (2026-10-26)

## Context

With several users and optional anonymous guests (ADR-0011) an operator needs to know who did what, manage accounts, see and kick live connections and tune the guest policy. Today the only record is the tracing log of the services: unstructured, not queryable from the UI, and without retention control.

## Options

- **Service logs only**: unstructured, per service, rotated by the container runtime.
- **External log stack (Loki/ELK)**: heavy for a self-hosted single-host tool; remains possible in addition.
- **Append-only table in PostgreSQL** (ADR-0006), shown in the admin page.

## Decision

**An append-only `audit_events` table in PostgreSQL and an admin console in the web UI.**

Table `audit_events`: `id` (bigserial), `ts` (timestamptz), `actor_kind` (`user|guest|system`), `actor_id` (user id or guest session id), `actor_name` (denormalised, survives user deletion), `role`, `ip` (inet, nullable), `user_agent` (truncated), `action` (dotted name, e.g. `auth.login`, `auth.login_failed`, `user.create`, `user.role_change`, `invite.create`, `session.kick`, `device.tune`, `device.lock`, `vfo.create`, `plugin.install`, `guest.policy_change`, `guest.session_start`), `target` (user, device, VFO, plugin ...), `outcome` (`ok|denied|error`), `details` (jsonb, small; never passwords, tokens, IQ or audio).

- Written by api; system events (plugin crashes, device errors) are published on NATS (`sh.v1.audit.*`) and persisted by the worker, so a restart of api loses nothing. The DB role `superhet_api` has `INSERT`/`SELECT` only on the table, never `UPDATE`/`DELETE`; retention pruning runs in the worker under its own role.
- High-frequency actions are coalesced: a tuning drag or VFO move is one event per burst (final value). Spectrum/audio frames are not logged; a `session.connect`/`session.disconnect` pair carries the duration.
- The client address follows ADR-0008: `X-Forwarded-For` is honoured only with `SUPERHET_TRUST_PROXY=true`.

Privacy (GDPR note; technical measures, not legal advice, the operator is the controller):
- Guests' IP addresses and user agents are personal data. Defaults: **retention 90 days for user/admin events, 7 days for guest events**, both configurable (`0` = keep, discouraged for guests).
- Setting `audit_ip_mode`: `full` (default for users), `truncated` (IPv4 /24, IPv6 /48; default for guests) or `none`. Truncation happens before the row is written. The rate limiter keeps working with the full address in memory only.
- Pruning is a worker job (like `SUPERHET_EVENT_RETENTION_DAYS` for decoder events); retention is stored in settings and editable on the admin page. Deleting a user pseudonymises `actor_name` but keeps the event.
- The guest page shows a privacy notice (what is stored, for how long); its text is admin-editable.

Admin console (`/admin`, role `admin` only; every endpoint checks the role server-side):
1. **Users and invites**: list, create, disable, delete, change role, reset password (revokes sessions), create/revoke invite links.
2. **Live sessions and connections**: who (user or guest), role, IP (in stored mode), since, frequency/mode of their VFOs, sockets; **kick** (revokes the session, closes sockets) and temporary address ban.
3. **Audit log**: newest first, filter by time range, actor, role, action, outcome, IP; paged; CSV and JSON export (the export is itself audited).
4. **Guest policy**: enable/disable, max guests, max VFOs per guest, allowed modes and frequency range, idle timeout, "only when no operator holds the lock", privacy text, audit retention and IP mode.
5. **Plugins**: installed plugins, state, claims, install/remove (P2+), later the plugin catalog.
6. **System**: devices and state, load (per-service CPU, dropped blocks, USB overruns), service health, version and update information (read-only check against GitHub Releases; no auto-update).

Contracts `docs/api-admin.md` and `docs/api-audit.md` are written with the implementation (P1.5).

## Consequences

- Volume stays small at default retention (a public guest produces one connect/disconnect pair plus a few actions); the admin page shows table size and oldest row.
- Append-only by DB grants protects against bugs and a compromised api, not against a compromised database superuser. This is an operator's audit trail, not tamper-proof evidence (no hash chain; can be added later).
- Logging guests' IPs has a legal cost; the defaults (truncated, 7 days) are deliberately conservative. Operators who need full addresses for abuse handling can raise them knowingly.
- Only the audit log and the admin page reveal other clients' addresses; listeners and guests never see them.
