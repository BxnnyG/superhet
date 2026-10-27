# ADR-0014: Plugin contract v1 and host supervision

**Status:** accepted (2026-10-27)

## Context

ADR-0003 chose separate processes with a versioned contract; ARCHITECTURE section 6 sketched it as
protocol version 0. P2 needs a fixed v1 before decoders are written against it, plus a supervisor.
Three points of the sketch did not survive contact with the bus and the threat model:

- Plugin ids are reverse-DNS (`org.superhet.decoder.pocsag`). Dots separate NATS subject tokens,
  so `sh.v1.plugin.<id>.ctl` and `sh.v1.events.<plugin>.<type>` do not address one plugin.
- The pluginhost holds NATS and database credentials in its environment; children inherit the
  environment by default.
- The framed transport is only needed by audio / voice-frame / codec / lookup plugins and needs
  a real consumer to be designed against, while the simple transport already covers the first
  decoders (multimon-ng, rtl_433, direwolf adapters).

## Decision

1. **Contract v1** is as in `docs/plugin-contract.md`; the Rust types in
   `superhet-plugin-protocol` are the source of truth, JSON Schemas are generated from them and a
   test pins the committed copies. `PROTOCOL_VERSION = 1`; the host rejects any other
   `plugin.protocol`. Manifests are strict (unknown keys are errors), paths cannot leave the
   plugin directory.
2. **Bus naming**: host control is `sh.v1.plugin.ctl.<cmd>` with plugin and instance ids in the
   body; instance state goes to `sh.v1.plugin.state`; plugin events go to
   `sh.v1.events.<plugin id with "." as "_">.<event type as tokens>`. `_` is outside the id
   alphabet, so the mapping is unambiguous. This supersedes the `sh.v1.plugin.<id>.ctl` row of
   ARCHITECTURE section 1.
3. **Event validation** applies to the complete event object (envelope plus `data`) against the
   plugin's JSON Schema, so one schema can switch on `type`. Invalid lines are dropped and
   counted, never fatal.
4. **Supervision**: a crash is any exit, spawn failure, external kill or silence beyond the
   manifest's `heartbeat_timeout_s`. Exponential backoff from 1 s to 30 s; the 5th crash in a
   sliding 5 minute window ends in state `error` until an explicit `restart`. Stopping closes stdin
   first and kills after a 5 s grace period.
5. **Environment scrubbing**: children get `PATH`, `LANG`, `LC_ALL`, `TZ`, `TMPDIR` and the
   `SUPERHET_*` instance variables only.
6. **Exclusive claims** are part of the manifest (`[device]`: `exclusive`, fixed centre or centre
   range, rate) so install dialogs and later enforcement have what they need; the host enforces
   only "one exclusive instance per device" for now. The retune/pause/restore protocol belongs to
   sdrd (ARCHITECTURE 3.4) and follows with the first exclusive plugin.
7. **Scope split**: the host implements the simple transport. The framed transport is specified
   and tested at wire level, but the host refuses framed, container and UI plugins (`unsupported`)
   until a plugin needs them.
8. **Install** copies a local directory into `<SUPERHET_PLUGIN_DIR>/<id>/<version>` after
   validation and refuses symbolic links; build commands are displayed, not executed.

## Consequences

- Third-party tools can be wrapped in any language; the contract suite
  (`superhet_plugin_protocol::contract`) gives authors the same checks as CI.
- Event subjects have a stable shape for the worker and API, but the durable stream and
  persistence of plugin events are still to be built.
- Plugins are trusted code, not sandboxed: scrubbed environment and process group are hygiene, not
  isolation. Resource limits (`prlimit`/cgroups) and killing whole process trees are open; a
  wrapped tool that outlives its adapter can leak until the container restarts.
- Instances are not persisted; after a host restart the API caller starts them again.
- Changing the contract later means a new protocol number and a new ADR.
