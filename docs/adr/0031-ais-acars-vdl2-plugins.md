# ADR-0031: AIS, ACARS and VDL Mode 2 plugins

**Status:** accepted (2026-11-14)

## Context

SCOPE lists the VHF datalinks next to ADS-B: ships on 162 MHz (AIS) and the text channel between
aircraft and airlines (ACARS on 129 to 132 MHz, VDL Mode 2 on 136.6 to 137 MHz). All three are
narrow channels in the part of the spectrum an RTL-SDR covers, none needs the whole device, and
all three have a mature open decoder that reads IQ from stdin. The plugin system (ADR-0014/0015)
fits: a manifest, one call of `superhet-plugin-adapter`, events into the decoder history, a view
in the output dock (ADR-0022 era "digital modes").

## Decision

1. **Three plugins, three tools, three separate processes.**
   - AIS: **AIS-catcher** (GPL-3.0, jvde). Alternatives: `rtl_ais` (no longer developed, RTL-SDR
     only), embedding a native Rust GMSK demodulator (a project of its own; the decoder's quality
     on weak, overlapping bursts is its main asset).
   - ACARS: **acarsdec** 4.x (GPL-2.0, the maintained fork by f00b4r0 of Thierry Leconte's
     decoder). Needs `--iqfile` input, which only 4.x has (3.x reads an RTL-SDR directly).
   - VDL2: **dumpvdl2** 2.x (GPL-3.0, szpajder) with **libacars** (MIT). Alternative: `vdlm2dec`
     (unmaintained, no ARINC 622 decoding).
   All are built in Dockerfile stages from pinned upstream commits (`ais`, `datalink`), with only
   what the plugins use (no SDR libraries, no web viewer, MQTT, databases or TLS); nothing is
   vendored in the repository; licences and sources are in `docs/third-party.md` and
   `docs/licenses.md`. Debian bookworm packages none of them in a usable version. The build stages
   run a self-test (AIS: a synthetic GMSK frame, `scripts/gen-ais-iq.py`; ACARS and VDL2: a second
   of silence through the exact command line of the adapter).
2. **Simple transport, VFO-bound IQ.** Unlike ADS-B (which needs the whole window at 2.4 MS/s),
   the three signals fit into a VFO: AIS 192 kS/s (150 kHz around 162.000 MHz holds both
   channels), ACARS 240 kS/s (a multiple of 12 kHz, as acarsdec's decimator wants; 200 kHz),
   VDL2 420 kS/s (105 kHz times 4, as dumpvdl2 wants; 300 kHz). They are non-exclusive, so
   other VFOs keep working and several channels or bands are several instances on several
   VFOs. The tap resamples to the manifest's rate (`superhet_bus::tap`), nothing new in the
   daemon.
3. **Events only, no status document.** AIS-catcher, acarsdec and dumpvdl2 print one line per
   message, and the decoder history (`decoder_events`, retention, WebSocket feed, export, role
   rules, notifications on events) already is the store. The ADS-B plugin needs a status document
   because the live picture is readsb's file; here the picture can be rebuilt from the events, as
   the APRS map does. This keeps the plugins in the contract's simple transport (no socket, no
   heartbeat) and needs no new API.
4. **AIS is decoded twice on purpose.** AIS-catcher could print JSON, but its field set changes
   between versions and flags; the NMEA sentence (`!AIVDM`, IEC 61162-1, `-o 1`) is a stable
   standard. The adapter decodes the ITU-R M.1371 payload itself (types 1, 2, 3, 4, 5, 9, 11, 18,
   19, 21, 24, 27), which makes the parser testable with published vectors and independent of
   AIS-catcher's output options.
5. **Event rationing instead of a track log.** An AIS receiver in a harbour hears thousands of
   position reports a minute; one event per message would put millions of rows a day into the
   history. The AIS adapter merges messages by MMSI and writes one event per vessel per
   `event_interval_s` (default 60) plus one for every news: first sight, first position,
   new or changed static data. Every event carries the merged picture, so the newest event per
   MMSI is the map and the older ones are the track. ACARS and VDL2 messages are rare and each
   is an event.
6. **Web: the digital-mode dock.** AIS, ACARS and VDL2 are three more "Digital" modes
   (`digital/modes.ts`): picking one sets the receiver's bandwidth, squelch off and starts the
   plugin; hints suggest them on their frequencies; the dock shows a message list and, for AIS,
   a **Map** view built on the shared Leaflet base map (`map/BaseMap.tsx`, the tile proxy of
   ADR-0017) with ship symbols by type and heading, tracks, labels from zoom 10 and a search
   list; speeds are shown in knots and km/h. ACARS and VDL2 rows get an **On the ADS-B map**
   button when the ADS-B decoder currently sees the aircraft: by ICAO address (VDL2) or by
   registration through the imported aircraft database (ACARS, ADR-0026). The ADS-B overlay
   takes the aircraft to select from its store (`focus`), no new endpoint.
7. **Bookmarks store the new modes.** The digital modes of bookmarks are a database check
   constraint (migration 0020). Migration 0021 extends it with `ais`, `acars`, `vdl2` and the
   weak-signal modes `ft8`, `ft4`, `wspr` (ADR-0032); the API enum, the web model and the
   picker bridge (`bookmarks/digital.ts`) follow, so a bookmark saved while one of them runs
   starts it again when loaded.
8. **Receiver frequency from the start request.** ACARS and VDL2 need the IQ's centre to
   place their channels. The adapter takes it from `SUPERHET_FREQ_HZ`, the frequency of the
   receiver when the instance was started; there is no channel change on retune, the user restarts
   the decoder (documented in the manifest, the mode's help text and `docs/plugins-acars.md`).
   AIS needs no centre (both channels at fixed offsets from 162.000 MHz are inside the filter,
   AIS-catcher looks at both).

## Consequences

- The image grows by AIS-catcher (about 1.5 MB), acarsdec (0.1 MB), dumpvdl2 (about 1.5 MB) and
  libacars with libcjson/glib shared libraries; the build stages need network access to GitHub.
- Receiving ACARS/VDL2 is legally sensitive in some countries (Germany: the eavesdropping ban of
  section 5 TDDDG, formerly TTDSG, for radio traffic not meant for the receiver). The install dialogs say so, installing is an admin
  decision, and the history is visible to everybody who may see decoders; operators who share a
  public receiver (share links, ADR-0029) should not enable them. AIS is a public broadcast.
- No recordings of the three signals are committed. The adapter tests run on stand-in tools and
  sample output in the documented format; the image checks run the real tools on a synthetic AIS
  signal and on silence. A real-antenna check of the three is manual (`docs/plugins-ais.md`,
  `docs/plugins-acars.md`).
- ACARS and VDL2 cannot be received at the same time on one dongle (5 MHz apart); two dongles
  (ADR-0020) or alternating instances are needed.
- The AIS map reads the newest events of the instance (2000); a very busy harbour shows the last
  minutes only unless `event_interval_s` is raised.
