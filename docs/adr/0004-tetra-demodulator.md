# ADR-0004: TETRA receive stack (TMO + DMO)

**Status:** accepted (2026-10-20)

## Context

Goal: listen to **our own TETRA equipment** – a self-operated TMO cell and DMO operation (e.g. an MTM-series mobile as DMO gateway/repeater/hotspot) – including **voice**, SDS, status messages and full signalling metadata. BOS networks are explicitly not a target (encrypted, legally off-limits).

TETRA uses π/4-DQPSK at 18 ksym/s (36 kbit/s) in 25 kHz channels. TMO (EN 300 392-2) and DMO (EN 300 396) share the physical layer but differ in burst structure, synchronisation and MAC/DLL.

Upstream status (2026-10):
- **osmo-tetra** (AGPL-3.0, gitea.osmocom.org; sq5bpf fork): GNU Radio demod + C decoder, TMO only in practice.
- **tetra-kit** (GPL-3.0): C++ decoder with good TMO metadata coverage, JSON output; expects bits from an external (GNU Radio) demod; no DMO.
- No maintained open-source DMO decoder is known to us → to be confirmed by a short spike, but we must plan for building DMO ourselves.

## Options

1. **GNU Radio container (osmo-tetra demod) + tetra-kit**: fastest TMO metadata, but ~1 GB image, Python/GNU Radio drift, and **no DMO** → would be a dead end for half of the requirement.
2. **Native Rust TETRA stack** (`superhet-tetra` crate + decoder plugin): PHY (RRC matched filter, frequency/timing recovery, π/4-DQPSK differential detection, burst sync for TMO and DMO training sequences), lower MAC (descrambling, deinterleaving, RCPC/Reed-Muller decoding, CRC), upper MAC / LLC / MLE / CMCE / SDS for TMO, DMAC/DCC for DMO, ACELP traffic frame extraction.
3. Hybrid: option 1 for TMO now, option 2 later for DMO.

## Decision

**Option 2 is the target and the default plugin.** We build one TETRA stack for TMO and DMO, structured in layers with separate test suites. upstream tools are used as **test oracles**, not as runtime:

- osmo-tetra/tetra-kit run in CI (dev container only) against our SigMF reference recordings; for TMO our decoded PDUs must match theirs (or be a documented superset).
- Reference recordings come from our own equipment (own cell, own DMO hotspot) – legally clean and covering exactly the target configurations.
- Option 1 may be packaged as an *interim* TMO plugin if the native stack is late, but nothing in the core depends on it.

## Consequences

- Significant effort (largest single item in P3); it is split into PHY → lower MAC → TMO signalling → DMO → voice frames, each with its own acceptance test.
- Voice output = `tetra-acelp` voice frames to the codec router; audio still requires the separately installed ETSI codec plugin (ADR-0005).
- No decryption features of any kind (TEA). Encrypted calls are shown as such.
- Same PHY code can later serve TETRA analysis features (signal quality, BER per burst, timing advance) that upstream tools do not expose.
