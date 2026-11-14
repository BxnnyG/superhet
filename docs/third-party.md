# Third-party software in the runtime image

Superhet itself is AGPL-3.0-only. The runtime image (`deploy/docker/Dockerfile`) also contains
Debian packages that keep their own licences. They are installed unmodified from the Debian
archive, run as **separate processes** (the plugin host starts them through
`superhet-plugin-adapter`, nothing is linked into Superhet) and no source of them is vendored in
this repository. The only code the project forbids shipping is the ETSI TETRA codec and mbelib
(`CLAUDE.md`); neither is involved here.

| Package (Debian bookworm) | Version | Licence | Used by | Source |
|---|---|---|---|---|
| multimon-ng | 1.2.0 | GPL-2.0-or-later | plugin `org.superhet.decoder.pocsag` | https://github.com/EliasOenal/multimon-ng |
| direwolf | 1.6 | GPL-2.0-or-later | plugin `org.superhet.decoder.aprs` | https://github.com/wb2osz/direwolf |
| rtl-433 | 22.11 | GPL-2.0-or-later | plugin `org.superhet.decoder.rtl433` | https://github.com/merbanan/rtl_433 |
| welle.io (only `/usr/bin/welle-cli`) | 2.4+ds-2 | GPL-2.0-or-later | plugin `org.superhet.decoder.dab` | https://github.com/AlbrechtL/welle.io |
| readsb (wiedehopf; built in the Dockerfile, not a Debian package) | 3.16.20 | GPL-3.0-or-later | plugin `org.superhet.decoder.adsb` | https://github.com/wiedehopf/readsb |
| m17-demod (mobilinkd m17-cxx-demod; built in the Dockerfile, one line patched) | commit 9b8cec2 (2.2) | GPL-3.0 | plugin `org.superhet.decoder.m17` | https://github.com/mobilinkd/m17-cxx-demod |
| libcodec2-1.0 (Codec2 vocoder of M17) | 1.0.5 | LGPL-2.1 | m17-demod (dynamic library) | https://github.com/drowe67/codec2 |
| libboost-program-options1.74.0 | 1.74 | BSL-1.0 | m17-demod (dynamic library) | `apt-cache show libboost-program-options1.74.0` |
| libzstd1 | - | BSD-3-Clause / GPL-2.0 (dual) | readsb (dynamic library) | `apt-cache show libzstd1` |
| AIS-catcher (jvde; built in the Dockerfile, not a Debian package) | v0.71 | GPL-3.0 | plugin `org.superhet.decoder.ais` | https://github.com/jvde-github/AIS-catcher |
| acarsdec (f00b4r0 fork, 4.x; built in the Dockerfile) | v4.7 | GPL-2.0-only | plugin `org.superhet.decoder.acars` | https://github.com/f00b4r0/acarsdec |
| dumpvdl2 (szpajder; built in the Dockerfile) | v2.7.0 | GPL-3.0 | plugin `org.superhet.decoder.vdl2` | https://github.com/szpajder/dumpvdl2 |
| libacars (szpajder; built in the Dockerfile) | v2.2.1 | MIT | acarsdec, dumpvdl2 (dynamic library) | https://github.com/szpajder/libacars |
| libglib2.0-0, libcjson1, libxml2 | - | LGPL-2.1+ / MIT / MIT | dumpvdl2, acarsdec, libacars (dynamic libraries) | `apt-cache show libglib2.0-0` |
| libfaad2 (HE-AAC decoder of welle-cli) | 2.10 | GPL-2.0-or-later | welle-cli | https://github.com/knik0/faad2 |
| libfftw3-single3, libmp3lame0, libmpg123-0, libasound2, libairspy0, libsoapysdr0.8, libusb-1.0-0 | - | GPL-2.0+ / LGPL-2+ / MIT / BSL / Boost (each per its copyright file) | welle-cli (dynamic libraries) | `apt-cache show <package>` |
| librtlsdr0, libopus0 | - | GPL-2.0-or-later, BSD-3-Clause | sdrd (dynamic libraries) | `docs/licenses.md` |

