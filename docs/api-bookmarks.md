# Bookmarks (v2)

A bookmark is a saved channel ("memory channel") holding the full receiver state needed to
listen to it: frequency, mode, bandwidth, squelch, step, CTCSS/DCS sub-tone and, for digital
channels, protocol parameters (TETRA/DMR timeslot, talkgroup, ...). Bookmarks are point
entries; band plan ranges (start/end, category) are a different thing and never appear here.
Bookmarks are stored in PostgreSQL (`superhet.bookmarks`), independent of any device; the client
decides what a click does (tune the selected VFO and apply the stored fields, alt-click creates
a new VFO, ...). They can be put into ordered groups (folders, `superhet.bookmark_groups`).
Related: profiles (`docs/api-profiles.md`) preset a whole receiver, bookmarks are single channels.

v2 is additive: every v1 request and response stays valid, the new fields are optional and
default to `null` / `false`. Migration `0005` leaves existing bookmarks untouched.

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
  "squelch_db": -80,
  "squelch_auto": false,
  "step_hz": 100000,
  "tone_mode": "ctcss",
  "ctcss_hz": 88.5,
  "dcs_code": null,
  "dcs_polarity": null,
  "digital": null,
  "group_id": null,
  "favourite": false,
  "scan": {"include": false, "priority": null, "dwell_ms": null},
  "created_ms": 1767225600000,
  "updated_ms": 1767225600000
}
```

- `name`: trimmed, 1..100 characters, no control characters. Not unique.
- `freq_hz`: integer, 1..10 000 000 000.
- `mode`: `wfm`, `nfm`, `am`, `usb`, `lsb`, `cw` (lower case) or `null`. The spelling is that of
  the VFO API (`docs/api-devices.md`), which realises all six modes.
- `bandwidth_hz`: integer 1..10 000 000 (for `usb`/`lsb` the passband width above the 300 Hz low cut) or `null` (= mode default).
- `tags`: at most 16 strings, each trimmed, 1..32 characters, no control characters. Duplicates
  are dropped (first occurrence wins), order is kept.
- `color`: `#rrggbb` (any case accepted, stored lower case) or `null`.
- `notes`: at most 1000 characters (newlines and tabs allowed) or `null`.
- `squelch_db`: integer -120..0 (dBFS) or `null` (leave the squelch as it is). `squelch_auto`:
  boolean, the squelch is meant to be set automatically (the client measures the noise floor);
  `squelch_db` then holds the last measured value.
- `step_hz`: tuning step, integer 1..10 000 000, or `null` (mode default).
- `tone_mode`: `none`, `ctcss`, `dcs` or `null` (unspecified). Only the fields of the active
  mode are kept: `ctcss` requires `ctcss_hz` (30..300, rounded to 0.1 Hz; the VFO applies it as
  tone squelch when it is a standard tone), `dcs` requires `dcs_code` (integer 1..777 whose
  digits are octal, e.g. `23` for code 023) and takes `dcs_polarity` `N` (default) or `R`. DCS
  is stored only; the receiver has no DCS detection yet.
- `digital`: `null` or an object `{protocol, timeslot, talkgroup, colour_code, mcc, mnc,
  decoder_plugin_id}`. `protocol` is `tetra`, `dmr`, `dpmr` or `p25` (required); `timeslot`
  1..4 for `tetra`, 1..2 for `dmr`, not allowed otherwise; `talkgroup` 0..16 777 215 (GSSI,
  talkgroup); `colour_code` 0..15 (`dmr` only); `mcc` 0..999; `mnc` 0..9999;
  `decoder_plugin_id` 1..64 of `A-Za-z0-9._-`. Other members default to `null`.
- `group_id`: id of a bookmark group or `null` (ungrouped).
- `favourite`: boolean. `scan`: `{include, priority, dwell_ms}` for scan lists, `priority`
  0..9, `dwell_ms` 100..60 000, both may be `null`; a patch replaces the whole object.
