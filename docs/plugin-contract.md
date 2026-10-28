# Plugin contract v1

The contract between the plugin host (`superhet-pluginhost`) and out-of-process plugins, plus the
host's control API on the bus. Implemented by `crates/superhet-plugin-protocol` (Rust types,
validation, the contract test suite); JSON Schemas of every structure are generated from those
types and committed in `crates/superhet-plugin-protocol/schema/` (a test fails when they drift;
`UPDATE_SCHEMAS=1 cargo test -p superhet-plugin-protocol committed_schemas_match` rewrites them).
Decision record: ADR-0014 (execution model: ADR-0003). Reference plugin: `plugins/example-echo`.

Status: protocol version `1`. Plugin `plugin.protocol` must equal it; incompatible changes bump it.

## What this step covers

| Part | State |
|---|---|
| Manifest `plugin.toml`, validation, JSON Schema | done |
| Simple transport (stdin samples, stdout JSON Lines events, stderr log) | done, supervised by the host |
| Framed transport (frame codec, hello, samples in; event, audio, log, status, media, tune out; control in) | done (ADR-0016); voice frames are accepted by the codec but dropped by the host |
| Supervisor: spawn, health, restart with backoff, `error` after 5 crashes in 5 min | done |
| Control API on the bus (list, install from local path, instances, start, stop, restart, reconfigure) | done |
| Feeding VFO audio / IQ and the whole window from sdrd into `stdin` | done ("Feeding input") |
| Exclusive device claims | done: manifest, one claim per device in the host, `claim.*` in sdrd |
| Instances survive a host restart | done (PostgreSQL, `plugin_instances`) |
| Events durable and in the decoder history | done (JetStream `EVENTS`, stored by the worker) |
| Process-group kill on stop / crash | done |
| Admin API in `superhet-api` (`docs/api-plugins.md`) | done |
| First plugins: POCSAG/FLEX, APRS, rtl_433 (`docs/plugins-decoders.md`), DAB/DAB+ (`docs/plugins-dab.md`), bundled in the image and installed on first start | done |
| Web UI (admin console, VFO picker, events) | done, `docs/plugins-ui.md` |
| Container plugins, install from Git URL, resource limits | not yet (P3) |

## Manifest

TOML, at most 256 KiB, unknown keys are errors. Schema: `schema/manifest.schema.json`. Example:
`plugins/example-echo/plugin.toml`.

```toml
[plugin]
id = "org.superhet.decoder.pocsag"   # reverse DNS, >= 3 segments of [a-z0-9-], <= 128 chars
name = "POCSAG / FLEX"
version = "0.1.0"                    # semver
protocol = 1                         # must equal the host's contract version
license = "GPL-2.0-only"             # SPDX
type = "decoder"                     # decoder | codec | lookup | ui
description = "..."
homepage = "https://..."

[runtime]
kind = "process"                     # process | container (container: not started by the host)
transport = "simple"                 # simple | framed
command = ["bin/adapter", "--mode", "pocsag"]   # first element with "/" is relative to the plugin dir
heartbeat_timeout_s = 10             # optional watchdog, default: none (simple), 15 (framed); 0 = off

[input]                              # decoders and codecs
kind = "discriminator"               # iq | discriminator | audio | voice_frames
sample_rate = 22050
format = "s16le"                     # cf32 | cs16 | cu8 | s16le | f32le
bandwidth_hz = 12500                 # channel width the host cuts out

[device]                             # exclusive claim, see below
exclusive = false

[output]
events_schema = "schema/events.json" # JSON Schema (2020-12) for the whole event object
# audio = { sample_rate = 48000, channels = 1 }   # framed only
# voice_frames = ["tetra-acelp"]                  # framed only

# [config] schema = "schema/config.json"  JSON Schema (2020-12) of the instance config; the host
#          checks it on `reconfigure` and answers a violation with 400 and the offending field

# [codec]  frames = ["tetra-acelp"], pcm = { sample_rate = 8000, channels = 1 }   (codec plugins)
# [lookup] keys = ["callsign"], cache_ttl_s = 3600, rate_limit_per_min = 60       (lookup plugins)
# [ui]     widget = "ui/dist/widget.js"                                           (ui plugins)

[install]
build = ["scripts/build.sh"]         # shown to the admin before install; not run by this host yet
acknowledge = { text = "licence / patent notice", url = "https://..." }
```

