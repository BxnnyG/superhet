# ADR-0013: DAB/DAB+ via plugin

**Status:** accepted (2026-10-26); implementation details in [ADR-0016](0016-dab-plugin-design.md)

## Context

DAB/DAB+ (ETSI EN 300 401) is the digital broadcast radio standard in Germany. One ensemble is a 1.536 MHz wide OFDM signal carrying several services (stations) with DLS text and slideshow (MOT). That fits into the RTL-SDR's 2.4 MS/s window (about 2.0 MHz usable, SCOPE "Hardware constraints"), so one device receives a whole ensemble, but then the device does nothing else. A native stack (OFDM sync, FIC/MSC, Viterbi, Reed-Solomon, DAB+ superframes, MOT) is a large project, and mature GPL implementations exist.

## Options

- **Native Rust DAB stack** in `superhet-dsp`: full control, no process boundary; very large effort for a feature that is not the project's differentiator.
- **Plugin wrapping welle-cli** (welle.io, GPL-2.0-or-later): complete receiver with service list, DLS, slideshow and audio output.
- **Plugin with eti-cmdline + dablin** (GPL): proven chain, more moving parts, less integrated.

## Decision

**DAB/DAB+ is an out-of-process plugin, not part of the core.** The first implementation wraps `welle-cli`; `eti-cmdline` + `dablin` is the fallback. Plugin type `decoder` with audio output (ADR-0003); the manifest declares it exclusive.

- **Exclusive claim**: the plugin requests a DAB block (Band III, e.g. 5C-13F) as centre frequency and 2.048-2.4 MS/s. sdrd applies the claim as in ARCHITECTURE 3.4 (snapshot state, set centre/rate, take a lock, pause every VFO and decoder that does not fit, restore on release). The plugin receives the whole window as IQ (the "full-band IQ, exclusive" path, SCOPE section 6) and feeds the wrapped tool through its adapter.
- **Outputs**: service list (ensemble and service labels, ids, bitrate, DAB vs DAB+), DLS text per service, slideshow images (MOT), signal quality (SNR, sync, FIC CRC rate) and audio of the selected service. Audio decoded by the wrapped tool is **re-encoded to Opus** and delivered over the normal audio path (`docs/protocol-audio.md`), so browser playback, recording and Instant Replay work as for analogue VFOs.
- **Roles** (ADR-0011): only operators/admins claim the device and change the ensemble; listeners pick among the services of the active ensemble; guest access follows the guest policy.
- **Licensing**: welle.io is GPL-2.0-or-later and runs as a separate process, so the AGPL-3.0 core is unaffected (ADR-0003, `docs/licenses.md`). **Nothing is vendored into this repository.** AAC/HE-AAC decoding uses the libraries of the wrapped tool (faad2, GPL-2.0; or fdk-aac, whose licence is non-OSI and carries patent clauses that restrict redistribution of some builds). The plugin manifest states which AAC decoder is used and the licence implications; the install flow shows them (as for codec plugins, ADR-0005). If a licence does not allow shipping binaries, the plugin builds locally.
- **Phase**: P2, with the other first plugins, after the plugin host exists. Verified with a committed reference IQ recording of an ensemble (golden test of service list, a DLS string and a decoded audio frame count).

## Consequences

- While DAB runs nothing else uses the device. The UI shows "exclusive: DAB", listeners' VFOs are paused and return on release.
- Full-ensemble IQ and AAC decoding cost CPU (measured when the plugin exists, documented in `docs/perf.md`).
- The 8-bit ADC and a good Band III antenna matter more than for FM; the plugin shows signal quality so users can aim the antenna.
- Dependence on welle.io maintenance is contained: the process-based contract lets another implementation (or a native stack) replace the plugin without touching the core.
- Receiving DAB broadcasts is legally unproblematic in Germany; the AAC patent/licence topic stays with the separately licensed components.
