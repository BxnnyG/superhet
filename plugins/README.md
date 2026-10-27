# Plugins

First-party plugins live here. The contract (manifest, transports, supervision, control API) is
`docs/plugin-contract.md`; JSON Schemas are in `crates/superhet-plugin-protocol/schema/`.

- `example-echo/` – reference plugin (simple transport) used by the contract test suite and the
  plugin host tests. Copy it as a starting point.

Install a plugin from a local path with the host's `install` command; the host copies it to
`$SUPERHET_PLUGIN_DIR/<id>/<version>` (default `/data/plugins`).