Rules the host enforces (`Manifest::from_toml`): paths (`command[0]` when it contains `/`,
`events_schema`, `config.schema`, `widget`, build programs) stay inside the plugin directory (no absolute paths, no
`..`); decoders and codecs need `[input]`; `iq` needs a complex format and `discriminator` /
`audio` a real one; codec and lookup plugins and anything with audio or voice-frame output need the
framed transport; `[codec]` / `[lookup]` / `[ui]` belong to their type only; ui plugins have no
command.

### Exclusive device claims

`[device] exclusive = true` declares that an instance takes the whole device and receives the
complete window as IQ (ADS-B at 1090 MHz, DAB blocks, ADR-0013). Required then: `sample_rate`,
`input.kind = "iq"`, and either a fixed `center_hz` or a range `center_hz_min` / `center_hz_max`
the instance picks from with `freq_hz` of its start request (DAB: Band III blocks).
`input.sample_rate` must equal `device.sample_rate`. Only decoders can claim. The host refuses a
second exclusive instance on the same device (`conflict`); the daemon takes the claim as
described under "Feeding input".

## Feeding input

An instance with an `[input]` section is fed by the device daemon (`sdrd`) over the bus
(`superhet_bus::tap`, implemented in `crates/superhet-bus/src/tap.rs`). Which feed an instance
gets follows from its binding in the `start` request:

| Start request | Input | Tap |
|---|---|---|
| `device` + `vfo` | what the VFO demodulates, mono, squelch included: `input.kind = audio` and `discriminator` get the audio of the VFO's mode (use an NFM VFO for a discriminator decoder), converted to `input.sample_rate` as `s16le` / `f32le` | `audio` |
| `device` + `vfo`, `input.kind = iq` | complex baseband of the VFO (shifted to DC, filtered to the VFO bandwidth) resampled to `input.sample_rate` as `cu8` / `cs16` / `cf32` | `iq` |
| exclusive plugin: `device` + `freq_hz` (centre, if not fixed by the manifest) | the whole sample window at the device rate; the host first claims the device | `window` |
| `device` only, or nothing | no input; `stdin` stays open and idle | - |

`vfo` without `device` and `voice_frames` input are refused; a VFO the daemon does not know is
reported in `input_error` of the instance (and retried); a `device` without `vfo` is only
metadata (environment, event fields).

Mechanics, per process start of an instance:

- The host subscribes to `sh.v1.dev.<dev>.tap.<instance id>` and sends `ctl.tap.open` (and for
  exclusive plugins `ctl.claim.acquire` first) to `sh.v1.dev.<dev>.ctl.`. The daemon replies with
  the subject; it publishes the sample bytes in chunks of about 20 ms (at most 64 KiB); the payloads
  concatenated in order are the plugin's `stdin`. Requests and replies: `superhet_bus::tap`.
- Taps and claims are **leases**: the host repeats the requests every 4 s, the daemon drops them
  15 s after the last one (a crashed host, a restarted daemon and a lost bus heal themselves; the
  claim is released and the tuning restored the same way). At most 16 taps per device.
