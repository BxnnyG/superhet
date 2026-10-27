# DAB / DAB+ plugin

`org.superhet.decoder.dab` (`plugins/dab`), decision record ADR-0013 and ADR-0016. An exclusive,
framed-transport plugin around `welle-cli` (welle.io, a separate GPL-2.0-or-later process):
ensemble scan, service list, station text (DLS), MOT slideshow and the audio of one selected
service, which reaches the browser as an ordinary Opus stream.

## What it does

- **Takes the whole device.** One instance claims the device at a Band III block (centre of the
  block, 2.048 MS/s, `cu8`). VFOs outside the window are paused while it runs, nobody can retune,
  stopping gives the previous tuning back (ARCHITECTURE 3.4, ADR-0015). An RTL-SDR v5 has no
  trouble with 1.536 MHz.
- **Ensemble and services.** Ensemble label and id, services with label, programme type,
  language, DAB (MP2) or DAB+ (HE-AAC), bit rate and protection. Signal quality (SNR of the OFDM
  demodulator, FIC CRC errors) for aiming the antenna.
- **Now playing.** DLS text and the slideshow image of the selected service. Both are decoded
  only for the selected service (welle-cli decodes services on demand, which keeps CPU low).
- **Audio.** The selected service as 48 kHz stereo Opus on `sh.v1.dev.<dev>.audio.<id>`
  (`docs/protocol-audio.md`, "Plugin audio"); the web UI mixes it like a VFO.
- **Scan.** Walks the 38 Band III blocks (5A-13F), about 5 s per empty block and 3-15 s per
  block with an ensemble, so a full scan takes 3-5 minutes. Every ensemble found is a
  `dab.ensemble` event in the decoder history (`docs/api-decoder-events.md`), which is how the
  results persist per device. The scan returns to the block it started on.

## Using it

Web UI: DAB tab (`docs/plugins-ui.md`) or click a block of the "DAB / DAB+ Band III" plan in the
Channels tab. Operators confirm the takeover, listeners see the same view read-only. API:

```
POST /api/v1/plugin-instances  {"plugin_id":"org.superhet.decoder.dab","device":"dev0","freq_hz":178352000}
POST /api/v1/plugin-instances/{id}/control  {"cmd":"scan"}
GET  /api/v1/plugin-instances/{id}/status
```

`freq_hz` must be a block centre (the plugin names the block from it; within 50 kHz).
Instance config (optional): `{"service":"D210"}` plays that service (hex id) from the start.

### Control messages (operator)

| Message | Effect |
|---|---|
| `{"cmd":"tune","channel":"5C"}` | Retune to a block (the plugin sends a `tune` frame, restarts welle-cli) |
| `{"cmd":"select","service":"D210"}` | Play a service (hex id as in the status); `null` stops the audio |
| `{"cmd":"scan"}` / `{"cmd":"scan","channels":["5C","12D"]}` | Scan all blocks / a list |
| `{"cmd":"scan_stop"}` | End the scan, back to the block it started on |

A refused message (unknown block, bad id) becomes a `dab.error` event, never a crash.

### Status document (`GET .../status`, `status`)

```json
{"v":1,"mode":"tuned","channel":"5C","freq_hz":178352000,"receiver_running":true,"synced":true,
 "snr":14.5,"fic_crc_errors":2,"iq_connected":true,
 "ensemble":{"label":"DR Deutschland","eid":"D210","ecc":"E0"},
 "services":[{"sid":"D210","label":"Deutschlandfunk","pty":"Information","language":"German",
   "codec":"DAB+","bitrate":72,"protection":"EEP 3-A","audio":true,"dls":"...","audio_errors":1}],
 "selected":"D210","playing":true,
 "scan":null,"last_scan":{"finished_unix_ms":0,"found":[{"channel":"5C","freq_hz":178352000,
   "label":"DR Deutschland","eid":"D210","ecc":"E0","services":8,"snr":14.5}]},"error":null}
```

`mode` is `idle`, `tuned` or `scanning`; during a scan `scan` is `{"active":true,"index","total",
"channel","started_unix_ms","found":[...]}`. The slide of the selected service is the media object
`slide.<sid in lower case>` (`GET .../media/slide.d210`, its `revision` in the reply of `status`
changes with every new slide).

### Events

All carry a `summary` (the decoder history shows it). `dab.ensemble` (`channel`, `freq_hz`,
`label`, `eid`, `ecc`, `services` [sid, label, codec, bitrate, pty], `snr`, `origin`
`scan|tune`), `dab.dls` (`text`, `sid`, `service`, `channel`), `dab.play` (`sid`, `service`,
`channel`, `ensemble`), `dab.scan` (`state` `started|finished|stopped`, `found`, `total`),
`dab.error` (`message`). Schema: `plugins/dab/schema/events.json`.

