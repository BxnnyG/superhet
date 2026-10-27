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
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates/ crates/
RUN cargo build --release --locked --features superhet-sdrd/rtlsdr \
    --bin superhet-api --bin superhet-sdrd --bin superhet-pluginhost --bin superhet-worker \
    --bin superhet-plugin-adapter

# ---- Stage 3: runtime ----
FROM debian:bookworm-slim
# multimon-ng, direwolf and rtl-433 are the external decoders of the first-party plugins
# (plugins/): separate GPL-2.0 processes started by the plugin host through
# superhet-plugin-adapter; about 27 MiB (docs/third-party.md).
RUN apt-get update \
    && apt-get install -y --no-install-recommends librtlsdr0 libopus0 multimon-ng direwolf rtl-433 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/superhet superhet \
    && mkdir -p /data/plugins \
    && chown -R superhet:superhet /data

# One image for every service; compose selects the binary via `command`.
COPY --from=rust /src/target/release/superhet-api /src/target/release/superhet-sdrd \
    /src/target/release/superhet-pluginhost /src/target/release/superhet-worker \
    /src/target/release/superhet-plugin-adapter /usr/local/bin/
# First-party plugins; the plugin host installs them into /data/plugins on first start.
COPY plugins/pocsag /usr/share/superhet/plugins/pocsag
COPY plugins/aprs /usr/share/superhet/plugins/aprs
COPY plugins/rtl433 /usr/share/superhet/plugins/rtl433
# The API does not serve the frontend yet; the build output is shipped for later use.
COPY --from=web /src/web/dist /usr/share/superhet/web

USER superhet
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8073 8074 8075 8076
CMD ["superhet-api"]
