# Weather satellites (Meteor-M LRPT)

Decision record: ADR-0034. Plugin `org.superhet.decoder.weather` (`plugins/weather/`), adapter
`crates/superhet-plugin-adapters/src/weather/`, pass prediction `crates/superhet-satpass`, REST
`docs/api-weather.md`, UI: the **Weather** tab and the **LRPT** digital mode.

## What can be received (verified 2026-11-23)

| Satellite | NORAD | State | Downlink |
|---|---|---|---|
| Meteor-M N2-4 | 59051 | in service | LRPT 72 ksym/s, 137.900 MHz, at times 137.100 MHz |
| Meteor-M N2-3 | 57166 | in service | LRPT 72 ksym/s, 137.900 MHz, at times 137.100 MHz |
| Meteor-M N2-2 | 44387 | no images | image downlink ceased 2019-12-18 (micrometeoroid hit) |
| NOAA 15 | 25338 | decommissioned 2025-08-19 | was APT 137.620 MHz |
| NOAA 18 | 28654 | decommissioned 2025-06-06 | was APT 137.9125 MHz |
| NOAA 19 | 33591 | decommissioned 2025-08-13 (battery failure) | was APT 137.100 MHz |

**NOAA APT is not live any more.** The NOAA satellites are still in orbit and still appear in
element sets, but their transmitters are off, so Superhet neither predicts nor offers them; the APT
decoder (`pipeline = "apt"`) exists for recordings of other APT sources. The rows come from the
Wikipedia articles of the satellites (quoting NOAA's decommissioning notices), the SatNOGS DB
transmitter lists (Meteor transmitters alive) and CelesTrak; they are written into
`superhet-satpass::catalogue` and shown in the tab with the date they were checked. "In service" is
what those sources say, not something this project has received on air. Which of the two
frequencies carries the picture changes from week to week; try 137.900 MHz first.

## How it works

1. You give a VFO the frequency (137.900 MHz) and start the plugin on it: Weather tab, **Tune** on
   a pass (sets the VFO to NFM / 144 kHz, squelch off, starts the plugin; the first time you confirm
   the notice), or the **Digital** picker, mode LRPT. Needs the plugin rights (operator / admin).
2. The plugin gets the VFO's baseband as `cs16`, 192 kS/s. It measures the level every 0.25 s
   and learns the noise floor of the quiet band. When the level stands `threshold_db` (default 3 dB)
   over the floor for `arm_seconds` (5), it starts a recording (4 s before are kept) and emits
   `weather.recording`. It ends `hang_seconds` (30) after the level has gone. Recordings shorter
   than `min_seconds` (60) are dropped; a pass is cut at `max_minutes` (20).
3. The IQ file is written to `<store>/.work/` and handed to
   `satdump meteor_m2-x_lrpt baseband <file> <dir> --samplerate 192000 --baseband_format cs16`
   (`meteor_m2-x_lrpt_80k` for `symbol_rate = "80k"`). SatDump demodulates, decodes and renders
   the images. While a pass is on disk it needs about 45 MB per minute.
4. The PNGs, thumbnails and `pass.json` are put into `<store>/<unix seconds>-<4 hex>/` and a
   `weather.pass` event (images, satellite, peak level) goes into the decoder history; a decode
   with no image gives `weather.failed` (`no_image`, `tool_failed`, `tool_missing`).
5. The API lists the directories as the gallery (`docs/api-weather.md`).

Leaving the decoder running on the VFO is the "auto-record every pass" mode: it needs no schedule
and never retunes the receiver, so exclusive claims and other users' control are not touched. A pass
that was cut off by a stop or a crash is decoded the next time the plugin starts (the IQ file and a
sidecar stay in `.work`); with SatDump missing the recordings are kept for 24 hours.

## Pass prediction

`superhet-satpass` (pure Rust, SGP4 through the `sgp4` crate) turns TLE or OMM element sets into
passes: rise, highest point, set, maximum elevation and azimuths, for the operational satellites
only. In a unit test eight passes of Meteor-M N2-4 agree with Skyfield within 6 s and 0.3 degrees.
Orbit data ages: a few days are fine for these satellites; the tab warns after three.

- **Position:** the one ADS-B already stores for the device (`adsb_range_meta`, set when ADS-B is
  started with a position), or the one typed in the tab (the same two fields and browser storage
  as ADS-B). It is only used by the API process to compute passes. It is not logged, not stored by
  this feature, not echoed and never part of any request to another host.
- **Element sets:** nothing is downloaded unless an admin switches it on (Weather tab, *Orbit
  data*, or `PUT /api/v1/admin/weather/elements`). Then the server fetches
  `https://celestrak.org/NORAD/elements/gp.php?GROUP=weather&FORMAT=json` (a few kB, no redirects,
  512 KiB cap) with the User-Agent `Superhet/<version>`, at most every 2 hours (CelesTrak refreshes
  its data every 2 hours and asks clients not to poll faster; a failed attempt counts) and
  automatically when the stored sets are 12 hours old. Only the catalogue's satellites are kept.
  CelesTrak's data terms apply; see <https://celestrak.org/>. The admin can instead paste TLE or OMM
  text (works with the download off, e.g. offline installations).

## Gallery and storage

Directory: `SUPERHET_WEATHER_DIR` (default `/data/recordings/weather`, i.e. in the recordings
volume, which `compose.yaml` now mounts into `superhet-pluginhost` as well). Quota:
`SUPERHET_WEATHER_MAX_GB` (default 5, `0` = none), checked every 5 minutes: the oldest passes that
are not kept are deleted until the store is under 90 %. Who sees what follows the recordings:
accounts that may view the device list, show and download; operators with control of the device
keep and delete. Guests see nothing. The maximum elevation is computed when an entry first shows
up (from the element sets and the position then known) and stored in `pass.json`.

## Configuration

Instance config (all optional, `plugins/weather/schema/config.json`): `pipeline` (`lrpt` default,
`apt`), `symbol_rate` (`72k`, `80k`), `satellite` (name to store when SatDump does not say),
`threshold_db`, `min_seconds`, `max_minutes`, `arm_seconds`, `hang_seconds`.

| variable | where | meaning |
|---|---|---|
| `SUPERHET_WEATHER_DIR` | pluginhost (passed to the plugin), api | image store, default `/data/recordings/weather` |
| `SUPERHET_WEATHER_MAX_GB` | api | quota, default 5 |
| `SUPERHET_SATDUMP_PATH` | pluginhost (passed to the plugin) | another `satdump` (file or directory), default the one in the image |

## Importing a recording (also NOAA APT)

`superhet-plugin-adapter weather-decode [--pipeline lrpt|lrpt80|apt] [--format cs16|cu8|cs8|cf32]
--rate <Hz> [--satellite NAME] [--freq HZ] <file>` decodes an IQ file (for example from the
recordings library: SigMF `ci16_le` is `cs16`) into the gallery, with the user's file left alone:
`docker compose exec superhet-pluginhost superhet-plugin-adapter weather-decode --rate 192000 ...`.
Exit status 1 means SatDump ran but found no image. Audio files (WAV) are not supported.

## Limits and honesty

- Not received on air: the chain is tested with a stand-in `satdump`, with the real SatDump on
  silence and noise (it starts, reads, finishes, finds nothing) and with synthetic level steps.
  Whether `threshold_db = 3` fits your antenna and noise floor is for you to find out; a pass that is
  too weak to trigger can be recorded with the IQ recorder and imported.
- A 137 MHz antenna (V-dipole, turnstile, QFH) with a clear sky is needed; an RTL-SDR with a
  wideband whip will not do. A bias-tee LNA and a 137 MHz filter help a lot.
- Scheduled recordings that retune a VFO at the start of a pass (and respect device claims and
  control locks) are not built.
- Licence: SatDump is GPL-3.0 and runs as a separate process (`docs/third-party.md`). Receiving
  weather satellites is legal in most countries; the plugin shows a notice before it starts.
