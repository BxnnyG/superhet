# ADR-0032: FT8, FT4 and WSPR through the WSJT-X decoders

**Status:** accepted (2026-11-14)

## Context

FT8, FT4 and WSPR are the most active weak-signal amateur modes and a natural fit for a web SDR:
many stations, a decode every 15 s, and a map of who is on the air. They differ from the streaming
modes the other decoder plugins handle (POCSAG, APRS, M17): a station transmits during a fixed UTC
time slot (FT8 15 s, FT4 7.5 s, WSPR 2 min), and the decoders work on the audio of one whole slot
as a file, not on a stream. They also need an exact dial frequency (the signals are 1.5 to 6.25 Hz
wide) and a correct clock. The question is which decoder to run and where the slot logic lives.

## Decision

1. **Decoder: the WSJT-X command-line tools.** `jt9` decodes FT8 and FT4 (`-8` / `-5`), `wsprd`
   WSPR; they are the reference implementations, the most sensitive and the ones every other
   program is measured against. They are GPL-3.0-or-later and run as **separate processes**
   (ADR-0003); nothing is linked or vendored. The image takes `jt9` and `wsprd` from Debian's
   `wsjtx` package in a build stage (the package pulls in the Qt GUI, so only the two programs and
   the copyright file are copied, like `welle-cli` for DAB), plus `libqt5core5a`, `libgfortran5`
   and `libgomp1` (about 60 MiB, most of it ICU data that `jt9` links through Qt Core).
   Considered and not taken: `ft8_lib` (MIT, builds to a few hundred kB, but FT8 only in our
   tests, FT4 did not decode a generated signal, and noticeably less sensitive than `jt9`), a
   Rust port (a large, security-neutral piece of numerical code to maintain for no gain), and
   building WSJT-X from source in the image (needs Qt for the build and a Fortran toolchain; the
   Debian binary is the same code).
2. **Slot logic in the adapter, one plugin for the three modes.** `plugins/ft8` is a simple-transport
   decoder with `input.kind = audio`, 12 kHz `s16le`: the host already feeds the audio of a VFO
   (`docs/plugin-contract.md`, "Feeding input"), so a USB VFO on the dial frequency is all it
   needs. `superhet-plugin-adapter ft8` cuts the stream into slots aligned to UTC, writes WAV
   files, runs the tool per slot on a second thread and parses its output into events
   (`ft8.message`, `wspr.spot`). The mode is a config key (`ft8`, `ft4`, `wspr`), like the DMR /
   dPMR / NXDN protocol of the dsd plugin; one instance decodes one mode.
3. **Time from the wall clock of the adapter.** The samples carry no timestamps. The adapter
   takes the time of a read as the time of the last sample, fills gaps with silence and trims a
   stream that runs ahead, so a slot stays aligned to about the jitter of the bus (tens of
   milliseconds, far below the roughly 2 s the decoders tolerate). The user must run NTP; the
   docs say so. A debug variable (`SUPERHET_FT8_STREAM_START_MS`) replaces the wall clock with the
   sample count so tests and recordings decode faster than real time.
4. **No database change, no new environment variable of the host.** Events are ordinary plugin
   events in the decoder history. The tools are found on `PATH` (the image, or a package install);
   a missing tool ends the plugin with status 127 and a clear message, and the supervisor's usual
   backoff and `error` state apply.
5. **UI: a digital mode like the others.** The mode bar gets FT8, FT4 and WSPR (USB, 3 kHz,
   squelch off), a band picker with the dial frequencies, a "Try FT8?" chip on those frequencies,
   a decode table with locator links to OpenStreetMap. A band change restarts the decoder, because
   the dial frequency the plugin reports absolute frequencies against is given at start. Plotting
   locators on the Leaflet map of the ADS-B / APRS views and reporting spots to PSK Reporter /
   WSPRnet (data about the user's station leaves the box) are follow-ups.

## Consequences

- The three modes work with any receiver that can tune the dial frequency in USB: 6 m and 2 m with
  a plain RTL-SDR, HF with an upconverter or direct sampling. Frequency accuracy (PPM) of the
  stick decides whether 2 m decodes at all; the docs say so.
- Decodes appear a few seconds after the end of a slot (the decoder runs on the finished slot), not
  during it like WSJT-X's early decode.
- The image grows by about 60 MiB because `jt9` links Qt Core (and with it ICU). A `jt9` built
  without Qt, or `ft8_lib` for FT8 once it decodes as well, would shrink it.
- The integration tests run real `jt9` / `wsprd` on synthetic signals when the tools are installed
  (CI installs `wsjtx`), and stand-in scripts otherwise, like the other adapters.
- Slot cutting by wall clock cannot compensate a wrong system clock; a decoder with a clock check
  (decode the time-sync from the signals themselves) is not planned.
