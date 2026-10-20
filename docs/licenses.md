# License matrix

Superhet core: **AGPL-3.0-only** with attribution requirement (AGPL § 7(b), see `NOTICE`).

External components run as **separate processes** (plugins) or are **dynamically linked system libraries**; they keep their own licenses. Licenses below were taken from the upstream repositories on 2026-10-20 and must be re-verified before a plugin is published.

| Component | Use | License | Distribution by Superhet | Notes |
|---|---|---|---|---|
| librtlsdr (osmocom/rtl-sdr) | device backend (dynamic lib, hand-written FFI, cargo feature `rtlsdr`) | GPL-2.0-or-later | system package in image (`librtlsdr0`; `librtlsdr-dev` only in the build stage) | no source vendored; AGPL-3.0 core links it dynamically, see ADR-0001 |
| SoapySDR | device backend (later) | BSL-1.0 | system package | – |
| PostgreSQL | database (separate container) | PostgreSQL License | official image | – |
| NATS server | message bus (separate container) | Apache-2.0 | official image | – |
| libopus | VFO audio encoding in `superhet-sdrd` (dynamic link, in-repo FFI in `superhet-bus` feature `opus`; also decodes in tests) | BSD-3-Clause | system package in image (`libopus0`; `libopus-dev` only in the build stage) | not vendored or bundled; hand-written FFI like librtlsdr |
| rustfft and other Rust crates | core | MIT / Apache-2.0 (checked by `cargo deny` in CI) | compiled in | – |
| multimon-ng | POCSAG/FLEX plugin | GPL-2.0 | plugin image | – |
| direwolf | APRS plugin | GPL-2.0 | plugin image | – |
| rtl_433 | ISM plugin | GPL-2.0 | plugin image | – |
| readsb (wiedehopf) | ADS-B plugin | GPL-3.0 (+ BSD parts) | plugin image | – |
| AIS-catcher | AIS plugin | GPL-3.0 | plugin image | – |
| acarsdec (f00b4r0 fork) | ACARS plugin | GPL-2.0 | plugin image | original TLeconte repo archived |
| dumpvdl2 | VDL2 plugin | GPL-3.0 | plugin image | – |
| redsea | RDS plugin | MIT | plugin image | – |
| dsd-fme | digital voice plugin (candidate) | ISC/GPL mix – verify | **fetch & build locally** if it links mbelib | ADR-0005 |
| digiham | digital voice plugin (candidate) | GPL-3.0 | plugin image | little maintenance since 2024 |
| codecserver | optional codec backend | GPL-3.0 | separate | little maintenance since 2024 |
| m17-cxx-demod | M17 plugin | GPL-3.0 | plugin image | – |
| Codec2 | M17 voice codec | LGPL-2.1 | plugin image | patent-free |
| osmo-tetra (incl. sq5bpf fork) | TETRA test oracle in CI | AGPL-3.0 | not distributed (CI dev container only) | ADR-0004 |
| tetra-kit | TETRA test oracle in CI | GPL-3.0 | not distributed (CI dev container only) | ADR-0004 |
| ETSI TETRA ACELP reference codec | TETRA voice | ETSI IPR / download terms | **never distributed**; downloaded from etsi.org and built by the user | separate repo `superhet-codec-tetra` |
| mbelib | AMBE/AMBE+2/IMBE voice | ISC; patent status unclear (DVSI) | **never distributed**; fetched and built by the user | separate repo `superhet-codec-mbe` |
| ft8_lib | FT8/FT4 plugin (later) | MIT | plugin image | – |
| SatDump | Meteor LRPT (optional, later) | GPL-3.0 | plugin image | – |
| FutureSDR / seify | not used (ADR-0001), re-evaluated later | Apache-2.0 | – | – |

## Data sources

| Source | Terms |
|---|---|
| RadioID.net dumps/API | free use, respect rate limits |
| HamQTH | free account |
| QRZ.com XML | paid subscription for full data, user's own account |
| adsbdb, hexdb.io | free APIs, rate limited |
| OpenSky aircraft DB | free download, attribution |
| OurAirports | public domain |
| ITU MID table | public reference data |
| IARU Region 1 bandplan | facts curated into own data file |
