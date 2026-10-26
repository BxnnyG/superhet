# Feature scope

Legend:
- **Priority:** `MVP` = phase 1 (core), `v1` = phases 2–4 (first release that is "complete"), `later` = phase 5+ or optional.
- **RTL-SDR v5:** feasibility with the reference hardware (NooElec NESDR SMArt v5, RTL2832U + R820T2, ~24–1766 MHz, 2.4 MS/s stable, 8-bit ≈ 45–50 dB usable dynamic range, 0.5 ppm TCXO, bias-T). ✅ fully · 🟡 limited · ❌ not realistic.
- Phase numbers refer to [ROADMAP.md](ROADMAP.md).

## Hardware constraints that shape the scope

- **One device, one window.** All simultaneous VFOs/decoders must fit into ~2.0 MHz of usable bandwidth (2.4 MS/s minus filter roll-off and the DC spike area). Monitoring e.g. ACARS (131 MHz) and VDL2 (136 MHz) at the same time is impossible.
- **HF (< 24 MHz) is not covered** by the R820T2. HF requires an upconverter (e.g. Ham It Up) or a different device. Direct-sampling mode is not a supported path for the v5 in Superhet. 10 m (28 MHz) and 6 m work with reduced performance.
- **8-bit ADC:** strong local signals (broadcast FM, DVB-T, cell towers) cause overload/intermod. Manual gain and profile-stored gain are mandatory; AGC of the tuner is optional.
- Exclusive decoders (ADS-B, DAB/DAB+) pin the device to their frequency and sample rate.
- With several users all of them share the single window (§8); only an operator/admin may retune it.

## 1. Reception & UI

| Feature | Prio | RTL-SDR v5 | Upstream / license | Risks |
|---|---|---|---|---|
| Device layer RTL-SDR (librtlsdr) | MVP | ✅ | osmocom/rtl-sdr (GPL-2.0, dynamically linked C lib) | USB overruns on weak hosts; kernel DVB driver conflict |
| Device trait for Soapy/rtl_tcp/SoapyRemote/Airspy/HackRF/SDRplay | MVP (trait), later (impl) | – | SoapySDR (BSL-1.0) | Trait must cover differing gain models and sample formats (u8/i16/f32) |
| Spectrum + waterfall (server FFT, compressed bins over WS, WebGL2) | MVP | ✅ | rustfft (MIT/Apache) | Bandwidth to browser; GPU fallback for old devices |
| Zoom, palettes, peak-hold, averaging, dB range | MVP | ✅ | – | – |
| Free tuning (drag/click, center follows) | MVP | ✅ | – | Retune glitches (~10–50 ms gap on R820T2 retune), must not break running decoders unnecessarily |
| Profiles (presets) incl. save-from-current | MVP | ✅ | – | Schema versioning of profiles |
| Tuning lock, decoder frequency binding, paused state | MVP (lock, binding), v1 (decoder UI) | ✅ | – | UX clarity |
| NFM, AM demodulation | MVP | ✅ | own DSP | – |
| WFM (mono/stereo) | MVP | ✅ | own DSP | CPU (stereo pilot PLL) |
| RDS | v1 | ✅ | native (`superhet-dsp::rds`, ADR-0010) | – |
| USB/LSB | MVP | 🟡 VHF/UHF only without upconverter | own DSP | – |
| CW (audio) | MVP | 🟡 VHF/UHF beacons; HF needs upconverter | own DSP | – |
| CW decoder (text) | later | 🟡 | own / ggmorse (MIT) | Accuracy |
| Multiple VFOs simultaneously | MVP (≥ 4) | ✅ within window | – | CPU per VFO (see ARCHITECTURE CPU budget) |
| Audio in browser (Opus over WS, WebAudio, AudioWorklet) | MVP | ✅ | libopus (BSD-3) | Latency/jitter buffer tuning, autoplay policies |
| Squelch, AGC, filter width by dragging in waterfall | MVP | ✅ | – | – |
| Keyboard shortcuts, command palette | MVP | – | cmdk (MIT) | – |
| Bandplan overlay (IARU Region 1) | v1 (P4) | – | curated JSON from IARU R1 / DARC bandplan (facts, own data file) | Keeping data current |
| Bookmarks, frequency lists, CSV import/export | v1 (P4) | – | – | – |
| SDR# Frequency Manager import/export (XML) | v1 (P4) | – | format reverse-engineered from files | Undocumented format |
| Scanner: list & range scan, squelch stop, hold/skip, priority channels | v1 (P4) | 🟡 range scan beyond window needs retune (~50 ms/step) | – | Scan speed is limited by retune + settle time |

