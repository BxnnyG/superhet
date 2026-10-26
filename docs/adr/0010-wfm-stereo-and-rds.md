# ADR-0010: WFM stereo, stereo audio streams and native RDS

**Status:** accepted (2026-10-23)

## Context

WFM was mono only: the discriminator output was low-passed at 15 kHz and played on one channel,
the pilot, the L-R subcarrier and the RDS data were thrown away. Broadcast listening on a
"modern SDR#-like" receiver needs stereo and the station name / RadioText. Two things had to be
decided: how stereo travels through the audio chain (the wire format, Opus and the browser mixer
were mono end to end), and where RDS is decoded (SCOPE and ROADMAP had it as a redsea plugin in
P2).

## Options

**Stereo on the wire**
- *Always stereo for every VFO*: wastes bitrate and decoder work for NFM/SSB.
- *Mid-stream switch between mono and stereo packets*: Opus decoders cannot change the channel
  count of a stream; the browser would need a decoder swap at an unpredictable moment, and a
  weak pilot would flip the format back and forth.
- *Channel count as a property of the VFO configuration* (WFM with stereo on = 2 channels, else
  1), mono-on-both-channels while the pilot is missing, and a flag for "pilot locked".
  The format only changes when the user switches the setting.

**RDS**
- *redsea plugin* (external process, MIT): needs demodulated 171 kS/s MPX as an extra stream,
  a plugin transport for IQ-derived data that does not exist yet, and a second implementation of
  the pilot tracking the stereo decoder needs anyway.
- *Native in `superhet-dsp`*: reuses the pilot phase (the RDS carrier is the third harmonic of
  the pilot), costs about 2-3 % of a core per WFM VFO (measured), no new process, no new dependency.

## Decision

**Stereo and RDS are decoded natively in the daemon; stereo is a per-VFO stream format.**

- **DSP** (`superhet-dsp::wfm`, `::rds`, `::channel`): a second-order PLL (25 Hz, damping 0.707)
  locks to the 19 kHz pilot of the discriminator output. Its phase gives the 38 kHz carrier for
  L-R and the 57 kHz carrier for RDS. L-R runs through the same 15 kHz decimating filter and
  de-emphasis as L+R, then the matrix `L = S + b D`, `R = S - b D`. The blend `b` fades in 50 ms
  to 1 while the pilot is locked (amplitude above 4 % of full deviation and phase error small for
  150 ms) and back to 0 when it drops below 2.5 % or the phase error grows for 80 ms (hysteresis),
  so a weak station plays mono without clicks. De-emphasis is 50 us (default) or 75 us as before.
  The PLL detector has no limiter: clipping the large stereo sidebands would bias the lock phase.
- **RDS** (`superhet-dsp::rds`): the multiplex is mixed with `exp(-j 3 phi)`, low-passed and
  resampled to 19 kHz (16 samples per bit). Carrier recovery is a decision-directed BPSK Costas
  loop with frequency-locked-loop assist on the derotated samples, so a jittering or slightly
  offset pilot-derived carrier (up to about +-150 Hz) does not matter. The matched filter is the
  pulse of IEC 62106 (Manchester pair of the `cos(pi f T / 4)` shape); bit timing is a
  second-order early/late loop with cubic interpolation that follows sample-clock errors of a few
  hundred ppm (the 16 sample phases only seed it). Soft symbols are differentially decoded, every
  bit carries a reliability. Blocks are found by the syndrome of the 10-bit check word with the
  five offset words, sync after two valid blocks at the right distance, loss after more than 24
  bad blocks of 40, a quick re-sync after three bad blocks in a row (bit slip). In sync, bursts
  of up to five bits are corrected through a syndrome table (these bursts have unique syndromes);
  the longer the burst, the more unreliable the flipped bits must be relative to the rest of the
  block. Groups 0A/0B and 2A/2B give PS, TA, M/S and RadioText; PI, PTY and TP come from every
  group. PI must be received twice before it is shown or replaces a station; PS and RadioText
  segments are taken over at once only from error-free blocks and only if nothing complete was
  there before, otherwise (corrected block, or a change of text that is already shown) they must
  be received twice in a row; the RadioText A/B flag from a corrected block counts only when
  repeated. Everything is forgotten on a PI change, on a retune of more than 5 kHz and after 10 s
  without sync.
  Unit tests verify the polynomial and offset words against the published syndromes, the burst
  syndromes, an encoder built from the specification round-trips through the full FM chain
  (stereo separation above 20 dB, correct PS / RT / PI / PTY, also with noise, 8-bit
  quantisation, a VFO 5 kHz off the station and a sample clock off by 120 ppm); waveform tests
  cover weak signals (7 dB Eb/N0) with clock drift, carrier offset and phase noise, and pure noise
  must decode nothing.
