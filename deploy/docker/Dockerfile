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

# ---- Stage 6: runtime ----
FROM debian:bookworm-slim
# multimon-ng, direwolf and rtl-433 are the external decoders of the first-party plugins
# (plugins/): separate GPL-2.0 processes started by the plugin host through
# superhet-plugin-adapter; about 27 MiB (docs/third-party.md). welle-cli (DAB plugin) needs the
# libraries on the second line: about 7 MiB together with the binary copied below; readsb (ADS-B
# plugin) needs zstd; m17-demod (M17 plugin) needs Codec2 and Boost.Program_options (about 3 MiB).
RUN apt-get update \
    && apt-get install -y --no-install-recommends librtlsdr0 libopus0 multimon-ng direwolf rtl-433 \
        libfaad2 libfftw3-single3 libmp3lame0 libmpg123-0 libasound2 libairspy0 libsoapysdr0.8 libusb-1.0-0 libzstd1 ca-certificates \
        libcodec2-1.0 libboost-program-options1.74.0 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/superhet superhet \
    && mkdir -p /data/plugins /data/tiles \
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
# DMR / dPMR / NXDN: the manifest and the adapter only. dsd-fme (with its AMBE vocoder) is NOT in
# this image; the user installs it (docs/plugins-dsd.md).
COPY plugins/dsd /usr/share/superhet/plugins/dsd
# The API does not serve the frontend yet; the build output is shipped for later use.
COPY --from=web /src/web/dist /usr/share/superhet/web

USER superhet
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8073 8074 8075 8076
CMD ["superhet-api"]
