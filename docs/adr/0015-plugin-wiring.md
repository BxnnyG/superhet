# ADR-0015: Plugin wiring: input taps, claims, persistence, durable events

**Status:** accepted (2026-10-27)

## Context

ADR-0014 fixed the plugin contract and the host's supervision but left the plugin host isolated:
nothing fed `stdin`, exclusive claims existed only in the manifest, instances and events were not
persistent, the API had no plugin routes and a stop could leave helper processes behind. The
first real plugins (multimon-ng, direwolf, rtl_433, readsb, welle-cli) need all of it.

## Decision

1. **Input taps over the bus, not a socket.** sdrd serves `ctl.tap.open` / `ctl.tap.close` and
   publishes raw sample bytes on `sh.v1.dev.<dev>.tap.<id>` (core NATS, lossy like audio). The
   VFO thread converts: mono audio (48 kHz) or the channel-filtered baseband of a VFO is
   resampled to the manifest's rate and encoded (`s16le`, `f32le`, `cu8`, `cs16`, `cf32`); a
   `window` tap forwards the device blocks unchanged. The pluginhost subscribes and writes the
   chunks to `stdin`. ARCHITECTURE sketched "IQ only via Unix sockets" for bulk IQ; a bus tap
   keeps one transport for everything, works across hosts and needs no new sockets, at the cost
   of NATS payload copies. The cost is acceptable for narrow channels (tens of kB/s) and for one
   exclusive window (about 5 MB/s at 2.4 MS/s cu8, one subscriber); the Unix socket stays an option
   for several large consumers.
2. **Leases instead of explicit lifetimes.** Taps and claims expire 15 s after the last
   `open` / `acquire`; the host repeats them every 4 s. A restarted sdrd regains its taps, a
   crashed host cannot leave the device claimed, no cleanup protocol is needed. Closing early is
   an optimisation.
3. **Claims are lock holders.** `ctl.claim.acquire` remembers centre and rate, tunes, adds a
   `claim` holder to the lock set (so the existing `locked` machinery blocks retunes and the
   state shows who holds the device) and `release` / expiry tunes back. One claim per device, a
   user lock or another claim refuses it. VFOs that no longer fit are paused by the unchanged
   window rules and un-paused by the restore. Known limit: the restore snapshot in KV is
   written while the claim is held.
4. **Backpressure drops, never blocks.** sdrd: bounded queue to the publisher, drops counted as
   `superhet_sdrd_dropped_frames_total{stream="tap"}`. Host: bounded queue between subscription
   and `stdin`, drops counted in `input_dropped`. A slow plugin loses samples; nothing else
   suffers.
5. **Instances persist in PostgreSQL** (`plugin_instances`, migration 0009), not JetStream KV.
   The table is the desired state (row from `start` until `stop`), the host restores it at
   start-up. Postgres because the project already requires it for state that must survive
   (ADR-0006), the schema is queryable and migrations are versioned; KV would have needed its own
   bucket, permissions and schema discipline. The host therefore needs `SUPERHET_DATABASE_URL`
   (optional: without it behaviour is as before, in memory).
6. **Events go through the durable JetStream stream `EVENTS`** (`sh.v1.events.>`), published with
   acknowledgement and a core-NATS fallback. The worker's durable pull consumer writes them to
   `decoder_events` (`source` = plugin id) and acknowledges after the insert, so an outage of
   worker or database loses nothing within 7 days / 512 MiB. One stream, both services create it
   with the same configuration from `superhet-bus`. The worker's own `core.decoder`
   announcements are captured by the stream as well (small) and skipped by the consumer.
7. **Plugin events share `decoder_events`.** Same history, export, retention and live feed as the
   built-in detectors; `source` tells them apart, `kind` is the plugin's event type, `vfo` is
   nullable. Column additions only; existing consumers see one more JSON field.
8. **Process groups are killed.** Whenever a process ends (stop, crash, watchdog), the host sends
   `SIGKILL` to its whole group, so helpers started by a wrapped tool cannot leak.
9. **The image does not ship python3.** The reference plugin needs it only for the integration
   test, which runs on the CI runner / developer host with the real binaries. Real plugins bring
   their own runtime (a wrapped native binary, or their own image/volume).
10. **Roles:** installing and removing plugins is `admin`; listing plugins and starting, stopping
    and restarting instances is `operator` (listing is not `admin`-only because the start dialog
    needs the manifests). All changes are audited.

## Consequences

- A decoder plugin can be installed, started on a VFO and its events appear in the history and
  the live feed; exclusive plugins can take the device and give it back.
- A `discriminator` plugin receives the VFO's demodulated NFM audio (de-emphasis and the audio
  band-pass applied), not the raw discriminator output; if a decoder needs the latter, a
  dedicated tap kind is a small addition.
- The framed transport is still not implemented by the host; framed input (`samples` frames with
  sequence numbers and tuning epochs) is the place to give plugins loss detection.
- Plugin resource limits, container plugins and Git installs remain P3.
- Existing `nats.conf` copies and the pluginhost's compose entry (database URL) must be updated;
  the installer's content-hashed config handles the former.
