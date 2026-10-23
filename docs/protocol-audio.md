# Audio frames (v1)

Demodulated VFO audio is published by **superhet-sdrd** on core NATS, subject
`sh.v1.dev.<dev>.audio.<vfo_id>` (one subject per VFO, `<vfo_id>` is the decimal VFO id, see
`docs/api-devices.md`). Core NATS means lossy live data: nothing is buffered, frames are dropped
when the bus or a consumer lags. Consumers detect loss through `seq` / `sample_pos`. The browser
facing audio socket of **superhet-api** (below) follows the conventions of `docs/protocol-ws.md`.

## Frame

One NATS message = one `AudioFrame`: a fixed 42-byte header followed by one Opus packet of 20 ms
(960 samples per channel at 48 kHz). Little-endian. Reference implementation and tests:
`crates/superhet-bus/src/audio.rs`.

| offset | type | field |
|---|---|---|
| 0 | `u8[4]` | `magic` = `"SHAU"` |
| 4 | `u8` | `version` = 1 |
| 5 | `u8` | `codec` = 1 (Opus) |
| 6 | `u8` | `channels`: 1 = mono, 2 = stereo (Opus stereo packet, interleaved L/R) |
| 7 | `u8` | `flags`: bit 0 = squelch open (or no squelch set), bit 1 = stereo pilot locked; other bits reserved, 0 |
| 8 | `u32` | `sample_rate` in Hz (48000) |
| 12 | `u32` | `vfo_id` |
| 16 | `u32` | `seq`: +1 per frame, wraps |
| 20 | `u64` | `sample_pos`: 48 kHz sample counter of the first sample, +960 per frame |
| 28 | `u64` | `capture_unix_ns`: wall time at which the IQ of the first sample was captured |
| 36 | `f32` | `power_dbfs`: channel power (S-meter), 0 dBFS = complex tone at full scale |
| 40 | `u16` | `payload_len` |
| 42 | `u8[payload_len]` | Opus packet |

A message whose length is not exactly `42 + payload_len`, with a wrong magic, an unknown version,
an unknown codec or a channel count other than 1 or 2 must be discarded.

## Semantics

- `seq` and `sample_pos` run per VFO from the creation of the VFO and are continuous. While a VFO
  is paused (outside the passband) or after a retune the channel restarts: buffered audio of the
  old tuning is discarded and the counters resume without gap, so a time gap is visible in
  `capture_unix_ns` only. After `vfo.delete` + `vfo.create` the id (and the counters) are new.
