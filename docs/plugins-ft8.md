# FT8, FT4 and WSPR plugin

`plugins/ft8` (`org.superhet.decoder.ft8`) decodes the weak-signal amateur modes **FT8**, **FT4**
and **WSPR**. It runs the command-line decoders of WSJT-X (`jt9` for FT8 / FT4, `wsprd` for WSPR;
GPL-3.0-or-later, separate processes) behind `superhet-plugin-adapter ft8`. Decision record:
ADR-0032. Contract: `docs/plugin-contract.md`.

Alternatives: WSJT-X itself (the reference, desktop, also transmits), JTDX, `ft8_lib` (MIT, FT8
only and a little less sensitive), OpenWebRX+ (decodes FT8 / FT4 / WSPR with the same WSJT-X
tools). Superhet only receives.

## What you need

- **A receiver on the dial frequency, upper sideband.** The plugin takes the demodulated USB audio
  of a VFO (12 kHz, 3 kHz wide) and decodes the signals at 200 to 3000 Hz above the dial. The VFO
  must sit on the dial frequency (the table below), not on the signal.
- **HF needs more than a plain RTL-SDR.** The stick tunes from about 24 MHz up, so the HF bands
  (160 m to 10 m) need an upconverter in front of it, a stick with direct sampling or a built-in
  upconverter, or another receiver (`docs/hf-comfort.md` covers the HF listening aids). **6 m
  and 2 m work without**; 2 m FT8 on 144.174 MHz is quiet but real. The weak-signal modes ask for
  a good antenna and, on 2 m, a low-noise preamplifier.
- **An accurate frequency.** FT8 signals are 6.25 Hz wide and WSPR 1.5 Hz: an RTL-SDR with an
  uncorrected crystal (tens of ppm) is off by hundreds of Hz on 2 m and decodes badly or not at
  all. Set the PPM correction of the device (or use a TCXO stick) before judging the decoder.
- **An accurate clock.** The decoders look at signals inside a fixed time slot (FT8 15 s, FT4
  7.5 s, WSPR 2 min, starting at multiples of the slot length in UTC). The plugin cuts the audio
  at those boundaries by the **system clock** of the machine that runs it, so run NTP (`chrony`,
  `systemd-timesyncd`); about 1 s of error is tolerated. The container uses the host's clock.

## Using it

The tuning strip's **Digital** select lists **FT8**, **FT4** and **WSPR** (when the plugin is
installed; it ships in the image). Picking one sets the receiver to USB with 3 kHz and starts the
decoder; the **Band** select next to it moves the receiver to the dial frequency of a band and
restarts the decoder there (the decoder is told the dial frequency when it starts, to report
absolute frequencies). A chip offers the mode when the receiver already sits on one of the
frequencies below.

The output panel lists decodes, newest first: time, station, SNR / time offset / frequency, the
message (FT8 / FT4) or locator, power and drift (WSPR), and an "Open locator on map" link
(OpenStreetMap, opened only when you click it). The first decodes come after the first complete
slot (up to 30 s for FT8, up to 4 min for WSPR).

### Dial frequencies (USB, as in WSJT-X)

| Band | FT8 | FT4 | WSPR |
|---|---|---|---|
| 160 m | 1.840 | | 1.8366 |
| 80 m | 3.573 | 3.575 | 3.5686 |
| 60 m | 5.357 | | 5.2872 |
| 40 m | 7.074 | 7.0475 | 7.0386 |
| 30 m | 10.136 | 10.140 | 10.1387 |
| 20 m | 14.074 | 14.080 | 14.0956 |
| 17 m | 18.100 | 18.104 | 18.1046 |
| 15 m | 21.074 | 21.140 | 21.0946 |
| 12 m | 24.915 | 24.919 | 24.9246 |
| 10 m | 28.074 | 28.180 | 28.1246 |
| 6 m | 50.313 | 50.318 | 50.2930 |
| 4 m | | | 70.0910 |
| 2 m | 144.174 | 144.170 | 144.4890 |

(MHz; the list is `web/src/ft8/model.ts`. Calling frequencies differ by region and change; check
the band plan of your country.)

