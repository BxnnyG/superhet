# First-party decoder plugins

Three plugins ship with the image and are installed on first start (`plugins/pocsag`,
`plugins/aprs`, `plugins/rtl433`). Each is a manifest plus a call of the `superhet-plugin-adapter`
binary, which starts the real tool as a child process, lets it read the plugin's `stdin` (the samples
the host feeds, `docs/plugin-contract.md`) and turns what the tool prints into contract events
(JSON Lines). The tools are separate programs with their own licences (`docs/third-party.md`).

| Plugin id | Tool | Input | Events |
|---|---|---|---|
| `org.superhet.decoder.pocsag` | multimon-ng | `discriminator`, `s16le` 22 050 Hz (bind to an NFM VFO, 12.5 kHz) | `pocsag.message`, `flex.message` |
| `org.superhet.decoder.aprs` | direwolf | `discriminator`, `s16le` 22 050 Hz (NFM VFO) | `aprs.packet` |
| `org.superhet.decoder.rtl433` | rtl_433 | `iq`, `cu8` 250 kHz (VFO bandwidth about 250 kHz) | `rtl433.reading` |
| `org.superhet.decoder.dab` | welle-cli | exclusive, `iq` `cu8` 2.048 MS/s, framed transport, audio out | `dab.ensemble`, `dab.dls`, `dab.play`, `dab.scan`, `dab.error` (own page: `docs/plugins-dab.md`) |

All events land in the decoder history (`source` = plugin id, `kind` = event type, `value` = the
`summary` field; `docs/api-decoder-events.md`). Event schemas: `plugins/<name>/schema/events.json`.

## Use

In the web UI: Decoders tab, "Decoders on VFO n" (`docs/plugins-ui.md`). Over the API:

```
# an NFM VFO (id 1) on the paging channel, then
POST /api/v1/plugin-instances  {"plugin_id":"org.superhet.decoder.pocsag","device":"dev0","vfo":1}
```

The VFO's mode must be NFM for the audio plugins: the host passes on what the VFO demodulates, so
a WFM or AM VFO gives the decoder the wrong signal. Squelch is applied before the tap; open it
(or set it low) for data channels, otherwise bursts that start below the squelch threshold are cut.

## POCSAG / FLEX

Config (all optional): `baud` (array of 512 / 1200 / 2400, default all), `flex` (bool, default
true), `invert` (bool, for receivers with inverted audio), `charset` (`US FR DE DK SE SI`, for
umlauts in alphanumeric pages). Unknown keys make the instance fail at start (visible as
`last_error`).

`pocsag.message` data: `baud`, `address` (RIC, `null` for continuation words), `function` (0-3),
`kind` (`alpha`, `numeric`, `skyper`), `message`, `summary`. `flex.message`: `baud`, `levels`,
`cycle`, `frame`, `phase`, `capcode` (+ `capcodes` for group calls), `kind`
(`alpha`, `numeric`, `tone`, `binary`), `message`, `summary`. FLEX is decoded with multimon-ng's
`FLEX_NEXT` (the legacy `FLEX` stamps wall-clock time into its lines and has no fragment
handling). Trailing padding (`<EOT>`, spaces) is stripped.

Privacy and law: pager traffic is private communication in many countries. Receiving or storing
the content of pages not addressed to you can be restricted or forbidden (Germany, for example,
protects the secrecy of telecommunications). The install dialog carries this notice; decoded
messages stay in the decoder history until `SUPERHET_EVENT_RETENTION_DAYS`.

Real-hardware hints (Germany; verify the frequencies and your rights before use): the amateur
radio paging network DAPNET (POCSAG, 1200 baud, around 439.9875 MHz, NFM) is meant to be received
by licensed amateurs; commercial public paging services in Germany have been shut down, so most
other pagers you find are company or hospital systems on private frequencies. There is no public
FLEX service in Germany (FLEX is mostly North American, 929/931 MHz); the FLEX path is therefore
tested with a generated recording only.

## APRS

No config. The plugin runs direwolf receive-only with `plugins/aprs/direwolf.conf` (audio from
stdin, `AGWPORT 0`, `KISSPORT 0`: no network ports, no transmit). `aprs.packet` data: `source`,
`dest`, `path` (digipeaters, `*` marks the one that repeated it), `info` (raw information field,
non-printable bytes as `<0x1c>`), `summary`, and by `type`:

- `position` (plain and compressed, with or without timestamp), `mic-e`, `object`, `item`: `lat`,
  `lon` (decimal degrees, 5 decimals), `symbol` (table + code), `comment`, optional `course`,
  `speed_kn`, `altitude_m`; objects and items also `name`, `live`
- `message`: `addressee`, `message`, optional `message_id`
- `status`: `status`
- `weather`, `telemetry`, `nmea`, `other`: the raw `info` only

Real hardware: 144.800 MHz (Europe, NFM, 12.5 kHz) or 144.390 MHz (North America); dense city
areas give several packets per minute. A packet that direwolf cannot repair (CRC) is not reported.

## rtl_433

Config: `protocols` (array of rtl_433 protocol numbers, `rtl_433 -R help`); default is rtl_433's
standard set. Bind to a VFO with a bandwidth of about 250 kHz centred on the sensor band (433.92
MHz; 868.3 MHz is common for European weather stations). The IQ tap is the VFO's channel filtered
baseband, so a narrow VFO misses wideband FSK sensors and OOK edges; rtl_433's frequency offset
and level measurements are not reported.

`rtl433.reading` data is rtl_433's JSON object (`model`, `id`, `channel`, `battery_ok`,
`temperature_C`, `humidity`, ... depending on the model) without the receiver fields (`time`,
`freq*`, `rssi`, `snr`, `noise`, `mod`) plus `summary`. rtl_433 reads its input in blocks of
256 KiB, so a reading appears up to about 0.5 s after the transmission ended. Expect false
positives on noisy bands; restrict `protocols` for a quiet history.

## Tests and recordings

`crates/superhet-plugin-adapters/tests/golden.rs` runs each SigMF recording in
`crates/superhet-plugin-adapters/tests/recordings/` through the plugin with the real tool and
compares the events with `<name>.expected.jsonl` (ROADMAP P2 criterion 2). The recordings are
synthetic (`scripts/gen-plugin-recordings.py`, fixed seed, invented payloads, CC0): POCSAG
512/1200/2400 baud alpha and numeric, FLEX 1600 (made with multimon-ng's public-domain `gen-ng`;
the script skips it when that tool is absent), four APRS frames (position, message, status,
Mic-E; direwolf's own decode of the Mic-E frame agrees with the adapter) and a Nexus-TH sensor
burst for rtl_433. Each file is below 2 MB.

```
cargo test -p superhet-plugin-adapters                 # parsers + golden (golden skips without the tools)
SUPERHET_REQUIRE_PLUGIN_TOOLS=1 cargo test -p superhet-plugin-adapters --test golden   # CI mode
UPDATE_GOLDEN=1 cargo test -p superhet-plugin-adapters --test golden                   # rewrite expectations
scripts/check-plugin-image.sh superhet:ci              # the same recordings through the built image
```

CI installs `multimon-ng direwolf rtl-433` on the runner for the first and checks the built image
in the `docker` job. Tool versions: Debian bookworm (the image) has multimon-ng 1.2.0, direwolf 1.6
and rtl_433 22.11; the parsers accept the newer FLEX_NEXT layout and direwolf channel tags as well.
