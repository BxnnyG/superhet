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
| Framed transport wire format (frame codec, hello, samples / voice-frame headers) | types and tests only; the host refuses to start framed plugins (`unsupported`) |
| Supervisor: spawn, health, restart with backoff, `error` after 5 crashes in 5 min | done |
| Control API on the bus (list, install from local path, instances, start, stop, restart) | done |
| Feeding IQ / audio from sdrd into `stdin` | not yet (next step) |
| Exclusive device claims | declared in the manifest; host allows one exclusive instance per device; sdrd claim command not yet |
| Container plugins, install from Git URL, resource limits | not yet (P3) |
| Admin API in `superhet-api`, UI | not yet |

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

# [codec]  frames = ["tetra-acelp"], pcm = { sample_rate = 8000, channels = 1 }   (codec plugins)
# [lookup] keys = ["callsign"], cache_ttl_s = 3600, rate_limit_per_min = 60       (lookup plugins)
# [ui]     widget = "ui/dist/widget.js"                                           (ui plugins)

[install]
build = ["scripts/build.sh"]         # shown to the admin before install; not run by this host yet
acknowledge = { text = "licence / patent notice", url = "https://..." }
```

Rules the host enforces (`Manifest::from_toml`): paths (`command[0]` when it contains `/`,
`events_schema`, `widget`, build programs) stay inside the plugin directory (no absolute paths, no
`..`); decoders and codecs need `[input]`; `iq` needs a complex format and `discriminator` /
`audio` a real one; codec and lookup plugins and anything with audio or voice-frame output need the
framed transport; `[codec]` / `[lookup]` / `[ui]` belong to their type only; ui plugins have no
command.

### Exclusive device claims

`[device] exclusive = true` declares that an instance takes the whole device and receives the
complete window as IQ (ADS-B at 1090 MHz, DAB blocks, ADR-0013). Required then: `sample_rate`,
`input.kind = "iq"`, and either a fixed `center_hz` or a range `center_hz_min` / `center_hz_max`
the instance config picks from (DAB: Band III blocks). Only decoders can claim. The daemon side
(snapshot state, retune, pause VFOs, restore on release, ARCHITECTURE 3.4) is not implemented yet;
today the host only refuses a second exclusive instance on the same device (`conflict`).

## Process environment and working directory

The process starts in the plugin directory, in its own process group, with an empty environment
plus `PATH`, `LANG`, `LC_ALL`, `TZ`, `TMPDIR` from the host (NATS and database credentials are
never passed on) and:

| Variable | Content |
|---|---|
| `SUPERHET_PLUGIN_ID`, `SUPERHET_INSTANCE_ID` | identity |
| `SUPERHET_PLUGIN_DIR` | plugin directory |
| `SUPERHET_DEVICE`, `SUPERHET_FREQ_HZ` | binding of the instance (when set) |
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
device, frequency, host time) and published on `sh.v1.events.<plugin>.<type...>` where `<plugin>`
is the plugin id with `.` replaced by `_` and the type contributes one token per segment, e.g.
`sh.v1.events.org_superhet_example_echo.echo.block`. Core NATS today; the durable `EVENTS` stream
and persistence come with the first real decoder plugin.

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

Implemented and tested as `framed::{encode, FrameDecoder, SamplesHeader, VoiceFrameHeader}`; the
host does not run framed plugins yet.

## Lifecycle and supervision

Instance states (`InstanceState`): `starting`, `running`, `backoff`, `error`, `stopped`.

- A process that exits on its own (any status), is killed from outside, fails to spawn, or stays
  silent longer than `heartbeat_timeout_s` is a crash. Any stdout line counts as activity.
- After a crash the instance waits `1 s * 2^(n-1)` (n = crashes inside the window, capped at 30 s)
  and starts again (`restarts` + 1).
- Crashes are counted over a sliding 5 minute window. The 5th crash inside it puts the instance
  into `error` and no further start is attempted; `last_error` says why. `restart` clears the
  history and starts again.
- `stop` closes stdin, waits up to 5 s, kills, and removes the instance. On host shutdown all
  instances are stopped the same way.
- Instances are not persisted yet: after a host restart they must be started again. Installed
  plugins are (`SUPERHET_PLUGIN_DIR`, default `/data/plugins`, layout `<dir>/<id>/<version>/`).

## Control API (bus)

Request/reply JSON on `sh.v1.plugin.ctl.<cmd>`. Requests carry `"v":1`; replies are
`{"v":1,"ok":true,...}` or `{"v":1,"ok":false,"code":"...","error":"..."}` with `code` one of
`invalid`, `not_found`, `conflict`, `unsupported`, `internal`. Request and reply schemas:
`schema/ctl-*.schema.json`.

| Command | Body | Reply |
|---|---|---|
| `list` | `{"v":1}` | `{"plugins":[{"manifest":{...}}]}` (active version of each plugin) |
| `install` | `{"v":1,"path":"/host/dir"}` | `{"plugin":{"manifest":{...}}}`; validates, refuses symlinks, copies to `<dir>/<id>/<version>`; same id and version is `conflict`; build commands are not run |
| `instances` | `{"v":1}` | `{"instances":[InstanceInfo]}` |
| `get` | `{"v":1,"id":"i-ab12cd34"}` | `{"instance":InstanceInfo}` |
| `start` | `{"v":1,"plugin_id":"...","device":"dev0","freq_hz":144800000,"config":{...}}` | `{"instance":InstanceInfo}`; `unsupported` for framed / container / ui plugins, `conflict` for a second exclusive claim on a device |
| `stop` | `{"v":1,"id":"..."}` | `{"instance":InstanceInfo}` with state `stopped`; the instance is gone afterwards |
| `restart` | `{"v":1,"id":"..."}` | `{"instance":InstanceInfo}`; only for state `error` (`conflict` otherwise) |

Every state change of an instance is also published as `InstanceInfo` JSON on
`sh.v1.plugin.state` (core NATS). `InstanceInfo`: `id`, `plugin_id`, `device`, `freq_hz`, `config`,
`exclusive`, `state`, `pid`, `restarts`, `crashes_in_window`, `retry_in_ms`, `last_error`,
`events_ok`, `events_invalid`, `updated_unix_ms` (`schema/instance-info.schema.json`).

NATS permissions (`deploy/nats/nats.conf`): the `pluginhost` user subscribes to
`sh.v1.plugin.ctl.>` and publishes `sh.v1.events.>` and `sh.v1.plugin.state`. The `api` user has no
plugin rights yet; the admin endpoints (install/remove `admin`, start/stop/restart `operator` or
`admin`, ROADMAP P2 criterion 8) must grant them when they are added, and declare their roles in
`access::RULES`. Existing copies of `nats.conf` need the changed `pluginhost` entry.

## Contract test suite

`superhet_plugin_protocol::contract` runs a plugin as the host would and judges it:
`check_static(dir)` loads manifest and schema and checks the executable exists;
`run_simple(&package, input, config, timeout)` feeds stdin, closes it and returns a `Report`;
`check(&report)` lists problems: every stdout line must pass the envelope and `events_schema`
checks, and the process must exit with status 0 by itself after stdin ended. Plugin authors call
these from an integration test; `crates/superhet-plugin-protocol/tests/contract_example_echo.rs` does
so for the reference plugin (it needs `python3`) and also proves the suite fails misbehaving
plugins (invalid lines, crash, hang).

```
cargo test -p superhet-plugin-protocol          # contract suite + schema drift check
cargo test -p superhet-pluginhost               # supervisor, control API; NATS test if SUPERHET_TEST_NATS_URL is set
```