Their dependencies (ALSA, hamlib, gpsd client library, SoapySDR, OpenSSL, ...) come with them via
`apt`; the complete list is `apt-cache depends multimon-ng direwolf rtl-433`. Corresponding source
for every Debian package is available from the Debian archive (`apt-get source <package>`); the
GPL obligations for redistributing the image are met by this and by the licence texts under
`/usr/share/doc/<package>/copyright` inside the image.

**DAB / welle-cli.** The Debian package `welle.io` also installs the Qt GUI (about 320 MB). The Dockerfile installs it in a build stage and copies only `welle-cli` and the package's copyright file into the runtime image; the libraries welle-cli needs come from their own Debian packages (about 7 MB in total). The binary is not modified or rebuilt; its source is `apt-get source welle.io`. The plugin decodes welle-cli's MP3 stream with the Rust crate `symphonia` (MPL-2.0, `crates/superhet-plugin-adapters`, linked into `superhet-plugin-adapter`). **Patents:** DAB+ audio is HE-AAC. Some AAC profiles are still covered by patents in some jurisdictions (the AAC-LC patents are expiring, SBR/PS are younger). faad2 is a GPL-licensed decoder without a patent licence; whoever redistributes the image or builds a binary for a jurisdiction where this matters should check; the plugin's install notice says so. MP2 (classic DAB) and MP3 (the hand-over format) are patent-free by now.

**ADS-B / readsb.** Debian bookworm has no readsb package. The Dockerfile compiles it in a build stage from the upstream commit of tag v3.16.20 (`READSB_COMMIT`), without ncurses and without SDR libraries (`make readsb DISABLE_INTERACTIVE=yes`), strips it and copies only `/usr/bin/readsb` and the upstream `COPYING` (as `/usr/share/doc/readsb/copyright`) into the runtime image. It is not modified; its source is the pinned upstream commit of https://github.com/wiedehopf/readsb. It runs as a separate process, reads IQ on stdin and opens no network port (ADR-0017).

**AIS, ACARS and VDL2 / AIS-catcher, acarsdec, dumpvdl2, libacars.** Debian bookworm packages none of them in a usable version. The Dockerfile compiles them in two build stages (`ais` and `datalink`) from the pinned upstream commits `AIS_CATCHER_COMMIT` (tag v0.71), `ACARSDEC_COMMIT` (v4.7), `DUMPVDL2_COMMIT` (v2.7.0) and `LIBACARS_COMMIT` (v2.2.1), with every SDR library, the web viewer, MQTT, databases, ZeroMQ and TLS switched off (the plugins feed IQ on stdin and read JSON or NMEA from stdout), strips them and copies only the binaries, libacars' shared library and the upstream licence texts into the runtime image. They are not modified; their sources are the pinned upstream commits. They run as separate processes and open no network port (ADR-0031).

**M17 / m17-demod.** Debian bookworm has no m17-cxx-demod package. The Dockerfile (stage `m17`) compiles `m17-demod` from the upstream commit `M17_COMMIT` (and `m17-mod` for tests, which is not copied into the runtime image) against Debian's `libcodec2-dev` and Boost, with the Blaze maths library (BSD-3-Clause, header only, upstream's git submodule) fetched from its pinned commit `BLAZE_COMMIT`. The only change to the upstream source is a `sed` expression (`deploy/docker/m17-demod-type.sed`) that makes the tool also print the raw TYPE field of the link setup frame; the licence texts of m17-cxx-demod and Blaze are installed under `/usr/share/doc/m17-cxx-demod/`. Corresponding source: the pinned upstream commit plus the sed file. M17 and Codec2 are open and free of patents (ADR-0018); no restricted codec is involved. Size: about 17 MB, almost all of it Codec2's library.

