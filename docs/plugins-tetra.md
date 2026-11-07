# TETRA receive plugin (TMO metadata and voice with your own ETSI codec)

`org.superhet.decoder.tetra` (`plugins/tetra`), decision record ADR-0021 (and ADR-0004, ADR-0018). A
non-exclusive, framed-transport decoder for **TETRA trunked radio, TMO downlink**: the cell, its
neighbours, calls with subscriber and group numbers, short data and status messages, the encryption
flags, and the voice of clear-mode calls. It is built for **your own TETRA equipment** (a cell you
run, a test network); public-safety and other organisations' networks are not what it is for (see the
legal notes).

**What this repository contains and what it does not.** The plugin, its adapter
(`crates/superhet-plugin-adapters/src/tetra/`), its own pi/4-DQPSK demodulator and the web UI contain
**no ETSI codec code and no encryption code**. `tetra-rx` of osmo-tetra-sq5bpf (AGPL-3.0) is built
into the image as a separate process. The **ETSI TETRA speech codec** (EN 300 395-2 reference
code, `cdecoder` and `sdecoder`) is **not** in the repository, the image, the CI or any release and the
plugin **never downloads or builds it**. You obtain and install it yourself; until you do, the plugin
decodes everything else and the panel says "voice needs the ETSI codec".

## What it does

- **Taps a VFO as complex baseband.** Bind it to a receiver on the downlink carrier. The VFO is a
  25 kHz channel; the host hands the plugin its IQ at 144 kHz (`cf32`, 8 samples per symbol). TETRA is
  pi/4-DQPSK at 18 ksym/s (36 kbit/s) in 25 kHz channels. The web UI sets the VFO to NFM, 25 kHz,
  squelch off; the NFM audio itself is noise and is muted while the decoder runs.
- **Demodulates itself** (`phy.rs`): root-raised-cosine matched filter (alpha 0.35), feed-forward
  symbol timing, differential detection, the carrier offset taken out of the phase steps (capture
  range about plus/minus 2.25 kHz, the point where a phase step becomes ambiguous). The bits go to
  `tetra-rx`, which finds burst and frame sync, decodes MAC/LLC/MLE/CMCE and reports over a local UDP
  socket.
