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

`{"v":1,"type":"rds","vfo":3,"synced":true,"pi":"D3C2","pty":10,"tp":true,"ta":false,"ms":true,"ps":"SUPERHET","rt":"Now playing","bler":3,"quality":97}`

- One message is the complete state of the VFO's station, not a delta. All fields except `v`,
  `type`, `vfo` and `synced` are `null` until decoded: `pi` (four upper-case hex digits), `pty`
  (0..31), `tp`/`ta` (traffic programme / announcement), `ms` (`true` = music, `false` = speech),
  `ps` (programme service name, 8 characters), `rt` (RadioText, up to 64 characters, trailing
  blanks trimmed). Text is UTF-8, mapped from the RDS basic character set. `synced` says the
  block synchronisation holds right now.
- Optional quality fields (absent until about 10 blocks were seen in the window, and from older
  daemons): `bler` is the share of failed blocks in percent (0..100) over the last ~10 s of block
  sync, `quality` is `100 - bler`, or 0 while the sync is lost. The UI grades them good (BLER up to
  10 %), fair (up to 35 %) and poor, and explains that station name and RadioText need a better
  signal when BLER is high and they are missing. After sync is lost the fields fade out within
  10 s.
- Published on every change and repeated once a second while there is anything to show, so a
  subscriber that joins late has the state within a second. An **empty state** (`synced:false`,
  everything `null`) is published when the data goes away: retune of the VFO or the device (more
  than 5 kHz), mode or bandwidth change, no block sync for about 10 s. Consumers clear their display on it.
- Robustness: blocks are checked by their syndrome; single-bit and two-bit bursts are corrected
  while in sync. Text from error-free groups is taken over at once, text that needed a
  correction only after it was received twice in a row; a PI change forgets everything of the old
  station. The PI is also recovered from block A repetitions that never decode on their own: while in
  sync, the bits of failed or burst-corrected block A are summed up weighted by their
  reliability (with fading memory), and a combined word that passes the block check counts as one
  PI reception. A PI is shown after two receptions out of a small candidate table, so a false
  block neither delays nor replaces the right one. Block sync is kept up to 30 failed blocks out
  of 40 so weak stations keep delivering. Group types 0A/0B (PS, TA, M/S) and 2A/2B (RadioText) are decoded, PI/PTY/TP from every
  group. Clock time, alternative frequencies and the other groups are not decoded yet.
- Messages are limited to 2048 bytes; the API discards messages that are malformed, have another
  `v`/`type`, or whose `vfo` differs from the subject.

## Tone state

An NFM or AM VFO with tone detection on (default on for NFM, off for AM, per-VFO switch) runs a
native CTCSS and DTMF detector on its 48 kHz audio (`superhet-dsp::tones`; taken before the
squelch gate so the sub-audible band is intact). The result is published as JSON on core NATS
(lossy), subject `sh.v1.dev.<dev>.tones.<vfo_id>`; the daemon needs publish permission on
`sh.v1.dev.*.tones.>` (`deploy/nats/nats.conf`). Reference: `crates/superhet-bus/src/tones.rs`.

`{"v":1,"type":"tones","vfo":3,"ctcss_hz":88.5,"dtmf_count":6,"dtmf_tail":"123*45"}`

- One message is the complete state. `ctcss_hz` is the sub-tone currently locked (one of the 50
  standard tones, 67.0 to 254.1 Hz) or `null`. `dtmf_count` counts the digits detected since the
  channel (re)started and `dtmf_tail` holds the last 32 of them (`0-9`, `A-D`, `*`, `#`, oldest
  first). A client appends the last `dtmf_count - previous count` characters of the tail to its
  own digit string; a lost or repeated message therefore cannot corrupt it, and a count that went
  backwards means a restart. If more digits arrived than the tail holds, the oldest are lost.
- Published on every change (a locked/released tone, a new digit) and repeated once a second
  while a tone is locked or digits exist. An **empty state** (`ctcss_hz:null`, `dtmf_count:0`) is
  published when the data goes away: retune of the VFO or the device (more than 5 kHz), mode or
  bandwidth change, tone detection switched off. Consumers clear their display on it.
