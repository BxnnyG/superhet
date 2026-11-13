# ADR-0028: Recordings and instant replay in the API service, no re-encoding

**Status:** accepted (2026-11-11)

## Context

ROADMAP P4 asks for audio recording per VFO, SigMF IQ recording and Instant Replay. The daemon
already publishes every VFO's audio as 20 ms Opus packets (`docs/protocol-audio.md`) and can
copy a VFO's baseband or the whole sample window to a consumer (the plugin taps,
`superhet_bus::tap`). The consumers that exist today (plugin host, worker) do not know users,
roles or device policies; the API service does, and it already serves files and audits actions.

## Decision

- **Where.** The recording engine lives in `superhet-api` (`recorder.rs`; REST in
  `recordings.rs`). It needs no new bus permission: the API may subscribe to `sh.v1.>` and publish
  `sh.v1.dev.*.ctl.>`. `superhet-sdrd` is unchanged. A small library crate
  `superhet-recording` holds the pure parts (Ogg writer and reader, SigMF metadata, ring buffer,
  squelch gate, quota planner, file-name rules) so they are unit-tested without I/O.
- **Audio: no re-encode.** The Opus packets of the audio stream are wrapped into Ogg pages as they
  are (RFC 7845, pre-skip 312, 50 packets per page). Stereo/mono switches mid-stream are not a
  new stream: the header carries the channel count of the first packet and Opus decoders follow
  the packet's own TOC. Optional squelch gating keeps a 100 ms pre-roll and 400 ms hang time and
  drops the rest; the wall-clock start of every kept segment is stored in the row (`meta.segments`),
  so timestamps survive the cut.
- **Instant replay.** One task subscribes to all audio subjects and keeps the last N seconds
  (`SUPERHET_REPLAY_SECONDS`, default 60, 5..600; 16 MiB cap per VFO) of encoded packets per
  (device, VFO) in memory. "Replay last 10/30/60 s" serves them as an Ogg file built on request
  (`GET .../vfos/{id}/replay`); "save replay" writes the same bytes to the library. Nothing about
  replay touches the disk until the user saves.
- **IQ: SigMF through taps.** An IQ recording opens a tap (window = whole device stream at the
  device rate and centre, or a VFO's decimated baseband at a chosen rate), appends the chunks to
  `<id>.sigmf-data` (`cu8`, `ci16_le` or `cf32_le`) and writes `<id>.sigmf-meta` (SigMF 1.2:
  datatype, sample rate, frequency, ISO 8601 start, annotation, SHA-512 computed while writing).
  Limits per recording: time, size; at most 16 recordings run at once. Taps ride on core NATS,
  which is lossy: the row stores a `completeness_percent` (samples received vs. wall time).
- **Library and quota.** Rows in PostgreSQL (migration 0018 `recordings`), files under
  `SUPERHET_RECORDINGS_DIR` (`audio/` and `iq/`). A file name is a generated stem
  (`rec<id>-<freq>hz`), re-derived from the row for every access; clients never choose a path, so
  there is no traversal surface (tested). `SUPERHET_RECORDINGS_MAX_GB` (default 20, 0 = off) is
  enforced before every start and by a janitor every 30 s: oldest finished recordings that are
  not marked **keep** go first, down to 90 %. If only kept recordings remain, new recordings are
  refused (507 `quota_full`) and running ones end with reason `quota`. Rows left in `recording`
  by a restart become `interrupted` (the file keeps what was written).
- **Access.** Accounts (not guests) list, play, download and replay, limited by the per-device
  `view` policy (ADR-0023) and, for replay, by the same own-or-shared-VFO rule as live audio.
  Starting, stopping, renaming, keep and delete need operator or admin *and* `control` on the
  recording's device. Audit events: `recording.start`, `.stop`, `.update`, `.delete`,
  `.save_replay`.
- **Scanner hook.** `record_hits: true` on `scanner/start` (handled by the API, not forwarded)
  records each hit (`scanner.hit` on the bus) as a gated audio recording that ends after 4 s of
  closed squelch, a new hit or the end of the scan; recordings shorter than 300 ms are dropped.
  The hook lives in API memory for the duration of the scan.

## Consequences

- No second encode and almost no CPU: 50 packets/s per VFO into memory, plus one file write per
  second per running recording.
- The ring buffers are always on while the API runs (about 0.5 MB per VFO and minute at 64 kbit/s).
  They hold audio that was audible to whoever could listen, and are reachable only through the
  replay route with the same rules as the live stream.
- Full-rate IQ over NATS is best effort. For long lossless captures of a whole band an
  `rtl_sdr` run on the host is the better tool; the completeness figure tells which recordings
  to trust.
- Recordings of a device with a restricted policy are visible only to accounts that may view the
  device, also in the list.
- Plugin audio (DAB, M17, DSD) is not buffered or recorded yet.