- **Claims** (ARCHITECTURE 3.4): the daemon remembers centre and rate, tunes to the claimed ones,
  adds a lock holder `{kind:"claim", id:<instance>}` (so nothing retunes; VFOs outside the new
  window are paused by the usual rules), and goes back to the remembered tuning on `claim.release`
  or lease expiry. One claim per device; a user lock or another claim answers `locked` /
  `conflict`. The state document carries `claim: {id, previous_center_hz, previous_sample_rate}`
  while a claim is held. A restarting daemon restores it from the snapshot (claimed tuning, claim
  lock, remembered previous tuning, a fresh 15 s lease); the host's next refresh (within 4 s) is
  an idempotent re-acquire of the same instance, so releasing it later still returns to the tuning
  from before the claim. If the host does not come back, the lease expires and the old tuning is
  restored.
- **Backpressure** never stalls the receiver or the bus: the daemon drops chunks when its publisher
  queue is full or the bus is down (`superhet_sdrd_dropped_frames_total{stream="tap"}` on
  `/metrics`), the host drops chunks when the plugin reads `stdin` too slowly (`input_dropped` in
  the instance state, first and every 1000th drop logged). A dropped chunk is a gap in the sample
  stream; plugins that care resynchronise on their own framing.
- When the process ends (any reason) the host closes `stdin`, releases the tap and the claim and
  kills the whole process group, so helpers a wrapped tool started cannot outlive it.

## Process environment and working directory

The process starts in the plugin directory, in its own process group, with an empty environment
plus `PATH`, `LANG`, `LC_ALL`, `TZ`, `TMPDIR` from the host (NATS and database credentials are
never passed on) and:

| Variable | Content |
|---|---|
| `SUPERHET_PLUGIN_ID`, `SUPERHET_INSTANCE_ID` | identity |
| `SUPERHET_PLUGIN_DIR` | plugin directory |
| `SUPERHET_DEVICE`, `SUPERHET_VFO`, `SUPERHET_FREQ_HZ` | binding of the instance (when set) |
| `SUPERHET_CONFIG` | instance config as one line of JSON (`null` when none) |
| `SUPERHET_SOCKET` | Unix socket path (framed transport only) |

## Simple transport

- `stdin`: raw samples in `input.format` (the host will feed them once sdrd integration lands). The
  host closes stdin to ask the plugin to end; the plugin must then exit by itself. After
  `stop_grace` (5 s) the host kills it.
- `stdout`: JSON Lines, one event per line, at most 64 KiB per line.
- `stderr`: log text, forwarded line by line to the host log with plugin and instance id.

### Events

```json
{"v":1,"type":"pocsag.message","ts":"2026-10-26T18:00:00.123Z","data":{"address":123456,"text":"..."}}
```

Envelope (`schema/event.schema.json`): `v` must be 1; `type` is 1-8 dotted segments of
`[a-z][a-z0-9-]*`, at most 64 characters; `ts` optional RFC 3339 (the host stamps its own receive
time anyway); `data` an object (default `{}`); no other keys. If the manifest names an
`events_schema`, the complete event object (envelope included, so the schema can switch on `type`)
must satisfy it. Invalid lines are dropped and counted (`events_invalid` in the instance state,
first 5 and every 1000th logged). Type `heartbeat` is reserved: a liveness marker that is neither
validated against `events_schema` nor forwarded.

Valid events are enriched (`HostEvent`, `schema/host-event.schema.json`: plugin id, instance id,
device, VFO, frequency, host time) and published on `sh.v1.events.<plugin>.<type...>` where
`<plugin>` is the plugin id with `.` replaced by `_` and the type contributes one token per
segment, e.g. `sh.v1.events.org_superhet_example_echo.echo.block`.

Events are published into the durable JetStream stream `EVENTS` (subjects `sh.v1.events.>`, file
storage, 7 days / 512 MiB, created by the host and the worker with the same configuration,
`superhet_bus::events_stream_config`) and acknowledged by the server; if the stream does not take
an event it is still published on core NATS for live subscribers. The worker's durable pull
consumer `worker-plugin-events` stores every event in `decoder_events` (`source` = plugin id,
`kind` = event type, `value` = the first string among `data.summary|text|message|value` or the
compact JSON of `data`, at most 256 characters; `meta` = `{instance, ts, data}`) and announces the
stored row on `sh.v1.events.core.decoder.<dev>`; it acknowledges after the insert, so a worker or
database outage loses nothing within the stream's retention. See `docs/api-decoder-events.md`.

