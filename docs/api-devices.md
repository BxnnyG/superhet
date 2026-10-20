# Device state and tuning (v1)

Runtime control of an SDR: `superhet-sdrd` serves control requests on the bus and publishes the
device state; `superhet-api` exposes both over REST. Architecture background: `docs/ARCHITECTURE.md`
sections 2 and 3. Not covered yet: VFOs, tuning locks/claims, profiles.

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

`{"v":1,"device","driver","tuner"?,"center_hz","sample_rate","gain":{"mode":"auto"|"manual","db":null|number}|null,"ppm","bias_tee","tuning_epoch","gains_db":[...]}`

`gain` is `null` for devices without gain control (file replay); `gains_db` lists the selectable
manual steps. `tuner` is omitted when unknown.

## Tuning epoch

Every applied change of centre, sample rate, gain or ppm increments `tuning_epoch` (bias tee does
not); repeating current values changes nothing. Each IQ block carries the epoch it was captured
in. The RTL-SDR backend discards the samples around a change (the hardware is being reconfigured,
two USB buffers may still hold old data); the spectrum pipeline resets its averaging and overlap
state when the epoch changes and drops the first block. Spectrum frames take `center_hz` and
`sample_rate` from the block, so they carry the new tuning from the first frame after the change;
the frame wire format is unchanged.

## Devices

- `file`: centre frequency is only a label and can be changed freely; sample rate, gain, ppm and
  bias tee are fixed by the recording (requests repeating the current rate are accepted).
- `rtlsdr` (feature `rtlsdr`): centre must fit 32 bit, rate 225001-300000 or 900001-3200000 Hz,
  ppm within +-1000. Changes are applied with librtlsdr while `read_async` keeps running (the same
  pattern `rtl_tcp` uses); a rate change re-applies the centre frequency afterwards. Not yet
  verified on hardware: run `cargo test -p superhet-device --features rtlsdr -- --ignored`.
