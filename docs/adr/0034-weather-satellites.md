# ADR-0034: Weather satellites: Meteor-M LRPT through SatDump, pass prediction, image gallery

**Status:** accepted (2026-11-23)

## Context

SCOPE and the backlog ask for weather-satellite pictures: a receiver on 137 MHz, a list of when a
satellite is overhead, and a gallery of the images. The facts that shape the design were checked
on 2026-11-23 and are recorded in `superhet-satpass::catalogue`:

- **NOAA-15, -18 and -19 are gone.** NOAA decommissioned NOAA-18 on 2025-06-06 (power problem in
  an S-band transmitter), NOAA-15 on 2025-08-19 and NOAA-19 on 2025-08-13 (battery failure); their
  transmitters are shut down. The element sets still circulate because the objects are still in
  orbit, which is exactly why a pass list must not predict them. NOAA APT (137.1 / 137.62 /
  137.9125 MHz) is therefore **not live** anywhere; Superhet supports it only for recordings.
  (Sources: Wikipedia articles of the three satellites quoting the NOAA notices; CelesTrak still
  lists their elements.)
- **Meteor-M N2-2 stopped sending images** on 2019-12-18 after a micrometeoroid hit.
- **Meteor-M N2-3 (NORAD 57166) and N2-4 (59051)** are the LRPT sources that remain: 72 ksym/s
  QPSK on 137.900 MHz, at times 137.100 MHz (SatNOGS DB lists both transmitters alive; the active
  one changes). Not verified here: reception on air.
- SatDump (GPL-3.0) decodes both LRPT and APT; its latest release is 1.2.2 (2024-11). A headless
  build of the CLI with only the Meteor and NOAA product plugins is 53 MiB with its resources plus
  about 14 MiB of shared libraries, well under the 150 MiB at which the tool would have had to be
  user-supplied.

## Decision

1. **SatDump as a separate process, built in the image.** Dockerfile stage `weather` compiles the
   pinned tag 1.2.2 (commit `7aef0fe`) without GUI, OpenCL and SDR drivers, with only the Meteor
   and NOAA/MetOp plugins, and self-tests on a second of silence. `SUPERHET_SATDUMP_PATH` points the
   adapter at another build (a newer SatDump). Alternatives: `meteor_demod` + `meteor_decoder`
   (smaller, but LRPT only, unmaintained), `noaa-apt` (APT only, no live satellite left), a native
   Rust LRPT decoder (a project of its own).
2. **The plugin records passes by itself.** SatDump 1.2.2 reads baseband from a file only (a FIFO
   fails), so the adapter writes the IQ of a pass to disk and decodes it afterwards. To need no
   schedule, the adapter watches the level of the VFO's IQ (`weather::detect`): the carrier standing
   3 dB over the learned noise floor for 5 s starts a recording (4 s of pre-roll are kept), 30 s
   below ends it, and a recording shorter than a minute is dropped. Leaving the decoder running on a
   VFO on 137.900 MHz is "auto-record every pass"; nothing retunes the receiver, so the exclusive
   claim and device-policy rules cannot be violated by it. Cost: 192 kS/s `cs16` is 750 kB/s, about
   0.5 GB for a 12 minute pass while it is on disk (`max_minutes` caps it at 20 by default).
   A pass cut off by a stop or crash stays in `<store>/.work` and is decoded at the next start.
3. **Images live in the recordings volume.** The plugin host mounts the `recordings` volume and the
   adapter writes `<store>/<pass id>/{pass.json, *.png, thumb-*.png}`, the store being
   `SUPERHET_WEATHER_DIR` (default `/data/recordings/weather`). The directory is built under
   `.work` and renamed, so readers never see half a pass. The API indexes the directories (no
   migration: `pass.json` is the record), serves images with the recordings' permissions (accounts
   that may view the device; operators with control keep and delete), and trims the store to
   `SUPERHET_WEATHER_MAX_GB` (default 5, kept passes survive). Thumbnails come from the `png` crate
   in the adapter (320 px wide, at most 480 high, cut to the middle).
4. **Pass prediction in pure Rust.** New crate `superhet-satpass`: the `sgp4` crate (MIT)
   propagates TLE or OMM records, our code does sidereal rotation, look angles and pass search; it
   agrees with Skyfield on rise, culmination, set (within 6 s) and maximum elevation (0.3 degrees)
   for eight passes in the unit test. Passes include those already under way.
5. **Element sets: opt-in download, manual paste.** Stored in `superhet.settings` (key
   `weather.elements`, no migration). Off by default. When the admin switches it on, one fixed URL
   on CelesTrak (`GROUP=weather`, OMM JSON, no redirects, 512 KiB cap) is fetched with the
   User-Agent `Superhet/<version>`, at most every 2 hours (CelesTrak's update interval and its
   request) and automatically when the stored sets are 12 hours old; failures count as attempts. The
   admin can instead paste TLE or OMM text. Only the catalogue's satellites are kept.
6. **The receiver position never leaves the server.** Prediction uses the position ADS-B stored
   for the device (`adsb_range_meta`) or a position in the request body of the browser (taken from
   the ADS-B form's local storage). The download request carries nothing but the fixed URL; a test
   asserts the request headers. The API does not echo the position.
7. **The maximum elevation of a gallery entry is computed once** when the entry first appears, from
   the stored elements and the position of its device, and written into `pass.json`. Honest
   limitation: it uses the element set known at that time, not the one of the pass.
8. **Web.** A Weather tab: upcoming passes (satellite, time, countdown, elevation, azimuths,
   a Tune button) and the gallery (thumbnail, full view, download, satellite, time, maximum
   elevation, keep, delete), with the admin's element-set controls. "Tune" puts a VFO on the
   satellite's frequency and starts the plugin through the existing digital-mode path (new mode
   LRPT).

## Consequences

- The image grows by about 67 MiB (SatDump with resources, VOLK, nng, libpng, libtiff, libcurl).
- New dependencies: `sgp4` 2.4 (MIT), `png` 0.18 (MIT / Apache-2.0); SatDump (GPL-3.0) in the
  image, source = pinned upstream commit.
- The pluginhost container now mounts the recordings volume and passes `SUPERHET_WEATHER_DIR` and
  `SUPERHET_SATDUMP_PATH` to plugins. Deployments with their own compose file need the same mount.
- **Not proven on air:** the chain was run with a stand-in `satdump` and with the real SatDump on
  silence and noise (it starts, reads and finishes), not on a real Meteor transmission. The detector
  threshold (3 dB over a 144 kHz noise floor) is a starting point that may need tuning per antenna;
  `threshold_db` is a setting.
- Open: starting a recording at AOS on a VFO the plugin retunes (a scheduler that respects the
  exclusive claim and control locks) is not built; leaving the decoder running covers the use case
  with one VFO and one frequency. Doppler is left to SatDump (about +-3.4 kHz at 137 MHz fits the
  filter). Element-set age is shown but not warned about beyond that. A new Meteor launch of
  October 2026 is not in the catalogue.
