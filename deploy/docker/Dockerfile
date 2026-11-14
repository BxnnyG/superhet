# syntax=docker/dockerfile:1

# ---- Stage 1: frontend build ----
FROM node:24-slim AS web
RUN npm install --global pnpm@12.14.0
WORKDIR /src/web
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

# ---- Stage 2: build all service binaries ----
FROM rust:1-bookworm AS rust
RUN apt-get update \
    && apt-get install -y --no-install-recommends librtlsdr-dev libopus-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# LD_PRELOAD shim that confines welle-cli's web interface to 127.0.0.1 (plugins/dab/loopback-bind.c).
COPY plugins/dab/loopback-bind.c /tmp/loopback-bind.c
RUN gcc -shared -fPIC -O2 -Wall -o /usr/local/lib/libsuperhet-loopback-bind.so /tmp/loopback-bind.c
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates/ crates/
RUN cargo build --release --locked --features superhet-sdrd/rtlsdr \
    --bin superhet-api --bin superhet-sdrd --bin superhet-pluginhost --bin superhet-worker \
    --bin superhet-plugin-adapter

# ---- Stage 3: welle-cli from the Debian package ----
# The DAB plugin wraps welle-cli. Debian's `welle.io` package also pulls in the Qt GUI (about
# 300 MB); only the unmodified command line binary is needed, so it is taken from the package
# here and its few runtime libraries are installed in the final stage.
FROM debian:bookworm-slim AS welle
RUN apt-get update \
    && apt-get install -y --no-install-recommends welle.io \
    && rm -rf /var/lib/apt/lists/*

# ---- Stage 4: readsb built from source ----
# The ADS-B plugin wraps readsb (wiedehopf's dump1090 fork, GPL-3.0-or-later; ADR-0017). Debian
# bookworm does not package it, so it is compiled here from a pinned upstream commit (tag
# v3.16.20) and only the binary and its licence reach the runtime stage. Nothing is vendored into
# the repository. No SDR libraries: the plugin feeds IQ on stdin (`--device-type ifile`).
FROM debian:bookworm-slim AS readsb
ARG READSB_COMMIT=dadfa3307a1951b7ef687cfe3fd66cdb708a4f6f
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential git ca-certificates \
        libzstd-dev zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src/readsb
RUN git init -q . \
    && git fetch -q --depth 1 https://github.com/wiedehopf/readsb "$READSB_COMMIT" \
    && git checkout -q FETCH_HEAD \
    && make -j"$(nproc)" readsb DISABLE_INTERACTIVE=yes OPTIMIZE="-O2" \
    && strip readsb \
    && ./readsb --version

# ---- Stage 5: m17-demod built from source ----
# The M17 plugin wraps m17-demod from mobilinkd's m17-cxx-demod (GPL-3.0; M17 is an open amateur
# radio standard whose voice codec, Codec2, is open and patent-free; ADR-0018). Debian does not
# package it, so it is compiled here from a pinned upstream commit; Codec2 (LGPL-2.1) and Boost
# come from Debian and are linked dynamically. The upstream build wants the Blaze maths library
# (BSD-3-Clause) as a git submodule: fetched from its pinned commit and used as headers only.
# m17-demod prints the link setup frame (callsigns, type, CAN) with `-l`; upstream leaves the
# encryption bits out of that line, so a one-line sed (deploy/docker/m17-demod-type.sed) adds the
# raw TYPE field. Only m17-demod and the licence texts reach the runtime stage; m17-mod is built
# for the self-test below (a synthetic call must decode) and to generate test recordings.
FROM debian:bookworm-slim AS m17
ARG M17_COMMIT=9b8cec24d3f8d5e9f7f6e9c23661439e32343d6b
ARG BLAZE_COMMIT=7a9086596e7783afb3cac0394bbc55d4c8b45a80
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake git ca-certificates \
        libcodec2-dev libboost-program-options-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src/m17
COPY deploy/docker/m17-demod-type.sed /tmp/m17-demod-type.sed
RUN git init -q . \
    && git fetch -q --depth 1 https://github.com/mobilinkd/m17-cxx-demod "$M17_COMMIT" \
    && git checkout -q FETCH_HEAD \
    && cd blaze \
    && git init -q . \
    && git fetch -q --depth 1 https://bitbucket.org/blaze-lib/blaze "$BLAZE_COMMIT" \
    && git checkout -q FETCH_HEAD \
    && cd .. \
    && rm -rf .git blaze/.git \
    && sed -i -f /tmp/m17-demod-type.sed apps/m17-demod.cpp \
    && grep -q 'TYPE: 0x' apps/m17-demod.cpp \
    && cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_FLAGS="-I/src/m17/blaze" \
    && cmake --build build --target m17-demod m17-mod -j"$(nproc)" \
    && strip build/apps/m17-demod build/apps/m17-mod
# Self-test: a synthetic call must come out with callsigns and the TYPE field.
RUN head -c 96000 /dev/zero | build/apps/m17-mod -S SELFTEST 2>/dev/null \
        | build/apps/m17-demod -l 2>&1 >/dev/null | grep -q 'SRC: SELFTEST.*TYPE: 0x'

# ---- Stage 6: tetra-rx built from source ----
# The TETRA plugin wraps tetra-rx from osmo-tetra-sq5bpf (AGPL-3.0; the "telive" fork of
# osmo-tetra, which sends the decoded signalling and the voice blocks as UDP datagrams), built
# against Debian's libosmocore. Only `src/` is built and only the tetra-rx binary and the licence
# text reach the runtime stage. The fork's `etsi_codec-patches/` directory holds patches for the
# ETSI speech codec, which is NOT part of this image, this repository or any release: the user
# installs it (docs/plugins-tetra.md, ADR-0021, ADR-0005).
FROM debian:bookworm-slim AS tetra
ARG TETRA_COMMIT=4e9f1b23b460a6b24270ece685ac68d4d7fd4cc8
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential git ca-certificates pkg-config \
        libosmocore-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src/tetra
RUN git init -q . \
    && git fetch -q --depth 1 https://github.com/sq5bpf/osmo-tetra-sq5bpf "$TETRA_COMMIT" \
    && git checkout -q FETCH_HEAD \
    && make -C src tetra-rx CFLAGS="-O2 -Wall $(pkg-config --cflags libosmocore) -I. -fcommon" \
    && strip src/tetra-rx
# Self-test: the sync burst that osmo-tetra's own test generator builds (MCC 262, MNC 42) must be
# decoded (tests/tetra/sync-burst.bits, one character per bit after the comment lines).
COPY crates/superhet-plugin-adapters/tests/tetra/sync-burst.bits /tmp/sync-burst.bits
RUN grep -v '^#' /tmp/sync-burst.bits | tr -d '\n' | tr '01' '\000\001' > /tmp/one.raw \
    && for i in $(seq 40); do cat /tmp/one.raw; done > /tmp/burst.raw \
    && src/tetra-rx /tmp/burst.raw | grep -q 'MCC 0100000110(262) MNC 00000000101010(42)'

# ---- Stage 7: AIS-catcher built from source ----
# The AIS plugin wraps AIS-catcher (GPL-3.0; ADR-0031). Debian does not package it, so it is
# compiled here from a pinned upstream commit (tag v0.71). Everything optional is switched off:
# no SDR libraries (the plugin feeds IQ on stdin, `-r cu8 .`), no web viewer, no database, no TLS;
# the binary only needs libc and zlib. Self-test: a synthetic GMSK frame (scripts/gen-ais-iq.py,
# gpsd's documented position report) must come out as the NMEA sentence it was made from.
FROM debian:bookworm-slim AS ais
ARG AIS_CATCHER_COMMIT=a3cb3bf8f30b462dff58c9b874dd9d932b45406a
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake git ca-certificates \
        pkg-config zlib1g-dev python3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src/ais
RUN git init -q . \
    && git fetch -q --depth 1 https://github.com/jvde-github/AIS-catcher "$AIS_CATCHER_COMMIT" \
    && git checkout -q FETCH_HEAD \
    && cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
        -DRTLSDR=OFF -DAIRSPY=OFF -DAIRSPYHF=OFF -DSDRPLAY=OFF -DHACKRF=OFF -DHYDRASDR=OFF \
        -DSOAPYSDR=OFF -DSOXR=OFF -DSAMPLERATE=OFF -DZMQ=OFF -DPSQL=OFF -DSQLITE=OFF \
        -DOPENSSL=OFF -DNMEA2000=OFF -DWEBVIEWER=OFF -DAPI=OFF -DDELIVERY=OFF \
    && cmake --build build -j"$(nproc)" \
    && strip build/AIS-catcher
COPY scripts/gen-ais-iq.py /tmp/gen-ais-iq.py
RUN python3 -I /tmp/gen-ais-iq.py 3 | build/AIS-catcher -r cu8 . -s 192000 -o 1 2>/dev/null \
        | grep -q '^!AIVDM,1,1,,[AB],177KQJ5000G?tO`K>RA1wUbN0TKH,0\*'

# ---- Stage 8: acarsdec and dumpvdl2 (with libacars) built from source ----
# The ACARS and VDL2 plugins wrap acarsdec (GPL-2.0, Thierry Leconte's decoder as maintained by
# Thibaut Varene, tag v4.7) and dumpvdl2 (GPL-3.0, tag v2.7.0); both use libacars (MIT, tag
# v2.2.1) to decode ARINC 622 content. Debian bookworm packages none of them in a usable version,
# so they are compiled from pinned commits. Only the IQ-from-stdin input is built (no SDR
# libraries, no MQTT, no statsd); libacars and cJSON are linked dynamically and copied below.
FROM debian:bookworm-slim AS datalink
ARG LIBACARS_COMMIT=9af09a0121d4ec577339cbd4c7420d7519da48fa
ARG ACARSDEC_COMMIT=4f0e5541240f930bc3b5d206e8a4033391120a5d
ARG DUMPVDL2_COMMIT=686e87f2294ef6275855ba7364bd4bb9a0f8f56a
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake git ca-certificates \
        pkg-config zlib1g-dev libglib2.0-dev libcjson-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src/libacars
RUN git init -q . \
    && git fetch -q --depth 1 https://github.com/szpajder/libacars "$LIBACARS_COMMIT" \
    && git checkout -q FETCH_HEAD \
    && cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr/local \
        -DCMAKE_INSTALL_LIBDIR=lib \
    && cmake --build build -j"$(nproc)" \
    && cmake --install build \
    && ldconfig
WORKDIR /src/acarsdec
RUN git init -q . \
    && git fetch -q --depth 1 https://github.com/f00b4r0/acarsdec "$ACARSDEC_COMMIT" \
    && git checkout -q FETCH_HEAD \
    && cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
        -DRTLSDR=OFF -DSOAPYSDR=OFF -DAIRSPY=OFF -DSDRPLAY=OFF -DSNDFILE=OFF -DALSA=OFF -DMQTT=OFF \
    && cmake --build build -j"$(nproc)" \
    && strip build/acarsdec
WORKDIR /src/dumpvdl2
RUN git init -q . \
    && git fetch -q --depth 1 https://github.com/szpajder/dumpvdl2 "$DUMPVDL2_COMMIT" \
    && git checkout -q FETCH_HEAD \
    && cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
        -DRTLSDR=OFF -DMIRISDR=OFF -DSDRPLAY=OFF -DSDRPLAY3=OFF -DSOAPYSDR=OFF \
        -DETSY_STATSD=OFF -DSQLITE=OFF -DZMQ=OFF -DRAW_BINARY_FORMAT=OFF \
    && cmake --build build -j"$(nproc)" \
    && strip build/src/dumpvdl2
# Self-test: both tools start with their IQ-from-stdin options and decode (nothing, but cleanly)
# a second of silence.
RUN head -c 480000 /dev/zero | /src/acarsdec/build/acarsdec -e --output json:file:path=- \
        --iqfile file=/dev/stdin,sr=240000,format=U8 -c 131.725 131.725 \
    && head -c 840000 /dev/zero | /src/dumpvdl2/build/src/dumpvdl2 --iq-file - --sample-format U8 \
        --oversample 4 --centerfreq 136975000 --output decoded:json:file:path=- 136975000

# ---- Stage 9: runtime ----
FROM debian:bookworm-slim
# multimon-ng, direwolf and rtl-433 are the external decoders of the first-party plugins
# (plugins/): separate GPL-2.0 processes started by the plugin host through
# superhet-plugin-adapter; about 27 MiB (docs/third-party.md). welle-cli (DAB plugin) needs the
# libraries on the second line: about 7 MiB together with the binary copied below; readsb (ADS-B
# plugin) needs zstd; m17-demod (M17 plugin) needs Codec2 and Boost.Program_options (about 3 MiB);
# tetra-rx (TETRA plugin) needs libosmocore; dumpvdl2 (VDL2 plugin) needs GLib and acarsdec (ACARS plugin)
# cJSON.
RUN apt-get update \
    && apt-get install -y --no-install-recommends librtlsdr0 libopus0 multimon-ng direwolf rtl-433 \
        libfaad2 libfftw3-single3 libmp3lame0 libmpg123-0 libasound2 libairspy0 libsoapysdr0.8 libusb-1.0-0 libzstd1 ca-certificates \
        libcodec2-1.0 libboost-program-options1.74.0 libosmocore19 libglib2.0-0 libcjson1 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/superhet superhet \
    && mkdir -p /data/plugins /data/tiles /data/recordings /data/backups \
    && chown -R superhet:superhet /data

# welle-cli as built by Debian, with its licence text (GPL-2.0-or-later; source: apt-get source welle.io).
COPY --from=welle /usr/bin/welle-cli /usr/bin/welle-cli
COPY --from=welle /usr/share/doc/welle.io/copyright /usr/share/doc/welle.io/copyright

# readsb as compiled above, with its licence text (GPL-3.0-or-later).
COPY --from=readsb /src/readsb/readsb /usr/bin/readsb
COPY --from=readsb /src/readsb/COPYING /usr/share/doc/readsb/copyright

# m17-demod as compiled above (GPL-3.0, one-line patch: deploy/docker/m17-demod-type.sed), with its
# licence and the one of the Blaze headers it was built with (BSD-3-Clause).
COPY --from=m17 /src/m17/build/apps/m17-demod /usr/bin/m17-demod
COPY --from=m17 /src/m17/LICENSE /usr/share/doc/m17-cxx-demod/copyright
COPY --from=m17 /src/m17/blaze/LICENSE /usr/share/doc/m17-cxx-demod/blaze-LICENSE

# tetra-rx as compiled above (AGPL-3.0), with its licence text. No ETSI codec code is in the image.
COPY --from=tetra /src/tetra/src/tetra-rx /usr/bin/tetra-rx
COPY --from=tetra /src/tetra/COPYING /usr/share/doc/osmo-tetra-sq5bpf/copyright

# AIS-catcher, acarsdec, dumpvdl2 and libacars as compiled above, with their licence texts
# (GPL-3.0, GPL-2.0, GPL-3.0, MIT).
COPY --from=ais /src/ais/build/AIS-catcher /usr/bin/AIS-catcher
COPY --from=ais /src/ais/LICENSE /usr/share/doc/ais-catcher/copyright
COPY --from=datalink /src/acarsdec/build/acarsdec /usr/bin/acarsdec
COPY --from=datalink /src/acarsdec/LICENSE.md /usr/share/doc/acarsdec/copyright
COPY --from=datalink /src/dumpvdl2/build/src/dumpvdl2 /usr/bin/dumpvdl2
COPY --from=datalink /src/dumpvdl2/LICENSE /usr/share/doc/dumpvdl2/copyright
COPY --from=datalink /usr/local/lib/libacars-2.so* /usr/local/lib/
COPY --from=datalink /src/libacars/LICENSE.md /usr/share/doc/libacars/copyright
RUN ldconfig

COPY --from=rust /usr/local/lib/libsuperhet-loopback-bind.so /usr/local/lib/superhet/libsuperhet-loopback-bind.so

# One image for every service; compose selects the binary via `command`.
COPY --from=rust /src/target/release/superhet-api /src/target/release/superhet-sdrd \
    /src/target/release/superhet-pluginhost /src/target/release/superhet-worker \
    /src/target/release/superhet-plugin-adapter /usr/local/bin/
# First-party plugins; the plugin host installs them into /data/plugins on first start.
COPY plugins/pocsag /usr/share/superhet/plugins/pocsag
COPY plugins/aprs /usr/share/superhet/plugins/aprs
COPY plugins/rtl433 /usr/share/superhet/plugins/rtl433
COPY plugins/dab /usr/share/superhet/plugins/dab
COPY plugins/adsb /usr/share/superhet/plugins/adsb
COPY plugins/m17 /usr/share/superhet/plugins/m17
COPY plugins/ais /usr/share/superhet/plugins/ais
COPY plugins/acars /usr/share/superhet/plugins/acars
COPY plugins/vdl2 /usr/share/superhet/plugins/vdl2
# DMR / dPMR / NXDN: the manifest and the adapter only. dsd-fme (with its AMBE vocoder) is NOT in
# this image; the user installs it (docs/plugins-dsd.md).
COPY plugins/dsd /usr/share/superhet/plugins/dsd
# TETRA: manifest, adapter and tetra-rx. The ETSI speech codec is NOT in this image; the user
# installs it and points SUPERHET_TETRA_CODEC_DIR at it (docs/plugins-tetra.md).
COPY plugins/tetra /usr/share/superhet/plugins/tetra
# The API does not serve the frontend yet; the build output is shipped for later use.
COPY --from=web /src/web/dist /usr/share/superhet/web

USER superhet
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8073 8074 8075 8076
CMD ["superhet-api"]
