# HANDOFF

## Ziel
Phase 0 (foundation) done and pushed → next: P1 core receiver (docs/ROADMAP.md), after user OK.

## Erledigt
- Name check: "Superhet" free on crates.io/npm/PyPI, no SDR project collision; Docker Hub `djbird/superhet` exists → images go to `ghcr.io/bxnnyg/superhet`.
- Repo `github.com/BxnnyG/superhet` (public), commits with GitHub noreply identity (repo-local git config).
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
- Done: web tuning UI (freq entry, click-to-tune, wheel zoom/shift-drag pan via WS view, rate/gain selects, TuningController 50 ms coalescing); live-checked. Nits: click tunes to odd Hz (add step snapping), digit stepping missing.
- Done: superhet-dsp channel path (NCO, Kaiser FIR decimation, polyphase resampler → 48 kHz, WFM/NFM/AM, squelch 3 dB hyst/100 ms hang, AGC); release 2.4 MS/s: WFM 3.2 %, NFM 1.9 % of a core. dsp+rustfft opt-level 2 in dev profile.
- Done: sdrd VFOs (ctl.vfo.{create,update,delete}, max 8, paused when outside span, in state doc `vfos[]`), one `sdrd-vfo` thread fed by Arc'd blocks (queue 16, drop+count), in-repo libopus FFI (superhet-bus feature `opus`), AudioFrame v1 on `sh.v1.dev.<dev>.audio.<vfo>` (docs/protocol-audio.md). Live NATS test green. Open: VFO restore after sdrd restart (from KV), capture_unix_ns is estimated.
- Done: api VFO REST (GET/POST /devices/{dev}/vfos, PATCH/DELETE /vfos/{id}, 404 via sdrd code not_found) + audio WS `/api/v1/ws/audio?dev=` (subscribe msg, refcounted NATS sub per (dev,vfo), per-client queue 32 drop-oldest). tests/vfo_live.rs needs sdrd binary built.
- Done: web audio (WebCodecs Opus → AudioWorklet, jitter 100 ms, PI drift ±0.5 %) + VFO UI (web/README.md gestures). Live on RTL-SDR: WFM 99.2 MHz, buf 100 ms, 0 underruns, drift settles ~0.002 %, latency ≈170 ms. Open: WASM Opus fallback, WFM stereo/RDS, VFO restore after sdrd restart, ear check by user.
- Done: CI release.yml – main → ghcr.io/bxnnyg/superhet:edge + sha-*, tag vX.Y.Z → semver image + GitHub Release (docs/releasing.md). amd64 only; first release needs version bump (0.0.0); set GHCR package public after first push.
- Done: UI redesign (modern NetBird/Linear look, light+dark, web/src/ui kit + web/src/shell layout, digit-tunable readout, docs/ui-design.md, ROADMAP P1 criterion 10). ESLint blocks raw <button>/<select> outside web/src/ui. Open: device dropped_blocks not shown in browser, tooltips not touch-friendly.
- Repo flow: short-lived feature branches → PR to main (CI green) → merge, delete branch; tags only on green main.
- Done: local admin auth (ADR-0008, docs/api-auth.md): setup token in api log, Argon2id, PG sessions (7 d idle / 30 d abs), rate limit 5/IP + 50 global per 15 min, all /api/v1 + WS require session; web Setup/Login/AuthGate, change password, sign out, 401 → login, sockets pause when logged out. Live-tested with throwaway PG/NATS + file device. Waterfall degrades gracefully without WebGL2. CI now runs NATS live tests.
- Dev: `SUPERHET_DEV_API=http://127.0.0.1:8083 vite --port 5174` to run next to a compose stack on :8073.
- Done: tuning lock (holders list, 409 `locked`), free-tuning retune in sdrd (usable window 0.85 × rate, VFO lands at ¼ window; locked → paused), profiles in PG (save/overwrite/load atomically, round-trip test), sdrd restores state from KV `restore.<dev>`, state push WS `/api/v1/ws/state` (web polling removed). Web: lock toggle + badge, window shading, Profiles section, toasts. Live-tested on RTL-SDR.
- Open: exclusive claims (P2 with plugins), profile list has no push (refetch on focus/open), state feed doesn't signal sdrd gone.
- Done: receiver dock under the spectrum (all active-VFO controls one click away; sidebar = setup only; first VFO auto-selected; single audio button in the dock).
- Done: compose passes the RTL-SDR to sdrd (`SUPERHET_USB_GID` = gid of the USB node, here 944/rtlsdr; `SUPERHET_DEVICE`, `SUPERHET_PORT`), nats.conf grants sdrd KV restore; real-stick smoke test via compose image passed. Releases attach `superhet-<tag>-compose.tar.gz` (scripts/package-compose.sh). Tagged v0.1.0-alpha.1.
- Open: user must set GHCR package public; top-bar lock button wraps on phones; arm64 image; USB gid auto-detect in gen-secrets.
- Done (alpha.2): api serves the web UI (`SUPERHET_WEB_DIR`, CSP etc.), one-command installer + standalone release compose.yaml (USB gid auto-detect, stale-volume guard, project name `superhet`).
- Done (alpha.3): waterfall history kept on zoom/pan/retune, click preview + glide, clearer usable window; top readout tunes active VFO (centre secondary), per-VFO listen, squelch Auto, default VFO on fresh install; bookmarks API (migration 0004, docs/api-bookmarks.md) + UI with gqrx/SDR++ import; built-in band plan overlay (docs/bandplan.md); main area scrolls on short viewports. QoL backlog in docs/qol-backlog.md.
- Done (alpha.4): image channels `edge` (main) / `next` (newest incl. pre-releases) / `latest` (stable); installer from raw main resolves the newest release, writes `SUPERHET_TAG=next`, update = `docker compose pull && docker compose up -d`. Direct spectrum controls (edge-drag bandwidth, wheel tune, drag pan, axis click/drag) + keyboard shortcuts (`?`). Flaky live-test port race fixed (flock).
- In progress: zoom FFT (real resolution when zoomed, sdrd computes visible span) + fix for markers offset after zoom-out (user report).
- Test tip: the user's own stack often holds the stick – test with a file device via a compose override (`SUPERHET_DEVICE=file:/iq/x.cu8?format=cu8&rate=2048000&freq=…`, scripts/gen-test-iq.py).
- Done (feat/ssb-cw): USB/LSB/CW demodulators end to end (dsp `Mode::Usb|Lsb|Cw`, wanted sideband shifted to DC, low-pass FIR, shift back + real part, peak AGC with hang; CW filter centred on the VFO, BFO pitch default 700 Hz, API field `cw_pitch_hz`), one-sided passband overlay + outer-edge drag, keys 4/5/6. Open: no UI control for the CW pitch yet; top-bar lock wraps on phones; arm64 image. "50 VFOs after loading a profile" not reproducible (server replaces, UI shows 1) – ask user if it recurs; band plan JSON import hook has no UI; SDR# XML import; WebGL waterfall remapping unverified in a WebGL browser.
- Next: /metrics + dropped_blocks to UI, then WFM stereo + RDS, perf doc (crit. 7), QoL "next" items.
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