- `capture_unix_ns` is the daemon's wall clock minus the block duration at the time the IQ block
  reached the VFO thread, interpolated to the frame start; it is meant for latency measurement
  (compare with the consumer's clock; accuracy is limited by clock skew and block size, 1-100 ms).
- While the squelch is closed frames keep flowing (the encoder makes silence tiny) with flag bit 0
  cleared; consumers may mute or skip them. The flag is the squelch state at the end of the
  processing step that completed the frame.
- Opus settings: 48 kHz, VBR, application `audio`, 20 ms frames; 64 kbit/s mono WFM, 96 kbit/s
  stereo WFM and 24 kbit/s for NFM/AM/SSB/CW. Decoders need no out-of-band configuration (no
  in-band FEC, no DTX).
- Channels: `channels` is a property of the VFO configuration, not of the signal. A WFM VFO with
  stereo on (the default, `docs/api-devices.md`) always sends `channels = 2`; with stereo off,
  and for every other mode, `channels = 1`. Switching the setting restarts the encoder, so the
  channel count changes between two frames with a short gap; a consumer must follow the
  `channels` of each frame (WebCodecs: reconfigure the decoder). `sample_pos` counts sample
  frames (one per channel pair), so it advances by 960 per packet either way.
- Flag bit 1 (`stereo`) is set on a stereo stream while the 19 kHz pilot of the station is
  locked and the matrix is (fading towards) full stereo. Without a pilot, or with a weak one, the
  daemon sends the mono sum on both channels and clears the flag (hysteresis: on at a pilot
  amplitude of 4 % of full deviation after 150 ms, off below 2.5 % after 80 ms; 50 ms crossfade).
  The UI's "ST" lamp follows this flag.
- Bandwidth: about 50 packets/s per VFO; a packet is 20-160 bytes at 24 kbit/s.

## RDS state

A WFM VFO decodes RDS (IEC 62106) natively in the daemon (`superhet-dsp::rds`, no external
plugin). The decoded state is published as JSON on core NATS (lossy, like audio), subject
`sh.v1.dev.<dev>.rds.<vfo_id>`; the daemon needs publish permission on `sh.v1.dev.*.rds.>`
(`deploy/nats/nats.conf`). Reference: `crates/superhet-bus/src/rds.rs`.

`{"v":1,"type":"rds","vfo":3,"synced":true,"pi":"D3C2","pty":10,"tp":true,"ta":false,"ms":true,"ps":"SUPERHET","rt":"Now playing"}`

- One message is the complete state of the VFO's station, not a delta. All fields except `v`,
  `type`, `vfo` and `synced` are `null` until decoded: `pi` (four upper-case hex digits), `pty`
  (0..31), `tp`/`ta` (traffic programme / announcement), `ms` (`true` = music, `false` = speech),
  `ps` (programme service name, 8 characters), `rt` (RadioText, up to 64 characters, trailing
  blanks trimmed). Text is UTF-8, mapped from the RDS basic character set. `synced` says the
  block synchronisation holds right now.
- Published on every change and repeated once a second while there is anything to show, so a
  subscriber that joins late has the state within a second. An **empty state** (`synced:false`,
  everything `null`) is published when the data goes away: retune of the VFO or the device (more
  than 5 kHz), mode or bandwidth change, no block sync for about 10 s. Consumers clear their display on it.
- Robustness: blocks are checked by their syndrome; single-bit and two-bit bursts are corrected
  while in sync. Text from error-free groups is taken over at once, text that needed a
  correction only after it was received twice in a row; a PI change forgets everything of the old
  station. Group types 0A/0B (PS, TA, M/S) and 2A/2B (RadioText) are decoded, PI/PTY/TP from every
  group. Clock time, alternative frequencies and the other groups are not decoded yet.
- Messages are limited to 2048 bytes; the API discards messages that are malformed, have another
  `v`/`type`, or whose `vfo` differs from the subject.

## WebSocket (superhet-api)

Endpoint: `GET /api/v1/ws/audio?dev=<device-id>` on **superhet-api** (port 8073). Connection rules
are those of the spectrum socket (`docs/protocol-ws.md`): upgrade refused with `400` (invalid or
missing `dev`), `403` (`Origin` not allowed, same `SUPERHET_ALLOWED_ORIGINS` policy), `503` (no
bus, more than 32 audio clients, too many streams, subscription failed); the route sits behind
session check (`401` without a valid cookie, `docs/api-auth.md`).

**Client to server** (text, at most 1024 bytes): `{"type":"subscribe","v":1,"vfos":[1,2]}`. The list
replaces the previous set; an empty list means none (a new socket starts with none). `v` is
optional. Duplicates collapse; at most 8 distinct VFO ids, ids are >= 1. Unknown fields are
rejected. VFOs need not exist yet: frames flow once the daemon publishes them.

**Server to client**
- Text, always with `"v":1`: `{"v":1,"type":"ack","vfos":[...]}` (the applied set) after a valid
  `subscribe`; `{"v":1,"type":"error","error":"<reason>"}` for an invalid message or a failed
  subscription (connection stays open, the previous set stays active). An oversized message closes
  the connection.
- Text, `{"v":1,"type":"rds",...}`: the RDS state of a subscribed WFM VFO (above), forwarded
  byte for byte. It shares the queue with the audio frames (so it can be dropped under load; the
  repeat fixes that within a second) and is never sent for a VFO that is not in the set.
- Binary: one `AudioFrame` v1 (above) per message, **byte for byte as published**; the frame
  carries its `vfo_id`. Only subscribed VFOs are forwarded, and no frame of a VFO that was
  dropped from the set follows its acknowledgement. Frames that fail validation (length, magic,
  version, codec, `vfo_id` differing from the subject) are discarded by the API.

**Flow control**: the API keeps one bus subscription per (device, VFO), shared by all clients and
dropped with the last one. Each client has a queue of 32 frames (shared by its VFOs); when it is
full the oldest frame is dropped and counted (logged when the client leaves). A slow client never
delays the bus subscription or other clients; it sees the loss as a gap in `seq` / `sample_pos`.
A client that cannot accept a message for 5 s is disconnected.
