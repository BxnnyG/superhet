# ADR-0017: ADS-B plugin (readsb) and the aircraft map

**Status:** accepted (2026-10-28)

## Context

SCOPE lists ADS-B at 1090 MHz as an exclusive decoder (v1, P2): it needs the whole device at one
frequency and a sample rate of its own, and the result is a picture of moving things rather than
a stream of messages. ADR-0014/0015/0016 built what such a plugin needs (exclusive claims, framed
transport with `status` frames, events into the decoder history, UI tabs that follow a claim).
This ADR records how ADS-B uses them and where it differs from DAB (ADR-0016).

## Decision

1. **Decoder: `readsb`** (wiedehopf's maintained fork of dump1090, GPL-3.0-or-later), as a
   separate process. Alternatives: `dump1090-fa` (same code base, FlightAware packaging, fewer
   releases), `dump1090-mutability` (unmaintained), a native Rust Mode S demodulator (a project of its
   own; readsb is mature and its fork is actively maintained). Debian
   bookworm does not package readsb, so the Dockerfile compiles it in a throw-away stage from a
   pinned upstream commit (tag v3.16.20, `READSB_COMMIT`), built without ncurses
   (`DISABLE_INTERACTIVE=yes`) and without any SDR library, and copies only the stripped binary
   and `COPYING` into the runtime image. Nothing is vendored into the repository. Updating readsb
   is a change of that one build argument; `scripts/check-plugin-image.sh` decodes a synthetic
   frame with the built binary (`scripts/gen-adsb-iq.py`) so a broken build fails CI.
2. **Exclusive claim at 1090 MHz, 2.4 MS/s, `cu8`.** readsb's demodulator works at exactly
   2.4 MS/s, so the manifest fixes both (`device.center_hz = 1090000000`, `sample_rate = 2400000`);
   there is nothing to configure. VFOs outside the window are paused while it runs, as for DAB.
3. **IQ in, files out, no sockets.** The adapter feeds the `samples` frames to readsb's stdin
   (`--device-type ifile --ifile - --iformat UC8`, with `--devel=ifile_no_synthetic` so messages
   are stamped with the wall clock, not a clock derived from the sample count) and reads
   `aircraft.json` that readsb replaces atomically once a second (`--write-json <dir>`). No
   `--net*` option is given, so readsb opens no port at all: unlike welle-cli (ADR-0016 item 8)
   there is nothing to confine. The directory lives under `TMPDIR` and is removed on exit. SBS
   (BaseStation) on a local TCP port was considered: it is event-like, but needs a port per
   instance, a reconnect loop and the tracker state (merging partial messages) that readsb
   already keeps. The JSON snapshot gives the merged state for free.
4. **Two outputs with different jobs.**
   - **Live picture = `status` frame** (plugin state, replaced about once a second, a few KiB to
     100 KiB): every aircraft heard in the last 60 s with all known fields. The browser polls
     `GET /api/v1/plugin-instances/{id}/status` (ADR-0016, same visibility as the decoder
     history). Nothing of it is stored, and no new endpoint or WebSocket was needed.
   - **History = `adsb.aircraft` events** into `decoder_events`, which keeps every event for the
     retention period (default 30 days). 50 aircraft at 1 Hz would be 4 million rows a day, so
     events are rationed: one when an aircraft first shows a callsign or a position, one when a
     callsign or position first appears later, then at most one per aircraft per
     `event_interval_s` (default 60, 5 to 86400) while it keeps sending, and at most 100 per
     snapshot. The history therefore reads "who flew by, where, how high" and not a track log.
     `adsb.error` reports a dying decoder.
5. **Supervision.** If readsb exits, the adapter reports an `adsb.error` event and an `error`
   status, closes the socket and exits with readsb's code, so the host's backoff / crash-window
   rules (ADR-0014) apply unchanged. Closing the host's side ends readsb and removes the
   directory.
6. **Config** (all optional, unknown keys refuse to start): `lat` / `lon` (receiver position,
   passed to readsb as `--lat/--lon` and shown as the station on the map) and
   `event_interval_s`. The position is data of the operator's home; the status document carries
   it, so everybody who may see decoders sees it. The start dialog says so and the field is
   optional.
7. **Web: an ADS-B tab and a map and table overlay.** The tab (operator: start / stop with the
   takeover confirmation, receiver position; everybody: headline numbers) follows the instance
   or the device claim exactly like the DAB tab, so listeners see the picture read-only. "Open
   map and table" opens a full-window overlay with a Leaflet map and a sortable, filterable
   table (callsign, ICAO, altitude, speed, track, signal, age; details of the selected
   aircraft). The overlay is plain React state in an external store like the other views; the map
   is imperative Leaflet code in a lazily loaded chunk (about 45 kB gzipped), so visitors who
   never open it do not download it. Aircraft that were not heard for 60 s leave the picture (the
   plugin drops them, the view also ages them between polls); before that they fade from full
   strength at 15 s to a faint ghost at 60 s, markers and table rows alike. Trails (the last 40
   positions) are kept in the browser only. Symbols are rotated to the track and coloured by
   altitude band.
8. **Map library and tiles.** Leaflet 1.9.4 (BSD-2-Clause), npm, exact version pinned in
   `package.json` and the lockfile (no CDN: the page's CSP is `script-src 'self'`). Tiles come
   from OpenStreetMap's standard tile server `https://tile.openstreetmap.org`, with the
   attribution its tile usage policy requires. The page CSP gets exactly that origin in
   `img-src` (never `script-src` / `connect-src`). Consequence for privacy: a viewer who opens the
   map sends tile requests (their IP address and the tile area) to the OpenStreetMap Foundation;
   the install notice says so. OSM's policy allows light use, not a public high-traffic service;
   an operator who needs more points a reverse proxy at a tile provider (a configurable tile URL
   is future work). A deployment without internet access shows aircraft on an empty background.
   Alternatives: MapLibre GL (vector tiles, needs a style and a vector source, heavier), a
   hand-drawn canvas without tiles (no geography).
9. **Roles.** Starting and stopping are operator actions (`plugin-instances` routes). Status,
   map and table follow the decoder history's visibility (`GuestDecoder` for guests per policy).

## Consequences

- `readsb` joins welle-cli as a third-party GPL binary in the image (`docs/third-party.md`); the
  build stage needs network access to GitHub during `docker build`.
- The image check and the adapter tests cover the chain in pieces: parser and tracker on recorded
  `aircraft.json` files in readsb 3.16's format, the adapter end to end against a stand-in
  `readsb` (`tests/adsb_plugin.rs`), the browser scenario in `web/e2e/zz-adsb.e2e.ts`, and a
  real readsb decoding a synthetic Mode S frame in `scripts/check-plugin-image.sh`. Reception
  quality on a real antenna is a manual check (`docs/plugins-adsb.md`).
- One device, one claim: ADS-B and DAB exclude each other (`conflict`), and nothing else tunes
  while either runs. A second device (ADS-B on one dongle, the general receiver on another) is
  the practical setup and needs no change.
- The inspector gets an eighth tab; its column grows from 320 to 352 px so the compact tab strip
  still shows the selected label.
- Aircraft databases (registration, type, route; SCOPE P3) and range statistics are not part of
  this; the status document has room for them.
