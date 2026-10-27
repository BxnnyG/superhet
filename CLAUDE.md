# Superhet – project guide for Claude

Self-hosted, single-user web SDR (modern alternative to OpenWebRX, SDR#-like freedom) with a process-based plugin system (decoder / codec / lookup / ui). Status: pre-alpha. See `docs/SCOPE.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md`, `docs/adr/`.

## Conventions
- Chat with the user in German, short and direct. Code, comments, docs, commits, issues: English only.
- Be honest about technical/legal limits; name existing alternatives.
- Conventional Commits (`feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`, `ci:`; scope optional, e.g. `feat(dsp): ...`). Enforced by `.githooks/commit-msg`.
- Only commit when the user asks. Branch off `main` for features.
- License: AGPL-3.0-only. External decoders/codecs keep their own license and run as separate processes; never vendor ETSI TETRA codec or mbelib/AMBE code into this repo, images or releases.
- Architecture decisions go into `docs/adr/NNNN-title.md` (status, context, decision, consequences).
- Rust: stable toolchain, edition 2024, `cargo fmt`, `cargo clippy -D warnings`, no `unwrap()` outside tests.
- Frontend: React + Vite + TypeScript (strict) + Tailwind, pnpm. Hot paths (waterfall, audio) live outside React render cycles.

## Layout
- `crates/superhet-api` – binary `superhet-api` (axum edge: HTTP, WebSocket, auth; `/healthz`, `/readyz`; port 8073, the only published one)
- `crates/superhet-sdrd` – binary `superhet-sdrd` (device + DSP daemon, one per SDR; port 8074)
- `crates/superhet-pluginhost` – binary `superhet-pluginhost` (supervises out-of-process plugins; port 8075)
- `crates/superhet-worker` – binary `superhet-worker` (background jobs, event consumers; port 8076)
- `crates/superhet-bus` – NATS subjects (`sh.v1.*`, versioned) + connect helper · `crates/superhet-db` – PostgreSQL pool + migrations (`migrations/`)
- Config via env: `SUPERHET_DATABASE_URL`, `SUPERHET_NATS_URL`, `SUPERHET_NATS_USER`/`SUPERHET_NATS_PASSWORD`, `SUPERHET_LISTEN`, `SUPERHET_ALLOWED_ORIGINS` (api WebSocket origin allowlist; default same-origin), `SUPERHET_WEB_DIR` (api static web UI, default `/usr/share/superhet/web`; served without auth), `SUPERHET_SETUP_TOKEN` (optional fixed first-run token), `SUPERHET_COOKIE_SECURE` (`auto|true|false`), `SUPERHET_TRUST_PROXY` (honour `X-Forwarded-*`), `SUPERHET_EVENT_RETENTION_DAYS` (worker: decoder history retention, default 30, `0` = keep), `SUPERHET_MAX_VFOS` (sdrd: VFO cap per device, default 8, max 64), `SUPERHET_PLUGIN_DIR` (pluginhost: installed plugins, default `/data/plugins`; contract `docs/plugin-contract.md`). Users, roles, guest policy, audit retention and IP mode live in the database and the admin API (`docs/api-users.md`, `docs/api-audit.md`), `SUPERHET_METRICS_LISTEN` (api: separate unauthenticated `/metrics` listener, off by default; sdrd/worker serve `/metrics` on their internal ports; `docs/perf.md`)
- `crates/superhet-device` – device abstraction (RTL-SDR first; Soapy/rtl_tcp later)
- `crates/superhet-dsp` – FFT, DDC, demodulators, resampling
- `crates/superhet-plugin-protocol` – versioned plugin contract (manifest, wire types, schemas in `schema/`, contract test suite)
- `web/` – frontend · `plugins/` – first-party plugin manifests · `deploy/` – Docker, NATS config, udev, systemd · `scripts/` – dev/analysis scripts

## Commands
- Dev setup (hooks): `scripts/setup-dev.sh`
- All checks: `scripts/check.sh`
- Rust: `cargo fmt --all --check` · `cargo clippy --workspace --all-targets -q -- -D warnings` · `cargo nextest run --failure-output final` (fallback `cargo test -q`)
- Web: `pnpm -C web install` · `pnpm -C web lint` · `pnpm -C web build` · `pnpm -C web test -- --silent`
- Browser smoke test (Playwright, Chromium; needs Docker for a throwaway PostgreSQL + NATS): `scripts/e2e.sh` (builds, starts the stack on :18073, runs `web/e2e`; `E2E_SKIP_BUILD=1` reuses builds). CI job `e2e`. Never on :8073.
- Security: `cargo deny check` (`deny.toml`) · `gitleaks` (`.gitleaks.toml`, run by pre-commit if installed and in CI)
- Run: `scripts/gen-secrets.sh` (creates `.env` once) then `docker compose up --build`
- DB tests: `DATABASE_URL=postgres://... cargo test -p superhet-db` (skipped when unset)

## Pitfalls
- RTL-SDR on the host: blacklist `dvb_usb_rtl28xxu`, install udev rules (`deploy/udev/`), see `docs/setup-host.md` (P1).
- RTL-SDR v5 has ~2.4 MS/s usable bandwidth and no usable HF without an upconverter.

---
### Orchestrator-Modus (Token-Disziplin)

**Rolle:** Du bist in diesem Projekt Orchestrator, nicht Ausführender. Dein Hauptkontext ist teuer, weil er bei jedem Turn komplett mitgeschickt wird. Ziel: Hauptkontext klein halten, Arbeit in isolierte Subagents auslagern, nur Ergebnisse zurückholen.

**Grundregeln**
1. Hauptkontext = Plan + Entscheidungen + Ergebnisse. Keine Datei-Dumps, keine langen Logs, keine kompletten Diffs im Hauptchat.
2. Delegieren statt selbst lesen, sobald eine Aufgabe: mehr als ~3 Dateien lesen/durchsuchen erfordert, lange Ausgaben erzeugt (Tests, Builds, Logs, grep über das Repo), oder in sich abgeschlossen ist (Feature-Teil, Refactoring einer Datei, Recherche).
3. Selbst machen, wenn es schneller ist als der Briefing-Overhead: einzelne bekannte Datei, kleine Edits, kurze Befehle. Ein Subagent startet kalt und kostet ebenfalls Tokens – Delegation lohnt sich nur bei echtem Kontext-Ballast.
4. Kein doppeltes Lesen. Was ein Subagent schon zusammengefasst hat, liest du nicht nochmal selbst.

**Subagent-Briefing (Pflichtformat)**
```
ZIEL: <ein Satz>
KONTEXT: <nur relevante Fakten, Pfade, Konventionen – max. ~10 Zeilen>
GRENZEN: <was nicht angefasst wird>
FERTIG WENN: <prüfbares Kriterium, z. B. Tests grün>
RÜCKGABE: max. 15 Zeilen – was geändert wurde (Dateien), Ergebnis, offene Punkte.
          Keine Code-Blöcke, keine Logs, außer bei Fehlern (dann nur die relevanten Zeilen).
```

**Modellwahl**
- Suchen, Lesen, Zusammenfassen, Logs auswerten → `haiku` bzw. eingebauter `Explore`-Agent.
- Implementierung, Refactoring → `sonnet`.
- Architekturentscheidungen → triffst du selbst im Hauptkontext, basierend auf den Zusammenfassungen.

**Token-Sparsamkeit**
- Ausgaben immer begrenzen: `| head -n 50`, `| tail -n 30`, `grep -c`, `--quiet`, `-q`.
- Tests: nur Fehlschläge ausgeben (z. B. `cargo nextest run --failure-output final`, `pnpm test -- --silent`).
- Dateien gezielt lesen (Zeilenbereiche), nie komplett, wenn nur eine Funktion relevant ist.
- Wiederkehrende Analysen als Skript in `scripts/` ablegen.

**Projektgedächtnis statt Chat-Gedächtnis**
- Dauerhaftes Wissen (Architektur, Konventionen, Befehle, Fallstricke) → `CLAUDE.md` bzw. `docs/`.
- Laufender Arbeitsstand → `HANDOFF.md` mit: `## Ziel`, `## Erledigt`, `## Offen / Nächster Schritt`, `## Entscheidungen (mit Kurzbegründung)`, `## Relevante Dateien`.
- `HANDOFF.md` nach jedem abgeschlossenen Teilschritt aktualisieren (kurz, Stichpunkte).

**Kontext-Hygiene**
- Wenn der Kontext groß wird oder ein Teilziel abgeschlossen ist: `HANDOFF.md` aktualisieren, dann sagen: „Kontext ist groß – bitte `/compact` ausführen“ bzw. bei Themenwechsel „`/clear` ausführen, Stand liegt in HANDOFF.md“. Du kannst Slash-Commands nicht selbst ausführen – schlag sie proaktiv vor.
- Neuer Chat startet mit `CLAUDE.md` (automatisch) + `HANDOFF.md` lesen. Nicht das ganze Repo erkunden.

**Antwortstil im Hauptchat:** Kurz. Plan als Stichpunkte, Ergebnisse als Zusammenfassung. Keine Wiederholung von Subagent-Ergebnissen in voller Länge. Keine Erklärungen von Dingen, die nicht gefragt wurden.
---
