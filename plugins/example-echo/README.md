# example-echo

Reference plugin for the Superhet plugin contract v1 (`docs/plugin-contract.md`): a decoder with
the simple transport. It counts the bytes it receives on stdin and reports them as `echo.block`
events, a heartbeat while idle and an `echo.summary` at end of input.

The instance config (`SUPERHET_CONFIG`) can set `"mode"` to `invalid` (also writes lines the host
must reject), `crash` (exits with status 3 at once) or `hang` (silent, for the heartbeat
watchdog). The contract suite and the plugin host tests use these modes.

Needs `python3`; plugins can be written in any language, only the contract matters.
It is licensed AGPL-3.0-only like the rest of the repository.
