# ADR-0016: DAB plugin design (addendum to ADR-0013)

**Status:** accepted (2026-10-27)

## Context

ADR-0013 decided that DAB/DAB+ is an exclusive out-of-process plugin wrapping `welle-cli`. The
framed transport it needs for audio and images was specified (ADR-0014) but not implemented. This
ADR records how the plugin and the framed transport were built and where that differs from
ADR-0013.

## Decision

1. **Decoder: `welle-cli` from the Debian package `welle.io`** (2.4, GPL-2.0-or-later, bookworm).
   Evaluated: `dablin` (in Debian) decodes ETI/EDI only and has no IQ input; the chain
   `eti-cmdline` + `dablin` needs a tool that Debian does not ship (and two processes).
   `welle-cli` is a complete receiver (sync, FIC, DAB and DAB+, DLS, MOT slideshow) with a web
   interface on loopback that serves the ensemble as JSON, the audio of a service as an MP3
   stream and the slide as an image. That is the simplest robust path: nothing of the DAB
   standard is reimplemented, the adapter only translates.
2. **Image size.** The Debian package pulls the Qt GUI (about 320 MB). The Dockerfile installs
   it in a throw-away stage and copies only `/usr/bin/welle-cli` into the runtime stage, next to
   its few runtime libraries (faad2, fftw3f, lame, mpg123, alsa, airspy, soapysdr, libusb): about
   7 MB. Nothing is vendored into the repository; the binary is Debian's unmodified build, its
   source is `apt-get source welle.io`.
3. **IQ path: a stand-in `rtl_tcp` server.** `welle-cli -F rtl_tcp,127.0.0.1:<port>` connects to
   a tiny server inside the adapter that serves the IQ of the `samples` frames. `rtl_tcp` is the
   one input of welle-cli that is a plain stream and paces itself (the raw file input rewinds at
   the end and throttles at half speed for a live stream). The commands welle sends (frequency,
   rate, gain) are read and ignored: sdrd already tuned the window and fixed 2.048 MS/s.
4. **Audio path: PCM in the plugin, Opus in the host.** The adapter decodes the MP3 stream of the
   selected service (symphonia, pure Rust, MPL-2.0), converts it to 48 kHz stereo s16le and sends
   `audio` frames. The plugin host cuts 20 ms blocks, encodes Opus (96 kbit/s stereo) and
   publishes `AudioFrame` v1 on `sh.v1.dev.<dev>.audio.<id>` where `<id>` is a pseudo VFO id
   (`1 << 30` plus a hash of the instance id, `InstanceInfo.audio_vfo`). Browser, API and
   recorders see an ordinary audio stream; the API audio socket lets every role subscribe to ids
   in that range (the instance is broadcast content and only operators start it). ADR-0013 said
   "re-encoded to Opus"; it did not say where. Encoding in the host keeps libopus out of
   plugins and gives every framed audio plugin the same path. The quality chain is HE-AAC (or
   MP2) -> PCM -> MP3 (welle's VBR q2, about 190 kbit/s) -> PCM -> Opus: three lossy steps, the
   MP3 one transparent at that rate.
5. **Framed transport additions** (contract v1, additive): plugin to host `status` (the plugin's
   complete state as JSON, latest kept), `media` (a binary object under a key, latest per key,
   at most 256 KiB, 16 keys) and `tune` (an exclusive plugin with a centre range moves its
   claimed window; the host validates the range, re-leases the claim at the new centre and the
   daemon retunes). Control API: `control` (JSON message to the plugin), `status`, `media`.
   `InstanceInfo.freq_hz` follows a retune and is persisted.
6. **Scan runs inside the plugin.** The plugin walks the 38 Band III blocks by sending `tune`
   frames and restarting welle-cli per block (clean state), leaving a block after 5 s without an
   ensemble id or once the service list was complete and unchanged for 2.5 s (at most 15 s). Each
   ensemble found is an event `dab.ensemble`, so scan results persist in `decoder_events` without
   a table of their own; the UI shows the newest event per block from the history plus the
   results of the running scan in the status document. The scan returns to the block it started
   on. A plugin-side scan survives a closed browser tab.
7. **Roles.** Starting and stopping DAB, tuning, scanning and choosing the service are operator
   actions (`control` is operator-only). ADR-0013 let listeners pick services; the stream is
   shared by everybody listening, so a choice changes the audio for all and is treated like
   tuning a VFO. Listeners and guests (per guest policy) see the DAB tab read-only and hear the
   stream. `status` and `media` follow the decoder history's visibility.
8. **Security.** `welle-cli` binds its web interface to all addresses of the container. The
   adapter picks a random port and the interface is up only while an instance runs; it is
   unauthenticated and can only retune its (ignored) input. On host networking the port is
   visible on the LAN; the install notice says so.

## Consequences

- The framed transport works for any plugin (codec plugins, the future ADS-B/TETRA ones); the
  contract's `status` / `media` / `tune` frames are available to them.
- Without a real ensemble recording in the repository (too large, and the project has none to
  publish) the DAB chain is tested in pieces: parsers on fixtures shaped after welle.io's source,
  the MP3 conversion on a generated tone, the adapter end to end against a stand-in `welle-cli`
  (`crates/superhet-plugin-adapters/tests/dab_plugin.rs`) and the host's framed transport against
  a fake plugin. `docs/plugins-dab.md` describes the manual check on real hardware (block 5C).
- DAB+ audio depends on faad2 (GPL-2.0) inside welle-cli; HE-AAC carries patent claims in some
  countries. The manifest's install notice says so.
- A dedicated welle.io-free path (native Rust stack) stays possible behind the same manifest.
