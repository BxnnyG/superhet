# Notifications API

Design: ADR-0027, user guide: `notifications.md`. Errors use `{"error":"<code>","message":"..."}`;
state-changing calls need an allowed `Origin` and `Content-Type: application/json`.

## Admin (role `admin`)

| Method and path | Purpose |
| --- | --- |
| `GET /api/v1/admin/notification-targets` | `{"targets":[Target]}` |
| `POST /api/v1/admin/notification-targets` | Create; body `{name, kind, config, secret?}`; 201 `{"target"}` |
| `PUT /api/v1/admin/notification-targets/{id}` | Replace name and config; `secret` absent keeps, `null` removes, text replaces; webhook `config.url` absent keeps the URL |
| `DELETE /api/v1/admin/notification-targets/{id}` | 204 |
| `POST /api/v1/admin/notification-targets/{id}/test` | Queue a test message; 202 `{"delivery_ids":[...]}` |
| `GET /api/v1/admin/notification-rules` | `{"rules":[Rule]}` |
| `POST /api/v1/admin/notification-rules` | Create; 201 `{"rule"}` |
| `PUT /api/v1/admin/notification-rules/{id}` | Replace |
| `DELETE /api/v1/admin/notification-rules/{id}` | 204 |
| `POST /api/v1/admin/notification-rules/{id}/test` | Queue the rule's message (templates over a sample event) to its targets; 409 `no_targets` |
| `GET /api/v1/admin/notification-deliveries` | `?limit=1..200&before=<id>&status=pending\|sent\|failed` -> `{"deliveries":[...],"next_before"}` |

Limits: 50 targets, 200 rules, 20 targets per rule. 409 `name_taken` for duplicate names.

**Target**: `{id, name, kind: "webhook"|"email"|"push", config, has_secret, created_at_ms,
updated_at_ms}`. The secret is never returned; a webhook's `config` has `url_host`
(`https://host[:port]`) instead of `url`.

- webhook config: `url`, `method` (`POST`), `content_type`, `body_template`, `headers`
  (`[{name,value}]`, max 8), `timeout_secs` (1-30, 10), `retries` (0-8, 4), `allow_private`,
  `sign` (needs a secret).
- email config: `to` (1-5 addresses), `retries`.
- push config: `user_ids` (empty = all subscribed accounts), `ttl_secs`, `urgency`, `retries`.

**Rule**: `{id, name, enabled, source, kind, conditions:[{field, op, value?, case_sensitive?}],
cooldown_secs, max_per_hour, quiet:{start:"HH:MM", end, tz}|null, title_template, body_template,
target_ids, created_at_ms, updated_at_ms}`. `op` is one of `equals`, `not_equals`, `contains`,
`not_contains`, `regex`, `gt`, `gte`, `lt`, `lte`, `exists`; `equals` and `contains` accept a
list of alternatives. At most 16 conditions.

**Delivery**: `{id, rule_id, rule_name, target_id, target_name, event_id, status, attempts,
last_error, title, body, created_at_ms, finished_at_ms}`.

Audit actions: `notification.target.create|update|delete`, `notification.rule.create|update|delete`,
`notification.test`, `push.subscribe`, `push.unsubscribe`. Details hold ids and flags, never
secrets or URLs.

## Own browser (any account, no guests)

| Method and path | Purpose |
| --- | --- |
| `GET /api/v1/me/push` | `{"public_key": "<VAPID applicationServerKey, base64url>", "subscriptions":[{id,label,created_at_ms}]}` |
| `POST /api/v1/me/push/subscriptions` | Body `{endpoint, keys:{p256dh, auth}, label?}` (a `PushSubscription` as JSON); 201 `{"subscription":{id,...}}`; 409 `too_many` (10 per account), `endpoint_in_use` |
| `DELETE /api/v1/me/push/subscriptions/{id}` | 204; only the owner |

The endpoint must be an https URL outside private ranges. Endpoints are not echoed back.
