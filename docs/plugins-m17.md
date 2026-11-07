# M17 digital voice plugin

`org.superhet.decoder.m17` (`plugins/m17`), decision record ADR-0018. A non-exclusive,
framed-transport decoder around `m17-demod` (mobilinkd's m17-cxx-demod, GPL-3.0, a separate
process) for M17, the open amateur radio digital voice protocol (4FSK, 4800 baud, 9 kHz, voice
coded with Codec2 3200). M17 and its codec are free of patents and licence fees, which is why this
is the one digital voice mode the image can decode completely. DMR, D-Star, NXDN, P25 and TETRA
need restricted vocoders and are not bundled (ADR-0018).

## What it does

- **Taps a VFO.** Bind it to an NFM receiver on the M17 channel. The VFO's FM audio at 48 kHz is the
  4FSK baseband `m17-demod` reads (`input.kind = discriminator`, `s16le`). Several M17
  receivers on different VFOs are separate instances.
- **Call events.** `m17.call.start` when a link setup frame is decoded and `m17.call.end` when the
  call is over (`reason`: `eos` end-of-stream marker of the transmitter, `lost` no signal for
  1.5 s, `new-call` another station took over, `stop` the plugin stopped). Both carry source and
  destination callsign, channel access number (CAN), content (`voice`, `voice+data`, `data`), the
  encryption flag and type, a GNSS position and a one-block text message when the link setup frame
  carries them; the end event adds `duration_ms`, `audio_ms` (decoded voice) and `muted`. The events
  are in the decoder history like every plugin event (`docs/api-decoder-events.md`); the schema is
  `plugins/m17/schema/events.json`.
- **Voice audio.** The decoded 8 kHz Codec2 voice is resampled to 48 kHz mono and goes out as an
  ordinary plugin audio stream (pseudo VFO `InstanceInfo.audio_vfo`, `docs/protocol-audio.md`,
  "Plugin audio"); the web UI mixes it like a VFO. The audio only flows during a call.
- **Encrypted calls** (AES or scrambler, TYPE bits 3 and 4) are reported and **not played**: the
  audio would be noise, and nothing is decrypted. `mute_encrypted: false` plays it anyway.
- **Status document** (`GET /api/v1/plugin-instances/{id}/status`): `now` (the call on the air with
  `started_unix_ms`, or `null`), `last` (the last finished call) and `calls` (count since the
  plugin started). The UI polls it once a second for the "now talking" indicator.

## Using it

Web UI: pick **M17** in the **Digital** select of the tuning strip (operators and admins). The
receiver is set to NFM, 12.5 kHz, squelch off, the plugin starts on it, the receiver's own audio
(the raw 4FSK buzz) is muted and the decoded voice plays instead. The output dock shows:

- a **"now talking"** bar: station on the air, destination, CAN, encryption, a running clock,
  text and position with an OpenStreetMap link; between calls "Channel idle" and the last call.
  While the dock is collapsed the callsign is shown as a badge in its header.
- a **call log**, newest first, one row per finished call (callsigns, CAN, encryption, duration,
  text, position). The log is also in the Decoders tab and exportable like all decoder events.

API: start with `POST /api/v1/plugin-instances` (`plugin_id`, `device`, `vfo`, `freq_hz`),
change settings with `.../reconfigure`:

| Config key | Default | Effect |
|---|---|---|
| `noise_blanker` | `true` | `m17-demod -b`: silence frames the Viterbi decoder is unsure about instead of playing noise |
| `invert` | `false` | `m17-demod -i`: inverted baseband (an inverting receiver chain) |
| `mute_encrypted` | `true` | do not play the audio of encrypted calls |

## Limits you will notice

- **The first second of a call is often missing.** The demodulator needs about 25 frames
  (a second) to lock its clock recovery, then it recovers the link setup frame from the LICH
  segments of the stream, so the call start is announced about a second after the transmission
  began. A 20 s synthetic call loses 0.9 s, a 3 s one about 1 s. This is upstream behaviour.
