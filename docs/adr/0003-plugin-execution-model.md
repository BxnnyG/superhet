# ADR-0003: Plugin execution model

**Status:** accepted (2026-10-20)

## Context

Decoders and codecs come mostly from existing C/C++/Python tools (multimon-ng, direwolf, rtl_433, readsb, dsd-fme, tetra-kit, GNU Radio flowgraphs) under GPL-2.0-only, GPL-3.0 and other licenses. Users must be able to replace them with improved versions. Codecs with unclear legal status must stay outside the main distribution. Crashes in a decoder must not take down the receiver.

## Options

1. **Dynamic libraries (in-process, C ABI)**: lowest overhead; but unstable ABI, crashes kill the server, and linking GPL-2.0-only code into an AGPL-3.0 process is license-incompatible.
2. **Separate processes** with a versioned IPC contract (stdin/stdout JSON Lines for simple tools, framed Unix socket for audio/frames/control), optionally inside their own container.
3. **WASM components (wasmtime)**: sandboxed, in-process, portable; but existing C tools rarely compile cleanly to WASM (threads, sockets, SIMD), and tooling for DSP-heavy WASM is still maturing.

## Decision

**Option 2 now; option 3 recorded as a future option** for small in-process DSP plugins (e.g. a custom demodulator) once the process contract is stable. Two runtime kinds: `process` (spawned by the host, same container/VM) and `container` (separate image, socket via shared volume; required for heavy stacks like GNU Radio). Contract details: ARCHITECTURE §6.

Process plugins run inside the `superhet-pluginhost` container (which ships the common decoder tools). Container plugins are **never** started through a mounted Docker socket (that would give the pluginhost root on the host): installing a container plugin generates an entry in `compose.plugins.yaml`, which the admin applies with `docker compose up -d`; the plugin container then connects to the sdrd sample socket and NATS with its own credentials.

## Consequences

- Process separation keeps external tools as "aggregate" works with their own license → compatible with AGPL-3.0 core.
- IPC copies cost CPU: channelised data only (tens to hundreds of kS/s per plugin), not full-band IQ, except for exclusive plugins.
- Need a supervisor (restart/backoff, heartbeats, resource limits) and adapters that turn text output of CLI tools into schema-validated events.
- Installing a plugin is executing third-party code; the UI must show manifest, build commands and license before installation.
