# ACARS and VDL Mode 2 plugins

`org.superhet.decoder.acars` (`plugins/acars`) and `org.superhet.decoder.vdl2` (`plugins/vdl2`),
decision record ADR-0031. Two non-exclusive, simple-transport decoders for the digital text
channel between aircraft and airline ground stations:

- **ACARS** (VHF, 129 to 132 MHz, 2400 baud AM-MSK, 25 kHz channels) through **acarsdec**
  (GPL-2.0, a separate process; Thierry Leconte's decoder as maintained by Thibaut Varène, version
  4.7).
- **VDL Mode 2** (VHF, 136.6 to 137 MHz, 10.5 kbaud D8PSK) through **dumpvdl2** (GPL-3.0, a
  separate process, by Tomasz Lemiech) with **libacars** (MIT). VDL2 is the successor that carries
  ACARS text, CPDLC (controller-pilot messages) and ADS-C reports; acarsdec cannot decode it.

Both read IQ from stdin, so they fit the process model unchanged. Alternatives (not used):
`acarsdecng`, `vdlm2dec` (unmaintained, no libacars), `dumpvdl2` for ACARS (VDL2 only).

## What they do

- **Tap a VFO.** The receiver's frequency is the channel decoded and the centre of the IQ
  (`input.kind = iq`, `cu8`): ACARS 240 kS/s with a receiver bandwidth of about 200 kHz, VDL2
  420 kS/s with about 300 kHz. The receiver's own audio is noise. `frequencies_mhz` adds channels
  within 100 kHz (ACARS) or 135 kHz (VDL2) of the receiver's frequency; without it only the
  receiver's own frequency is decoded. The decoder reads the frequency when it starts: **after
  retuning the receiver, restart the decoder.**
- **`acars.message` events**: `tail` (registration), `flight`, `label` and `block_id`,
  `direction` (`downlink`: aircraft to ground, `uplink`), `mode`, `msgno`, `text`, `end`
  (last block of the message), `freq_mhz`, `level`, OOOI fields of the standard labels
  (`depa`, `dsta`, `eta`, `gtout`, `gtin`, `wloff`, `wlin`), `decoded` (ARINC 622 content, ADS-C
  and CPDLC, as libacars decodes it) and a `summary`. Messages without text (acknowledgements)
  are left out unless `include_empty` is set.
