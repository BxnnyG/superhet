# ADR-0018: Digital voice plan: M17 bundled, everything encumbered stays external

**Status:** accepted (2026-10-28)

## Context

P3 adds digital voice decoders. What a voice mode needs on top of the decoder is a vocoder, and the
vocoders differ completely in what we may ship (ADR-0005 set the principle, this ADR applies it to
each mode and records the first one that is built):

| Mode | Vocoder | Can the project ship it? |
|---|---|---|
| **M17** | Codec2 3200 (LGPL-2.1, David Rowe) | **Yes.** The protocol is an open amateur radio specification, Codec2 is open source and designed to be free of patents. |
| DMR, NXDN, dPMR, YSF (DN/VW) | AMBE+2 / AMBE+ (DVSI) | **No.** DVSI patents; the open implementation (`mbelib`) has an unclear patent status. |
| D-Star | AMBE (DVSI) | **No**, same reason. |
| P25 phase 1 | IMBE (DVSI) | **No**, same reason. |
| TETRA | ACELP (ETSI EN 300 395-2) | **No.** The reference code is under ETSI's IPR terms and may only be downloaded from ETSI. |

"Not shippable" is a statement about this repository, its container images and its releases, not
about what a user may do on their own machine; the legal situation of building and running
`mbelib` or the ETSI codec depends on the country and on what is received. The project therefore
takes no position and keeps the responsibility with the user (ADR-0005).

The user's TETRA goal is unchanged: own TMO cell and own MTM DMO hotspot, voice is the primary
goal (ADR-0004), and that voice needs the ETSI ACELP codec, so it will always arrive through a
user-installed codec plugin.

## Decision

1. **M17 is bundled now** as a regular decoder plugin (`org.superhet.decoder.m17`,
   `plugins/m17`, `docs/plugins-m17.md`). It is the first P3 mode because it is the only one that
   delivers voice without any restricted component, which makes the whole path (VFO tap,
   decoder, voice audio to the browser, call log, "now talking") testable and shippable.
2. **Decoder: `m17-demod` of mobilinkd's m17-cxx-demod** (GPL-3.0), a separate process, built in
   the Dockerfile (stage `m17`) from a pinned upstream commit, with Codec2 and Boost from
   Debian (dynamic) and the Blaze maths headers (BSD-3-Clause) from a pinned commit. Nothing
   is vendored into the repository. The tool decodes Codec2 itself and writes 8 kHz PCM, so M17
   does not use the codec router (`voice_frames` stay reserved for the restricted codecs below).
   Alternatives considered: `dsd-fme` (also decodes M17, but is built against `mbelib`, which
   must not enter the image); `libm17` (MIT, a library and no demodulator program); a native Rust
   M17 demodulator (a reasonable later project, M17 is small, but m17-cxx-demod exists, is
   in use by OpenWebRX+ and is good enough to start).
3. **One patched line.** Upstream prints the stream type only as `STR:V/V CAN:03`; the encryption
   bits are not visible. A single `sed` expression (`deploy/docker/m17-demod-type.sed`) adds the raw
   TYPE field to that line at build time. The adapter works without it (encryption then unknown,
   no muting), and the image build fails if the patch does not apply.
4. **Encrypted calls are recognised and not played** (the audio would be noise). They are
   reported in the events (`encrypted`, `encryption`, `muted`) and shown in the UI; they are never
   decrypted. A config switch (`mute_encrypted`) turns the muting off.
5. **Everything else stays external**, as ADR-0005 says and now without exception:
   - DMR, D-Star, YSF, NXDN, dPMR, P25: decoder plugins may be written (digiham, dsd-fme with the
     vocoder removed so it emits raw frames), but the vocoder is a **user-installed codec
     plugin** (`superhet-codec-mbe`, separate repository) that fetches mbelib from its origin,
     verifies a pinned checksum, builds locally and shows the licence/patent notice for
     confirmation. A decoder that can only be built with `mbelib` linked is distributed as a
     "fetch and build locally" plugin as well. Without the codec the decoder still produces all
     events and the UI says "codec not installed".
   - TETRA voice: ETSI ACELP through `superhet-codec-tetra` (user-installed), signalling decoded by
     the native Rust stack in this repository (ADR-0004).
   - Hardware vocoders (DV3000 / ThumbDV) are the legal route for AMBE and may come as an adapter
     later.
6. **Guard rail.** ROADMAP P3 criterion 6 (a CI script scanning repository, image and release
   artifacts for the known restricted file names and hashes) is implemented together with the first
   restricted-codec plugin; until then the rule is enforced by review and by `CLAUDE.md`.

## Consequences

- The runtime image grows by about 17 MB: Codec2's library is 16.8 MB (it carries the
  codebooks of all its modes), `m17-demod` 0.1 MB, Boost.Program_options 0.4 MB. The build stage
  adds a few minutes to a cold image build (C++20, Blaze templates).
- A second GPL-3.0 tool in the image; like the others it runs as a separate process and its
  licence texts are in the image (`docs/third-party.md`).
- Upstream limits are inherited and documented: the decoder needs about a second to lock onto a
  call (the beginning of each transmission can be missing), packet-mode M17 is not decoded, only
  voice streams.
- The M17 plugin binds to a VFO like POCSAG does, but its audio is a plugin stream (pseudo VFO) and
  the VFO's own audio (the raw 4FSK baseband) is muted by the web UI while it runs.
- Listeners without the operator role cannot discover plugin instances, so they cannot hear M17
  audio until plugin audio streams are exposed to them read-only (open point, same gap as for
  decoder status).
