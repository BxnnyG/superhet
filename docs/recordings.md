# Recordings and instant replay

Design: ADR-0028. Everything runs in `superhet-api`; the files live in `SUPERHET_RECORDINGS_DIR`
(default `/data/recordings`, a volume in `compose.yaml`).

## What it does

- **Audio recording** of a VFO: the daemon's Opus packets in an Ogg file (`.ogg`, plays in every
  browser and player), no re-encode. Optional *skip silence* keeps only the transmissions
  (100 ms pre-roll, 400 ms hang); the wall-clock time of every segment is in `meta.segments`.
- **IQ recording** as SigMF: `<id>.sigmf-data` + `<id>.sigmf-meta`. Source `window` is the whole
  device stream (device rate and centre, default `cu8`), `vfo` is the decimated baseband of one VFO
  (default 48 kHz `ci16_le`; `cu8`, `cs16`, `cf32` selectable). Defaults 300 s / 2048 MB, hard caps
  3600 s / 16384 MB. `core:sha512` is in the metadata. IQ travels over core NATS, which can drop
  data under load: `meta.completeness_percent` says how much of the wall time arrived. A retune
  during a window recording leaves a gap (blocks of another centre are not sent).
- **Instant replay**: the last `SUPERHET_REPLAY_SECONDS` (default 60) of every VFO are kept in
  memory. The Receiver tab plays the last 10 / 30 / 60 s, or saves them as a recording.
- **Library** (Recordings tab): play with seeking (HTTP Range), download (IQ: data and meta),
  rename, *Keep*, delete, stop a running recording.
- **Scanner**: *Record hits* in the Scanner tab (`record_hits` on `scanner/start`).

## Configuration

| variable | default | meaning |
|---|---|---|
| `SUPERHET_RECORDINGS_DIR` | `/data/recordings` | where files go (`audio/`, `iq/`) |
| `SUPERHET_RECORDINGS_MAX_GB` | 20 | quota; `0` = unlimited |
| `SUPERHET_REPLAY_SECONDS` | 60 | replay window, 5..600 |

Quota: before every start and every 30 s the oldest finished recordings that are not *kept* are
deleted until the total is below 90 % of the quota. If only kept ones remain a new recording is
refused (HTTP 507 `quota_full`) and running ones stop with `end_reason` `quota`.

## Access

| action | who |
|---|---|
| list, show, play, download, replay | accounts (no guests) that may view the device (ADR-0023); replay of a VFO: own and shared VFOs, operators all |
| start, stop, rename, keep, delete, save replay | operators and admins with *control* of the device |

## REST (`/api/v1`, session cookie, mutating calls need a same-origin `Origin`)

- `GET /recordings?dev=&kind=audio|iq&state=&keep=&q=&before=&limit=` ->
  `{"v":1,"recordings":[...],"next_before":id|null}`, newest first.
- `GET /recordings/config` -> `{available, replay_seconds, max_bytes, used_bytes, quota_exhausted,
  replay_buffers, hit_recording, limits}`.
- `GET /recordings/{id}` · `PATCH` `{"title"?,"keep"?}` · `DELETE` (409 `recording_active` while
  running) · `POST /recordings/{id}/stop`.
- `GET /recordings/{id}/file[?download=true][&part=meta]` (Range supported).
- `POST /devices/{dev}/recordings` `{"kind":"audio","vfo","gated"?,"title"?,"max_seconds"?,"max_mb"?}`
  or `{"kind":"iq","source":"window"|"vfo","vfo"?,"format"?,"rate"?,"max_seconds"?,"max_mb"?}`
  -> 201 with the recording.
- `GET /devices/{dev}/vfos/{id}/replay?seconds=&gated=true` -> `audio/ogg`; 404 `empty` until
  audio was buffered.
- `POST /devices/{dev}/vfos/{id}/replay/save` `{"seconds"?,"gated"?,"title"?}` -> 201.

A recording is `{id, kind, source: manual|replay|scanner, state: recording|done|interrupted|failed,
title, device, vfo, freq_hz, mode, owner (operators only), started_ms, stopped_ms, duration_ms,
size_bytes, keep, meta}`. Errors: `{"v":1,"ok":false,"code","error"}`; codes `quota_full` (507),
`too_many` (409), `empty` (404), `recording_active`, `not_running`, `device_read_only`,
`device_forbidden`, `invalid`.

## Limits

Plugin audio (DAB, M17, DSD) is not buffered or recorded. Full-band IQ at 2.4 MS/s generates about
5 MB/s: use a fast disk. For gap-free long captures use `rtl_sdr` on the host.
