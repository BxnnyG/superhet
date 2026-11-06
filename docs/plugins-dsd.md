# DMR / dPMR / NXDN digital voice plugin (user-installed dsd-fme)

`org.superhet.decoder.dsd` (`plugins/dsd`), decision record ADR-0018. A non-exclusive,
framed-transport decoder around **`dsd-fme`**, the digital speech decoder fork by lwvmobile, for
DMR (MOTOTRBO, Hytera, ...), dPMR and NXDN (12.5 kHz). It is the first plugin of the "everything
encumbered stays external" side of ADR-0018.

**What this repository contains and what it does not.** The plugin manifest, the adapter
(`crates/superhet-plugin-adapters/src/dsd/`) and the web UI contain **no codec**. `dsd-fme`, and with
it the AMBE+2 / AMBE vocoder it links (`mbelib`), is **not** in the repository, the container image,
the CI or any release, and the plugin **never downloads it**. You install `dsd-fme` yourself; until
you do, the plugin shows "needs dsd-fme" and does nothing else.

## What it does

- **Taps a VFO.** Bind it to an NFM receiver on the channel (12.5 kHz raster). The VFO's FM audio at
  48 kHz is the 4FSK baseband `dsd-fme` reads from its standard input (`-i -`,
  `input.kind = discriminator`, `s16le`). Several receivers on different VFOs are separate
  instances.
- **Call events.** `dsd.call.start` when link control names a call and `dsd.call.end` when it is
  over (`reason`: `eos` terminator seen, `lost` no sign of life for 2 s, `new-call` another station
  took over the slot or channel, `stop` the plugin stopped). Fields: `mode` (`dmr`, `dpmr`, `nxdn`),
  `slot` (DMR 1 or 2; DMR can carry two calls at once), `src` (source ID), `dst` with `group`
  (talkgroup of a group call, radio ID of a private call), `cc` (DMR / dPMR colour code), `ran`
  (NXDN radio access number), `encrypted` and `algorithm`; the end event adds `duration_ms`,
  `audio_ms`, `muted`. The events are in the decoder history like every plugin event
  (`docs/api-decoder-events.md`); the schema is `plugins/dsd/schema/events.json`.
- **Voice audio.** The decoded 8 kHz voice that `dsd-fme` writes to its standard output (`-o -`) is
  resampled to 48 kHz mono and goes out as an ordinary plugin audio stream (pseudo VFO
  `InstanceInfo.audio_vfo`, `docs/protocol-audio.md`, "Plugin audio"); the web UI mixes it like a VFO
  and mutes the receiver's own audio (the raw 4FSK buzz) while the decoder runs.
- **Encrypted calls** (privacy flag in the DMR service options, or an encryption algorithm in the
  privacy header) are reported and, by default, **not played**; nothing is decrypted
  (`mute_encrypted: false` plays whatever `dsd-fme` produces). The encryption state can arrive after
  the call started (DMR sends the privacy header with the first frames), so the start event may still
  say `encrypted: false` for a call whose end event says `true`.
- **Status document** (`GET /api/v1/plugin-instances/{id}/status`): `tool` (`state` `ready`,
  `missing` or `failed`, the `path`, and for `missing` the places that were searched), `now` (the
  calls on the air, with `started_unix_ms`), `last` (the last finished call) and `calls` (count since
  the plugin started). The UI polls it once a second.

## Installing dsd-fme (you do this)

