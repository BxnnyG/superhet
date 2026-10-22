# Quality-of-life backlog

Small and medium features that make daily use nicer. Collected from user testing; each item names
the roadmap phase it belongs to. Items move to "Done" with the release that ships them.

Status: **now** = in progress, **next** = planned for the next alpha, **later** = needs a bigger
phase (plugins, recording) first.

## Tuning and display

| Item | Status | Phase / notes |
| --- | --- | --- |
| Waterfall history kept on zoom, pan, retune | done (alpha.3) | P1 |
| Click preview: cursor line, snapped frequency, ghost passband, glide | done (alpha.3) | P1 |
| Clear marking of the usable tuning window | done (alpha.3) | P1 |
| Band plan overlay (broadcast, amateur, PMR446, Freenet, LPD, CB, air, marine, ISM, BOS/TETRA range, business radio), per-category toggles | now | pulled forward from P4 |
| Click a band-plan channel to tune with its mode and bandwidth | now | P4 |
| Keyboard shortcuts (step up/down, next/previous VFO, mute, bookmark) | next | P1 polish |
| Peak hold / average trace in the spectrum | next | P1 polish |
| Snap-to-signal (click lands on the nearest peak within the passband) | next | P1 polish |
| Drag a VFO's passband edges to change bandwidth | next | P1 polish |
| Mini-map / overview of the full span while zoomed in | later | P1 polish |

## VFOs and audio

| Item | Status | Phase / notes |
| --- | --- | --- |
| Top readout tunes the active VFO; centre as secondary control | done (alpha.3) | P1 |
| Per-VFO listen toggle, several VFOs mixed | done (alpha.3) | P1 |
| Squelch Auto (noise floor of the last 3 s + margin) | done (alpha.3) | P1 |
| Default VFO on a fresh install | done (alpha.3) | P1 |
| Per-VFO volume and stereo pan (left/right split for two VFOs) | next | P1 polish |
| WFM stereo + RDS (station name, radio text) | next | P1 crit. 4 / RDS decoder in P2 (redsea) |
| USB/LSB/CW demodulators | next | P1 crit. 4 |
| Noise reduction / notch filter | later | P1 polish |

## Memory and data

| Item | Status | Phase / notes |
| --- | --- | --- |
| Bookmarks (own memory channels) in PostgreSQL, markers on the spectrum | now | pulled forward from P4 |
| Import/export: Superhet JSON, gqrx CSV, SDR++ frequency manager | now | P4 crit. 4 (SDR# XML still open) |
| SDR# Frequency Manager XML import/export | next | P4 crit. 4 |
| External frequency data | later | BNetzA publishes the German Frequenzplan only as PDF, with no API, so the built-in band plan is curated in the repo. Community lists (e.g. RadioReference) need API keys or forbid redistribution. A JSON import hook allows user-supplied lists. |
| Scanner over bookmarks / band-plan channels | later | P4 |
| Recording, Instant Replay | later | P4 |

## Digital modes

| Item | Status | Phase / notes |
| --- | --- | --- |
| RDS | next | see above |
| DAB/DAB+ | later | Full OFDM receiver over ~1.5 MHz; best as a decoder plugin wrapping welle.io or dablin (GPL, separate process) in P2+. |
| POCSAG, APRS, rtl_433, ADS-B | later | P2 plugins |
| DMR/TETRA etc. | later | P3 (own equipment only, see README legal notice) |
