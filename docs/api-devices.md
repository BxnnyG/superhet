# Device state and tuning (v1)

Runtime control of an SDR: `superhet-sdrd` serves control requests on the bus and publishes the
device state; `superhet-api` exposes both over REST. Architecture background: `docs/ARCHITECTURE.md`
sections 2 and 3. VFOs are bus-only so far (no REST route yet). Not covered yet: tuning
locks/claims, profiles.

## REST (superhet-api, port 8073)

Both routes sit behind `auth::require_session` (currently a pass-through, see `docs/protocol-ws.md`).
`{dev}` is the device id (`SUPERHET_DEVICE_ID` of the daemon).

### `GET /api/v1/devices/{dev}/state`

Returns the state object (below). Read from the JetStream KV bucket `STATE` (key `dev.<dev>`),
falling back to a `ctl.state` request if the bucket or key is missing.

### `POST /api/v1/devices/{dev}/tune`

Body (at most 4 KiB, `Content-Type: application/json`), every field optional but at least one:

`{"v":1, "center_hz":145500000, "sample_rate":2048000, "gain":"auto"|28.0, "ppm":0, "bias_tee":false}`

- `v` defaults to 1. Unknown fields are rejected. `gain` is `"auto"` or manual dB; the device snaps
  to the nearest supported step and the reply reports the applied value.
- The API forwards the body as `ctl.tune` request and returns the daemon's reply unchanged:
  `{"v":1,"ok":true,"state":{...}}` or `{"v":1,"ok":false,"code":"invalid|unsupported|device","error":"..."}`.
- Status codes: `200` applied; `400` invalid body/value (`invalid`) or a setting the device lacks
  (`unsupported`, e.g. gain on a file replay); `403` `Origin` not allowed; `413` body too large;
  `415` wrong content type; `502` device or bus failure; `503` no bus, or nobody serves this device;
  `504` no answer within 2 s.
- CSRF: same `Origin` policy as the WebSocket (`SUPERHET_ALLOWED_ORIGINS`, default same-origin;
  requests without `Origin` pass, they are subject to authentication only).

## Bus

- `sh.v1.dev.<dev>.ctl.tune` (request/reply): body as above (`v` required). Requests are validated
  on arrival (errors are answered immediately), then **coalesced**: after the first request the
  daemon waits until the bus has been quiet for 30 ms (at most 150 ms in total), folds all requests
  of the burst into one (per field the latest value wins), applies it once and sends the same reply
  to every requester. A caller whose request was superseded sees the final state in the reply.
- `sh.v1.dev.<dev>.ctl.state` (request/reply): reply `{"v":1,"ok":true,"state":{...}}`.
- Unknown commands are answered with `ok:false`, `code:"invalid"`.
- State is published after start-up and every change on `sh.v1.dev.<dev>.state` (core NATS) and
  written to KV bucket `STATE` (history 1, file storage, created by sdrd if missing), key
  `dev.<dev>`; it is re-published every 30 s and the key is removed on graceful shutdown. After a
  crash the last state stays in KV until the daemon is back, so treat it as "last known".
  KV is updated asynchronously, i.e. shortly *after* the tune reply.

## State object

`{"v":1,"device","driver","tuner"?,"center_hz","sample_rate","gain":{"mode":"auto"|"manual","db":null|number}|null,"ppm","bias_tee","tuning_epoch","gains_db":[...],"vfos":[...]}`

`vfos` lists the device's VFOs (see below) with their current `status`; it is re-published on
every VFO change and every retune (status follows the tuning). VFOs live in the daemon's memory
and in this document; they are not restored after a daemon restart.

`gain` is `null` for devices without gain control (file replay); `gains_db` lists the selectable
manual steps. `tuner` is omitted when unknown.

## VFOs

A VFO is a demodulated channel at an absolute frequency inside the passband of the device. The
daemon runs one channelizer per VFO on a dedicated thread, encodes the audio to Opus (48 kHz mono,
20 ms frames, VBR; 64 kbit/s WFM, 24 kbit/s NFM/AM) and publishes it as binary `AudioFrame` v1 on
`sh.v1.dev.<dev>.audio.<vfo_id>` (core NATS, lossy; format in `docs/protocol-audio.md`). At most 8
VFOs per device.