`dsd-fme` is third-party software under its own licences; build or obtain it from its origin
(https://github.com/lwvmobile/dsd-fme) according to its documentation. Superhet only needs the
executable, found in this order:

1. `SUPERHET_DSD_FME_PATH`: the full path of the executable (or of a directory containing it). The
   plugin host passes this one variable on to plugins (`docs/plugin-contract.md`).
2. `dsd-fme` on the plugin host's `PATH`.
3. `bin/dsd-fme` inside the installed plugin's directory (`/data/plugins/org.superhet.decoder.dsd/<version>/`).
   Updating the plugin installs a new version directory, so prefer 1 or 2 for anything permanent.

With the container setup the binary has to be visible to the `superhet-pluginhost` container **and**
runnable in it: the image is `debian:bookworm-slim`, and a dynamically linked `dsd-fme` brings its own
library needs (PulseAudio, ITPP, mbelib, ...). Two practical ways:

- A statically linked build (or one built against the image's glibc), mounted read-only, for
  example in a `compose.override.yaml`:
  `volumes: ["/opt/dsd-fme/dsd-fme:/opt/tools/dsd-fme:ro"]` and
  `environment: { SUPERHET_DSD_FME_PATH: /opt/tools/dsd-fme }` on the `superhet-pluginhost` service.
- Your own image `FROM` the Superhet image that installs `dsd-fme` and its libraries. That image is
  yours to build and keep; do not publish it.

Restart the decoder (or the plugin host) after installing; the "needs dsd-fme" notice goes away on the
next start. Without any installation the plugin is harmless: it keeps running, takes the samples it is
sent and reports `tool.state = "missing"`.

## Using it

Web UI: pick **DMR**, **dPMR** or **NXDN** in the **Digital** select of the tuning strip (operators
and admins). The receiver is set to NFM, 12.5 kHz, squelch off and the plugin starts on it with that
protocol. The first start asks you to confirm the plugin's notice (it repeats the legal points
below). The output dock shows:

- a **"now talking"** area: one line per call on the air (DMR: per slot), with source ID, target
  (`TG 91` for a group call, `ID 4000123` for a private one), slot and colour code or RAN,
  encryption and a running clock; between calls "Channel idle" and the last call. While `dsd-fme` is
  missing it explains what to install instead, with the places the plugin looked.
- a **call log**, newest first, one row per finished call. The log is also in the Decoders tab and
  exportable like all decoder events.

API: start with `POST /api/v1/plugin-instances` (`plugin_id`, `device`, `vfo`, `freq_hz`, and
`config`), change settings with `.../reconfigure`:

| Config key | Default | Effect |
|---|---|---|
| `protocol` | `auto` | `auto`, `dmr`, `dpmr` or `nxdn`: the frame-type switch passed to `dsd-fme` (`-fa`, `-fs`, `-fm`, `-fn`). Events of a protocol the instance is not set to are ignored. |
| `audio` | `true` | `false` runs `dsd-fme` with `-o null`: no voice, call events only. |
| `mute_encrypted` | `true` | do not play the audio of encrypted calls |
| `frame_flag` | none | overrides the frame-type switch (`-f` plus one or two letters or digits) for `dsd-fme` builds that spell it differently |

## Without the vocoder

The call metadata (who, to whom, slot, colour code, encryption flag) comes from link control that
`dsd-fme` decodes **before and independently of** the vocoder, so with `audio: false` the plugin
still produces every event. Whether a given `dsd-fme` build runs at all without `mbelib`, or prints
the same log with `-o null`, depends on that build; this was not verified (see below). The plugin
itself does not touch the vocoder in any case: it passes `dsd-fme` samples and reads its output.

## Legal notes (read before you use it)

- **Patents.** The AMBE+2 / AMBE vocoders are covered by patents held by DVSI in many countries, and
  `mbelib`, the open implementation inside `dsd-fme`, has an unclear patent status. That is why this
  project does not ship, build or link any of it (ADR-0005, ADR-0018) and why the plugin asks you to
  install `dsd-fme` yourself. Hardware vocoders (DV3000 / ThumbDV) are the licensed route; an adapter
  for them may come later.
- **Receive only.** The plugin cannot transmit. It does not decrypt: encrypted calls are flagged
  and withheld, nothing more.
- **Your responsibility.** Whether you may build or run `dsd-fme`, and whether you may listen to and
  decode digital voice radio, depends on the country and on what is received. Amateur radio and
  licence-free services are one thing, commercial and public-safety systems another. In Germany, for
  example, section 5 TKG (Telekommunikationsgesetz) protects the content of telecommunications and
  forbids using or passing on content that was not meant for you, which can reach what you overhear
  when decoding voice; that is mentioned here as a pointer, not as legal advice. The project takes no
  position and keeps the responsibility with you.
- **Personal data.** Source IDs, talkgroups and call times are kept in the decoder history until the
  retention period ends (`SUPERHET_EVENT_RETENTION_DAYS`). Do not share what you hear without
  checking the rules where you live.

## Limits you will notice

- **Tune accurately.** A 12.5 kHz channel with 4FSK leaves little margin. An RTL-SDR with an
  uncorrected clock is several kHz off at 430 MHz; click-tune onto the signal.
- **One protocol per instance.** `auto` asks `dsd-fme` to detect; for a known channel set the
  protocol, it locks faster and avoids false calls.
- **NXDN is the 12.5 kHz variant** (NXDN96, 9600 baud). NXDN48 (6.25 kHz) needs a narrower filter
  than the 12.5 kHz this plugin asks for and a different `frame_flag`; it is not a tested path.
- **DMR Tier III / trunked systems:** only what the channel carries is decoded; there is no control
  channel following or talkgroup hunting.
- **Listeners can hear it.** The instance and its audio stream are announced to listeners (and to
  guests while the guest policy shows decoder output) through the read-only
  `GET /api/v1/plugin-streams` (no configuration); the web UI then shows the call log and mixes the
  voice like any VFO, without start / stop / settings. Only operators start and control it.
- **The log layout of `dsd-fme` is not a stable interface.** The adapter reads its human-oriented
  console log (see below). A release that changes the wording can make calls disappear until
  `lines.rs` is adapted; unrecognised lines are mentioned in the plugin log to help with that.

## How it is built

`superhet-plugin-adapter dsd` (`crates/superhet-plugin-adapters/src/dsd/`):

- `lines.rs` parses the log `dsd-fme` writes on stderr: ANSI colours are stripped, then keywords and
  `KEY=value` / `Key: value` pairs are looked for (`Sync: +DMR ... Color Code=01`, `Slot 1`,
  `TGT=` / `Target:`, `SRC=` / `Source:`, `FLCO=`, `SVC=`, `ALG ID:`, terminators). The service
  options byte (bit 6, privacy) and the link control opcode (0 group, 3 unit to unit) are decoded
  per the DMR specification.
- `call.rs` is the pure call state machine (one call per DMR slot, start, refine, end, mute).
- `mod.rs` locates and starts the tool (via `stdbuf -o0`), feeds it the `samples` frames, resamples
  its 8 kHz voice and sends events, status and audio to the host. A tool that dies ends the plugin
  with an error (the last lines of its output are in the message), so the host's supervisor
  restarts it. A missing tool does **not** end the plugin: it stays up and says so in its status.

## Tests

- `lines.rs` / `call.rs` / `mod.rs` unit tests: line parsing (DMR, dPMR, NXDN, colours, private and
  group calls, privacy bit, algorithm, terminators, chatter), the call state machine, configuration
  and the search for the tool.
- `tests/dsd_golden.rs`: **golden parser test.** Transcripts of the tool's log
  (`tests/dsd/transcripts/*.log`: two DMR slots with a group and a private call, an encrypted DMR
  call with ANSI colours, NXDN with a release and dPMR ending by silence) must give exactly the
  events in `*.expected.json`, and every event must satisfy the events schema.
- `tests/dsd_plugin.rs` against a fake host with a **stand-in for dsd-fme** (`tests/dsd/fake-dsd-fme`,
  needs `python3`, replays the transcripts and writes a sine as "voice"): calls on both slots
  announced, played and closed, the tool's command line, an encrypted call withheld (and played
  with `mute_encrypted: false`), a vanished transmitter, `audio: false`, the tool named by
  `SUPERHET_DSD_FME_PATH` only, a dying tool, **the missing-tool state** (status `missing`, the
  places searched, the plugin stays up and ends cleanly), and the manifest.
