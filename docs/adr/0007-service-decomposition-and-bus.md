# ADR-0007: Service decomposition and message bus

**Status:** accepted (2026-10-20)

## Context

A single server process would mix hard-real-time sample processing with web traffic, auth, DB writes, network lookups and plugin supervision. A crash or slow DB query must never cause sample drops; the web/API part must be restartable without interrupting reception; multiple devices (possibly on other hosts) must be added later without restructuring.

## Decision

Split into services that communicate over **NATS (with JetStream)** for control, state and events, and over **Unix domain sockets** for high-rate sample data:

| Service | Responsibility | Talks to |
|---|---|---|
| `superhet-sdrd` | Owns exactly one device: tuning, locks/claims, IQ bus, FFT, channelizer, analog demod, Opus audio, audio/IQ ring buffers and recording. One instance per SDR. | NATS; sample socket server for pluginhost |
| `superhet-api` | HTTP API, WebSocket gateway to the browser, auth (local admin + OIDC), serves the web app; translates browser commands into NATS requests and fans spectrum/audio/events out to clients. Only service exposed to the network. | NATS, PostgreSQL |
| `superhet-pluginhost` | Plugin registry, install, supervision of plugin processes/containers, channel subscriptions at sdrd, event validation, codec router (voice frames → codec plugins → PCM → Opus). | NATS, sdrd sockets, PostgreSQL |
| `superhet-worker` | Event persistence (JetStream consumer → PostgreSQL), lookups with cache and rate limits, partition/retention jobs, recording post-processing. | NATS, PostgreSQL, Internet (lookups only) |
| PostgreSQL | Persistent data (ADR-0006) | – |
| NATS + JetStream | Request/reply for control, pub/sub for spectrum/audio/state, JetStream stream `EVENTS` (durable, replay), KV buckets for live state (tuning, locks, decoder states). | – |

Subjects are versioned (`sh.v1.…`) and defined in one crate (`superhet-bus`). Every service authenticates to NATS with its own user and subject permissions (e.g. only sdrd may publish `sh.v1.dev.<device>.spectrum`, only worker may consume `EVENTS` durably).

Sample data (full-band or channelised IQ, discriminator audio) never goes over NATS: sdrd exposes a Unix socket (shared volume) with a framed channel-subscription protocol (see ARCHITECTURE §6). A later remote device = sdrd on another host with NATS leaf node; sample consumers then run on that host.

### Alternatives considered

- **Monolith**: simplest, but no fault isolation; rejected.
- **gRPC point-to-point**: no extra service, but fan-out, persistence/replay and decoupling must be hand-built.
- **Redis/Valkey Streams**: weaker messaging model, no subject-level permissions comparable to NATS accounts.
- **Kafka/Redpanda**: far too heavy for a single-node deployment.

## Consequences

- More moving parts: compose defines health checks and start order; each service exposes `/healthz`, `/readyz`, `/metrics`.
- Message schemas on the bus are versioned like a public API; breaking changes need a new subject version.
- End-to-end tests run the full compose stack with the file device in CI.
- Latency budget: NATS adds well under 1 ms on localhost, irrelevant compared to audio buffering.