## How it works (ADR-0016)

```
sdrd --window tap--> host --samples frames--> adapter --rtl_tcp stand-in--> welle-cli -w <port>
                                               |  GET /mux.json /slide/<sid> /mp3/<sid>  |
host <-- event / status / media / audio / tune --                                        |
```

The adapter (`crates/superhet-plugin-adapters/src/dab`) serves the IQ to welle-cli as an
`rtl_tcp` server, polls `/mux.json` every 0.7 s, fetches slides when `mot.lastchange` changes,
decodes the MP3 stream of the selected service (welle's hand-over format) to PCM, resamples to
48 kHz stereo and sends `audio` frames. The host encodes Opus and publishes. A new block means a
fresh welle-cli process after the claim moved (IQ is dropped for 0.8 s meanwhile, so no samples of
the old block reach the new decoder).

## Operating notes

- **CPU.** One ensemble in welle-cli (OFDM, FIC, one service) plus MP3 and Opus: about one core's
  fraction on a desktop CPU; measure on your hardware (`docs/perf.md` method).
- **Gain.** welle-cli's gain commands are ignored; set the tuner gain in the Source tab. A weak
  or overloaded front end shows as a low SNR / many FIC errors in the badge. A good Band III
  antenna (a dipole or a DAB whip, not the FM whip indoors) matters more than for FM.
- **Network.** welle-cli opens an unauthenticated web interface on a random port of the
  container while an instance runs. With the default compose network it is not published; with
  host networking (`deploy/compose.hostnet.yaml`) it is visible on the LAN. It only serves what
  is broadcast anyway.
- **Not covered.** TII (transmitter id) is switched off, no EPG/Journaline, no data services, no
  recording of the DAB stream yet (it is an ordinary audio stream, so recording and Instant Replay
  can use it later).

## Tests

- `crates/superhet-plugin-adapters`: parsers and conversions as unit tests (channel table,
  `mux.json` on `tests/dab/mux-*.json`, dwell decision, MP3 to PCM on `tests/dab/tone-32k.mp3`,
  rtl_tcp stand-in, HTTP client, control vocabulary); `tests/dab_plugin.rs` runs the real adapter
  binary against a fake host and `tests/dab/fake-welle-cli` (python3): IQ into the stand-in, ensemble
  event, audio frames, DLS, slide as media, a scan with retunes and its return, events validated
  against the manifest's schema. The fixtures are **shaped after welle.io 2.4's source**
  (`jsonconvert.cpp`), regenerate with `scripts/gen-dab-fixtures.py`; the tone is made with
  `ffmpeg -f lavfi -i sine=... -c:a libmp3lame -b:a 48k`.
- `crates/superhet-pluginhost/tests/framed.rs`: the framed transport end to end with a fake plugin
  (`tests/fixtures/fake_framed.py`).
- `web/e2e/zz-dab.e2e.ts`: browser scenario against the real stack with the stand-in welle-cli.
- `scripts/check-plugin-image.sh` checks that `welle-cli` of the image starts and links.
- Real welle-cli in Debian bookworm was checked against the stand-in `rtl_tcp` server with noise:
  it connects, reports the dongle and serves `/mux.json` (id `0x0000`, no services).

## Manual check on real hardware (block 5C, 178.352 MHz)

1. Start the stack (own instance, not production) with an RTL-SDR and a Band III antenna; make
   sure the device is not locked (Receiver tab).
2. Channels tab, "DAB / DAB+ Band III", click 5C; confirm in the DAB tab. Within about 10 s the
   ensemble label appears, services follow after a few more seconds; the badge should show Good or
   Fair. A line `RTL_TCP_CLIENT: Successful connected` in the plugin host log
   (`docker compose logs superhet-pluginhost`, lines prefixed `welle-cli:`) confirms the IQ path.
3. **Enable audio**, play a service: audio within 3-5 s, DLS text next to it, slides after a
   while on stations that send them.
4. **Scan all blocks**: progress bar; found ensembles appear in the list, each as a `dab.ensemble`
   row in the Decoders tab history. **Stop scan** returns to 5C.
5. **Stop DAB**: VFOs return, the tuning from before is restored.
6. Failure checks: no antenna gives "No signal" and no ensemble; a missing `welle-cli` shows
   its error in the panel.
