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
    --bin superhet-api --bin superhet-sdrd --bin superhet-pluginhost --bin superhet-worker

# ---- Stage 3: runtime ----
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends librtlsdr0 libopus0 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/superhet superhet \
    && mkdir /data \
    && chown superhet:superhet /data

# One image for every service; compose selects the binary via `command`.
COPY --from=rust /src/target/release/superhet-api /src/target/release/superhet-sdrd \
    /src/target/release/superhet-pluginhost /src/target/release/superhet-worker /usr/local/bin/
# The API does not serve the frontend yet; the build output is shipped for later use.
COPY --from=web /src/web/dist /usr/share/superhet/web

USER superhet
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8073 8074 8075 8076
CMD ["superhet-api"]