**dsd-fme is not in the image.** The DMR / dPMR / NXDN plugin (`org.superhet.decoder.dsd`) only runs a `dsd-fme` the user installed (`SUPERHET_DSD_FME_PATH`, `PATH`, or `bin/` of the plugin directory). Neither `dsd-fme` nor `mbelib` (AMBE) is built, fetched or shipped by the Dockerfile, CI or releases; the image contains the plugin manifest and the adapter only (`docs/plugins-dsd.md`, ADR-0018).

**TETRA / tetra-rx.** Debian bookworm has no osmo-tetra package. The Dockerfile (stage `tetra`) compiles `tetra-rx` from the pinned upstream commit `TETRA_COMMIT` of osmo-tetra-sq5bpf (https://github.com/sq5bpf/osmo-tetra-sq5bpf, AGPL-3.0, the "telive" fork of Osmocom's osmo-tetra) with `make -C src tetra-rx` against Debian's `libosmocore-dev` (GPL-2.0-or-later, runtime package `libosmocore19`), runs a self-test that decodes osmo-tetra's own test burst, and copies only the binary and the licence text (`/usr/share/doc/osmo-tetra-sq5bpf/copyright`) into the runtime image. It runs as a separate process (`docs/plugins-tetra.md`, ADR-0021). The fork's `etsi_codec-patches/` directory contains patches for the ETSI speech codec and is not built, copied or used: **the ETSI codec is not in the image**; the user installs it (`SUPERHET_TETRA_CODEC_DIR`).

**Leaflet (web UI).** The ADS-B map uses Leaflet 1.9.4 (BSD-2-Clause, https://leafletjs.com) from npm, bundled into the web build, version pinned in `web/package.json`. Map tiles are fetched by `superhet-api` (caching proxy, User-Agent `Superhet/<version>`) from OpenStreetMap by default (`https://tile.openstreetmap.org`, replaceable with `SUPERHET_MAP_TILE_URL`; `SUPERHET_MAP_TILE_MODE=direct` lets the viewer's browser fetch them instead; data (c) OpenStreetMap contributors, ODbL; tile usage policy https://operations.osmfoundation.org/policies/tiles/); the attribution is shown on the map.

Size (multimon-ng, direwolf, rtl-433): the three packages and their dependencies add about 27 MiB of installed files to the
runtime image (measured on `debian:bookworm-slim` with librtlsdr0 and libopus0 present), i.e. the
image grows from about 111 MB to about 140 MB.

The plugin install dialog shows the licence and a privacy notice of each tool
(`[install] acknowledge` in the manifests). The test recordings are generated by
`scripts/gen-plugin-recordings.py` (CC0); the FLEX recording is produced with the public-domain
generator `gen-ng` from the multimon-ng repository, which is not shipped.

Changing a package version is a normal image change; re-check the licence column and the golden
tests (`docs/plugins-decoders.md`) when you do.

## ADS-B aircraft data (not shipped)

Superhet ships no aircraft data. The admin console can download, on request, the
[Mictronics aircraft database](https://github.com/Mictronics/aircraft-database) (registration and
type per ICAO address, ICAO type descriptions, airline names) under the **Open Data Commons
Attribution License 1.0** (<https://opendatacommons.org/licenses/by/1-0/>), or import a copy of it
from a file. It is stored in the operator's PostgreSQL, not in the image or the repository, and the
ADS-B view credits the source and links the licence. The country table (`web/src/adsb/icaoRanges.ts`)
is the ICAO Annex 10 allocation of 24-bit address blocks, compiled with the help of the assignment
list kept by the tar1090 project (GPL-3.0-or-later; data of regulatory facts, AGPL-compatible).
Photo sites (Planespotters, JetPhotos) are only linked, never fetched. Crate added for reading the
archive: `zip` (MIT) with `flate2` and `zlib-rs` (MIT/Apache-2.0 / Zlib).
