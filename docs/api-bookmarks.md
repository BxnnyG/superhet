# Bookmarks (v1)

A bookmark is a saved frequency ("memory channel"): name, frequency and optionally mode,
bandwidth, tags, colour and notes. Bookmarks are stored in PostgreSQL (`superhet.bookmarks`),
independent of any device; the client decides what a click does (tune, create a VFO, ...).
Related: profiles (`docs/api-profiles.md`) preset a whole receiver, bookmarks are single channels.

## Resource

```
{
  "id": "0f8fad5b-d9cb-469f-a165-70867728950e",
  "name": "Radio Example",
  "freq_hz": 100300000,
  "mode": "wfm",
  "bandwidth_hz": 180000,
  "tags": ["fm", "music"],
  "color": "#aabbcc",
  "notes": null,
  "created_ms": 1767225600000,
  "updated_ms": 1767225600000
}
```

- `name`: trimmed, 1..100 characters, no control characters. Not unique.
- `freq_hz`: integer, 1..10 000 000 000.
- `mode`: `wfm`, `nfm`, `am`, `usb`, `lsb`, `cw` (lower case) or `null`. The spelling is that of
  the VFO API (`docs/api-devices.md`), which currently realises only `wfm|nfm|am`; `usb|lsb|cw`
  are storable ahead of their demodulators.
- `bandwidth_hz`: integer 1..10 000 000 or `null` (= mode default).
- `tags`: at most 16 strings, each trimmed, 1..32 characters, no control characters. Duplicates
  are dropped (first occurrence wins), order is kept.
- `color`: `#rrggbb` (any case accepted, stored lower case) or `null`.
- `notes`: at most 1000 characters (newlines and tabs allowed) or `null`.
- `created_ms`, `updated_ms`: Unix time in milliseconds; set by the server.
- At most 10 000 bookmarks in total.

## REST (superhet-api, port 8073)

All routes require a session cookie (`docs/api-auth.md`). Mutating routes check `Origin` like the
device routes (`SUPERHET_ALLOWED_ORIGINS`) and require `Content-Type: application/json` where a
body is sent. Bodies are at most 16 KiB, except `bulk` (2 MiB); larger ones get `413`. Errors
have the shape `{"v":1,"ok":false,"code":"...","error":"..."}`; without a database every route
answers `503` `unavailable`.

| Route | Body | Success | Errors |
|---|---|---|---|
| `GET /api/v1/bookmarks` | - | `200 {"v":1,"bookmarks":[resource...]}` sorted by `freq_hz` (then name) | |
| `POST /api/v1/bookmarks` | `name`, `freq_hz` required; `mode`, `bandwidth_hz`, `tags`, `color`, `notes` optional | `201` resource + `Location` | `400 invalid`, `409 limit` |
| `PATCH /api/v1/bookmarks/{id}` | any subset of the create fields | `200` resource | `400 invalid`, `404 not_found` |
| `DELETE /api/v1/bookmarks/{id}` | - | `204` | `404 not_found` |
| `POST /api/v1/bookmarks/bulk` | `{"bookmarks":[create bodies...],"replace":false}` | `200 {"v":1,"created":n}` | `400 invalid`, `409 limit`, `413` |

- Create bodies reject unknown fields. `id`, `created_ms` and `updated_ms` are accepted and
  ignored, so the list response can be fed back into `bulk` as an export/import round trip; new
  ids and timestamps are assigned.
- `PATCH`: absent fields stay as they are. `null` clears `mode`, `bandwidth_hz`, `color` and
  `notes`; `name`, `freq_hz` and `tags` cannot be `null` (clear tags with `[]`). The merged
  result is validated as a whole; on `400` nothing changes. `updated_ms` advances on every
  successful `PATCH`.
- `bulk` runs in one transaction. Any invalid item rejects the whole request (`400`, the message
  names the index), a request that would exceed 10 000 bookmarks in total is `409 limit`, and in
  both cases nothing is changed. With `"replace": true` all existing bookmarks are deleted first
  (so only the request counts towards the limit); the default `false` appends.
- A malformed `{id}` is `404 not_found`, like an unknown one.
