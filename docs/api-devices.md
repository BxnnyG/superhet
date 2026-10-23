# Device state and tuning (v1)

Runtime control of an SDR: `superhet-sdrd` serves control requests on the bus and publishes the
device state; `superhet-api` exposes both over REST. Architecture background: `docs/ARCHITECTURE.md`
sections 2 and 3. Profiles: `docs/api-profiles.md`. Bookmarks: `docs/api-bookmarks.md`. Not covered yet: recording/decoder lock holders
and exclusive claims.

## REST (superhet-api, port 8073)

All routes require a session cookie and answer `401 {"error":"unauthenticated"}` without one
(`docs/api-auth.md`).
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
  `{"v":1,"ok":true,"state":{...}}` or `{"v":1,"ok":false,"code":"invalid|not_found|unsupported|device","error":"..."}`.
- Status codes: `200` applied; `400` invalid body/value (`invalid`) or a setting the device lacks
  (`unsupported`, e.g. gain on a file replay); `409` the tuning is locked (`locked`, see "Tuning
  lock"); `403` `Origin` not allowed; `413` body too large;
  `415` wrong content type; `502` device or bus failure; `503` no bus, or nobody serves this device;
  `504` no answer within 2 s.
- CSRF: same `Origin` policy as the WebSocket (`SUPERHET_ALLOWED_ORIGINS`, default same-origin;
  requests without `Origin` pass, they are subject to authentication only).

### Tuning lock routes

`POST /api/v1/devices/{dev}/lock` takes the user lock, `DELETE /api/v1/devices/{dev}/lock`
releases it. No body. Both are idempotent and answer `200` with `{"v":1,"ok":true,"state":{...}}`
(the state carries `locks`). Same CSRF policy and error shape as `/tune`. Semantics: "Tuning lock".

### VFO routes

`GET /api/v1/devices/{dev}/vfos`, `POST /api/v1/devices/{dev}/vfos`,
`PATCH /api/v1/devices/{dev}/vfos/{id}`, `DELETE /api/v1/devices/{dev}/vfos/{id}`. VFO semantics,
fields and limits: section "VFOs" below. Bodies are at most 4 KiB, `Content-Type: application/json`
(POST, PATCH); `v` defaults to 1.

- `GET`: `{"v":1,"vfos":[...]}`, taken from the device state (same source as `/state`, so it may
  lag a change by a moment).
- `POST`: body as `ctl.vfo.create`. `201` with the daemon's reply unchanged
  (`{"v":1,"ok":true,"vfo":{...},"retune":"none|applied|locked|failed","state":{...}}`) and `Location: /api/v1/devices/{dev}/vfos/{id}`.
- `PATCH`: partial update as `ctl.vfo.update`; the id comes from the URL (a different `"id"` in the
  body is a `400`). At least one field is required; `null` resets `bandwidth_hz`, `squelch_dbfs`
  `deemphasis`, `cw_pitch_hz` and `stereo`. `200` with the daemon's reply.
- `DELETE`: `204` without body.
- Errors use the shape of the tune route: `400` invalid body/value, non-numeric id, or the daemon's
  `invalid`/`unsupported` (including the limit of 8 VFOs); `404` the daemon's `not_found` (unknown
  VFO id, PATCH and DELETE); `403` `Origin` not allowed; `413`; `415`; `502`; `503`; `504` as for
  `/tune`.
- Same CSRF policy as `/tune`; audio is fetched through the audio WebSocket
  (`docs/protocol-audio.md`).

## Bus

- `sh.v1.dev.<dev>.ctl.tune` (request/reply): body as above (`v` required). Requests are validated
  on arrival (errors are answered immediately), then **coalesced**: after the first request the
  daemon waits until the bus has been quiet for 30 ms (at most 150 ms in total), folds all requests
  of the burst into one (per field the latest value wins), applies it once and sends the same reply
  to every requester. A caller whose request was superseded sees the final state in the reply.
- `sh.v1.dev.<dev>.ctl.state` (request/reply): reply `{"v":1,"ok":true,"state":{...}}`.
- Unknown commands are answered with `ok:false`, `code:"invalid"`.
- `sh.v1.dev.<dev>.ctl.lock.acquire` / `.ctl.lock.release` (request/reply): body `{"v":1}`, reply
  as `ctl.state`. Take or drop the user lock (idempotent).
- `sh.v1.dev.<dev>.ctl.profile.load` (request/reply): `{"v":1,"profile":{...}}` (the profile
  document, `docs/api-profiles.md`), reply as `ctl.state`. Section "Profiles" below.
- State is published after start-up and on **every change** (tune, gain/ppm/bias, VFO create/update/
  delete, lock, profile load, free-tuning retune) on `sh.v1.dev.<dev>.state` (core NATS, payload
  = the state object, nothing else) and written to KV bucket `STATE` (history 1, file storage,
  created by sdrd if missing), key `dev.<dev>`; it is re-published every 30 s and the key is
  removed on graceful shutdown. After a crash the last state stays in KV until the daemon is back,
  so treat it as "last known". KV is updated asynchronously, i.e. shortly *after* the tune reply.
  Browsers get the same documents pushed through `/api/v1/ws/state` (`docs/protocol-ws.md`).
