# Decoder event history

A persistent log of what the decoders heard: DTMF bursts, CTCSS tone changes and RDS station
name / RadioText / PI changes, each with time, device, VFO, tuned frequency and mode. Stored in
PostgreSQL (table `superhet.decoder_events`, migration 0006), so it survives restarts and can be
searched, filtered and exported. DCS is part of the schema (`dcs`) but nothing detects it yet.

## Write path

`superhet-worker` subscribes to the live subjects of every device (`sh.v1.dev.*.tones.>`,
`sh.v1.dev.*.rds.>`, `sh.v1.dev.*.state`; tuning of a VFO also from the `STATE` KV bucket), turns the
periodic state messages into discrete events and inserts them. Every stored event is announced as
JSON on `sh.v1.events.core.decoder.<dev>`; the API forwards it on `/api/v1/ws/events`.
The worker needs `SUPERHET_DATABASE_URL` and `SUPERHET_NATS_URL`; without either it runs idle and
only serves `/healthz`.

NATS note: the `worker` user needs subscribe rights on `sh.v1.dev.*.state`, `sh.v1.dev.*.tones.>`
and `sh.v1.dev.*.rds.>` (see `deploy/nats/nats.conf`). If you keep your own copy of `nats.conf`,
add them, otherwise the worker records nothing.

### Rules

- **DTMF**: digits arriving within 1.5 s of each other are one event whose `value` is the digit
  sequence (`0-9 A-D * #`); `ts_ms` is the first digit. A burst also ends when the VFO is retuned or
  removed, or after 64 digits. `meta`: `{"digits":n,"duration_ms":ms}`.
- **CTCSS**: a tone has to be present 0.8 s before it is reported, and gone 1.5 s before it is
  reported as ended; shorter dropouts are bridged. Two events per tone, `value` `"88.5 Hz"`,
  `meta`: `{"hz":88.5,"edge":"start"}` and `{"hz":88.5,"edge":"stop","duration_ms":ms}` (`ts_ms` of
  the stop is the moment the tone vanished). A different tone ends the old one and starts the new.
- **RDS**: `rds_ps` (station name, trimmed), `rds_rt` (RadioText, trimmed), `rds_pi` (four hex
  digits) when the value changes; `meta`: `{"pi":"D220","pty":10}`. A cleared state (retune, signal
  gone) forgets the last values, so tuning back to a station logs it again.
- Retuning a VFO closes its open bursts and tones with the old frequency.

## Retention

`SUPERHET_EVENT_RETENTION_DAYS` (worker, default 30, `0` keeps everything) – pruned at start and
hourly. Independent of the age, only the newest 2 000 000 rows are kept.

## REST

All routes need a session (like the other `/api/v1` routes); `DELETE` also checks the Origin.

`GET /api/v1/decoder-events` – newest first. Query parameters (all optional):
`kind` (comma list of `dtmf,ctcss,dcs,rds_ps,rds_rt,rds_pi`), `dev`, `vfo`, `from` (Unix ms,
inclusive), `to` (Unix ms, exclusive), `q` (case-insensitive substring of `value`, max 100
characters), `limit` (1..=1000, default 100), `cursor` (the previous `next_cursor`).
Unknown parameters and invalid values are `400 {"code":"invalid"}`.

Response: `{"v":1,"events":[Event...],"next_cursor":null|"<ts_ms>.<id>"}`; `next_cursor` is `null`
on the last page. Event: `{"id","ts_ms","device","vfo","freq_hz":null|int,"mode":null|string,
"kind","value","meta":{}}`.

`GET /api/v1/decoder-events/export.csv` – same filters (not `limit` / `cursor`), up to 100 000
rows, `text/csv`, attachment `superhet-decoder-events.csv`. Columns:
`time,ts_ms,device,vfo,freq_hz,mode,kind,value,meta`; `time` is ISO 8601 UTC. Fields starting with
`= + - @` get a leading `'` so over-the-air RadioText cannot become a spreadsheet formula.

`DELETE /api/v1/decoder-events` – removes the whole history: `200 {"v":1,"deleted":n}`.

Without a database every route answers `503 {"code":"unavailable"}`.

## Live feed

`GET /api/v1/ws/events?dev=<id>` (WebSocket, Origin checked, 503 without bus). Text frames
`{"v":1,"type":"decoder_event","event":Event}` for each newly stored event of the device. One-way;
client messages are ignored. Clients load history through REST and append live frames, de-duplicating
by `id`.
