# Audio frames (v1)

Demodulated VFO audio is published by **superhet-sdrd** on core NATS, subject
`sh.v1.dev.<dev>.audio.<vfo_id>` (one subject per VFO, `<vfo_id>` is the decimal VFO id, see
`docs/api-devices.md`). Core NATS means lossy live data: nothing is buffered, frames are dropped
when the bus or a consumer lags. Consumers detect loss through `seq` / `sample_pos`. The browser
facing audio socket of **superhet-api** will follow the conventions of `docs/protocol-ws.md`.

## Frame

One NATS message = one `AudioFrame`: a fixed 42-byte header followed by one Opus packet of 20 ms
(960 samples at 48 kHz). Little-endian. Reference implementation and tests:
`crates/superhet-bus/src/audio.rs`.

| offset | type | field |
|---|---|---|
| 0 | `u8[4]` | `magic` = `"SHAU"` |
| 4 | `u8` | `version` = 1 |
| 5 | `u8` | `codec` = 1 (Opus) |
| 6 | `u8` | `channels` = 1 |
| 7 | `u8` | `flags`: bit 0 = squelch open (or no squelch set); other bits reserved, 0 |
| 8 | `u32` | `sample_rate` in Hz (48000) |
| 12 | `u32` | `vfo_id` |
| 16 | `u32` | `seq`: +1 per frame, wraps |
| 20 | `u64` | `sample_pos`: 48 kHz sample counter of the first sample, +960 per frame |
| 28 | `u64` | `capture_unix_ns`: wall time at which the IQ of the first sample was captured |
| 36 | `f32` | `power_dbfs`: channel power (S-meter), 0 dBFS = complex tone at full scale |
| 40 | `u16` | `payload_len` |
| 42 | `u8[payload_len]` | Opus packet |

A message whose length is not exactly `42 + payload_len`, with a wrong magic, an unknown version
or an unknown codec must be discarded.

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
- Opus settings: 48 kHz mono, VBR, application `audio`, 20 ms frames, 64 kbit/s for WFM and
  24 kbit/s for NFM/AM. Decoders need no out-of-band configuration (no in-band FEC, no DTX).
- Bandwidth: about 50 packets/s per VFO; a packet is 20-160 bytes at 24 kbit/s.