- Restore: every published state is also written to KV key `restore.<dev>`, which (unlike
  `dev.<dev>`) is kept on shutdown. On start the daemon reads it before it serves requests or
  publishes anything, and applies it if `device` and `driver` match: centre, rate, gain, ppm and
  bias tee (each on its own; what the device refuses is skipped with a warning), the VFOs with
  their ids and the user lock. If the bus is not usable within 30 s the daemon starts with the
  device's own settings and leaves the snapshot alone until the next start.
- Default VFO: when no snapshot is applied at start (first run, a snapshot of another device or
  driver, or an unusable bus) the daemon creates one VFO (id 1) at the centre frequency, WFM if
  the centre is within 87.5-108 MHz, NFM otherwise. An applied snapshot is never topped up: if the
  user deleted every VFO, the set stays empty across restarts.

## State object

`{"v":1,"device","driver","tuner"?,"center_hz","sample_rate","gain":{"mode":"auto"|"manual","db":null|number}|null,"ppm","bias_tee","tuning_epoch","gains_db":[...],"vfos":[...],"locks":[...],"window":{"start_hz","end_hz"}}`

`vfos` lists the device's VFOs (see below) with their current `status`; it is re-published on
every VFO change and every retune (status follows the tuning). VFOs live in the daemon's memory
and in this document, and survive a daemon restart through the restore snapshot (above).

`window` is the usable window (85 % of the sample rate around the centre, ARCHITECTURE 3); the
lower edge can be negative for very low centres. `locks` lists the lock holders (empty = tuning
is free), see "Tuning lock".

`gain` is `null` for devices without gain control (file replay); `gains_db` lists the selectable
manual steps. `tuner` is omitted when unknown.

## VFOs

A VFO is a demodulated channel at an absolute frequency inside the passband of the device. The
daemon runs one channelizer per VFO on a dedicated thread, encodes the audio to Opus (48 kHz, mono
or stereo, 20 ms frames, VBR; 64 kbit/s mono WFM, 96 kbit/s stereo WFM, 24 kbit/s NFM/AM) and publishes it as binary `AudioFrame` v1 on
`sh.v1.dev.<dev>.audio.<vfo_id>` (core NATS, lossy; format in `docs/protocol-audio.md`). At most 8
VFOs per device.

VFO object (in `vfos` and in replies):
`{"id","freq_hz","mode":"wfm"|"nfm"|"am"|"usb"|"lsb"|"cw","bandwidth_hz","squelch_dbfs":null|number,"deemphasis":"none"|"50us"|"75us"|null,"stereo":true|false|null,"cw_pitch_hz"?:number,"offset_hz","status":"active"|"paused"}`

- `id` is assigned by the daemon (starting at 1, increasing, not reused while the daemon runs).
- `bandwidth_hz` is the effective value (mode default: WFM 180 kHz, NFM 12.5 kHz, AM 10 kHz,
  USB/LSB 2.4 kHz, CW 500 Hz). For USB/LSB it is the width of the passband, which starts 300 Hz
  from `freq_hz` (default 300 Hz ..= 2.7 kHz above for USB, below for LSB); the carrier side and
  the opposite sideband are rejected (> 40 dB, tested). For CW it is the width of the filter centred on
  `freq_hz`. `deemphasis` is the effective value (WFM 50 us, NFM none) and `null` for the other
  modes. `stereo` is the effective stereo setting of a WFM VFO (default `true`) and `null` for the
  other modes; see "WFM stereo and RDS" below. `cw_pitch_hz` is present for CW only (effective value, default 700): a carrier exactly
  on `freq_hz` is heard as a tone of this frequency, one 100 Hz above as pitch + 100 Hz.
- USB/LSB/CW audio goes through a peak AGC with fast attack (2-3 ms), a hang time (250 ms SSB,
  100 ms CW) and a slow decay (400 ms SSB, 200 ms CW); the squelch measures the power after the
  channel filter like in the other modes.
- `offset_hz` = `freq_hz` minus the device centre, recomputed on every retune.
- `status` is `paused` when `|offset_hz| + span / 2 > 0.85 * sample_rate / 2` (outside the
  usable window; `span` is `bandwidth_hz`, for USB/LSB `2 * (300 + bandwidth_hz)`): no DSP, no audio frames; the VFO resumes (channel reset) as soon as a retune
  brings it back into the window.
  While the squelch is closed frames keep flowing (they are tiny) with the squelch flag cleared.

Requests (request/reply, JSON, `v` required, unknown fields rejected). Success replies are
`{"v":1,"ok":true,"vfo":{...},"state":{...}}` (create, update) or
`{"v":1,"ok":true,"state":{...}}` (delete); failures use the same `{"ok":false,"code","error"}`
shape as `ctl.tune`. VFO requests are not coalesced.