- **Events** (schema `plugins/tetra/schema/events.json`, in the decoder history like every plugin
  event, `docs/api-decoder-events.md`):
  - `tetra.cell`: MCC, MNC, colour code, location area, downlink and uplink carrier, `encryption`
    (`none`, `air-interface`, `unknown`: what the system information announces),
    `encrypted_traffic_seen`, the BS `services` (voice, circuit-mode data, air interface encryption,
    ...). Sent when the cell is first seen and whenever something about it changes.
  - `tetra.neighbour`: neighbour cells from the network broadcast (carrier, MCC, MNC, LA).
  - `tetra.call.start` / `tetra.call.end`: `cid` (call identifier), `usage` (traffic channel), `dst`
    (called party: the group (GSSI) of a group call), `src` (calling party named by the set-up),
    `talker` (who got the right to transmit last), `encrypted`; the end event adds `duration_ms`,
    `voice_ms` (voice on the air, 60 ms per traffic slot), `audio_ms` (voice handed to the codec),
    `muted`, `reason` (`release`, `lost` after 12 s without a sign of life, `new-call`, `stop`) and the
    network's notice text.
  - `tetra.sds`: short data messages (source, destination, protocol identifier, text of text
    messages, the decoder's one-line description). `tetra.status`: status values.
  - Repeats of the same SDS or status within 5 s are one event (the cell repeats them).
- **Voice.** The traffic-channel blocks `tetra-rx` sends go through the ETSI `cdecoder` and
  `sdecoder` (two processes joined by a pipe); the 8 kHz speech is resampled to 48 kHz mono and goes out
  as an ordinary plugin audio stream (pseudo VFO `InstanceInfo.audio_vfo`, `docs/protocol-audio.md`,
  "Plugin audio"). Listeners can hear it like M17/DMR (`GET /api/v1/plugin-streams`).
- **Encryption.** A call is flagged `encrypted` when the MAC header of one of its PDUs carries an
  encryption mode other than clear. By default (`mute_encrypted`) its audio is **not played**. **Nothing
  is decrypted, no keys are handled, no TEA code is present**, and the content of encrypted PDUs is not
  interpreted (`tetra-rx` runs without `-e`), so the parties of an encrypted call are often unknown.
  What the cell broadcasts about encryption is shown as it is (air interface encryption on or off);
  the *class* (1/2/3) and the algorithm (TEA1..4) are **not** distinguished: the decoder does not
  expose them.
- **Status document** (`GET /api/v1/plugin-instances/{id}/status`): `tool` (`tetra-rx`: `ready`,
  `missing`, `failed`), `codec` (`ready`, `missing` with the places searched, `failed`, `disabled`),
  `signal` (`sync`, the measured `offset_hz`), `cell`, `neighbours`, `now` (calls on the air), `last`,
  `calls`, `messages`. The UI polls it once a second.

## Installing the ETSI codec (you do this, for voice)

The speech codec is third-party reference code published by ETSI under its own IPR terms; it is
not under a free licence and must not be redistributed by this project. Obtain it from ETSI
(EN 300 395-2, the reference C source in the download `en_30039502v010301p0.zip`; see the
Osmocom TETRA wiki page "Speech Codec" for the procedure that the osmo-tetra community documents,
and mind the terms you accept there), build `cdecoder` and `sdecoder` yourself, and put both
executables into one directory. Superhet only needs that directory:

1. `SUPERHET_TETRA_CODEC_DIR` names it. The plugin host passes this one variable (and
   `SUPERHET_TETRA_RX_PATH`) on to plugins (`docs/plugin-contract.md`). There is no other search
   path: not `PATH`, not the plugin directory.
2. The directory must contain executable files `cdecoder` and `sdecoder`. They are started as
   `cdecoder /dev/stdin /dev/stdout | sdecoder /dev/stdin /dev/stdout`; wrap them in scripts of the same
   names if your builds take their arguments differently, or buffer too much (below).

With the container setup the directory has to be visible to the `superhet-pluginhost` container and
the programs runnable in it (`debian:bookworm-slim`; build them static or against bookworm's glibc), for
example in a `compose.override.yaml`:

    services:
      superhet-pluginhost:
        environment:
          SUPERHET_TETRA_CODEC_DIR: /opt/tetra-codec
        volumes:
          - /home/you/tetra-codec:/opt/tetra-codec:ro

Restart the decoder after installing; "voice needs the ETSI codec" goes away on the next start. Without
the codec the plugin is useful anyway: it keeps running, decodes everything else and reports
`codec.state = "missing"`. `audio: false` in the instance config starts without the codec on purpose.

`tetra-rx` is part of the image (`/usr/bin/tetra-rx`). Outside the image, put it on `PATH` or point
`SUPERHET_TETRA_RX_PATH` at it (a file or a directory containing it); the plugin sets
`TETRA_HACK_PORT` and friends itself.

## Using it

Web UI: pick **TETRA** in the **Digital** select of the tuning strip (operators and admins) after
tuning onto the downlink carrier of the cell; the receiver is set to NFM, 25 kHz, squelch off and the
plugin starts on it. The first start asks you to confirm the plugin's notice (it repeats the legal
points below). The output dock shows:

- the **cell** (MCC/MNC, LA, colour code, carrier), whether it is **in sync**, the measured carrier
  offset, the number of neighbour cells and an encryption badge when the cell announces air interface
  encryption or encrypted traffic was seen;
- the **calls on the air** with the talking party, the group, the traffic channel, encryption and a
  clock, and "voice needs the ETSI codec" while the codec is missing;
- a **log** of finished calls, short messages and status values (cell and neighbour events are in the
  Decoders tab).

API: start with `POST /api/v1/plugin-instances` (`plugin_id`, `device`, `vfo`, `freq_hz`, `config`);
change settings with `.../reconfigure`:

| Config key | Default | Effect |
|---|---|---|
| `audio` | `true` | `false` does not start the codec: signalling only |
| `mute_encrypted` | `true` | do not play the audio of calls flagged as encrypted |
| `invert` | `false` | conjugate the input, for a receiver chain that mirrors the spectrum (if nothing syncs on a signal you know is there, try it) |

## Legal notes (read before you use it)

- **Receive only.** The plugin cannot transmit. It does not decrypt: encrypted calls are flagged and
  withheld, nothing more; no keys, TEA code or key-recovery of any kind is in this project.
- **Your own network.** The design target is your own cell and equipment. Networks of authorities and
  organisations with security tasks (BOS) and commercial operators are not yours to listen to, and
  their traffic is largely encrypted.
- **Your responsibility.** Whether you may receive and decode TETRA, and whether you may listen to
  voice, depends on the country and on what is received. In Germany, for example, section 5 TKG
  (Telekommunikationsgesetz) protects the content of telecommunications and forbids using or passing on
  content that was not meant for you; that is mentioned here as a pointer, not as legal advice. The
  project takes no position and keeps the responsibility with you.
- **The codec.** The ETSI reference code is published under ETSI's IPR terms and is part of a standard
  with essential patents. That is why this project does not ship, download, build or link it
  (ADR-0005, ADR-0018) and why the plugin asks you to install it yourself. Whether you may build or run
  it depends on those terms and on where you live; read what you accept when you download it.
- **Personal data.** Subscriber and group numbers, call times and short messages are kept in the
  decoder history until the retention period ends (`SUPERHET_EVENT_RETENTION_DAYS`). Do not share what
  you hear without checking the rules where you live.

## Limits you will notice

- **TMO downlink only.** Direct mode (DMO: the MTM as gateway, repeater or hotspot) is **not
  decoded**: osmo-tetra has no DMO support and no maintained open-source DMO decoder is known
  (ADR-0004). On a DMO channel the plugin simply never finds a cell. Uplink is not received either, so
  you see the base station's side of calls.
- **Tune accurately and give it a signal.** The demodulator corrects about plus/minus 2.25 kHz, which an
  RTL-SDR with an uncorrected clock at 400 MHz can exceed (1 ppm is 400 Hz). Use the device's ppm
  setting. Weak signals fail without warning: there is no soft decision, the decisions are hard, and
  `tetra-rx` needs several error-free bursts to sync.
- **Registrations are not reported as events.** The upper layers of `tetra-rx` print them to its
  console, but the datagram interface the adapter uses does not carry them. Cell, neighbours, call
  set-up and release, transmit grants, short data and status are what is reported.
- **A call is known by its set-up.** If the receiver tunes in during a call the call starts from its
  first voice block, without parties until a late-entry set-up or transmit grant names them.
- **Voice latency and tails depend on your codec build.** The ETSI programs use C stdio on the files
  they are given; on a pipe that is block buffered, so speech can arrive in bursts of up to a second and
  the last words of a call can wait for the next call. If that bothers you, wrap the programs (a script
  that runs them under `stdbuf` or on a pty, or a patched build that flushes after each frame). The
  adapter drops voice blocks rather than lag when the codec is slow.
- **Listeners can hear it.** The instance and its audio stream are announced to listeners (and guests
  while the guest policy shows decoder output) through the read-only `GET /api/v1/plugin-streams`; only
  operators start and control it.
- **The datagram format of the fork is not a stable interface.** The image pins the commit.
  Unrecognised datagrams are ignored.

## How it is built

`superhet-plugin-adapter tetra` (`crates/superhet-plugin-adapters/src/tetra/`):

- `phy.rs`: the demodulator (`Demod`) and, for tests, a transmitter (`modulate`).
- `tetmon.rs`: parser of the `tetra-rx` datagrams (`TETMON_begin FUNC:... TETMON_end`, the `TRA:` voice
  datagram) and of the system information console line.
- `call.rs`: the pure state machine (cell, neighbours, calls by call identifier and traffic channel,
  encryption, repeat suppression, sync).
- `mod.rs`: locates and starts the processes, wires them (bits to `tetra-rx`'s stdin, a loopback UDP
  socket back, voice blocks to the codec pipe) and talks to the host. A decoder that dies ends the plugin
  with an error so the host restarts it; a missing decoder or codec does **not** end it.

Dockerfile stage `tetra` builds `tetra-rx` from the pinned commit of osmo-tetra-sq5bpf (AGPL-3.0) with
Debian's `libosmocore`, and decodes osmo-tetra's own test burst as a self-test; the `etsi_codec-patches`
directory of the fork is not used and not copied (`docs/third-party.md`).

## Tests

- `phy.rs` unit tests: filter, phase-step mapping, and **bit-exact recovery of osmo-tetra's own test
  burst** (`tests/tetra/sync-burst.bits`, made by `build_sb()` of its `conv_enc_test.c`) from a synthetic
  transmitter at every sampling phase, with carrier offsets of plus/minus 2 kHz, with noise, mirrored
  with `invert`.
- `tetmon.rs` / `call.rs` / `mod.rs` unit tests: every datagram kind, padding and garbage, the call
  state machine (set-up, repeats, transmit grants, release, a call from voice alone, a channel
  reused, encryption and muting, idle timeout), repeat suppression, sync loss, configuration and the
  search for the decoder and the codec.
- `tests/tetra_golden.rs`: **golden event test.** Transcripts of the datagrams
  (`tests/tetra/transcripts/*.udp`) must give exactly the events in `*.expected.json`, every event
  validated against the schema. `generator-sync.udp` is **captured**: a real `tetra-rx` (built from the
  pinned commit) decoded the generator burst (MCC 262, MNC 42) and sent these datagrams. The others are
  written by hand (group call, encrypted call, neighbours with SDS and status).
- `tests/tetra_plugin.rs` against a fake host with a **stand-in for `tetra-rx`** (`tests/tetra/fake-tetra-rx`,
  needs `python3`; it replays the transcripts as UDP datagrams and records the bits it gets) and
  **stand-ins for the codec** (`tests/tetra/codec`, a sine): the host feeds the generator burst as real
  modulated IQ, so the adapter's demodulator runs and the test checks the bits the decoder received; call
  announced, played and closed; the **missing-codec state** (all events, no audio, `codec.state =
  "missing"`); `audio: false`; a broken codec; an encrypted call withheld (and played with
  `mute_encrypted: false`); neighbours, status and SDS once each; the decoder named by
  `SUPERHET_TETRA_RX_PATH` only; a dying decoder; the **missing-decoder state**; the manifest.
- `plugins/tetra` schemas: `crates/superhet-plugin-protocol/tests/shipped_plugin_configs.rs`.
- Web (vitest): model, event rendering, "now" with sync/calls/encryption, the missing-codec and
  missing-decoder notices, the dock, the picker mode.

## Not verified

No TETRA network, no real recording and no ETSI codec were available when this was written. What was
and was not checked:

- **Checked against a real `tetra-rx`:** it builds on Debian bookworm, decodes osmo-tetra's own generator
  burst, and sends `NETINFO1` (MCC and MNC as hexadecimal of the decimal value, `0106` is 262) and `ENCINFO1`
  as parsed here. The demodulator reproduces that burst bit-exactly from synthetic IQ.
- **Read from the sources only, never seen running:** the layouts of `DSETUPDEC`, `DCONNECTDEC`,
  `DTXGRANTDEC`, `DRELEASEDEC`, `DSTATUSDEC`, `SDSDEC`, `FREQINFO1`, the generic PDU line and the voice
  datagram, the meaning of the usage marker as traffic channel, and `ENCR` as the encryption flag. The
  hand-written transcripts follow those layouts; they are not captures.
- **Not tested on air:** the demodulator against a real transmitter (real pulse shaping, phase
  noise, fading, a base station that is not continuous), the capture range and sync time, `invert`
  against a real chain, and what the host's 25 kHz channel filter (the VFO bandwidth) does to the
  outer part of the signal's roll-off.
- **The ETSI codec's command line and buffering** (`cdecoder in out`, `sdecoder in out`, two processes
  on a pipe, 690 `int16` in, 8 kHz s16le out) are assumptions from how it is commonly used; the stand-ins
  mirror the assumption. Voice quality and latency are unknown.
- **Encryption detection is best effort:** a clear call on a cell that encrypts others, a missed
  encrypted header, or encrypted voice before its first flagged PDU play as noise.
- The ROADMAP P3 criterion 3 (PDUs match osmo-tetra/tetra-kit on reference recordings; DMO) is therefore
  **not** met; this plugin is the TMO part, without recordings.

Open: a recording of the user's own cell (SigMF) to turn into a golden test of the whole chain, a first
run with the real codec, DMO (the native stack of ADR-0004), and a stable event for registrations.