VFO object (in `vfos` and in replies):
`{"id","freq_hz","mode":"wfm"|"nfm"|"am","bandwidth_hz","squelch_dbfs":null|number,"deemphasis":"none"|"50us"|"75us"|null,"offset_hz","status":"active"|"paused"}`

- `id` is assigned by the daemon (starting at 1, increasing, not reused while the daemon runs).
- `bandwidth_hz` is the effective value (mode default: WFM 180 kHz, NFM 12.5 kHz, AM 10 kHz).
  `deemphasis` is the effective value (WFM 50 us, NFM none) and `null` for AM.
- `offset_hz` = `freq_hz` minus the device centre, recomputed on every retune.
- `status` is `paused` when `|offset_hz| + bandwidth_hz / 2 > sample_rate / 2`: no DSP, no audio
  frames; the VFO resumes (channel reset) as soon as a retune brings it back into the passband.
  While the squelch is closed frames keep flowing (they are tiny) with the squelch flag cleared.

Requests (request/reply, JSON, `v` required, unknown fields rejected). Success replies are
`{"v":1,"ok":true,"vfo":{...},"state":{...}}` (create, update) or
`{"v":1,"ok":true,"state":{...}}` (delete); failures use the same `{"ok":false,"code","error"}`
shape as `ctl.tune`. VFO requests are not coalesced.

- `sh.v1.dev.<dev>.ctl.vfo.create`: `{"v":1,"freq_hz":..,"mode":"wfm"|"nfm"|"am","bandwidth_hz"?:..,
  "squelch_dbfs"?:..,"deemphasis"?:..}`. `freq_hz` 1..1 THz, `bandwidth_hz` 1000..1000000 (and
  realisable for the mode at the device's sample rate), `squelch_dbfs` -200..0 (absent or `null`
  = off), `deemphasis` only for WFM/NFM. A VFO outside the passband is created `paused`.
- `sh.v1.dev.<dev>.ctl.vfo.update`: `{"v":1,"id":..}` plus any of the fields above (partial patch,
  at least one). `null` for `bandwidth_hz` / `squelch_dbfs` / `deemphasis` resets to the default /
  off. Changing `mode` drops a `deemphasis` override unless given in the same request. Changing
  mode or bandwidth rebuilds the channel (a short audio gap); frequency and squelch changes do not.
- `sh.v1.dev.<dev>.ctl.vfo.delete`: `{"v":1,"id":..}`.

Error codes: `invalid` (malformed or out-of-range request, unknown id, unrealisable bandwidth,
de-emphasis on AM) and `unsupported` (the limit of 8 VFOs is reached).

## Tuning epoch

Every applied change of centre, sample rate, gain or ppm increments `tuning_epoch` (bias tee does
not); repeating current values changes nothing. Each IQ block carries the epoch it was captured
in. The RTL-SDR backend discards the samples around a change (the hardware is being reconfigured,
two USB buffers may still hold old data); the spectrum pipeline resets its averaging and overlap
state when the epoch changes and drops the first block. Spectrum frames take `center_hz` and
`sample_rate` from the block, so they carry the new tuning from the first frame after the change;
the frame wire format is unchanged. The VFO thread does the same: on an epoch change it recomputes
every offset, resets the channels and ignores the first block.

## Devices

- `file`: centre frequency is only a label and can be changed freely; sample rate, gain, ppm and
  bias tee are fixed by the recording (requests repeating the current rate are accepted).
- `rtlsdr` (feature `rtlsdr`): centre must fit 32 bit, rate 225001-300000 or 900001-3200000 Hz,
  ppm within +-1000. Changes are applied with librtlsdr while `read_async` keeps running (the same
  pattern `rtl_tcp` uses); a rate change re-applies the centre frequency afterwards. Not yet
  verified on hardware: run `cargo test -p superhet-device --features rtlsdr -- --ignored`.