- `sh.v1.dev.<dev>.ctl.vfo.create`: `{"v":1,"freq_hz":..,"mode":"wfm"|"nfm"|"am"|"usb"|"lsb"|"cw","bandwidth_hz"?:..,
  "squelch_dbfs"?:..,"deemphasis"?:..,"cw_pitch_hz"?:..,"stereo"?:..}`. `freq_hz` 1..1 THz, `bandwidth_hz`
  100..1000000 (and realisable for the mode at the device's sample rate: at least 1000 for
  WFM/NFM/AM, 300 for USB/LSB, 100 for CW), `squelch_dbfs` -200..0 (absent or `null` = off),
  `deemphasis` only for WFM/NFM, `cw_pitch_hz` 200..2000 only for CW, `stereo` (boolean) only for
  WFM. A VFO outside the passband is created `paused`.
- `sh.v1.dev.<dev>.ctl.vfo.update`: `{"v":1,"id":..}` plus any of the fields above (partial patch,
  at least one). `null` for `bandwidth_hz` / `squelch_dbfs` / `deemphasis` / `cw_pitch_hz` /
  `stereo` resets to the default / off. Changing `mode` drops a `deemphasis`, `cw_pitch_hz` or
  `stereo` override unless given in the same request. Changing
  mode or bandwidth rebuilds the channel (a short audio gap); switching `stereo` restarts the
  audio filters and the encoder (a short gap, the RDS data stays); frequency and squelch changes
  do not.

### WFM stereo and RDS

A WFM VFO decodes the stereo multiplex (19 kHz pilot PLL, 38 kHz L-R carrier from the doubled
pilot phase, 15 kHz audio filters, de-emphasis per channel, matrix) and RDS (57 kHz carrier from
the tripled pilot phase) natively. The RDS carrier sits at 57 kHz, so RDS needs a channel
bandwidth of at least about 130 kHz (default 180 kHz); the 100 kHz preset still plays stereo but
cuts the top of the difference signal and loses RDS. `stereo` (default on) chooses between a 2-channel and a
1-channel audio stream (`docs/protocol-audio.md`: `channels`, flag bit 1 = pilot locked). A
station without a pilot, or with a weak one, plays as mono on both channels automatically. RDS
is always decoded and published as VFO-scoped JSON events (`sh.v1.dev.<dev>.rds.<vfo>`, forwarded
on the audio WebSocket), whatever the stereo setting.
- `sh.v1.dev.<dev>.ctl.vfo.delete`: `{"v":1,"id":..}`.

Free tuning: a VFO that is created, or updated in `freq_hz`, `mode` or `bandwidth_hz`, and ends up
outside the usable window makes the centre follow (rule: ARCHITECTURE 3.1). The daemon does this
itself, so all clients behave the same. The create/update reply says what happened in `retune`:
`none` (inside the window, or not moved), `applied` (centre moved, the reply's `state` has it),
`locked` (outside the window but the tuning is locked: the VFO stays where it is, `paused`, no
retune) or `failed` (the device refused the retune; the VFO is outside the window). Retunes are
applied immediately, not debounced: every one moves the window by most of its width, so a drag
causes few of them.

Error codes: `invalid` (malformed or out-of-range request, unrealisable bandwidth, de-emphasis on
anything but WFM/NFM, `cw_pitch_hz` on anything but CW), `not_found` (unknown VFO id in update/delete) and `unsupported` (the limit of 8 VFOs is
reached).

## Tuning lock

While any lock holder exists, requests that change the centre or sample rate are refused with
`code:"locked"` (REST `409`): `ctl.tune` (a request that only repeats the current centre/rate is
fine; gain, ppm and bias tee are not covered) and `ctl.profile.load` when the profile has a
different centre or rate. VFOs keep working: they can be created, moved and deleted inside the
window, and one moved outside simply pauses (`retune:"locked"`). The error reply:

`{"v":1,"ok":false,"code":"locked","error":"tuning is locked by: user","holders":[{"kind":"user","id":"user","since_ms":1760000000000}]}`

`holders` entries (also in the state's `locks`) have `kind`, `id` (identifies the holder within its
kind) and `since_ms` (Unix ms). Only `kind:"user"` exists today (`ctl.lock.*`, the REST lock
routes, or a profile with `lock:true`); recordings, decoders and exclusive claims will add their
own kinds without changing this shape. The lock survives a daemon restart (restore snapshot) but
nothing else releases it: loading a profile never does.

## Profiles

`ctl.profile.load` applies a profile document in one go: nothing is touched if the lock refuses the
tuning, a VFO is not realisable at the profile's rate, or the device refuses the settings (e.g. a
different sample rate on a file replay: `unsupported`). Otherwise centre, rate, gain (if the
profile has one), ppm and bias tee are applied, the VFO set is **replaced** (new ids, continuing
the sequence) and, if the profile has `lock:true`, the user lock is taken. Reply: the new state.
The only non-atomic corner is a device that snaps the sample rate to something a VFO cannot use.

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
