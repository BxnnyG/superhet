# ADR-0021: TETRA receive plugin: own demodulator, osmo-tetra's tetra-rx, the user's ETSI codec

**Status:** accepted (2026-11-07)

## Context

ADR-0004 chose a native Rust TETRA stack (PHY to voice frames, TMO and DMO) as the long-term decoder
and named a packaged osmo-tetra as an acceptable *interim* TMO plugin if the native stack is late.
ADR-0018 settled that the ETSI ACELP speech codec can never be shipped (ETSI IPR terms; the reference
code may only be obtained from ETSI) and must arrive through something the user installs. The user's
goal is unchanged: their own TMO cell and DMO equipment, voice first.

What exists today (checked, 2026-11):

- **osmo-tetra** (Osmocom, AGPL-3.0) decodes TMO signalling from a bit stream; its demodulator is a
  GNU Radio flow graph. It has no DMO support.
- **osmo-tetra-sq5bpf** (AGPL-3.0, the "telive" fork) adds, controlled by `TETRA_HACK_PORT`, a UDP
  interface with structured messages for the cell (`NETINFO1`), call set-up (`DSETUPDEC`,
  `DCONNECTDEC`, `DTXGRANTDEC`, `DRELEASEDEC`), status and short data, and with the voice blocks in the
  exact input format of the ETSI channel decoder. It builds on Debian bookworm against
  `libosmocore` with a plain `make`; a built `tetra-rx` decoded the burst that osmo-tetra's own test
  generator makes (MCC 262, MNC 42), which is how the parser here was verified.
- The ETSI reference codec is two programs, `cdecoder` (channel decoder) and `sdecoder` (speech
  decoder), that read and write files (`cdecoder in out`, `sdecoder in out`).

## Decision

1. **Interim TMO plugin now** (`org.superhet.decoder.tetra`, `plugins/tetra`, adapter mode `tetra`).
   ADR-0004's native stack stays the target for DMO and for deeper analysis; nothing in the core
   depends on this plugin, and the events (`tetra.*`) are what a later native decoder would also emit.
2. **Own demodulator in Rust** (`crates/superhet-plugin-adapters/src/tetra/phy.rs`) instead of GNU
   Radio: the plugin takes the VFO's complex baseband (`input.kind = iq`, `cf32`, 144 kHz, 25 kHz
   bandwidth), runs a root-raised-cosine matched filter (alpha 0.35), feed-forward symbol timing
   (Oerder and Meyr), differential pi/4-DQPSK detection with the carrier offset taken out of the phase
   steps, and writes one byte per bit to `tetra-rx`. No GNU Radio, no Python in the image. Burst and
   frame synchronisation stay in `tetra-rx`.
3. **`tetra-rx` of the sq5bpf fork is built into the image** (Dockerfile stage `tetra`, pinned
   commit, `make -C src tetra-rx`, `libosmocore19` at runtime, licence text copied). It runs as a
   separate process. Only `src/` is built and only the binary and `COPYING` are copied; the fork's
   `etsi_codec-patches/` directory (patches against the ETSI code) is not part of the image. A
   self-test in the build decodes the generator burst.
4. **The ETSI codec is the user's.** The adapter looks for `cdecoder` and `sdecoder` in the directory
   `SUPERHET_TETRA_CODEC_DIR` (passed through by the plugin host) and nowhere else, starts them as two
   processes connected by a pipe, and never downloads, builds or ships them. Without them the plugin
   runs signalling-only and says `codec.state = "missing"`; the web UI shows "voice needs the ETSI
   codec". This does **not** use the codec router or a `superhet-codec-tetra` plugin: those remain the
   long-term shape (ADR-0005, ADR-0018) and the user-installed directory is the smallest thing that
   gives voice today, in line with the DSD plugin.
5. **Encrypted traffic is recognised and never decrypted.** `tetra-rx` runs without `-e`; the MAC
   header's encryption mode flags a call (and the cell), its audio is withheld by default
   (`mute_encrypted`). No keys, no TEA code, no attempt.
6. **DMO is not decoded.** osmo-tetra has no DMO; the plugin says so. DMO remains the job of the
   native stack of ADR-0004.

## Consequences

- TMO downlink metadata and clear-mode voice work end to end in the design; **what is unverified** is
  recorded in `docs/plugins-tetra.md` (no real network or ETSI codec was available): the demodulator
  is tested against a synthetic transmitter and osmo-tetra's own test burst only, the datagram formats
  other than `NETINFO1` / `ENCINFO1` are read from the sources, and the codec's command line and
  buffering are assumptions from how it is commonly used.
- The image grows by `tetra-rx` (a few hundred kB) plus `libosmocore19` and its dependencies.
- The datagram interface of the fork is not a stable API; `tetmon.rs` is the one place to adapt, and the
  image pins the commit.
- Uplink is not received (the downlink carrier is what an SDR on the base station's frequency hears),
  so only the base station's side of calls is seen.
- A third plugin (after M17 and DMR) puts voice on a plugin pseudo VFO; nothing new in the host. The
  host passes two more path variables to plugins (`SUPERHET_TETRA_CODEC_DIR`,
  `SUPERHET_TETRA_RX_PATH`).