- `created_ms`, `updated_ms`: Unix time in milliseconds; set by the server.
- At most 10 000 bookmarks in total.

## REST (superhet-api, port 8073)

All routes require a session cookie (`docs/api-auth.md`). Mutating routes check `Origin` like the
device routes (`SUPERHET_ALLOWED_ORIGINS`) and require `Content-Type: application/json` where a
body is sent. Bodies are at most 16 KiB, except `bulk` (8 MiB); larger ones get `413`. Errors
have the shape `{"v":1,"ok":false,"code":"...","error":"..."}`; without a database every route
answers `503` `unavailable`.

| Route | Body | Success | Errors |
|---|---|---|---|
| `GET /api/v1/bookmarks` | - | `200 {"v":1,"bookmarks":[resource...]}` sorted by `freq_hz` (then name) | |
| `POST /api/v1/bookmarks` | `name`, `freq_hz` required; all other fields optional | `201` resource + `Location` | `400 invalid` (also an unknown `group_id`), `409 limit` |
| `PATCH /api/v1/bookmarks/{id}` | any subset of the create fields | `200` resource | `400 invalid`, `404 not_found` |
| `DELETE /api/v1/bookmarks/{id}` | - | `204` | `404 not_found` |
| `POST /api/v1/bookmarks/bulk` | `{"bookmarks":[create bodies...],"replace":false}` | `200 {"v":1,"created":n}` | `400 invalid` (also an unknown `group_id`), `409 limit`, `413` |

- Create bodies reject unknown fields. `id`, `created_ms` and `updated_ms` are accepted and
  ignored, so the list response can be fed back into `bulk` as an export/import round trip; new
  ids and timestamps are assigned.
- `PATCH`: absent fields stay as they are. `null` clears every optional field
  (`mode`, `bandwidth_hz`, `color`, `notes`, `squelch_db`, `step_hz`, `tone_mode`, `digital`,
  `group_id`, ...); `name`, `freq_hz`, `tags`, `squelch_auto`, `favourite` cannot be `null`
  (clear tags with `[]`). A nested `digital` or `scan` object replaces the whole one. `id`,
  `created_ms` and `updated_ms` are read-only (`400`). The merged
  result is validated as a whole; on `400` nothing changes. `updated_ms` advances on every
  successful `PATCH`.
- `bulk` runs in one transaction. Any invalid item rejects the whole request (`400`, the message
  names the index), a request that would exceed 10 000 bookmarks in total is `409 limit`, and in
  both cases nothing is changed. With `"replace": true` all existing bookmarks are deleted first
  (so only the request counts towards the limit); the default `false` appends.
- A malformed `{id}` is `404 not_found`, like an unknown one.

## Groups

A group is an ordered folder: `{"id","name","position"}`. `name` is trimmed, 1..60 characters, no
control characters, unique ignoring case. At most 200 groups. Deleting a group keeps its
bookmarks (they become ungrouped).

| Route | Body | Success | Errors |
|---|---|---|---|
| `GET /api/v1/bookmark-groups` | - | `200 {"v":1,"groups":[...],"max":200}` in display order | |
| `POST /api/v1/bookmark-groups` | `{"name"}` | `201` group + `Location` (appended at the end) | `400 invalid`, `409 duplicate`, `409 limit` |
| `PATCH /api/v1/bookmark-groups/{id}` | `{"name"}` | `200` group | `400`, `404 not_found`, `409 duplicate` |
| `PUT /api/v1/bookmark-groups/order` | `{"ids":[...]}` | `200` like the list | `400 invalid` (unknown id) |
| `DELETE /api/v1/bookmark-groups/{id}` | - | `204` | `404 not_found` |

`order` places the listed groups first in that order; groups not listed keep their relative
order after them. Bulk imports that reference `group_id` need the groups to exist first (the
web client creates missing groups by name before importing).