## 1b. SDR# feature parity – and beyond

Goal: everything a SDR# user relies on daily exists in Superhet, reachable by mouse *and* keyboard.

| SDR# feature | Prio | Notes |
|---|---|---|
| Source settings: sample rate, decimation, RF/IF gain (per-stage where the device allows), PPM, bias-T, offset tuning, swap I/Q | MVP | device capabilities drive the UI |
| IQ correction (auto IQ imbalance), DC removal | MVP | – |
| Tuning modes: free, sticky, center | MVP | "free" is default (§3 ARCHITECTURE) |
| Frequency entry, step sizes, snap to grid, mouse-wheel / keyboard tuning | MVP | – |
| FFT size, window function, contrast/range/offset, averaging, peak hold, spectrum fill, waterfall speed, palette editor | MVP | – |
| Zoom: fast zoom, Zoom FFT / IF spectrum per VFO | MVP (zoom), v1 (IF spectrum) | zoom-DDC in sdrd |
| Audio spectrum, MPX spectrum (WFM) | v1 | – |
| Demod modes NFM, WFM, AM, DSB, USB, LSB, CW, RAW (IQ out) | MVP | – |
| Filter type / bandwidth / order / shift, drag-edges in waterfall | MVP | – |
| FM stereo, de-emphasis (50/75 µs), RDS | MVP (stereo), v1 (RDS) | – |
| AM synchronous (lock carrier/SAM), anti-fading, hybrid mode | v1 | – |
| AGC (threshold, decay, slope, hang), unity gain, audio filter | MVP | – |
| Squelch (carrier and noise squelch, auto squelch) | MVP | – |
| Noise blanker (baseband + demodulator) | v1 | – |
| Noise reduction (IF + AF) | v1 | spectral subtraction first; ML-based (RNNoise, BSD) later |
| Audio notch / auto notch, CW peak filter | v1 | – |
| Co-channel canceller | later | complex, AM only |
| Frequency manager (groups, import/export) | v1 (P4) | incl. SDR# XML format |
| Frequency scanner | v1 (P4) | – |
| Audio / baseband (IQ) / IF recorder | v1 (P4) | SigMF for IQ |
| Band plan | v1 (P4) | IARU R1 |
| Signal diagnostics (SNR, level meter, S-units) | MVP (S-meter), v1 (SNR tool) | – |
| SpyServer / rtl_tcp **client** sources | later (P5) | device backends |
| Digital voice via external plugins (DSD+) | v1 (P3) | native decoder plugins instead |

### Beyond SDR#