- `plugins/dsd` schemas: `crates/superhet-plugin-protocol/tests/shipped_plugin_configs.rs`.
- Web (vitest): model, event rendering, "now talking" with two slots, the missing-tool notice, the
  dock, the three picker modes of one plugin.

## Not verified

No real `dsd-fme` was available when this was written, and no DMR / dPMR / NXDN transmission was
received. Two things are therefore assumptions that need a first run with the real tool:

1. **The log layout.** The transcripts in `tests/dsd/transcripts` are written by hand in the style of
   what `dsd-fme` is known to print (colour code on the `Sync:` line, `Slot n`, `TGT=` / `SRC=`,
   `FLCO` / `FID` / `SVC`, `ALG ID`). They are **not captures** of a particular release. The parser is
   keyword-based to tolerate differences, but a first run with a real recording should be turned into a
   transcript and a test.
2. **The command line and the audio format.** `-i -` (stdin), `-o -` (stdout) and the frame-type
   switches `-fs` / `-fm` / `-fn` / `-fa` match the option list in the tool's `dsd_main.c`, not yet a running build; the
   voice output is 8 kHz s16le, two channels (one per slot) for DMR and auto, mono for dPMR and NXDN (checked against `dsd_main.c`, not against a running build). Check `dsd-fme -h` of your build; `frame_flag`
   overrides the switch if the spelling differs. If your build writes another rate, the audio will
   sound wrong and the adapter needs a format option.

Open: a real DMR recording (an amateur repeater, DMR hotspot or a DMR-MARC / BrandMeister network
channel on 70 cm) through an RTL-SDR, to confirm call start, slot, IDs, colour code and the audio.
