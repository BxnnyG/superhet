# Notifications

Rules that send a webhook, an e-mail or a browser push when a decoder event matches (ADR-0026).
Admins manage rules and targets under **Admin console > Notifications**; every account can turn
on push notifications for its own browser (account menu > Notifications). REST contract:
`api-notifications.md`.

Pager text and call details can be private communications. A webhook, mail or push service
receives what a rule's template contains. Only send where you are allowed to, and see the legal
notes in `plugins-decoders.md`.

## How it works

The worker reads every decoder event after it is stored (core detectors and plugins alike),
checks the enabled rules and queues one delivery per target of each rule that fires. A sender
loop works the queue off with retries; the **Delivery log** shows status, attempts and the last
error. Rules are re-read every 5 seconds. Events older than an hour are not notified about.

## Rules

- **Source / event type**: a plugin id (`org.superhet.decoder.pocsag`) or `core`, and an event type
  (`pocsag.message`, `dtmf`). Empty = any.
- **Conditions** (all must hold) over the event row and the decoder payload:
  `value` (one-line summary), `kind`, `source`, `device`, `vfo`, `freq_hz`, `mode`, and `data.*`
  (the plugin's own fields, for example `data.address` (POCSAG RIC), `data.text`,
  `data.callsign`, `data.icao`, `data.talkgroup`). Operators: equals, not equals, contains, not
  contains (text, ignoring case unless "match case"; a list matches any of), regex, greater /
  less (numbers), is present. A missing field satisfies only the negative operators.
- **Regex** uses a linear-time engine (no catastrophic backtracking); patterns are limited to 256
  characters and a compiled size, and only the first 4096 bytes of a field are searched.
- **Cooldown**: minimum seconds between two notifications of the rule. **Hourly cap**: at most
  N per sliding hour. **Quiet hours**: no notifications between two local times in an IANA zone
  (may wrap past midnight). Cooldown and cap are kept in the worker's memory and reset on restart.
- **Templates** for title and message: `{{path}}` placeholders only, e.g.
  `{{rule}}: RIC {{data.address}}` / `{{data.text}}`. Extra values: `rule`, `time` (UTC),
  `freq_mhz`, `where` (device, frequency, time). Unknown names render empty. No logic.
  Defaults: title `{{rule}}`, message `{{kind}}: {{value}}` and `{{where}}`.

## Targets

- **Webhook**: JSON POST by default:
  `{"v":1,"delivery":<id>,"rule":{"id","name"},"title","body","event":{...history row...}}`.
  Options: method (POST/PUT), content type, body template (replaces the JSON document; values are
  escaped for JSON when the content type contains `json`), extra headers (templates; `{{secret}}`
  is available in header values), timeout, retries, **sign**. With signing on, requests carry
  `X-Superhet-Timestamp` (Unix seconds) and `X-Superhet-Signature: sha256=<hex>`, the
  HMAC-SHA256 of `<timestamp>.<body>` under the target's secret; receivers should recompute it
  and reject old timestamps. `X-Superhet-Delivery` is the delivery id, stable across retries.
  Presets: ntfy (`Title` header, bearer token from the secret, plain body), Gotify (JSON body,
  `X-Gotify-Key` from the secret; or put `?token=` in the URL).
  The URL and secret are write-only: after saving, the console shows the host only.
- **E-mail**: recipients (up to 5). The server is configured on the worker:
  `SUPERHET_SMTP_HOST`, `_PORT` (default 587 / 465 / 25 by mode), `_USER`, `_PASSWORD`,
  `_FROM` (`Superhet <alerts@example.com>`), `_TLS` (`starttls` default, `tls`, `none`; no
  fallback to plain text unless `none`).
- **Browser push**: goes to every account that turned it on. The server generates a VAPID key
  pair on first use and stores it in the database; browsers get the public key. The permission
  prompt appears only after the user presses the button. Payloads are encrypted to the browser.
  `SUPERHET_VAPID_SUBJECT` (a `mailto:` or `https:` URL, default `SUPERHET_PUBLIC_URL`) is the
  contact the push services see. Needs HTTPS (or localhost) for the service worker `/sw.js`.
  Expired subscriptions (HTTP 404/410) are removed automatically.

## Outbound requests and the SSRF guard

All webhook and push requests go through one guarded client. Loopback, private (RFC 1918),
link-local, unique-local and CGNAT addresses are refused for hostnames and IP literals, also
after redirects (only 307/308 are followed, at most 3). To reach a service on your own network
(a local ntfy or Gotify) enable **Allow private network addresses** on that target. Unspecified,
multicast, reserved ranges and the cloud metadata addresses (169.254.169.254 and friends) stay
blocked. The worker joins the `edge` network in `compose.yaml` for outbound access.

## Retries and the log

A failed send is retried after 15 s, 1 min, 4 min, 15 min, 30 min, then hourly, up to the
target's retry count; client errors (HTTP 4xx other than 408/425/429), blocked addresses and
configuration problems fail at once. Log entries are pruned with `SUPERHET_EVENT_RETENTION_DAYS`
(365 days if that is 0). "Send test" on a rule or target queues a message through the same path.

## Metrics

Worker `/metrics`: `superhet_worker_notifications_sent_total`, `..._failed_total`,
`..._suppressed_total` (quiet hours, cooldown, cap).