- **`vdl2.message` events**: `kind` (`acars`, `x25`, `xid`, `frame`), the AVLC addresses
  (`src_addr`, `src_type`, `dst_addr`, `dst_type`: 24-bit addresses as six hex digits, the type
  says aircraft or ground station), `icao` (the aircraft's address, whichever side it is on),
  and for `acars` the same fields as above; `x25` carries libacars' decoding in `decoded`.
  Link management (`xid`, supervisory frames) is dropped unless `all_frames` is set, as are ACARS
  blocks that failed their CRC.
- **Message list.** In the output dock of the receiver (pick **ACARS** or **VDL2** in the
  **Digital** select): newest first, aircraft (registration, else ICAO address), flight and label,
  the text on one line; the Decoders tab shows the same rows with all fields in the detail dialog
  (text with its line breaks, frequency, level, the decoded ARINC 622 content, the raw event).
- **Link to the aircraft.** When the ADS-B decoder sees the aircraft, the row gets an **On the
  ADS-B map** button that opens the ADS-B view with that aircraft selected. The match is by ICAO
  address (VDL2; no database needed) or, for ACARS, by registration through the imported aircraft
  database (`docs/plugins-adsb.md`, Settings, Aircraft database). Without ADS-B data or an
  imported database there simply is no button.

| ACARS config key | Default | Effect |
|---|---|---|
| `frequencies_mhz` | the receiver's frequency | channels to decode, 1 to 8, within 100 kHz of it |
| `include_empty` | `false` | also report messages without text |

| VDL2 config key | Default | Effect |
|---|---|---|
| `frequencies_mhz` | the receiver's frequency | channels to decode, 1 to 8, within 135 kHz of it |
| `all_frames` | `false` | also report link management frames and corrupt blocks |

## Channels

ACARS (a few; the full lists differ by region): Europe 131.525, 131.725, 131.825, 131.850 MHz;
North America 131.550 (primary), 130.025, 130.425, 130.450, 129.125 MHz. VDL2: 136.975 MHz is the
common signalling channel and always active near airports; Europe also 136.725, 136.775,
136.825, 136.875 MHz; the receiver at 136.850 MHz with 300 kHz bandwidth covers all of them.

## Limits you will notice

- **Line of sight again**, and ACARS at 2400 baud AM is a weak mode: a simple antenna sees traffic
  within about 100 to 200 km of the airways. VDL2 is stronger (D8PSK) but needs a clean signal;
  an RTL-SDR with an uncorrected clock several kHz off still works for ACARS (the demodulator
  tracks) and for VDL2 up to a few kHz.
- **The IQ rate is a compromise.** One receiver cannot cover ACARS and VDL2 at once (the bands are
  5 MHz apart); run two receivers on two sticks (`docs/multi-device.md`) or one decoder after
  another. Several channels of one band share one receiver.
- **The events are what the tools print.** Fields can differ between acarsdec and dumpvdl2 versions;
  the adapter reads the keys of acarsdec 4.7 and dumpvdl2 2.7 and ignores what it does not know.
  The decoded ARINC 622 / X.25 tree is passed on as it is, not interpreted.
- **Bookmarks** save and restore both modes (migration 0021).
- **Aircraft messages are third-party data** (see below). Do not feed them anywhere public.

## Privacy and law

ACARS and VDL2 are not meant for the public. Receiving them is allowed for hobby use in many
countries and restricted in others; in Germany the eavesdropping ban of section 5 TDDDG (formerly
TTDSG) forbids listening to or passing on the content of radio traffic that is not meant for
you or the public, so check the rules where you live. ACARS text can
contain passenger or crew names, positions, maintenance data and company information. The decoder
history keeps the messages until the retention period ends (`SUPERHET_EVENT_RETENTION_DAYS`), and
they are visible to everybody who may see decoders (guests if the guest policy allows). The install
dialog says so; installing is an admin decision.

## How it is built

`superhet-plugin-adapter acars` starts `acarsdec -e --output json:file:path=- --iqfile
file=/dev/stdin,sr=240000,format=U8 -c <MHz> <channels...>`, `superhet-plugin-adapter vdl2` starts
`dumpvdl2 --iq-file - --sample-format U8 --oversample 4 --centerfreq <Hz> --output
decoded:json:file:path=- <channels...>` (sample rate 105 kHz times 4). The adapters map the JSON
lines to events (`src/acars.rs`, `src/vdl2.rs`). The image builds libacars (v2.2.1), acarsdec
(v4.7) and dumpvdl2 (v2.7.0) from pinned upstream commits (Dockerfile stage `datalink`) with only
the stdin input and JSON output, and starts both once on a second of silence as a self-test.

## Tests

- Unit tests of both parsers and the command lines on the output formats of acarsdec 4.7 and
  dumpvdl2 2.7 (`cargo test -p superhet-plugin-adapters`).
- `tests/datalink_plugins.rs`: the real adapter binary with stand-in tools (`tests/acars/`,
  `tests/vdl2/`): command lines, samples reach stdin, events satisfy the manifests' schemas,
  filtering, missing tool (exit 127), bad configuration (exit 2).
- Web: `web/src/datalink/datalink.test.tsx` (rows, digital modes, the ADS-B link).

## Not verified on air

No recording of ACARS or VDL2 is committed (acarsdec's own `test.wav` is GPL-licensed audio, not copied here). Once, by hand, that file's first channel was AM-modulated onto a 240 kS/s `cu8` stream and fed through the exact acarsdec command line of the adapter in the image: it decoded a real ACARS message (registration, flight, label, text), which proves the stdin IQ path and the JSON format the parser reads. dumpvdl2 was only started on silence; its output format is taken from its source and documentation. Reception on a real antenna is a manual check for both.
