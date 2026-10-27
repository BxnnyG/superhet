# Plugin API

REST endpoints to manage plugins and their instances. They are a thin, audited edge in front of
the plugin host's bus API (`docs/plugin-contract.md`, ADR-0014 / ADR-0015): the API checks the
role and the Origin, validates the shape of the request, forwards it to
`sh.v1.plugin.ctl.<cmd>` and relays the host's answer. Without a bus every route answers
`503 {"code":"unavailable"}`; if the plugin host is not running, `503` with a message saying so.

All routes need a session; mutations also check the Origin and, for bodies, `Content-Type:
application/json`. Roles (`access::RULES`, `docs/api-users.md`):

| Route | Role |
|---|---|
| `GET /api/v1/plugins`, `GET /api/v1/plugin-instances[/{id}]` | `operator` |
| `POST /api/v1/plugin-instances`, `POST .../{id}/stop`, `POST .../{id}/restart`, `POST .../{id}/control` | `operator` |
| `GET .../{id}/status`, `GET .../{id}/media/{key}` | everybody who may see the decoder history (listener, guests per guest policy) |
| `POST /api/v1/plugins`, `DELETE /api/v1/plugins/{id}` | `admin` |

`listener` and guests get 403 everywhere. (Listing plugins is `operator` rather than `admin`
because an operator needs the plugin ids and manifests to start an instance.)

Errors are `{"v":1,"ok":false,"code":"...","error":"..."}`: `invalid` 400 (also `unsupported`),
`not_found` 404, `conflict` 409, `unavailable` 503, `timeout` 504, anything else 502.

## Plugins

`GET /api/v1/plugins` – `{"v":1,"ok":true,"plugins":[{"manifest":{...}}]}`, the active (highest)
version of every installed plugin with its full manifest (`schema/manifest.schema.json`). The
image brings POCSAG/FLEX, APRS and rtl_433 (`docs/plugins-decoders.md`); the host installs them on
its first start, so they are listed without any install call.

`POST /api/v1/plugins` `{"path":"/dir/with/plugin.toml"}` – install a plugin from a directory
**on the host running the plugin host** (inside its container: a mounted volume). The host
validates the manifest, refuses symbolic links and copies the tree to
`<plugin dir>/<id>/<version>`; build commands are shown in the manifest but not run. `201` with
`{"plugin":{"manifest":{...}}}`; `409 conflict` when that id and version is installed.

`DELETE /api/v1/plugins/{id}` – remove every version. `204`; `404` when not installed; `409`
while an instance of the plugin exists (stop it first).

## Instances

`GET /api/v1/plugin-instances` – `{"instances":[InstanceInfo]}`;
`GET /api/v1/plugin-instances/{id}` – `{"instance":InstanceInfo}` (`404` for unknown ids).
`InstanceInfo`: `id`, `plugin_id`, `device`, `vfo`, `freq_hz`, `config`, `exclusive`, `state`
(`starting|running|backoff|error|stopped`), `pid`, `restarts`, `crashes_in_window`, `retry_in_ms`,
`last_error`, `events_ok`, `events_invalid`, `input_dropped`, `input_error`, `updated_unix_ms`.
The same documents stream on the bus subject `sh.v1.plugin.state` (no WebSocket yet: poll).

`POST /api/v1/plugin-instances` `{"plugin_id", "device"?, "vfo"?, "freq_hz"?, "config"?}` – start
an instance, `201` with `{"instance":InstanceInfo}`. Unknown fields are `400`.
- `device` + `vfo`: the plugin is fed with what that VFO demodulates (audio plugins, converted to
  the rate and format of the manifest) or with its baseband (IQ plugins). The VFO must exist; it
  can be any VFO of the device (also other users' VFOs: operators see all).
- exclusive plugin: `device` and, unless the manifest fixes it, `freq_hz` (the centre the device
  is tuned to while the instance runs; it must lie inside the manifest's range). The device is
  claimed, other VFOs outside the window are paused, the previous tuning returns on stop. `409`
  when another exclusive instance or a user lock holds the device.
- `device` only: informational binding, no input. `vfo` without `device`: `400`.
- The instance is stored and starts again after a plugin host restart.

`POST /api/v1/plugin-instances/{id}/stop` – stops the process (stdin closed, killed after 5 s,
whole process group), forgets the instance. `200` with the final `InstanceInfo` (`stopped`).
`POST /api/v1/plugin-instances/{id}/restart` – only for state `error`; clears the crash history
and starts again (`409` otherwise).

`InstanceInfo` has one more field for plugins with audio output: `audio_vfo`, the pseudo VFO id of the instance's audio stream on the audio WebSocket (`docs/protocol-audio.md`).

## Framed plugins: status, media, control

`GET /api/v1/plugin-instances/{id}/status` – `{"v":1,"ok":true,"plugin_id","device","state","freq_hz","audio_vfo","status":{...}|null,"updated_unix_ms","media":[{"key","mime","size","revision"}]}`. `status` is the document the plugin last reported (its shape is the plugin's, DAB: `docs/plugins-dab.md`), `null` before the first one. Everybody who sees decoder output may read it, which is also how a listener finds out that DAB holds the device without being allowed to list instances.

`GET /api/v1/plugin-instances/{id}/media/{key}` – the latest binary object the plugin stored under `key` (a DAB slide `slide.<sid>`). Only `image/jpeg|png|gif|webp` are labelled as images, everything else is `application/octet-stream`; `X-Content-Type-Options: nosniff`, `ETag` is the revision, `Cache-Control: private, no-cache`. `404` for an unknown key.

`POST /api/v1/plugin-instances/{id}/control` `{...}` – the body (a JSON object, at most 16 KiB) is sent to the plugin as a control message (`docs/plugins-dab.md` lists the DAB commands). `409` while the plugin has no session.

## Audit

`plugin.install` (target: plugin id, details: path, version), `plugin.remove` (plugin id,
versions), `plugin.start` (instance id; plugin id, device, vfo, freq_hz), `plugin.stop` and
`plugin.restart` (instance id), `plugin.control` (instance id; details: the message). Refusals by the host (conflict, not found, invalid) are recorded
with outcome `denied`, other failures with `error`. Role refusals (403) are not recorded by the
route (like the other routes). Plugin crashes are the host's to report (`sh.v1.audit.>`).

## Events

What instances emit lands in the decoder history (`docs/api-decoder-events.md`): `source` is the
plugin id, `kind` the event type; filter with `source=` and `kind=`. The live feed
`/api/v1/ws/events?dev=` carries them too. The web UI for all of this is described in
`docs/plugins-ui.md`.