## Framed transport (wire format)

Unix domain socket whose path the host passes in `SUPERHET_SOCKET`. Frames, little-endian:
`u32 payload_len | u8 kind | payload`, payload at most 4 MiB; an unknown kind or an oversized frame
ends the session.

| Kind | Dir | Payload |
|---|---|---|
| 1 `hello` | both | JSON: `HostHello` (host to plugin: protocol, ids, device, freq, config) then `PluginHello` (protocol, capabilities) |
| 2 `samples` | host to plugin | `u64 seq | u64 timestamp_ns | u32 tuning_epoch` + raw samples |
| 3 `control` | host to plugin | JSON (config change, flush) |
| 4 `event` | plugin to host | JSON event, same rules as a simple-transport line |
| 5 `audio` | plugin to host | PCM s16le interleaved at `output.audio` |
| 6 `voice_frame` | plugin to host | `u8 codec_len | codec | u64 call_id | u64 timestamp_ns` + codec bytes |
| 7 `log` | plugin to host | UTF-8 text |
| 8 `heartbeat` | both | empty |
| 9 `status` | plugin to host | JSON object: the plugin's complete current state (replaces the previous one, at most 256 KiB) |
| 10 `media` | plugin to host | `u8 key_len | key | u8 mime_len | mime | bytes`; key `[a-z0-9._-]` up to 64, at most 256 KiB, the host keeps the latest object per key (16 keys) |
| 11 `tune` | plugin to host | JSON `{"freq_hz":N}`: an exclusive plugin with `center_hz_min/max` moves its claimed window |

Implemented and tested as `framed::{encode, FrameDecoder, SamplesHeader, MediaHeader, VoiceFrameHeader}`
and run by the host (`crates/superhet-pluginhost/src/framed_session.rs`, end-to-end test
`tests/framed.rs` with a fake plugin).

Session (ADR-0016):

- The host creates a Unix socket in a private directory (`$TMPDIR/superhet-plugin-<instance>-<rand>/`, mode
  0700), passes it in `SUPERHET_SOCKET`, accepts one connection (20 s) and sends its `hello`; the
  plugin answers with its own within 10 s and must speak protocol 1. `stdin` stays open and idle: the
  host closes it (and the socket) to ask the plugin to end, so the plugin should exit on either.
- Input arrives in `samples` frames, never on `stdin`. `seq` counts frames from 0 and jumps over
  chunks the host had to drop (`input_dropped`), so a plugin sees losses; `tuning_epoch` counts the
  retunes the plugin requested (`tune`). Input starts after the hello, so nothing is dropped while
  the plugin starts.
- Every frame from the plugin counts as activity for the watchdog (`heartbeat_timeout_s`, default 15 s
  for framed plugins); the host sends a `heartbeat` every 5 s. A closed socket, a malformed frame or
  a host-only kind (`hello` twice, `samples`, `control`) ends the session and counts as a crash.
- `event` frames carry the same JSON as stdout lines (validated against `events_schema`). `audio`
  frames carry PCM s16le at `[output.audio]` (only 48000 Hz is supported by the host's Opus
  publisher); the host publishes Opus on the audio subject of the pseudo VFO
  `InstanceInfo.audio_vfo` (`docs/protocol-audio.md`).
- `tune`: allowed only for exclusive plugins with a centre range, inside it; the host moves the claim
  (`ctl.claim.acquire` with the new centre, which the daemon applies for the holder), updates
  `InstanceInfo.freq_hz` and stores it, so a host restart brings the instance back on that frequency.
  Anything else is logged and ignored.
- `status` and `media` are cleared when the session ends.

