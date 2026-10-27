# Roadmap

Each phase ends with: acceptance criteria checked, `HANDOFF.md` updated, a tagged pre-release (`v0.N.0`) with changelog. Criteria must be verifiable (automated test, measurement or a short manual check script in `docs/`).

## P0 – Foundation (current)

- [x] Name check, repository, license (AGPL-3.0-only + NOTICE), README with legal notice
- [x] CLAUDE.md, agents, SCOPE, ARCHITECTURE, ADRs 0001–0006, ROADMAP
- [x] Build skeleton (Cargo workspace, web app, Docker, CI, git hooks)

## P1 – Core receiver

Scope: services sdrd / api / worker with NATS JetStream and PostgreSQL in compose; device layer (RTL-SDR + file backend), IQ bus, spectrum/waterfall, free tuning (free/sticky/center) + profiles + lock, analog demodulation incl. SDR# MVP items (SCOPE §1b), Opus audio in the browser, local admin login, settings persistence, host setup docs.

Acceptance criteria:
1. `docker compose up` on a fresh Arch host (after following `docs/setup-host.md`) shows the login page; first-run creates the admin; login/logout works; unauthenticated WebSocket upgrade is rejected (test).
2. With the RTL-SDR attached, the waterfall runs at ≥ 25 fps in Chromium and Firefox with 2.4 MS/s for 1 h without sample drops (metric `superhet_device_dropped_blocks_total` = 0) and without browser memory growth > 50 MB.
3. Free tuning: dragging a VFO beyond the window edge retunes the center; a bound VFO outside the window is shown as paused; tuning lock rejects center changes with a visible reason (integration test with file backend + manual check).
4. NFM, AM, WFM (mono + stereo), USB, LSB, CW demodulate correctly: unit tests with synthetic signals (frequency response, SNR ≥ 30 dB on clean test tones) and a manual check on a local FM broadcast station and a 2 m/70 cm repeater.
5. ≥ 4 simultaneous VFOs with independent audio streams; glass-to-ear audio latency ≤ 500 ms on LAN (measured with a test tone + timestamps).
6. Profiles: save current state as profile, reload restores center, rate, gain, PPM, VFOs exactly (round-trip test).
7. CPU: core + waterfall + 4 NFM VFOs ≤ 25 % of one core on the dev PC (documented measurement in `docs/perf.md`).
8. CI green (fmt, clippy -D warnings, tests incl. PostgreSQL/NATS integration, cargo-deny, gitleaks, web lint/typecheck/build), Docker image builds in CI.
9. Restarting `superhet-api` or `superhet-worker` does not interrupt reception or audio recording in sdrd (dropped blocks stay 0, test with compose stack).
10. UI: consistent design system (tokens + shared components), SDR-style layout (top bar with digit-tunable frequency readout, collapsible sidebar, full-height spectrum/waterfall, status bar), responsive down to 375 px, keyboard-accessible controls.

## P1.5 – Multi-user, roles, guest access, admin console & audit

Decisions: ADR-0011 (users, roles, guests), ADR-0012 (audit log, admin console), amends ADR-0008. Scope: roles `admin`/`operator`/`listener` with per-route minimum role; user management, per-user sessions, invite links; VFO ownership and per-role limits; optional guest access (off by default) with admin-set limits; append-only `audit_events` with retention and IP truncation; admin console (users and invites, live sessions with kick, audit log with filter/CSV export, guest policy, plugins stub, system); docs `docs/api-admin.md`, `docs/api-audit.md`; update `docs/api-auth.md`.

Status: implemented. Backend: PR #48. Web UI: PR #49. Address bans and the browser smoke test (`web/e2e`, CI job `e2e`, run locally with `scripts/e2e.sh`): this phase's last PR. Open: the CPU measurement for 8 listeners (criterion 9, first half) and Firefox for criterion 8.