- Detection. CTCSS: 50 Hann-windowed Goertzel filters over 0.5 s every 125 ms on audio decimated
  to 2 kHz; the strongest tone must stand 10.8 dB above the mean of the tones more than 6.5 Hz
  away, above -48 dBFS, and stationary over both window halves (rejects a gliding voice pitch).
  It locks after two consecutive hits (about 0.6 s after the tone starts) and is released after
  five misses. Tones 2.3 Hz apart (67.0/69.3, 159.8/162.2) are told apart; a tone between two
  standard ones locks to the nearer one or to none. DTMF: Goertzel filters for the eight
  frequencies plus the second harmonics of the rows on audio decimated to 8 kHz, a 25.6 ms
  window every 8 ms. A window is a digit when both tones are above -34 dBFS, carry 70 % of the
  window energy, stand 10 dB above the rest of their group, the twist is within +4 dB / -8 dB
  (ITU-T Q.24) and the row tone's second harmonic is 10 dB down (talk-off guard). A digit needs
  three consecutive windows (tones of at least about 42 ms) and is reported once; a repeat of
  the same digit needs a pause of at least about 34 ms, a different digit follows without one.
  Detection only runs while the power squelch is open; a closed squelch resets the filters (not
  the digit history).
- Limits. A sustained music note on a CTCSS frequency (bass, organ) looks like the tone, as in
  any hardware decoder; CTCSS is a transmitter-side convention for NFM, so this does not matter
  on its intended use. DCS (digital coded squelch) is not detected.
- Messages are limited to 1024 bytes; the API discards messages that are malformed, have another
  `v`/`type`, or whose `vfo` differs from the subject.

## WebSocket (superhet-api)

Endpoint: `GET /api/v1/ws/audio?dev=<device-id>` on **superhet-api** (port 8073). Connection rules
are those of the spectrum socket (`docs/protocol-ws.md`): upgrade refused with `400` (invalid or
missing `dev`), `403` (`Origin` not allowed, same `SUPERHET_ALLOWED_ORIGINS` policy), `503` (no
bus, more than 32 audio clients, too many streams, subscription failed); the route sits behind
session check (`401` without a valid cookie, `docs/api-auth.md`).

**Client to server** (text, at most 1024 bytes): `{"type":"subscribe","v":1,"vfos":[1,2]}`. The list
replaces the previous set; an empty list means none (a new socket starts with none). `v` is
optional. Duplicates collapse; at most 64 distinct VFO ids, ids are >= 1. Unknown fields are
rejected. Operators and admins may subscribe to any VFO id (it need not exist yet: frames flow once
the daemon publishes it). Listeners and guests may subscribe only to VFOs that exist and are their
own or shared (no owner); anything else is answered with `{"v":1,"type":"error","error":
"forbidden"}` and the previous set stays active (`docs/api-users.md`).

**Server to client**
- Text, always with `"v":1`: `{"v":1,"type":"ack","vfos":[...]}` (the applied set) after a valid
  `subscribe`; `{"v":1,"type":"error","error":"<reason>"}` for an invalid message or a failed
  subscription (connection stays open, the previous set stays active). An oversized message closes
  the connection.
- Text, `{"v":1,"type":"rds",...}`: the RDS state of a subscribed WFM VFO (above), forwarded
  byte for byte. It shares the queue with the audio frames (so it can be dropped under load; the
  repeat fixes that within a second) and is never sent for a VFO that is not in the set.
- Text, `{"v":1,"type":"tones",...}`: the tone state of a subscribed NFM / AM VFO (above), with
  the same rules as the RDS state.
- Binary: one `AudioFrame` v1 (above) per message, **byte for byte as published**; the frame
  carries its `vfo_id`. Only subscribed VFOs are forwarded, and no frame of a VFO that was
  dropped from the set follows its acknowledgement. Frames that fail validation (length, magic,
  version, codec, `vfo_id` differing from the subject) are discarded by the API.

**Flow control**: the API keeps one bus subscription per (device, VFO), shared by all clients and
dropped with the last one. Each client has a queue of 32 frames (shared by its VFOs); when it is
full the oldest frame is dropped and counted (logged when the client leaves). A slow client never
delays the bus subscription or other clients; it sees the loss as a gap in `seq` / `sample_pos`.
A client that cannot accept a message for 5 s is disconnected.
