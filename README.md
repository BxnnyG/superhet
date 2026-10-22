# Superhet

> **Status: pre-alpha.** Nothing usable yet – this repository currently contains the project plan, architecture and build skeleton.

Superhet is a self-hosted web SDR: a modern alternative to OpenWebRX with the tuning freedom of SDR#, delivered as a pure web application.

- **Free tuning** – drag through the spectrum, the center frequency follows; any number of VFOs inside the current window.
- **Profiles** – one-click presets (center frequency, sample rate, gain, PPM, VFOs, active decoders).
- **Fast UI** – dark-by-default, keyboard shortcuts, command palette, WebGL2 waterfall.
- **Plugins** – decoders, codecs, online lookups and UI widgets as separate, versioned plugins.
- **Recording & Instant Replay** – save the last N minutes of audio (and optionally IQ) as a clip with one keystroke.
- **Deployment** – `docker compose up`; later an install script for a dedicated VM.

Initial hardware target: RTL-SDR (RTL2832U/R820T2, e.g. NooElec NESDR SMArt v5). The device layer is designed for SoapySDR, rtl_tcp, Airspy, HackRF and SDRplay later.

## Documentation

| Document | Content |
|---|---|
| [docs/SCOPE.md](docs/SCOPE.md) | Feature scope, priorities, feasibility with an RTL-SDR |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Components, data flow, tuning model, plugin contract |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phases and acceptance criteria |
| [docs/adr/](docs/adr/) | Architecture decision records |
| [docs/licenses.md](docs/licenses.md) | License matrix of external components |

## Quick start (not functional yet)

```bash
scripts/gen-secrets.sh   # creates .env with random passwords (never commit it)
docker compose up --build
```

Services: `superhet-api` (web UI, port 8073), `superhet-sdrd` (device + DSP), `superhet-pluginhost`, `superhet-worker`, PostgreSQL, NATS JetStream – see [ARCHITECTURE](docs/ARCHITECTURE.md).

Host preparation for the RTL-SDR (kernel module blacklist, udev rules, USB group) is described in [docs/setup-host.md](docs/setup-host.md).

## Install from GHCR

No build needed. Every release (see [Releases](https://github.com/BxnnyG/superhet/releases)) ships a compose bundle with the stack pinned to the matching image `ghcr.io/bxnnyg/superhet:<version>`:

```bash
tar xzf superhet-v0.1.0-compose.tar.gz && cd superhet-v0.1.0-compose
scripts/gen-secrets.sh             # creates .env with random passwords
# one-time host setup for the RTL-SDR: docs/setup-host.md
docker compose up -d
docker compose logs superhet-api   # one-time setup token for the first admin account
```

Then open `http://<host>:8073`. The bundle's `INSTALL.md` has the details. The image is public on GHCR (pull without login); the development tag is `ghcr.io/bxnnyg/superhet:edge`.

From a git checkout, `docker compose up --build` builds the image locally; `SUPERHET_IMAGE=ghcr.io/bxnnyg/superhet:<version>` in `.env` followed by `docker compose pull && docker compose up -d --no-build` uses the published one instead.

## Legal notice

Superhet is intended for **amateur radio** and **public services meant for general reception** (e.g. broadcast, ADS-B, AIS, APRS, ISM sensors, weather satellites).

In Germany, § 5 TDDDG (Telekommunikation-Digitale-Dienste-Datenschutz-Gesetz, formerly TTDSG) prohibits listening to, or otherwise taking note of, radio transmissions that are not intended for the receiving station, and prohibits disclosing their content or even the fact of their existence to others – even if they were received unintentionally. Violations are a criminal offence (§ 27 TDDDG). This applies in particular to BOS, business radio, paging and trunked networks (TETRA, DMR, POCSAG …). Similar laws exist in many other countries.

Monitoring your **own** equipment and networks (e.g. your own TETRA cell or DMO hotspot, your own DMR repeater) is the intended use for trunked/digital voice features.

The software provides technical capabilities; **you are solely responsible** for using it in compliance with the law of your jurisdiction. Codec plugins for proprietary or patented voice codecs (ETSI TETRA ACELP, AMBE/IMBE) are not distributed with Superhet; installing them is your decision and responsibility.

## Security

See [SECURITY.md](SECURITY.md) for how to report vulnerabilities.

## License

Copyright (C) 2026 BxnnyG (Benny).
Licensed under the [GNU Affero General Public License v3.0](LICENSE) with an attribution requirement per AGPL § 7(b) – see [NOTICE](NOTICE).
