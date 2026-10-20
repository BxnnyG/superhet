# HANDOFF

## Ziel
Phase 0 (foundation) done and pushed → next: P1 core receiver (docs/ROADMAP.md), after user OK.

## Erledigt
- Name check: "Superhet" free on crates.io/npm/PyPI, no SDR project collision; Docker Hub `djbird/superhet` exists → images go to `ghcr.io/bxnnyg/superhet`.
- Repo `github.com/BxnnyG/superhet` (private), commits with GitHub noreply identity (repo-local git config).
- LICENSE (AGPL-3.0), NOTICE (§7(b) attribution), README, SECURITY.md, CLAUDE.md, `.claude/agents/{scout,worker}.md`.
- docs: SCOPE (incl. SDR# parity + beyond), ARCHITECTURE (services, bus subjects), ROADMAP, licenses, ADR 0001–0007.
- Skeleton: crates superhet-{api,sdrd,pluginhost,worker} (bins, /healthz), superhet-{bus,db,device,dsp,plugin-protocol} (libs); web (React 19, Vite 8, Tailwind 4); compose with postgres:18.6-alpine + nats:2.15.1-alpine (JetStream, per-service users); gen-secrets.sh; CI (rust w/ Postgres+NATS, web, deny, gitleaks, docker); git hooks; check.sh green.

## Offen / Nächster Schritt
- Branch feat/p1-spectrum committed: FileDevice (cu8/cf32) → SpectrumProcessor (rustfft) → SpectrumFrame v1 (32B LE header + u8 bins) on core NATS `sh.v1.dev.<dev>.spectrum`; sdrd background NATS connect (backoff 250ms..10s), drop-on-lag, /readyz (device + NATS). Live-tested vs real NATS: 25 fps, peaks in correct bins. Integration test gated on SUPERHET_TEST_NATS_URL.
- api WS gateway done: `GET /api/v1/ws/spectrum?dev=`, JSON view control, per-client crop + peak decimation, re-encoded SpectrumFrame v1 (docs/protocol-ws.md); shared NATS sub per device, slow clients skip/drop; Origin allowlist `SUPERHET_ALLOWED_ORIGINS`; auth is a pass-through stub (`auth::require_session`) – must land before any exposure. Live tests (ws_live, nats_live) green vs throwaway NATS.
- Done (uncommitted): web/src/spectrum/ – SpectrumFrame decoder, SpectrumSocket (reconnect, view per canvas width), WebGL2 waterfall (512-row R8 ring, turbo LUT), 2D spectrum plot; vite proxies /api to :8073. Live-tested in browser against file device (tones at 99.7/100.1 MHz correct).
- Done (uncommitted): librtlsdr FFI backend (feature `rtlsdr`, spec `rtlsdr:<idx|serial>?rate=&freq=&gain=&ppm=&bias=`), Docker build uses it; HW test OK (R820T, 2.048 MS/s, 0 drops); live FM band visible in browser.
- Done: web display range (shader uniforms, auto-level via u8 histogram percentiles + EMA, manual sliders, localStorage); live-checked on FM band.
- Done: runtime device tuning – Device::control() handle, tuning_epoch, sdrd ctl.tune/ctl.state (30 ms coalescing), state in KV STATE + state subject, api GET /api/v1/devices/{dev}/state, POST .../tune (Origin check, 415/413/504). Docs: docs/api-devices.md. Live-tested on RTL-SDR.
- Next: web tuning UI (freq input, click-to-tune, wheel zoom via view, gain select), then VFO/demod (WFM/NFM/AM) + Opus audio WS. Was: tuning model + sdrd ctl handlers (tune/rate/gain) + zoom/click-to-tune UI. Later: sdrd ctl handlers (tune/rate), /metrics, api/worker/pluginhost onto connect_background.
- P1 start order: file device backend + IQ bus + FFT in sdrd → spectrum via NATS → api WS gateway → WebGL waterfall; then librtlsdr backend, tuning model, demods, Opus audio, local auth.
- Spike before P3: confirm no usable open-source TETRA DMO decoder; collect own TMO/DMO reference recordings (review for private data before committing, SECURITY.md).
- Tooling: TypeScript pinned 6.0.3 (typescript-eslint), gitleaks installed locally, cargo-nextest not; gitleaks-action needs license if repo moves to an org.
- Postgres/NATS have no host ports; local DB tests need container IP or a compose override.

## Entscheidungen (mit Kurzbegründung)
- DSP: own pipeline + rustfft + librtlsdr FFI (dynamic channels, drop control) – ADR-0001.
- Frontend: React + Vite SPA – ADR-0002.
- Plugins: separate processes/containers; container plugins via `compose.plugins.yaml`, never a Docker socket – ADR-0003.
- TETRA: native Rust stack for TMO + DMO (own cell + MTM hotspot, voice is primary); osmo-tetra/tetra-kit only as CI oracles – ADR-0004.
- Codecs: separate repos, fetch from origin + checksum + local build + UI acknowledgement – ADR-0005.
- DB: PostgreSQL only (user requirement: real DB), monthly-partitioned events, tsvector + pg_trgm – ADR-0006.
- Services sdrd/api/pluginhost/worker + NATS JetStream; IQ only via Unix sockets; subjects `sh.v1.dev.<dev>.*`, `sh.v1.events.*` – ADR-0007.
- User preference: production-grade choices from the start, no shortcuts.

## Relevante Dateien
- docs/SCOPE.md, docs/ARCHITECTURE.md, docs/ROADMAP.md, docs/adr/, docs/licenses.md, SECURITY.md
- Cargo.toml, crates/*, web/, compose.yaml, deploy/, .env.example, scripts/{check,gen-secrets,setup-dev}.sh, .github/workflows/ci.yml
