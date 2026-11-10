# ADR-0025: Scanner in the device daemon, detection on the spectrum

**Status:** accepted (2026-11-10)

## Context

ROADMAP P4 asks for a scanner: frequency ranges and channel lists, stop on a signal, resume after a
hang time, skip and lock out channels, a priority channel. SDR# scans by stepping a VFO and
watching its squelch; OpenWebRX does the same per profile. With one RTL-SDR that is slow and
audible (the VFO hops through every channel), and it cannot look at more than one channel at a
time. Superhet already has what a faster scanner needs: the FFT of the whole window is computed
anyway, and `superhet-sdrd` owns the device, the VFOs and the tuning lock.

## Decision

1. **The scanner lives in `superhet-sdrd`**, one per device, as its own thread
   (`crates/superhet-sdrd/src/scanner/`). It is controlled with `ctl.scanner.<action>` on the bus,
   exposed as `POST /api/v1/devices/{dev}/scanner/{action}` (operators and admins) and reports in
   the `scanner` object of the device state document, so every viewer sees it.
2. **Detection uses the spectrum frames** the daemon publishes (the DSP thread hands the encoded
   frame to a probe that is switched off unless a scan runs). All channels inside the current
   window are measured on the same frame: the power of a channel (mean over its bins) against a
   noise floor estimate (30th percentile of the usable bins), or against an absolute dBFS level.
   A scan inside the visible span therefore never retunes and its dwell time is per window, not
   per channel. Channels that do not fit are grouped into windows (one device centre each, centred
   on the group) and the device is retuned between them; the first window is the current one.
   Rejected: stepping a VFO and reading its squelch (slow, audible, one channel at a time).
3. **A pure state machine decides** (`machine.rs`: settling, dwell, holding with signal/hang,
   blocked), driven by time and frames, so stop, hang, resume, lockout, skip, pause and priority
   are tested with synthetic spectra and with a synthetic IQ file through the real pipeline.
4. **The scanner is a lock holder** (`kind: "scanner"`, alongside `user` and `claim`). While it
   runs nobody else changes centre or sample rate, so the window it plans stays valid. A user lock
   that appears keeps the scan inside the current window (a retune is refused, the scan carries
   on there; starting is refused if nothing qualifies). An **exclusive plugin claim wins**: it
   releases the scanner's hold and the scan ends with `stopped: "claimed"`; a scan cannot start
   while a claim exists.
5. **It listens through one existing VFO** (the active one, or `vfo`): on a stop the daemon moves
   that VFO to the channel (frequency, mode, bandwidth). Audio, recording, decoders and the
   audio WebSocket work as for any VFO.
6. **Hits are history.** Each stop is published on `sh.v1.dev.<dev>.scanner.hit` and stored by the
   worker in the decoder history (`source` `scanner`, kind `scanner.hit`); the last 25 are also in
   the state document so the tab works without the database. No migration: the history schema
   already allows any source with a plugin-style kind.

## Consequences

- Dwell and detection quality follow the spectrum settings: narrow channels need enough bins
  (default 2048 bins, about 1 kHz per bin at 2 MS/s). The noise floor estimate is poor on a band
  that is mostly occupied; the absolute level is the answer there.
- Narrow signals shorter than a frame (40 ms at 25 fps) can be missed; the confirmation needs two
  consecutive frames after the dwell.
- While the scanner holds the lock, manual centre changes get `locked` ("tuning is locked by:
  scanner"); stop the scan first. VFOs still move inside the window.
- The scanner does not survive a daemon restart (the VFOs and the user lock do); the lockout list
  is kept in memory per daemon run.
- Priority checks across windows happen between windows; while holding, only a priority channel in
  the same window can interrupt.