## Lifecycle and supervision

Instance states (`InstanceState`): `starting`, `running`, `backoff`, `error`, `stopped`.

- A process that exits on its own (any status), is killed from outside, fails to spawn, or stays
  silent longer than `heartbeat_timeout_s` is a crash. Any stdout line counts as activity.
- After a crash the instance waits `1 s * 2^(n-1)` (n = crashes inside the window, capped at 30 s)
  and starts again (`restarts` + 1).
- Crashes are counted over a sliding 5 minute window. The 5th crash inside it puts the instance
  into `error` and no further start is attempted; `last_error` says why. `restart` clears the
  history and starts again.
- `reconfigure` replaces the instance's config and restarts the process with it: the old process
  is stopped like on `stop` (stdin closed, killed after 5 s), the new one is spawned with the new
  `SUPERHET_CONFIG`. The instance keeps its id, binding, counters and event history; the restart
  is not a crash (no backoff, crash history cleared), an instance in `backoff` starts at once and
  one in `error` is revived. The new config is stored for host restarts. The host does not
  validate the config (the plugin does at start): a bad config shows up as `last_error` and the
  usual crash handling, and the next `reconfigure` fixes it.
- `stop` closes stdin, waits up to 5 s, kills, and removes the instance. On host shutdown all
  instances are stopped the same way.
- Instances are persistent: with `SUPERHET_DATABASE_URL` the host stores every started instance in
  `superhet.plugin_instances` (migration 0009) and deletes the row on `stop`; after a restart it
  starts all rows again with the same instance id and binding (a row whose plugin is no longer
  installed or whose binding is refused is logged and kept). Without a database instances live
  in memory only. Installed plugins are persistent too (`SUPERHET_PLUGIN_DIR`, default
  `/data/plugins`, layout `<dir>/<id>/<version>/`). Plugins shipped with the image
  (`SUPERHET_BUNDLED_PLUGIN_DIR`, default `/usr/share/superhet/plugins/<name>/`) are installed
  when the host starts; a marker file `.bundled` in the plugin directory records what was
  seeded, so a plugin an admin removed stays removed until the image ships a newer version.

## Control API (bus)

Request/reply JSON on `sh.v1.plugin.ctl.<cmd>`. Requests carry `"v":1`; replies are
`{"v":1,"ok":true,...}` or `{"v":1,"ok":false,"code":"...","error":"..."}` with `code` one of
`invalid`, `not_found`, `conflict`, `unsupported`, `internal`. Request and reply schemas:
`schema/ctl-*.schema.json`.

| Command | Body | Reply |
|---|---|---|
| `list` | `{"v":1}` | `{"plugins":[{"manifest":{...}}]}` (active version of each plugin) |
| `remove` | `{"v":1,"plugin_id":"..."}` | `{"plugin_id","versions":[...]}`; deletes every version from disk; `conflict` while an instance of it exists, `not_found` if unknown |
| `install` | `{"v":1,"path":"/host/dir"}` | `{"plugin":{"manifest":{...}}}`; validates, refuses symlinks, copies to `<dir>/<id>/<version>`; same id and version is `conflict`; build commands are not run |
| `instances` | `{"v":1}` | `{"instances":[InstanceInfo]}` |
| `get` | `{"v":1,"id":"i-ab12cd34"}` | `{"instance":InstanceInfo}` |
| `start` | `{"v":1,"plugin_id":"...","device":"dev0","vfo":1,"freq_hz":144800000,"config":{...}}` | `{"instance":InstanceInfo}`; `invalid` for a binding that does not fit the plugin, `unsupported` for framed / container / ui plugins, `conflict` for a second exclusive claim on a device |
| `stop` | `{"v":1,"id":"..."}` | `{"instance":InstanceInfo}` with state `stopped`; the instance is gone afterwards |
| `restart` | `{"v":1,"id":"..."}` | `{"instance":InstanceInfo}`; only for state `error` (`conflict` otherwise) |
| `control` | `{"v":1,"id":"...","message":{...}}` | `{}`; the object is sent to a framed plugin as a `control` frame (`conflict` while it has no session, `invalid` for a non-object or more than 64 KiB) |
| `status` | `{"v":1,"id":"..."}` | `{"plugin_id","device","state","freq_hz","audio_vfo","status":{...}\|null,"updated_unix_ms","media":[{"key","mime","size","revision"}]}` |
| `media` | `{"v":1,"id":"...","key":"slide.d210"}` | success: one line of JSON (`ok`, `key`, `mime`, `revision`), a newline, the object bytes; failure: the usual JSON error reply |
| `reconfigure` | `{"v":1,"id":"...","config":{...}}` | `{"instance":InstanceInfo}` with the new config; `config` is the complete new configuration (object or `null`), `invalid` for anything else, `not_found` for unknown ids |

