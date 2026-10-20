# WebSocket protocol: spectrum gateway (v1)

Endpoint: `GET /api/v1/ws/spectrum?dev=<device-id>` on **superhet-api** (port 8073). Part of the
browser-facing edge described in `docs/ARCHITECTURE.md` (section 2, step 3a). The audio socket
(`docs/protocol-audio.md`) follows the same conventions; the event socket will too.

## Connection

- Upgrade is refused with `400` (invalid or missing `dev`), `403` (`Origin` not allowed) or `503`
  (no bus, too many clients/devices, subscription failed).
- `Origin` check: with `SUPERHET_ALLOWED_ORIGINS` (comma separated, e.g. `https://sdr.example.org`)
  only exact matches pass; without it the origin must equal the `Host` header (same-origin).
  Requests without `Origin` (non-browser clients) pass the check.
- Auth: the route sits behind `auth::require_session`. **Currently a pass-through** until local
  auth (session cookie, ARCHITECTURE section 7) lands; do not expose the API publicly before.

## Server to client

- **Binary**: one spectrum frame per message in the `superhet-bus` wire format v1 (32-byte header,
  one `u8` per bin, lowest frequency first; see `crates/superhet-bus/src/spectrum.rs`). The header
  describes the cropped region: `sample_rate` is the spanned bandwidth, `center_hz` its centre,
  `bin_count` the decimated count. `timestamp_us`, `min_db`, `max_db` are passed through.
  Cropping snaps outwards to source bin edges, so the header always matches the bins exactly.
  Frames are never upsampled (fewer source bins than requested are sent as-is). Decimation takes
  the maximum of each source group, so narrow peaks survive.
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

## Flow control

One bus subscription per device is shared by all clients through a small broadcast buffer
(4 frames). A client that does not keep up silently skips the oldest frames; it never delays other
clients or the bus subscription. A client that cannot accept a message for 5 s is disconnected.
Spectrum is lossy live data by design.
