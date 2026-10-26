# WebSocket protocol: spectrum gateway and state feed (v1)

The state feed (`/api/v1/ws/state`) is described at the end; the decoder event feed (`/api/v1/ws/events`) in `api-decoder-events.md`. The audio socket, including the
`{"type":"rds"}` text messages with the decoded RDS state of a VFO (PS, RadioText, PI, PTY), is
specified in `docs/protocol-audio.md`. Spectrum endpoint: `GET /api/v1/ws/spectrum?dev=<device-id>` on **superhet-api** (port 8073). Part of the
browser-facing edge described in `docs/ARCHITECTURE.md` (section 2, step 3a). The audio socket
(`docs/protocol-audio.md`) follows the same conventions; the event socket will too.

## Connection

- Upgrade is refused with `400` (invalid or missing `dev`), `403` (`Origin` not allowed) or `503`
  (no bus, too many clients/devices, subscription failed).
- `Origin` check: with `SUPERHET_ALLOWED_ORIGINS` (comma separated, e.g. `https://sdr.example.org`)
  only exact matches pass; without it the origin must equal the `Host` header (same-origin).
  Requests without `Origin` (non-browser clients) pass the check.
- Auth: the upgrade requires a valid session cookie (`docs/api-auth.md`); without one the server
  answers `401` before upgrading. The `Origin` check applies on top.

## Server to client

- **Binary**: one spectrum frame per message in the `superhet-bus` wire format v1 (32-byte header,
  one `u8` per bin, lowest frequency first; see `crates/superhet-bus/src/spectrum.rs`). The header
  describes the cropped region: `sample_rate` is the spanned bandwidth, `center_hz` its centre,
  `bin_count` the decimated count. `timestamp_us`, `min_db`, `max_db` are passed through.
  Cropping snaps outwards to source bin edges, so the header always matches the bins exactly.
  Frames are never upsampled (fewer source bins than requested are sent as-is). Decimation takes
  the maximum of each source group, so narrow peaks survive.
- **Zoom frames.** When the view is narrow, the frames come from the zoom spectrum of the
  daemon instead (see "Zoom" below): the same wire format, but the source bins are much finer, so
  the header describes a span that matches the request to within a few bins and a zoomed view
  shows real detail instead of stretched pixels. Clients need no change: always draw a frame
  at the span its header states. Rows from different zoom levels have different spans and noise
  floors; place each row by its own header.
- **Text** (JSON, always with `"v":1`): `{"v":1,"type":"ack"}` after a valid `view`, or
  `{"v":1,"type":"error","error":"<reason>"}` for a rejected message (connection stays open).

## Client to server

Text frames, at most 4096 bytes. Until the first `view` the full frame is sent at 1024 bins.

`{"type":"view", "v":1, "start_hz":..., "end_hz":..., "bins":..., "max_fps":...}`

- Span: either `start_hz` + `end_hz`, or `center_hz` + `span_hz`, or none (full frame). Mixing is
  an error. Values must be finite, within 0..1 THz, with a span of at least 1 Hz; a lower edge
  below 0 is clamped to 0. The span is clamped to what the device delivers; if it does not
  overlap the current frame, no frame is sent.
- `bins`: target bin count (client pixel width), clamped to 16..4096 (default 1024).
- `max_fps`: frame rate limit, clamped to 1..30 (default 30; sdrd publishes about 25 fps).
- `v`, `bins`, `max_fps` are optional. A new `view` replaces the previous one.

## Zoom

A `view` whose span is at most 40 % of the full band makes the gateway lease a **zoom
spectrum** from the daemon (ADR-0009): the daemon shifts the slice to DC, decimates by a power of
two and runs an FFT with the same bin count, so the bin width shrinks by that factor (2.4 MS/s:
1172 Hz per bin full band, 73 Hz for a 100 kHz view, 9 Hz for a 10 kHz view).

- The lease is requested 100 ms after the view stopped changing and repeated while it is needed;
  until zoom frames arrive (about one FFT window, at most 125 ms) the client keeps receiving the
  stretched full-band rows. Zoom frames are used while they arrive and cover the view; the
  full-band frames are held back for that session meanwhile.
- **One slice per device, last writer wins.** With several tabs, the tab whose view changed last
  owns the slice; tabs whose view the slice does not cover keep the full-band spectrum. Closing the
  owner hands the slice to the most recent other tab or releases it.
- **Limits.** The slice cannot be narrower than about 15 kHz of usable band at 2.4 MS/s (the
  decimated rate stays at 12.5 kHz or more and the FFT window at 125 ms or less, so the bin width
  stays at 9 Hz or more); narrower views are served from that slice and cropped. Spans above about
  35 % of the sample rate are served from the full band. Panning inside the slice is free; leaving
  it re-plans the slice (one FFT window without zoom frames).
- No new client message: the existing `view` carries everything. Bus subjects and the lease
  message are described in `docs/ARCHITECTURE.md` (bus table) and ADR-0009.

## Flow control

One bus subscription per device is shared by all clients through a small broadcast buffer
(4 frames). A client that does not keep up silently skips the oldest frames; it never delays other
clients or the bus subscription. A client that cannot accept a message for 5 s is disconnected.
Spectrum is lossy live data by design.

# State feed

Endpoint: `GET /api/v1/ws/state?dev=<device-id>` on **superhet-api**. Pushes the device state
(`docs/api-devices.md`, "State object": tuning, gain, VFOs with status, locks, window) whenever
it changes, so clients do not poll `GET /api/v1/devices/{dev}/state`. Origin check, session
cookie and the `400`/`403`/`503` refusals work as for the spectrum socket (`400` for an invalid
`dev`, at most 32 clients).

## Server to client

Text frames only, JSON, always `{"v":1,"type":"state","state":{...}}` with the complete state
object. There are no diffs: the newest frame replaces everything the client knew. The document is
prepared for the viewer as in `GET /api/v1/devices/{dev}/state` (`docs/api-users.md`): every VFO
has `"mine"` and `"shared"`, `owner` only for operators/admins, lock holder names and ids are removed for guests.

- The first frame is the current state (asked from the daemon, falling back to the last known
  one in KV). If the device has no state yet (daemon not running), no frame is sent until it
  appears.
- Then one frame per published state: every tune, gain/ppm/bias change, free-tuning retune,
  VFO create/update/delete, lock change and profile load, plus a re-publish every 30 s. The frames
  are the daemon's state documents on `sh.v1.dev.<dev>.state`, in order; right after connecting a
  client can briefly see a document it already has, or one just older than the first frame, so
  treat the last frame as the truth.
- The feed does not announce that the daemon went away; clients that need to know watch their
  own request failures (`503` on control routes) or the missing 30 s re-publish.

A client that cannot accept a frame for 5 s is disconnected. Text sent by the client is ignored.
All sockets of a session are closed by the server when an admin kicks the session, disables or
deletes the account, or resets its password, and when a guest session ends.
Spectrum frames stay on `/api/v1/ws/spectrum`; audio on `/api/v1/ws/audio`.