Every state change of an instance is also published as `InstanceInfo` JSON on
`sh.v1.plugin.state` (core NATS). `InstanceInfo`: `id`, `plugin_id`, `device`, `vfo`, `freq_hz`,
`config`, `exclusive`, `state`, `pid`, `restarts`, `crashes_in_window`, `retry_in_ms`,
`last_error`, `events_ok`, `events_invalid`, `input_dropped`, `input_error`, `audio_vfo`
(plugins with `[output.audio]`), `updated_unix_ms` and the liveness counters `input_bytes` (sample bytes written to the plugin's stdin since the
instance was created, survives process restarts), `input_level_db` (smoothed RMS level of the input
in dBFS, `null` before the first chunk), `last_input_unix_ms`, `last_event_unix_ms` (`null` until
the first), `created_unix_ms`, `reconfigures` (`schema/instance-info.schema.json`). The counters
are not published on every change; poll `get` / `instances` for them. "Running but nothing
decoded" shows as `input_bytes` growing with `events_ok` at 0, "no input" as `input_bytes` standing
still.

The API edge (`docs/api-plugins.md`) is the only caller in production: install and remove are
`admin`, listing plugins and start / stop / restart / reconfigure are `operator` or `admin` (ROADMAP P2
criterion 8), declared in `access::RULES`, every change audited.

NATS permissions (`deploy/nats/nats.conf`): the `pluginhost` user subscribes to
`sh.v1.plugin.ctl.>` and `sh.v1.dev.*.tap.>`, publishes `sh.v1.events.>`, `sh.v1.plugin.state`,
`sh.v1.dev.*.ctl.tap.>`, `sh.v1.dev.*.ctl.claim.>` and `sh.v1.dev.*.audio.>` (plugin audio) and uses JetStream (`$JS.API.>`). The `api`
user publishes `sh.v1.plugin.ctl.>`; the `worker` user consumes the `EVENTS` stream. Existing
copies of `nats.conf` need the changed `pluginhost` entry.

## Contract test suite

`superhet_plugin_protocol::contract` runs a plugin as the host would and judges it:
`check_static(dir)` loads manifest and schema and checks the executable exists;
`run_simple(&package, input, config, timeout)` feeds stdin, closes it and returns a `Report`
(`run_simple_with_env` adds environment variables, e.g. a `PATH` that finds the plugin's helper
binary; the first-party golden tests use it);
`check(&report)` lists problems: every stdout line must pass the envelope and `events_schema`
checks, and the process must exit with status 0 by itself after stdin ended. Plugin authors call
these from an integration test; `crates/superhet-plugin-protocol/tests/contract_example_echo.rs` does
so for the reference plugin (it needs `python3`) and also proves the suite fails misbehaving
plugins (invalid lines, crash, hang).

```
cargo test -p superhet-plugin-protocol          # contract suite + schema drift check
cargo test -p superhet-pluginhost               # supervisor, control API; NATS test if SUPERHET_TEST_NATS_URL is set
```