Acceptance criteria:
1. Migration of an existing single-user database: the account becomes `admin`, guest mode is off, sessions stay valid (migration test).
2. Role matrix test: for every route and WebSocket (`ctl.tune`, gain, lock, profiles, VFO create/update/delete, bookmarks, admin endpoints) an `admin`, `operator`, `listener`, guest and unauthenticated client get the expected `200/401/403` (table-driven integration test); a route without a declared minimum role fails the test.
3. VFO ownership: a listener cannot modify, delete or subscribe to the audio of another user's VFO (403); operator/admin can; a listener's centre/rate/gain request is rejected without side effects; tuning lock rejection names the holder (integration test with file backend).
4. Invites: a link works once, expires, can be revoked, is stored hashed, and creates exactly the role fixed at invite time; the last active admin cannot be deleted, disabled or demoted (tests).
5. Login hardening: ADR-0008 limits still hold; per-username throttling blocks spraying across accounts from one address; a login with correct credentials of a disabled user fails with the same response time class as an unknown user (test).
6. Guest mode: off by default (guest endpoints answer 404/403, test); when on, limits hold (max guests, VFOs per guest, allowed modes, idle timeout, "only without operator lock") and a guest can never reach admin, user, audit, profile or tuning endpoints (test); decoder output flagged as personal is hidden from guests.
7. Audit: every action in the ADR-0012 list creates exactly one `audit_events` row with actor, role, IP, user agent, outcome; tuning drags are coalesced; the DB role of api cannot `UPDATE`/`DELETE` rows (test); retention job prunes guest events after 7 days and user events after 90 days by default; `truncated` IP mode stores /24 (IPv4) and /48 (IPv6) (tests); `SUPERHET_TRUST_PROXY` is honoured for the stored address.
8. Admin console: sections users/invites, live sessions (kick closes sockets within 2 s, test), audit log (filter by time/actor/action; CSV and JSON export match the table), guest policy, system render in Chromium and Firefox; non-admins get 403 and no admin navigation (Playwright or equivalent smoke test). **Done for Chromium**: `web/e2e/multiuser.e2e.ts` (setup, invite, listener without operator controls, guest, kick, bans, audit, CSV) runs in the CI job `e2e`; Firefox is not part of the smoke test yet.
9. Load: 8 listeners with one NFM VFO each plus the waterfall stay within the CPU budget documented in `docs/perf.md`; restarting `superhet-api` does not interrupt sdrd, and audit events of system origin are not lost across an api restart (compose test). **Restart half done**: `crates/superhet-api/tests/restart_stack.rs` runs the compose topology as processes (api + sdrd + worker, one NATS, one PostgreSQL), restarts the api and checks that sdrd stays ready, the session survives and a system event published while the api was down is in the log. **Open**: the 8-listener CPU measurement (`scripts/perf-sdrd.py` with 8 VFOs; 4 NFM VFOs measured 15 % of a core).
10. CI green as in P1; OpenAPI/markdown contracts updated; ADR-0008 consequences section notes the amendment.

## P2 – Plugin system and first plugins

Scope: plugin contract v1 (manifest, simple + framed transport), plugin host + supervisor, install from local path, plugin management UI, decoder states in the UI, exclusive claims. Plugins: POCSAG/FLEX (multimon-ng), APRS (direwolf), rtl_433, ADS-B (readsb, exclusive), DAB/DAB+ (welle-cli, exclusive, ADR-0013). Plugin installation and claims respect the roles of P1.5 (admin installs; operator/admin claim the device). (RDS moved forward: it is decoded natively in the daemon since the stereo/RDS alpha, ADR-0010.)

Acceptance criteria:
1. Contract documented in `docs/plugin-contract.md` with JSON Schemas; a reference plugin in `plugins/example-echo` passes the contract test suite (`cargo test -p superhet-plugin-protocol`).
2. Each of the five plugins decodes a committed reference recording (SigMF) in CI to the expected events (golden-file test); for DAB: service list, a DLS string and the number of decoded audio frames of one service.
3. Live: APRS packets on 144.800 MHz and rtl_433 sensors appear in the UI within 2 s of reception.
4. ADS-B profile claims the device, pauses all other decoders (visible state), and restores the previous state on release (integration test).
5. Killing a plugin process restarts it with backoff; after 5 crashes in 5 min the instance enters `Error` (test).
6. Invalid event lines are rejected and counted (test).
7. DAB: the plugin claims the device (centre of the selected Band III block, ≥ 2.048 MS/s), the UI lists the ensemble's services with DLS and slideshow, and audio of the selected service plays in the browser as Opus (manual check on a local ensemble; reference-recording test in CI). Release of the claim restores the previous state and un-pauses listeners' VFOs (integration test). The manifest and install dialog show the AAC decoder and its licence; no welle.io/faad2/fdk-aac code is in the repo, image or release (CI scan).
8. Role checks: plugin install/remove requires `admin`, claims and decoder start/stop require `operator` or `admin`; a `listener` gets 403 (extends the P1.5 role matrix test).

