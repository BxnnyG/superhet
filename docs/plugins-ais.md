# AIS ship tracking plugin

`org.superhet.decoder.ais` (`plugins/ais`), decision record ADR-0031. A non-exclusive,
simple-transport decoder around AIS-catcher (GPL-3.0, a separate process) for the Automatic
Identification System of ships on the two marine VHF channels, 161.975 MHz (AIS 1, "channel A")
and 162.025 MHz (AIS 2, "channel B"). It decodes position reports of class A and class B
transponders, the static data of ships (name, callsign, IMO number, type, size, destination, ETA,
draught), shore base stations, aids to navigation (buoys, lights, beacons) and search-and-rescue
aircraft, and shows them on the same map the ADS-B and APRS views use.

Alternatives (not used): `rtl_ais` (a plain two-channel decoder, no longer developed),
OpenCPN with an AIS plugin (a chart plotter, not a web receiver), a hardware receiver such as
the dAISy. AIS-catcher is the actively developed SDR decoder and reads IQ from stdin.

## What it does

- **Taps a VFO.** Put a receiver on **162.000 MHz** and set its bandwidth to about 150 kHz: both
  channels are inside (`input.kind = iq`, `cu8`, 192 kS/s). The receiver's own audio is noise; the
  decoder takes the complex baseband.
- **Vessel events.** One `ais.vessel` event per vessel at most every `event_interval_s` (default
  60) while it sends positions, plus one when a vessel is heard first, when it first has a position
  and when its static data arrives or changes. Every event carries everything known about the
  vessel (the adapter merges position reports and static data by MMSI), so the newest event per
  MMSI is the picture and the older ones are the track. The events are in the decoder history like
  every plugin event (`docs/api-decoder-events.md`); the schema is `plugins/ais/schema/events.json`.
  Fields: `mmsi`, `class` (`A`, `B`, `base`, `aton`, `sar`), `name`, `callsign`, `imo`,
  `ship_type` and `ship_type_text` (or `aton_type`, `aton_type_text`), `status`, `status_text`,
  `lat`, `lon`, `sog_kn` and `sog_kmh`, `cog_deg`, `heading_deg`, `altitude_m` (SAR aircraft),
  `destination`, `eta` (UTC, `MM-DD HH:MM`), `draught_m`, `length_m`, `beam_m`, `summary`.
- **Map and list.** In the output dock of the receiver, **List / Map**. The map has one symbol per
  vessel: an arrow in the heading (else the course) for vessels under way, a dot for vessels at
  rest, a diamond for aids to navigation, a square for base stations, a small aircraft for SAR
  aircraft; the colour is the ship type (cargo, tanker, passenger, fishing, sailing, pleasure,
  tug, high-speed, special, other). Each vessel has a track of its last positions, a label with
  its name from zoom level 10 on, and fades after 10 minutes without a report. The list beside it
  can be searched by name, MMSI, callsign, IMO number or destination; picking a vessel shows its
  details with the speed in **knots and km/h**, course, heading, status, destination, ETA,
  draught, size and an OpenStreetMap link. Vessels not heard for 3 hours leave the view.
- **Map tiles** come through the server's caching tile proxy like the ADS-B map
  (`docs/plugins-adsb.md`, `SUPERHET_MAP_TILE_URL`).

## Using it

Web UI: pick **AIS** in the **Digital** select of the tuning strip (operators and admins); the
receiver is set to 150 kHz bandwidth, squelch off, the plugin starts on it. Tune it to
162.000 MHz first (the select suggests AIS on 161.9 to 162.1 MHz). API: start with
`POST /api/v1/plugin-instances` (`plugin_id`, `device`, `vfo`, `freq_hz`).

| Config key | Default | Effect |
|---|---|---|
| `event_interval_s` | `60` | shortest time between two history events of one vessel (5 to 3600) |

## Limits you will notice

- **Line of sight.** AIS is VHF at about 12.5 W (class A) or 2 W (class B): a good antenna with
  a view of the water matters far more than gain. Typical range with a simple antenna at a window
  is 10 to 30 km; from a roof or a hill 50 km and more.
- **An RTL-SDR with an uncorrected clock** is a few kHz off at 162 MHz. That is fine for AIS (the
  channels are 25 kHz apart and the decoder tracks the offset), but make sure the receiver is
  centred on 162.000 MHz.
- **The map is built from the decoder history**, not from a live status: the newest ~2000 events
  of the instance are loaded into the browser. In a busy harbour that can be the last 10 to 20
  minutes only; raise `event_interval_s` to cover a longer time. The event rate is rationed
  per vessel (ADR-0031), so the history is "who passed where", not every message.
- **Not decoded:** binary messages (weather, ferry timetables), safety text messages, type 20
  and the regional extensions. Messages with a bad checksum are dropped.
- **Do not use it for navigation.** AIS-catcher's own disclaimer applies, and the position of a
  vessel can be minutes old when it is shown.
- **Not saved with bookmarks:** the stored digital protocol is a database constraint that has no
  AIS yet (ADR-0031); a bookmark made while AIS runs is a plain analogue bookmark.

## Privacy and law

AIS is a broadcast for everybody and receiving it is legal in most countries; check the rules where
you live. The decoder history keeps vessel identities and positions until the retention period ends
(`SUPERHET_EVENT_RETENTION_DAYS`), and the map is visible to everybody who may see decoders (guests
if the guest policy allows). The install dialog says so.

## How it is built

`superhet-plugin-adapter ais` starts `AIS-catcher -r cu8 . -s 192000 -o 1` (`.` is AIS-catcher's
name for standard input, `-o 1` prints plain NMEA `!AIVDM` sentences). `src/ais/payload.rs`
checks the NMEA checksum, joins multi-sentence messages and decodes the 6-bit payload (ITU-R
M.1371-5: types 1, 2, 3, 4, 5, 9, 11, 18, 19, 21, 24, 27), `src/ais/tracker.rs` merges by MMSI and
rations the events. The image builds AIS-catcher from a pinned upstream commit (Dockerfile stage
`ais`, tag v0.71) without any SDR library, web viewer, database or TLS; the build stage decodes a
synthetic GMSK frame made by `scripts/gen-ais-iq.py` as a self-test, and
`scripts/check-plugin-image.sh` repeats that on the finished image.

## Tests

- Unit tests: the decoder against gpsd's published sentence and against sentences built field by
  field, multi-sentence reassembly, the tracker's rationing and merging (`cargo test -p
  superhet-plugin-adapters`).
- `tests/datalink_plugins.rs`: the real adapter binary with a stand-in `AIS-catcher`
  (`tests/ais/`): command line, samples reach stdin, events satisfy the manifest's schema.
- Web: `web/src/ais/ais.test.tsx` (model, rows, list, details).

## Not verified on air

Everything above is tested with synthetic data. Reception quality on a real antenna, with real
traffic, and the behaviour of AIS-catcher on a long live stream are not yet checked.