| Feature | Prio | Notes |
|---|---|---|
| Access from any browser (desktop, tablet, phone), several tabs/clients in sync | MVP | – |
| Several users with roles and optional guest listening on one instance | v1 (P1.5) | §8 |
| Instant Replay (audio always, IQ optional) | v1 (P4) | §5 |
| Waterfall history: scroll back in time (from IQ ring buffer) | later | – |
| Integrated decoders with event log, search, timeline, maps | v1 | – |
| Online lookups (callsigns, DMR IDs, aircraft, vessels) | v1 (P3) | §4 |
| Alert rules on events (e.g. SDS to ISSI X, callsign seen) → notification (ntfy, webhook, e-mail) | v1 (P4) | – |
| Scheduled recordings / profile timers | v1 (P4) | – |
| Automatic PPM calibration (known carrier, ADS-B, GSM/LTE reference) | v1 | – |
| rtl_tcp / SpyServer **server** output (share a channel slice with SDR#, SDR++ etc.) | later | – |
| REST + NATS API for automation | v1 | versioned |
| TETRA network analysis for own cells (burst quality, neighbour map, subscriber activity) | v1 (P3) | – |
| Multiple devices, remote devices via NATS leaf | later (P5+) | ADR-0007 |
| OIDC single sign-on | later (P5) | – |

## 2. Digital decoders (plugins)

All decoders are plugins (separate processes). Without a codec plugin, voice decoders still deliver metadata/events.

| Decoder | Prio | RTL-SDR v5 | Upstream / license | Risks / notes |
|---|---|---|---|---|
| POCSAG / FLEX | v1 (P2) | ✅ | multimon-ng (GPL-2.0) | Legal: most German POCSAG traffic is not intended for you (§ 5 TDDDG) |
| APRS / AX.25 (144.800 MHz) | v1 (P2) | ✅ | direwolf (GPL-2.0) | – |
| ISM sensors 433/868 MHz | v1 (P2) | ✅ | rtl_433 (GPL-2.0), reads cu8/cs16 IQ from stdin | Needs wider channel (250 kHz–1 MHz) |
| ADS-B 1090 MHz (exclusive) | v1 (P2) | ✅ | readsb (GPL-3.0) | Pins the device; needs a 1090 MHz antenna for decent range |
| DAB / DAB+ (exclusive, whole ensemble): service list, DLS, slideshow (MOT), AAC → Opus audio | v1 (P2) | ✅ (1.536 MHz ensemble fits the 2.4 MS/s window; Band III antenna needed) | welle-cli / welle.io (GPL-2.0+), fallback eti-cmdline + dablin (ADR-0013) | Pins the device; AAC decoder licence (faad2 vs fdk-aac) shown at install; nothing vendored |
| AIS 161.975/162.025 MHz | v1 (P2/P3) | ✅ | AIS-catcher (GPL-3.0) | – |
| DMR Tier I/II (Tier III metadata) | v1 (P3) | ✅ | dsd-fme (ISC/GPL mix, verify) or digiham (GPL-3.0) | dsd-fme links mbelib at build time → frame output needs patching or the plugin itself is "fetch & build locally" (ADR-0005) |
| D-Star | v1 (P3) | ✅ | digiham / dsd-fme | AMBE codec |
| YSF / C4FM | v1 (P3) | ✅ | digiham / dsd-fme | AMBE+2 / IMBE |
| NXDN | v1 (P3) | ✅ | dsd-fme | AMBE+2 |
| P25 Phase 1 | v1 (P3) | ✅ | dsd-fme | IMBE; rare in Europe |
| M17 | v1 (P3) | ✅ | m17-cxx-demod (GPL-3.0) / dsd-fme | Codec2 (LGPL-2.1) is free → **voice works without restricted codec** |
| dPMR | later | ✅ | dsd-fme | – |
| TETRA TMO – own cell: MCC/MNC, LA, cell/neighbour info, registrations, ISSI/GSSI, group attach, call setup/teardown, SDS (incl. status, text, LIP location), encryption state, signal quality per burst | v1 (P3) | ✅ (25 kHz, π/4-DQPSK 18 ksym/s) | native Rust stack `superhet-tetra` (ADR-0004); osmo-tetra (AGPL-3.0) / tetra-kit (GPL-3.0) as CI test oracles only | Large effort; reference recordings from own equipment |
| TETRA DMO – own hotspot/MTM (direct mode, DM-GATE / DM-REP): call setup, SDS, presence, gateway/repeater signalling | v1 (P3) | ✅ | native Rust stack (no maintained open-source DMO decoder known) | Must be implemented from EN 300 396; reference recordings essential |
| TETRA voice (TMO + DMO, unencrypted own traffic) – **primary goal** | v1 (P3) | ✅ | `tetra-acelp` frames → ETSI codec plugin | Codec only via separate plugin (ADR-0005); encrypted calls shown, never decrypted |
| ACARS (VHF) | v1 (P3) | ✅ | acarsdec (f00b4r0 fork, GPL-2.0; original TLeconte repo is archived) | – |
| VDL Mode 2 | v1 (P3) | ✅ | dumpvdl2 (GPL-3.0) | Cannot run together with ACARS (window) |
| FT8 / FT4 / WSPR | later | 🟡 6 m / 2 m / 10 m only without upconverter | ft8_lib (MIT), wsprd (GPL-3.0) | Needs accurate frequency (PPM calibration) and time sync |
| Meteor-M LRPT (137.9 MHz) | later (optional) | 🟡 needs QFH/V-dipole, pass scheduling | SatDump (GPL-3.0) | Large dependency; satellite availability changes |
| NOAA APT | ❌ dropped | – | – | NOAA-15/18/19 APT ended in 2025; only kept as file-replay demo if at all |

## 3. Codec plugins

| Codec | Prio | Source | License / patents | Notes |
|---|---|---|---|---|
| Codec2 (M17, FreeDV) | v1 (P3) | drowe67/codec2 | LGPL-2.1, no patents | May be bundled as a normal plugin |
| ETSI TETRA ACELP | v1 (P3) | etsi.org (EN 300 395-2 reference C code), downloaded at install time | ETSI IPR policy/terms; **not redistributable by us** | Separate repo `superhet-codec-tetra`; UI acknowledgement required |
| AMBE / AMBE+2 / IMBE (software) | v1 (P3) | mbelib (ISC), fetched at install time | Patent status unclear (DVSI); user responsibility | Separate repo `superhet-codec-mbe`; UI acknowledgement required |
| AMBE hardware (DV3000/ThumbDV via serial) | later | – | Licensed in hardware | Clean legal path for AMBE |

## 4. Online lookups (local cache, rate limits, individually switchable)

| Source | Prio | Data | Access / terms | Notes |
|---|---|---|---|---|
| RadioID.net | v1 (P3) | DMR/NXDN IDs → callsign, name, location | Free; prefer the published bulk CSV dump over per-request API | Refresh daily at most |
| HamQTH | v1 (P3) | Callsign lookup | Free, account required (XML API) | – |
| QRZ.com XML | v1 (P3) | Callsign lookup | Full data requires paid XML subscription | Optional, user's own account |
| adsbdb.com | v1 (P3) | Aircraft by ICAO24, callsign → route | Free API, rate limited | – |
| hexdb.io | v1 (P3) | Aircraft, routes, airports | Free API | Fallback for adsbdb |
| OpenSky aircraft database | v1 (P3) | ICAO24 → registration, type, operator | Downloadable CSV (bulk, offline) | Large file, monthly refresh |
| planespotters.net photos | later | Aircraft photos | Free API, attribution required, non-commercial | – |
| OurAirports | later | Airports, frequencies | Public domain CSV | – |
| ITU MID table | v1 (P3) | MMSI → flag state | Public table, bundled as data | – |
| AIS static data (msg 5/24) | v1 (P3) | MMSI → name, callsign, type | Derived from own reception | **Best MMSI source**: no free, complete MMSI API exists (MarineTraffic/VesselFinder are paid) |
| repeatermap.de | v1 (P4) | Amateur repeaters DE/EU incl. DMR/C4FM/D-Star | Public data, check API terms | – |
| RepeaterBook | later | Repeaters (mostly North America, some international) | API token required | – |
| BrandMeister API | later | Talkgroup names, last heard | Public API | DMR context |
| CelesTrak / SatNOGS DB | later | TLEs, satellite transmitters | Free | For pass prediction / satellite profiles |
| aprs.fi | later | APRS station info | API key, terms restrict bulk use | Optional |
| TETRA network/cell info | – | – | **No reliable public database known** | Superhet keeps a local, self-learned cell database from received broadcasts |

## 5. Recording, logging, replay

| Feature | Prio | RTL-SDR v5 | Notes / risks |
|---|---|---|---|
| Audio recording per VFO / per decoder call | v1 (P4) | ✅ | Opus/OGG or FLAC; call boundaries from decoder events |
| IQ recording (SigMF) | v1 (P4) | ✅ | 4.8 MB/s at 2.4 MS/s (cu8) |
| Instant replay – audio ring buffer (default 5 min, always on) | v1 (P4) | ✅ | ~2.4 MB per VFO for 5 min Opus @ 64 kbit/s |
| Instant replay – IQ ring buffer (optional, RAM or disk) | v1 (P4) | ✅ | 2.4 MS/s × 2 B = 4.8 MB/s → ~1.44 GB for 5 min; UI shows requirement before enabling; disk mode = SSD wear |
| Event log in DB, full-text search, filters, timeline, export (CSV/JSON) | v1 (P4) | – | PostgreSQL tsvector/GIN + pg_trgm, monthly partitions, retention policy |

## 6. Plugin system

| Feature | Prio | Notes |
|---|---|---|
| Plugin types `decoder`, `codec`, `lookup`, `ui` | v1 (P2: decoder; P3: codec, lookup; P5: ui) | – |
| Manifest `plugin.toml`, versioned contract | v1 (P2) | See ARCHITECTURE §6 |
| Host-side channelisation (DDC/resampling) | v1 (P2) | Plugins never see the full-band IQ unless they request it (exclusive) |
| JSON Lines events (schema validated), audio/frame output | v1 (P2) | – |
| Execution as local process or own container | v1 (P2 process, P3 container) | Container plugins declared in `compose.plugins.yaml`, no Docker socket (ADR-0003) |
| Install from Git URL / local path; plugin management UI | v1 (P2 path, P3 Git) | Supply-chain risk: show manifest + commands before install |
| UI plugins (panels/widgets, documented frontend API) | later (P5) | ES modules, sandboxing limited (single user) |
| WASM plugins (wasmtime) for in-process DSP | later | ADR-0003 keeps this as option |

## 7. Auth & operations

| Feature | Prio | Notes |
|---|---|---|
| Local admin login (first-run setup, Argon2id) | MVP | First account is the admin; required since the instance is network-exposed |
| Multi-user with roles, guest access, admin console, audit log | v1 (P1.5) | See §8, ADR-0011, ADR-0012 |
| OIDC login (generic, tested against Zitadel), mapped to roles | later (P5) | Authorization Code + PKCE |
| Persistent settings | MVP | PostgreSQL |
| Health endpoints (`/healthz`, `/readyz`) | MVP | – |
| Prometheus metrics (CPU, sample drops, USB overruns, decoder state) | later (P5) | Basic counters exist from P1 |
| `docker compose up` (main service + optional plugin containers) | MVP | USB passthrough, host udev/blacklist required |
| PostgreSQL as the database | MVP | ADR-0006 |
| Service split sdrd / api / pluginhost / worker + NATS JetStream | MVP | ADR-0007 |
| Secrets generation, NATS per-service auth, non-root hardened containers | MVP | – |
| Install script Debian/Arch VM with systemd | later (P5) | USB passthrough to VM must be documented |
| Multi-device operation | later (P5 prep, then own phase) | – |

## 8. Users, roles, guest access, admin console (P1.5)

Superhet can be shared: several accounts with roles on one instance, optional anonymous guest listening, and an admin console with an audit log. One SDR still means one capture window shared by everybody; per-user VFOs are server-side DDC channels inside it (OpenWebRX model). Decisions: ADR-0011 (users, roles, guests), ADR-0012 (audit log, admin console).

| Feature | Prio | Notes |
|---|---|---|
| Roles `admin` / `operator` / `listener` | v1 (P1.5) | admin: everything; operator: tune centre/rate/gain, device settings, bookmarks, recordings; listener: own VFOs inside the window, own audio, decoder view |
| Local accounts, per-user sessions, invite links, rate-limited login | v1 (P1.5) | Argon2id, ADR-0008 mechanics; no open self-registration |
| Per-client VFOs with ownership and per-role limits | v1 (P1.5) | CPU scales per VFO; sdrd VFO cap becomes configurable |
| Guest access without login (public page) | v1 (P1.5), **off by default** | Limits: max guests, VFOs per guest, allowed modes/frequency range, idle timeout, "only without operator lock"; decoder output with personal data hidden by default |
| Audit log (`audit_events`, append-only) | v1 (P1.5) | Retention setting; guest IPs truncated and kept 7 days by default (GDPR) |
| Admin console: users and invites, live sessions with kick, audit log with filter/CSV export, guest policy, plugins, system | v1 (P1.5) | Plugins and system sections grow with P2/P5 |
| OIDC login mapping to roles | later (P5) | Local admin stays as fallback |

Terminology: **plugin** means a process plugin of type `decoder`, `codec`, `lookup` or `ui` (ADR-0003). A browsable plugin catalog (listing first- and third-party plugins with licence, source and checksum, installable from the admin page) is planned after the plugin host exists (P3 install from Git URL, catalog as a later step); there is no separate "extension" concept.

## Explicit non-goals

- Decryption of any encrypted traffic (TEA, ARC4/BP in DMR, etc.).
- Per-user capture windows or per-user devices on one SDR (all users share the one window; only VFOs are per user, ADR-0011).
- Open self-registration; accounts are created by an admin or by invite link.
- Billing, quotas beyond the admin-set guest/user limits, multi-tenant isolation between independent operators.
- Transmit (TX) support.
- Bundling of ETSI TETRA codec or AMBE/IMBE code.