## P3 – Digital voice, TETRA, codecs, lookups

Scope: spike dsd-fme vs. digiham (ADR addendum); DMR, D-Star, YSF, NXDN, P25, M17 decoder plugins; codec router; Codec2 plugin; separate repos `superhet-codec-mbe` and `superhet-codec-tetra` (fetch & build locally with acknowledgement); native TETRA stack (PHY → lower MAC → TMO signalling → DMO → voice frames; osmo-tetra/tetra-kit as CI oracles); AIS, ACARS, VDL2; container runtime for plugins; install from Git URL; lookups (RadioID dump, HamQTH, QRZ optional, adsbdb/hexdb, OpenSky DB, ITU MID).

Acceptance criteria:
1. DMR reference recording: correct source/destination IDs, talkgroup, slot, call start/end events in CI. With codec plugin installed locally: intelligible audio (manual check); without: identical events, UI shows "codec not installed".
2. M17 voice plays with the bundled Codec2 plugin (reference recording).
3. TETRA TMO reference recordings (own cell): MCC/MNC, LA, cell, neighbour cells, registrations, group calls, SDS/status and encryption state decoded; PDUs match osmo-tetra/tetra-kit output on the same recordings (CI). TETRA DMO reference recordings (own MTM hotspot, gateway and repeater mode): call setup and SDS decoded. Voice from own unencrypted TMO and DMO calls is intelligible with the ETSI codec plugin (manual check), live on own equipment within 1 s latency.
4. Codec plugin install shows license/patent notice and only proceeds after confirmation; checksum mismatch aborts install (test).
5. Lookups: cache hit rate observable, per-source switch disables all network calls of that source (test with mock server), configured rate limits are never exceeded (test).
6. No restricted codec code in the main repo, images or release artifacts (CI check script scanning for known file names/hashes).

## P4 – Recording, Instant Replay, event log, scanner, bookmarks

Scope: audio recording per VFO/call, SigMF IQ recording, Instant Replay (audio always on, IQ optional RAM/disk), event log with FTS5 search, filters, timeline, export; bookmarks, frequency lists (CSV, SDR# Frequency Manager), bandplan overlay; scanner.

Acceptance criteria:
1. Instant Replay: shortcut saves the last N minutes (default 5) of all active VFO audio as clips within 2 s; IQ ring buffer shows its memory/disk requirement before enabling and produces a valid SigMF file (validated with `sigmf` reference tooling in CI).
2. IQ recording at 2.4 MS/s for 10 min without dropped blocks on the dev PC (SSD).
3. Event search over 1 M synthetic events returns filtered results in < 200 ms (benchmark test).
4. SDR# frequency manager file round-trips (import → export → identical entries); CSV import/export round-trips.
5. Scanner: list scan stops on squelch open, resumes after hold time, skips skipped channels, priority channel checked at the configured interval (integration test with file backend).

## P5 – OIDC, UI plugins, install script, metrics, hardening, multi-device prep

Scope: OIDC (generic, tested with Zitadel; identities map to local users and therefore to a role from ADR-0011, with a configurable default role and optional claim → role mapping), UI plugin API + two UI plugins (ADS-B map, TETRA cell map), install script for Debian/Arch VM with systemd, Prometheus metrics, security hardening, device manager prepared for multiple devices, SoapySDR backend.

Acceptance criteria:
1. OIDC login against a Zitadel test instance (CI with a containerised Zitadel or mock OIDC provider); a new OIDC identity gets the configured default role (never `admin` unless mapped explicitly), a mapped claim yields the expected role, role changes in the admin console apply to the next request, OIDC logins and failures appear in the audit log; local admin fallback can be disabled but not while no OIDC admin exists (test).
2. UI plugin API documented; both UI plugins load from their own packages without changes to the core frontend.
3. Install script brings up a working instance on fresh Debian 13 and Arch VMs (scripted test in CI with containers/VM where possible, otherwise documented manual run).
4. `/metrics` exposes CPU, dropped blocks, USB overruns, decoder states, plugin restarts; example Grafana dashboard in `deploy/`.
5. Security review: CSP, session hardening, rate-limited login, dependency audit (`cargo deny`, `pnpm audit`) in CI, plugin install warnings.
6. Two device managers can run in the same process in tests (file backend ×2); SoapySDR backend works with at least the RTL-SDR via SoapyRTLSDR.
