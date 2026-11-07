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

- `m17/` – M17 digital voice (m17-demod behind `superhet-plugin-adapter m17`): non-exclusive, taps
  an NFM VFO, framed transport with call events, a status document and the decoded voice as plugin
  audio (`docs/plugins-m17.md`, ADR-0018).

- `dsd/` – DMR / dPMR / NXDN (a **user-installed** dsd-fme behind `superhet-plugin-adapter dsd`):
  manifest and adapter only, no codec in the repository, image or releases; without the tool the
  plugin reports "needs dsd-fme". Non-exclusive, framed transport, call events per DMR slot, status
  document and decoded voice as plugin audio (`docs/plugins-dsd.md`, ADR-0018).

- `tetra/` – TETRA TMO (downlink): the adapter demodulates the 25 kHz channel itself and feeds `tetra-rx` (osmo-tetra-sq5bpf, built into the image); cell, neighbours, calls, SDS and status events, encryption flags (never decrypted), voice through the **user-installed** ETSI codec (`SUPERHET_TETRA_CODEC_DIR`, never shipped; without it the plugin reports "voice needs the ETSI codec"). Non-exclusive, framed transport, IQ input (`docs/plugins-tetra.md`, ADR-0021).

Install a plugin through the admin API (`POST /api/v1/plugins`, `docs/api-plugins.md`) or the host's `install` command; the host copies it to
`$SUPERHET_PLUGIN_DIR/<id>/<version>` (default `/data/plugins`).