## Configuration

`plugins/ft8/schema/config.json`; the mode bar sets `mode`, the settings dialog takes JSON.

| Key | Meaning |
|---|---|
| `mode` | `ft8` (default), `ft4` or `wspr`. One instance decodes one mode; start several on different receivers for several bands. |
| `depth` | 1 to 3, default 2. Decoder search depth (`jt9 -d`; for `wsprd` 3 adds its deeper search). More finds more and needs more CPU; FT8 at depth 3 can take several seconds per slot on a small board. |
| `dial_hz` | Dial frequency the audio offsets are added to. Default: the frequency of the receiver when the decoder started. |

## Events

`plugins/ft8/schema/events.json`. Both carry a `summary` line; times are UTC.

- `ft8.message` (FT8 and FT4): `mode`, `time` (slot start, ISO 8601, FT4 with milliseconds),
  `snr` (dB in 2500 Hz), `dt` (seconds from the slot start), `freq_hz` (audio offset), optional
  `dial_hz` and `rf_hz`, `message`, `cq`, and where the message has a standard shape `callsign`
  (the sender), `to` and `grid`. Free text, contest exchanges and hashed calls (`<...>`) stay a
  plain `message`.
- `wspr.spot`: `mode`, `time`, `snr`, `dt`, `freq_hz`, optional `dial_hz` / `rf_hz`, `drift`
  (Hz/min), `callsign`, optional `grid`, `power_dbm`, `message`.

They go through the normal decoder history (retention `SUPERHET_EVENT_RETENTION_DAYS`) and can
drive notification rules (`docs/notifications.md`), for example "a new DXCC on 6 m".

## How it works

The host feeds the plugin the USB audio of the VFO as 12 kHz `s16le` (`input.kind = audio`). The
adapter timestamps each read with the wall clock, fills gaps (dropped chunks) with silence so the
slot stays aligned, drops samples if the stream runs ahead of the clock, and hands every complete
slot to a second thread that writes `YYMMDD_HHMMSS.wav` into a work directory
(`$TMPDIR/superhet-ft8-<pid>`, removed at the end; the tools keep their FFT wisdom and callsign
hash table there), runs the decoder on it and turns each output line into an event. Silent slots
(closed squelch, no audio) are not decoded. A slot that started late is decoded when at most
0.5 s (FT8, FT4) or 1 s (WSPR) is missing; the slot in progress when the decoder starts is
skipped. A decoder run is killed after twice the slot length (at least 20 s). If the decoder
cannot keep up, slots are skipped (and logged) rather than holding the audio back.

`SUPERHET_FT8_STREAM_START_MS` (adapter environment, not set by the host) replaces the wall clock
with the sample count counted from that UTC time in milliseconds: it decodes a recording as fast as
it can be read, which is how the integration tests and `scripts/check-plugin-image.sh` run
(`scripts/gen-ft8-audio.py` writes a synthetic slot).

## Tools and licences

`jt9` and `wsprd` come from the Debian `wsjtx` package (WSJT-X 2.6.1, GPL-3.0-or-later). The
Dockerfile installs the package in a build stage and copies only the two programs and the copyright
file; the runtime image adds `libqt5core5a`, `libgfortran5` and `libgomp1` (about 60 MiB in all,
most of it the ICU data of Qt). They run as separate processes, receive only, and open no network
ports. Outside the image: install `wsjtx` (or build WSJT-X) so that `jt9` and `wsprd` are on the
plugin host's `PATH`; without them the plugin exits with status 127 and the message
"cannot find jt9 on PATH".

## Limits

- Receive only; no transmit, no logging to PSK Reporter / WSPRnet (a follow-up could report spots,
  but that sends the user's station data to a third party and needs its own opt-in).
- FT8 / FT4 are decoded from the audio of the VFO the plugin is bound to; there is no wideband
  multi-band decoder yet (one receiver per band).
- Message types beyond the standard shapes (DXpedition mode, contest exchanges, telemetry,
  non-standard calls) appear as plain messages without `callsign` / `grid`.
- Slots are cut by the wall clock; a machine with a wrong clock decodes nothing or misses signals.
