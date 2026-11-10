# ADR-0027: Notifications: rules, targets, outbox delivery

**Status:** accepted (2026-11-10)

## Context

Decoders produce events (pager messages, callsigns, talkgroup activity, aircraft). Nobody watches
the decoder list all day; people want a message when a specific RIC, word, callsign, talkgroup or
ICAO shows up. The events are already in PostgreSQL (`decoder_events`) and announced on
`sh.v1.events.core.decoder.<dev>` inside the durable JetStream stream `EVENTS` (ADR-0007). What is
missing is matching, delivery and the guard rails: a busy channel must not flood a phone, a
webhook URL is an SSRF primitive, secrets must not leak, and a restart or a dead webhook must not
lose or duplicate alerts silently.

## Decision

1. **Rules and targets live in PostgreSQL** (migration `0017_notifications.sql`) and are managed
   by admins only (`/api/v1/admin/notification-*`, `docs/api-notifications.md`). A rule has a
   source and event type filter, field conditions (equals / contains / regex / numeric compare /
   exists over the event and its decoder payload `data.*`), a cooldown, an hourly cap, quiet
   hours in an IANA zone, message templates and a set of targets.
2. **Evaluation happens in the worker.** A durable JetStream consumer (`worker-notifications`,
   `DeliverPolicy::New`, filter `sh.v1.events.core.decoder.>`) reads the announcements the
   recorder publishes after storing a row, so core detectors and every plugin are covered by one
   path, each event with its row id. Rules are re-read every 5 s. Events older than an hour are
   skipped so an outage does not replay stale alerts.
3. **At-least-once with dedupe by event id through an outbox.** A matching rule inserts one
   `pending` row per target into `notification_deliveries` with a unique index on
   (rule, target, event id); the stream message is acknowledged after that insert. A redelivery
   inserts nothing. A second loop claims due rows (`FOR UPDATE SKIP LOCKED`, leased), sends them
   and records `sent`, a retry time (15 s, 1 min, 4 min, 15 min, 30 min, then hourly) or `failed`
   with the last error. Retries live in the row, so they survive restarts. The table is the
   delivery log shown in the UI; finished rows are pruned with the event retention.
4. **Cooldown and rate limit are in memory** (per rule, in the worker). A restart forgets them, so
   the first match after it fires; that errs towards notifying. Suppressed matches are counted in
   a metric, not logged per event.
5. **Targets.** Webhook (JSON POST by default, templated body and headers so ntfy and Gotify are
   presets rather than code, optional HMAC-SHA256 signature over `<timestamp>.<body>`, timeout,
   retries), e-mail (SMTP from `SUPERHET_SMTP_*`, STARTTLS/TLS, `lettre`), Web Push (RFC 8030 +
   VAPID RFC 8292 + `aes128gcm` RFC 8291 on `ring`; the server generates and stores its key pair,
   each account stores its own browser subscription, the permission prompt appears only after a
   click). Test sends go through the same outbox and show up in the log.
6. **SSRF guard.** Every outbound HTTP request goes through one client whose DNS resolver drops
   blocked addresses (so the checked addresses are the connected ones), IP literals are checked
   up front, redirects are followed by hand (307/308 only, at most 3) with the same checks, and
   no system proxy is used. Loopback, RFC 1918, link-local, ULA and CGNAT are blocked unless the
   admin enables "allow private addresses" on that target. Unspecified, multicast, broadcast,
   reserved ranges and the cloud metadata addresses are blocked always.
7. **Templates have no logic.** `{{path}}` placeholders only; unknown paths render empty; each
   value is capped at 500 characters, stripped of control characters and escaped for its
   destination (plain text, JSON string, header value), so event text cannot inject headers or
   break a JSON body. Regex conditions use the linear-time `regex` crate with pattern, compiled
   size and haystack limits.
8. **Secrets are write-only.** The webhook secret, the webhook URL (often a token), the SMTP
   password (environment only) and the VAPID private key are never returned by the API, never
   written to the audit log and never part of an error text stored in the delivery log; `Debug`
   output of the types that hold them is redacted. The database holds the webhook secret and the
   VAPID key in plain columns (there is no server master key to encrypt them with; protect the
   database like the rest of the stack).
9. **The worker needs outbound network access** (compose: it joins the `edge` network next to
   the api).

## Consequences

- One matching path for all event sources; a new plugin's events are matchable the day it ships.
- Alerts can be late (retries, the 2 s queue poll) or, if NATS drops an announcement before the
  stream stores it, missing; the history table still has the event. They are not duplicated.
- Event text is sent to third parties by design (webhook receivers, push services; push payloads
  are encrypted end to end to the browser). Pager text can be sensitive; the admin console and
  docs say so, and rules are admin-only.
- Cooldowns reset on a worker restart.
- Web Push needs a secure context (HTTPS or localhost) for the service worker.
