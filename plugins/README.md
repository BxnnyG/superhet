# Plugins

First-party plugins live here. The contract (manifest, transports, supervision, control API) is
`docs/plugin-contract.md`; JSON Schemas are in `crates/superhet-plugin-protocol/schema/`.

- `example-echo/` – reference plugin (simple transport) used by the contract test suite and the
  plugin host tests. Copy it as a starting point.

- `pocsag/`, `aprs/`, `rtl433/` – the first decoder plugins (multimon-ng, direwolf, rtl_433 behind
  `superhet-plugin-adapter`, `docs/plugins-decoders.md`). The image installs them on first start
  (`SUPERHET_BUNDLED_PLUGIN_DIR`, default `/usr/share/superhet/plugins`).
- `dab/` – DAB / DAB+ (welle-cli behind `superhet-plugin-adapter dab`): the first exclusive, framed-transport
  plugin with audio, status and slideshow output (`docs/plugins-dab.md`, ADR-0016).

- `adsb/` – ADS-B aircraft on 1090 MHz (readsb behind `superhet-plugin-adapter adsb`): exclusive,
  framed transport, live picture as the status document, rationed events (`docs/plugins-adsb.md`,
  ADR-0017).

Install a plugin through the admin API (`POST /api/v1/plugins`, `docs/api-plugins.md`) or the host's `install` command; the host copies it to
`$SUPERHET_PLUGIN_DIR/<id>/<version>` (default `/data/plugins`).