- **Tune accurately.** A 12.5 kHz channel and 4FSK with ±2.4 kHz deviation leave little margin. An
  RTL-SDR with an uncorrected clock is several kHz off at 430 MHz; click-tune onto the signal.
- **Voice streams only.** Packet-mode M17 (SMS, APRS gateways) is ignored; extended callsign data
  and multi-block text messages are not decoded.
- **Listeners can hear it.** The instance and its audio stream are announced to listeners (and to
  guests while the guest policy shows decoder output) through the read-only
  `GET /api/v1/plugin-streams` (no configuration); the web UI then shows the call log and mixes the
  voice like any VFO, without start / stop / settings. Only operators start and control it.
- Codec2 3200 is narrow-band speech at 8 kHz; do not expect more than a good telephone.

## How it is built

`superhet-plugin-adapter m17` (`crates/superhet-plugin-adapters/src/m17/`): `lsf.rs` parses
what `m17-demod -l` prints (link setup frame: callsigns, type, CAN, META) and decodes TYPE
(encryption) and META (GNSS position, text) per the M17 specification v2.0; `call.rs` is the pure
call state machine; `mod.rs` runs the tool (via `stdbuf -o0` so audio is not held back by stdio
buffering), feeds the `samples` frames to its stdin, resamples the 8 kHz voice and sends events,
status and audio to the host. A tool that dies ends the plugin with an error, so the host's
supervisor restarts it.

The Dockerfile stage `m17` builds the tool (and `m17-mod` for tests) from pinned commits and adds
one line to upstream's output (`deploy/docker/m17-demod-type.sed`: the raw TYPE field after the
CAN, without which the encryption bits are unknown). The image build runs a self-test: a
synthetic call must decode with the TYPE field.

## Tests

- `lsf.rs` / `call.rs` unit tests: line parsing (patched and upstream format), encryption bits,
  GNSS (north/south, west, validity) and text decoding, call start/end/timeout/takeover, muting.
- `tests/m17_plugin.rs` against a fake host:
  - **golden** with the real `m17-demod`: the committed synthetic recording
    (`tests/recordings/m17-voice.sigmf-*`, 3 s from DL1ABC to DB0XYZ, CAN 3, made with `m17-mod`
    of the same project by `scripts/gen-m17-recording.sh`, CC0) must come out as a call between
    those callsigns with at least 1.5 s of audible decoded voice, with the events valid against
    the events schema; a second case sends the recording through the production NFM channel
    (FM-modulated at 240 kS/s, `superhet-dsp`) first;
  - with a stand-in tool (`tests/m17/fake-m17-demod`, needs `python3`): call announced, played and
    closed, encrypted call muted, position forwarded, vanished transmitter ends by timeout,
    switches reach the tool, a dying or missing tool fails the plugin.
- `plugins/m17` schemas: `crates/superhet-plugin-protocol/tests/shipped_plugin_configs.rs`.
- Web (vitest): model, event rendering, "now talking", the dock with call log, the status poll.
- Image: `scripts/check-plugin-image.sh` decodes the recording with the tool inside the built
  image. CI's Rust job builds the Dockerfile stage `m17` and puts a wrapper that runs it on `PATH`,
  so the golden tests run there (`SUPERHET_REQUIRE_PLUGIN_TOOLS=1`). Locally, without
  `m17-demod` on `PATH` the real-tool cases are skipped.

Running the golden tests locally without installing anything: `docker build --target m17 -t
superhet-m17 -f deploy/docker/Dockerfile .`, then a wrapper script named `m17-demod` on `PATH`
that runs `docker run --rm -i superhet-m17 /src/m17/build/apps/m17-demod "$@"`.

## Not verified on air

No real M17 transmission was received yet: all tests use synthetic signals. First real-hardware
run (an M17 hotspot or radio on 70 cm / 2 m, RTL-SDR tuned on the channel) is open; check that the
call start, the audio and the CAN match what the transmitting station shows.
