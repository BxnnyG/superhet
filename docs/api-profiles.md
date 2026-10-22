# Profiles (v1)

A profile is a named preset of a receiver: tuning, gain, VFOs and an optional tuning lock.
"Save as profile" snapshots the device, "load" applies a profile atomically. Background:
`docs/ARCHITECTURE.md` section 3.2; device semantics (lock, free tuning): `docs/api-devices.md`.

Profiles are stored in PostgreSQL (`superhet.profiles`), not on the device; any profile can be
loaded onto any device the settings fit (a file replay refuses other sample rates, gain, ...).

## Profile document (`data`)

```
{
  "v": 1,
  "center_hz": 433920000,
  "sample_rate": 2048000,
  "gain": {"mode": "manual", "db": 28.0},
  "ppm": 0,
  "bias_tee": false,
  "vfos": [
    {"freq_hz": 433970000, "mode": "nfm", "bandwidth_hz": 12500, "squelch_dbfs": -45.0, "deemphasis": "none"}
  ],
  "lock": false,
  "decoders": []
}
```

- `v` (required, 1), `center_hz` (1..1e12) and `sample_rate` (> 0) are required; the rest is
  optional with the defaults shown (`gain` absent/`null` = leave the gain alone; `vfos` empty).
  Unknown fields are rejected.
- `gain`: `{"mode":"auto"}` or `{"mode":"manual","db":-20..100}` (`db` required for manual,
  absent/`null` for auto). `ppm` -1000..1000.
- `vfos` (at most 8): `freq_hz` (1..1e12) and `mode` (`wfm`|`nfm`|`am`) required;
  `bandwidth_hz` (1000..1000000), `squelch_dbfs` (-200..0) and `deemphasis` (`none`|`50us`|`75us`,
  never with `am`) optional, `null`/absent = mode default / squelch off. Whether a VFO can be
  realised at the profile's sample rate is checked when loading.
- `lock`: take the user tuning lock after loading. Loading never releases a lock.
- `decoders`: reserved for decoder instances (P2); at most 32 objects, stored and returned
  untouched, ignored by the daemon.
- "Save" stores effective values (the VFO's actual bandwidth and de-emphasis), so a profile keeps
  its meaning if mode defaults change. The API stores documents normalised (defaults filled in).

## REST (superhet-api, port 8073)

All routes require a session cookie (`docs/api-auth.md`). Mutating routes check `Origin` like the
device routes (`SUPERHET_ALLOWED_ORIGINS`), bodies are at most 16 KiB, `Content-Type:
application/json` is required where a body is sent. Errors have the shape
`{"v":1,"ok":false,"code":"...","error":"..."}`; without a database every route answers `503`
`unavailable`.

Profile resource: `{"id":"<uuid>","name":"...","data":{...},"created_at":"<RFC 3339 UTC>","updated_at":"..."}`.
`name` is trimmed, 1..100 characters, no control characters, unique ignoring case.

| Route | Body | Success | Errors |
|---|---|---|---|
| `GET /api/v1/profiles` | - | `200 {"v":1,"profiles":[resource...]}` ordered by name | |
| `POST /api/v1/profiles` | `{"name","data"}` | `201 {"v":1,"profile":resource}` + `Location` | `400 invalid`, `409 conflict` (name taken) |
| `GET /api/v1/profiles/{id}` | - | `200 {"v":1,"profile":resource}` | `404 not_found` (also for a malformed id) |
| `PUT /api/v1/profiles/{id}` | `{"name","data"}` (both required) | `200 {"v":1,"profile":resource}` | `400`, `404`, `409` |
| `DELETE /api/v1/profiles/{id}` | - | `204` | `404` |
| `POST /api/v1/devices/{dev}/profile/save` | `{"name","overwrite"?}` | `201 {"v":1,"profile":resource}` + `Location`; with `overwrite` replacing an existing profile `200` | `400`, `409`, `502/503/504` (device) |
| `POST /api/v1/devices/{dev}/profile/load` | `{"id"}` | `200 {"v":1,"ok":true,"state":{...}}` (the new device state) | see below |

- `save` asks the daemon for its current state (not the KV copy) and stores it, including
  `lock:true` if the user lock is held.
- `save` answers `409 conflict` when the name is taken (case-insensitive). With
  `"overwrite": true` (default `false`) it instead replaces the profile of that name with the
  fresh snapshot: id and `created_at` are kept, `updated_at` advances, the name takes the
  requested spelling, and the reply is `200`. If no profile has the name yet, `overwrite` creates
  one (`201` + `Location`).
- `load` errors are the daemon's, passed through (shape of `docs/api-devices.md`): `400`
  `invalid`/`unsupported`, `409` `locked` (with `holders`, when the profile would change centre or
  rate while locked), `404` `not_found` (unknown profile), `502`, `503`, `504`. On any error the
  device is unchanged.
- Loading *replaces* the device state, it never appends: every existing VFO is deleted, the
  profile's VFOs are created (a profile without VFOs leaves none), and centre, rate, gain, ppm
  and bias tee are applied, all in one daemon operation under the tuning gate. Clients see a
  single consistent state push (`/api/v1/ws/state`, `docs/protocol-ws.md`), never a half-loaded
  state. VFO ids are not preserved: the profile's VFOs get fresh ids (continuing after the
  highest id ever used), so clients must re-read ids from the reply or the state feed. Loading
  the same profile twice yields the same state (apart from the ids and `tuning_epoch`).
- The daemon's default VFO (`docs/api-devices.md`, "Default VFO") is not part of this: a profile
  decides the VFO set completely.

## Bus

`sh.v1.dev.<dev>.ctl.profile.load` (request/reply), body `{"v":1,"profile":<document>}`, reply
`{"v":1,"ok":true,"state":{...}}` or the usual `{"ok":false,"code","error"}` (`holders` for
`locked`). See `docs/api-devices.md` for the semantics. The document type is shared as
`superhet_bus::profile::ProfileData`.
