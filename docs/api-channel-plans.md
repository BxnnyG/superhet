# Custom channel plans

Channel plans list fixed-channel services (PMR446, marine VHF, CB, ...). The built-in plans ship
with the web UI (`web/src/channels/plans/*.json`, see `docs/channel-plans.md`) and need no API.
This API only stores the plans operators **import** (a plan JSON or a CHIRP CSV converted by the
browser). Custom plans are shared: every signed-in user and guest reads them, operators and admins
edit them (same model as bookmarks). Stored in PostgreSQL (`superhet.channel_plans`, migration
`0010`).

All routes are under `/api/v1`, behind the session check. Mutations check the `Origin` header like
the other write routes. Errors are `{"v":1,"ok":false,"code":"...","error":"..."}`.

| Route | Role | Body | Success |
| --- | --- | --- | --- |
| `GET /channel-plans` | listener, guest | - | `200 {"v":1,"plans":[plan, ...]}` sorted by name |
| `POST /channel-plans` | operator | plan document | `201` plan, `Location: /api/v1/channel-plans/{id}` |
| `PUT /channel-plans/{id}` | operator | plan document | `200` plan (replaces name and document) |
| `DELETE /channel-plans/{id}` | operator | - | `204` |

A stored plan:

```
{ "id": "0f8fad5b-...", "name": "Club", "doc": { ...plan document... },
  "created_ms": 1767225600000, "updated_ms": 1767225600000 }
```

Errors: `400 invalid` (structure, see below), `404 not_found`, `409 limit` (at most 100 plans),
`413` (body above 512 KiB), `503 unavailable` (no database).

## Plan document (version 1)

```
{
  "v": 1,
  "name": "Club channels",            // 1-100 characters, required
  "service": "Club", "chip": "Club",  // card label, readout chip prefix ("Club 3")
  "aliases": ["club"],                // palette words: "club 3"
  "regions": ["DE", "EU"],            // ISO 3166-1 alpha-2, EU, CEPT, ITU-R1..3, WORLD
  "licence": "licence-free" | "licensed" | "receive-only",
  "licence_note": "...",
  "mode": "nfm", "bandwidth_hz": 12500, "deemphasis": "50us", "step_hz": 12500,
  "note": "...", "source": "...",
  "channels": [
    { "number": "1", "name": "Call", "freq_hz": 145500000,
      "ctcss_hz": 88.5, "mode": "nfm", "bandwidth_hz": 12500, "note": "..." }
  ],
  "rasters": [
    { "id": "25k", "label": "25 kHz", "start_hz": 118000000, "end_hz": 136975000,
      "step_hz": 25000, "naming": "plain" | "icao-8.33" }
  ]
}
```

`mode` is a VFO mode (`wfm nfm am usb lsb cw`) or `dab` (a block: selecting it moves the device
centre). The API checks what can hurt the database or the list: `v` is 1, `name` is 1-100
characters, `channels` has at most 5000 entries with an integer `freq_hz` of 1 to 10^10 Hz, at most
8 rasters, and at least one channel or raster exists. The web UI validates the rest (modes,
regions, CTCSS tones, duplicate channel numbers) with the same rules when importing and skips a
stored document it cannot read, counting it in the Channels tab. The API stores the document as
sent, with the trimmed name written back.

`deemphasis` is informational for now; the VFO has no de-emphasis setting yet.
