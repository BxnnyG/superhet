# ADR-0005: Codec plugin concept

**Status:** accepted (2026-10-20)

## Context

Digital voice decoders produce compressed voice frames: TETRA ACELP (ETSI EN 300 395-2 reference code, ETSI IPR terms), AMBE / AMBE+2 / IMBE (DVSI patents, software implementation mbelib under ISC with unclear patent status), Codec2 (LGPL-2.1, patent-free). Superhet's repository, images and releases must not contain the restricted codecs, but users must be able to add them. OpenWebRX solves the same problem with *codecserver* (GPL-3.0, separate daemon with mbelib and DV3000 hardware modules) – the concept is proven; codecserver itself has seen little maintenance since 2024.

## Decision

- New plugin type **`codec`** (framed transport): input `voice_frame` (codec id, call id, timestamp, bytes), output PCM s16le with declared rate. Routing by codec id is done by the host's codec router (ARCHITECTURE §6.4).
- Restricted codecs live in **separate repositories** (`superhet-codec-tetra`, `superhet-codec-mbe`). Their repos contain only build scripts, adapter code and checksums. At install time they **download the source from the original location** (etsi.org, the mbelib repository), verify a pinned checksum, and build locally. The UI shows the license/patent notice from `[install].acknowledge` and requires explicit confirmation.
- Codec2 is a regular plugin and may be shipped as an optional prebuilt image.
- Decoders never depend on a codec: missing codec → frames dropped, metadata and events continue, UI shows "voice: codec not installed".
- Decoder plugins whose upstream *links* a restricted codec at build time (e.g. dsd-fme with mbelib) are either patched to emit raw frames, or are themselves distributed only as "fetch & build locally" plugins. Which DMR upstream we use (dsd-fme vs. digiham) is decided by a spike at the start of P3.
- A codec adapter for an existing codecserver instance (incl. DV3000/ThumbDV hardware) is an optional later plugin.

## Consequences

- Clear legal separation; responsibility for codec installation is with the user.
- Install flow needs network access to the upstream source; upstream URL changes break installs → pinned URL + checksum, fail loudly with instructions for manual download.
- Frame formats per codec id must be specified exactly (bit order, FEC applied or not) in the protocol docs before P3 implementation.
