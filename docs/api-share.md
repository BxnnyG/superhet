# Share links API

Public receiver pages for people without an account (ADR-0029). The page, the player and the API
below take the **token** as their only credential; it is part of the path, so no cookie is needed
and none is set.

## Pages

| Path | What |
| --- | --- |
| `/r/<token>` | Public receiver: station name, location and description, waterfall, play button with volume, listener count, decoder events if allowed, a frequency field if the link allows tuning. `frame-ancestors 'none'`. |
| `/embed/<token>` | Compact player for an iframe. `frame-ancestors` = the origins on the link (none by default: not embeddable); no `X-Frame-Options` only while the link is active and lists origins. Audio starts only after a click. |

Embed code (shown with the link when it is created): an `<iframe src=".../embed/<token>"
allow="autoplay">`. The browser needs the host page to delegate `autoplay`; playback still only
begins after the visitor clicks play.

## Public API (token in the path)

Errors are `{"error": code}`: `404 not_found` (unknown token), `410 expired` / `revoked`,
`403 feature_off` (the link does not allow this) / `device_restricted` (an admin hid the device
from guests, ADR-0023), `429 rate_limited` (`Retry-After`; also `too_many_from_address`),
`503 link_full` / `server_full` / `unavailable`.

- `GET /api/v1/share/{token}` - `{v, title, description, location, device, features[], tuning,
  listeners, max_listeners, expires_at_ms}`. `tuning` is `{mode:"follow"}` or `{mode:"fixed",
  freq_hz, demod, range:{min_hz,max_hz}|null}`. Nothing else about the link (no label, creator or
  embed origins).
- `GET /api/v1/share/{token}/state` (needs `waterfall`, `audio` or `plugin_audio`) - `{v,
  center_hz, sample_rate, vfos[{id,freq_hz,mode,bandwidth_hz}], plugin_streams[{id,plugin_id,
  audio_vfo}], listeners}`. `vfos` is the operator's on a following link (needs audio), empty on a
  fixed one.
- `GET /api/v1/share/{token}/ws/spectrum` (`waterfall`) - the spectrum socket of
  `docs/protocol-ws.md`, always for the link's device.
- `GET /api/v1/share/{token}/ws/audio` (`audio` or `plugin_audio`) - frames as in
  `docs/protocol-audio.md`. Extra server messages: `{"type":"hello","vfo":id|null,"freq_hz":..,
  "tune":{min_hz,max_hz}|null}` after the upgrade and `{"type":"tuned","freq_hz":..}`. Client
  messages: `subscribe` (refused with `forbidden` for anything the link does not allow, including
  unknown ids) and `{"type":"tune","freq_hz":N}` (fixed links with a range, `audio`, at most 10
  per second; otherwise an `error` message).
- `GET /api/v1/share/{token}/ws/events` (`decoder_events`) - the live decoder feed
  (`docs/api-decoder-events.md`).

Caps: `max_listeners` connections per kind and link, `SUPERHET_SHARE_MAX_LISTENERS` (default 100)
for all links, eight per client address. Link changes, revoking, rotating, deleting and expiry
close the open sockets (within five seconds for expiry).

## Admin API (admin role)

- `GET /api/v1/admin/share-links` - `{v, links[]}`; a link is `{id, token_hint, label, title,
  description, location, device, tuning:"follow"|"fixed", freq_hz, mode, tune_min_hz, tune_max_hz,
  features[], max_listeners, embed_origins[], expires_at_ms, revoked_at_ms, created_at_ms,
  created_by, status:"active"|"expired"|"revoked", listeners}`. Never the token.
- `POST /api/v1/admin/share-links` - body: `label` (required), `title`, `description`, `location`,
  `device`, `tuning`, `freq_hz`/`mode` (fixed), `tune_min_hz`/`tune_max_hz` (fixed, both or
  neither, the frequency inside), `features` (at least one), `max_listeners` (default 5, 1..1000),
  `embed_origins` (`https://host[:port]`, `https://*.host`), `expires_at_ms`. Reply `201 {v, link,
  token}`: **the token is shown only here.**
- `PUT /api/v1/admin/share-links/{id}` - the same body; open connections are closed.
- `POST /api/v1/admin/share-links/{id}/revoke`, `POST .../{id}/rotate` (reply includes the new
  `token`, also revives a revoked link), `DELETE /api/v1/admin/share-links/{id}`.

Audit events: `share.link_create|update|revoke|rotate|delete`, `share.tune`, `share.denied`,
plus `session.connect` / `session.disconnect` of share connections (see `docs/api-audit.md`).