- **Audio format** (`docs/protocol-audio.md`, still `AudioFrame` v1): `channels` may be 2 (Opus
  stereo packet, 96 kbit/s), flag bit 1 = stereo pilot locked. `sample_pos` stays in sample
  frames. Old consumers that assumed `channels = 1` must follow the field; the only consumer
  today is the web UI. The wire version is not bumped because the header layout is unchanged and
  mono streams are byte-compatible.
- **Per-VFO switch**: `stereo` (WFM only, default on) in the VFO object, create/update requests
  and profiles. Switching restarts the audio filters and the encoder, not the pilot or RDS.
- **RDS delivery**: JSON on core NATS `sh.v1.dev.<dev>.rds.<vfo>` (complete state, on change and
  every second, empty state to clear), forwarded by the API as a text frame on the audio WebSocket
  of subscribed VFOs. A separate subject, not `audio.>`, so consumers of the audio subjects
  (plugin host, recorder) never see JSON. `nats.conf` grants sdrd `sh.v1.dev.*.rds.>`; the API
  already reads `sh.v1.>`.
- **Browser**: Opus decoder per VFO follows `channels`; the mixer keeps a left and a right jitter
  buffer per VFO that always receive the same pushes (mono feeds both), so the two channels share
  timing and drift control. The dock shows a Stereo switch, an "ST" lamp and an RDS row
  (service name, programme type name, TP/TA, PI, RadioText); PTY names switch to RBDS with 75 us.

## Consequences

- WFM costs more CPU (PLL at the channel rate, second audio filter, RDS chain): measured in a
  release build at 2.4 MS/s, one WFM VFO needs 5.5 % of a core without and 6.1 % with stereo
  (3.2 % before). Stereo off skips the L-R filter, RDS always runs for WFM.
- RDS needs a channel bandwidth of about 130 kHz or more; narrower WFM VFOs lose RDS and part of
  the stereo difference signal.
- The P2 plugin list loses redsea. Clock time, alternative frequencies, EON and RadioText+ are not
  decoded yet; the group decoder is the place to add them.
- Stereo streams are about 50 % larger on the bus than mono WFM.
- The synthetic broadcast generator (`superhet-dsp` feature `testsig`, `scripts/gen-wfm-iq.py`)
  doubles as a test source for file-device runs.

## Real-hardware findings (alpha.7)

- A real recording (RTL-SDR v5, 2.4 MS/s, tuner gain 30 dB, 99.19 and 100.79 MHz) shows the pilot
  lock and an RDS bump in the 57 kHz band, but no decodable data: carrier-to-noise in the 180 kHz
  channel is about 13 dB, the demodulated noise at 57 kHz is about 0.045 of full deviation against
  an RDS amplitude of about 0.027, i.e. Eb/N0 of about -1 dB (bit error rate > 10 %). That is below
  what any RDS decoder can use; the synthetic decoder limit is a carrier-to-noise of about 12 to
  14 dB. RDS needs roughly 16 dB or more, so antenna and gain decide, not the DSP chain.
- With the tuner on automatic gain the same band reached about 0 dBFS total power (heavy clipping
  of the 8-bit ADC). `superhet-sdrd` now logs `ADC overload` when more than 1 % of the samples sit
  at the rails. The end-to-end tests `rds_e2e_tests` (programme-material multiplex, 2.048 and
  2.4 MS/s, station 500 to 800 kHz off centre, 8-bit quantisation, DC offset, IQ imbalance, a
  crowded clipping band) guard the full channel path; `real_recording_probe` (ignored, set
  `SUPERHET_RDS_IQ`) runs a real file through it.
