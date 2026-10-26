# Performance and metrics

How to measure what Superhet costs, what the numbers are today, and what to turn when a
machine cannot keep up. Roadmap criterion: core + waterfall + 4 NFM VFOs <= 25 % of one core on
the dev PC.

## Where the time goes

Per device, `superhet-sdrd` runs these threads (names as shown by `top -H`):

| Thread | Work | Scales with |
| --- | --- | --- |
| device thread (`file-device` for the file backend) | USB read, 8-bit to float conversion | sample rate |
| `sdrd-dsp` | spectrum FFT (+ zoom FFT while a client zooms), ADC clip check | sample rate, FFT size, fps |
| `sdrd-vfo` | per VFO: mixer, decimating FIR, demodulator, resampler to 48 kHz, Opus; RDS/CTCSS/DTMF | sample rate (first filter stage), number of VFOs |
| `tokio-rt-worker` | NATS publish, control requests | frames per second |

Everything else (`superhet-api`, `superhet-worker`, PostgreSQL, NATS) is idle compared to that.
The browser does its own work: the WebGL waterfall and the Opus decode are cheap, but a software
WebGL fallback (no GPU) can be the slowest part of the whole chain.

## Measuring CPU

Without hardware, with the file backend (synthetic noise plus four FM carriers, paced to real
time):

    docker run -d --rm --name sh-perf-nats -p 127.0.0.1:14222:4222 nats:2.15.1-alpine -js
    cargo build --release -p superhet-sdrd
    scripts/perf-sdrd.py --vfos 4 --mode nfm --rate 2400000 --seconds 20

The script starts `superhet-sdrd`, creates the VFOs over NATS, waits for the settle time and
prints the CPU time per thread in percent of one core, plus the drop counters from `/metrics`.
Options: `--mode` (wfm, nfm, am, usb, lsb, cw), `--rate`, `--fft-size`, `--vfos`. It needs numpy.
On a real stick use `top -H -p $(pgrep superhet-sdrd)` and compare.

Debug builds are far slower than release; never measure those. (The dev profile
optimises `superhet-dsp` and `rustfft`, but not everything.)

## Current numbers

Release build, Intel Core i7-7700 (4 cores / 8 threads, 3.6 GHz), file backend, FFT 4096,
25 fps, 15 s window, percent of one core, no drops in any run (`/metrics` shows 0):

| Setup | Total | `sdrd-vfo` | `sdrd-dsp` |
| --- | --- | --- | --- |
| 2.4 MS/s, no VFO (waterfall only) | 2.7 % | 0.1 % | 1.9 % |
| 2.4 MS/s, 1 WFM VFO (stereo + RDS) | 9.9 % | 6.8 % | 2.0 % |
| 2.4 MS/s, 4 NFM VFOs | 14.6 % | 11.1 % | 1.9 % |
| 2.4 MS/s, 4 USB VFOs | 15.2 % | 11.5 % | 2.0 % |
| 2.048 MS/s, 4 NFM VFOs | 13.5 % | 10.3 % | 1.7 % |
| 3.2 MS/s, 4 NFM VFOs | 17.2 % | 12.9 % | 2.5 % |
| 2.4 MS/s, 4 NFM VFOs, FFT 16384 | 15.1 % | 10.8 % | 2.8 % |

Core + waterfall + 4 NFM VFOs is about 15 % of one core: criterion met with margin. The totals
include the file reader and the NATS runtime; a real RTL-SDR adds a similar USB thread. They do
not include the browser or other containers. Numbers move by +-20 % with the CPU governor and
temperature; compare runs on the same machine only.

## Latency

- Audio (capture to speaker): the status bar shows the estimate ("latency"), the jitter buffer
  fill and target ("buffer") and the underrun count. On a LAN with the default 100 ms jitter
  target it is about 170 ms: 100 ms buffer + 20 ms Opus frame + USB buffer, DSP block and network.
  A growing latency or underruns mean the network or the browser tab is the bottleneck, not the
  daemon.
- Waterfall: the status bar shows the frame rate. Spectrum frames carry the capture time in
  their header (`docs/protocol-ws.md`), so a client can compare it to its own clock.
- The daemon side adds one IQ block (a few ms at 2.4 MS/s) plus the FFT length; it is small
  against the audio buffer.

## Tuning

- Sample rate is the strongest lever: every stage scales with it. 2.048 MS/s costs about 8 %
  less than 2.4 MS/s; 3.2 MS/s about 18 % more, and RTL-SDRs lose USB data above roughly
  2.4 to 2.56 MS/s anyway. Pick the lowest rate that covers the band you look at.
- FFT size (`SUPERHET_FFT_SIZE`, default 4096) only matters for `sdrd-dsp`, which is cheap:
  16384 costs about 1 % more and gives finer resolution; it must not be smaller than the bin
  count (`SUPERHET_SPECTRUM_BINS`, default 2048) or the daemon refuses to start.
- Spectrum frame rate (`SUPERHET_SPECTRUM_FPS`, default 25): lowering it to 10 cuts NATS and
  WebSocket traffic and the browser's drawing work by 60 %.
- Each active VFO costs about 3 % (NFM, AM, SSB, CW) or 7 % (WFM with stereo and RDS) of a core
  at 2.4 MS/s. VFOs outside the usable window are paused and cost nothing.
- Turn off what you do not use: stereo (WFM) saves the pilot PLL and the second channel; tone
  detection (NFM/AM) saves the CTCSS/DTMF analysis.
- If the daemon logs `dropped IQ blocks` or the UI shows "Dropping", the machine cannot keep up:
  lower the sample rate first, then close CPU-heavy programs, then check the CPU governor
  (`performance`). USB 3 hubs and long cables are the usual cause of real USB problems, which
  show up as gaps in the waterfall without any drop counter moving (librtlsdr does not report
  USB overruns to the application).

## Metrics (`/metrics`)

Prometheus text format (0.0.4). Only CPU-independent counters and gauges are exposed; host CPU
and memory belong to node-exporter or cAdvisor.

**Where.** `superhet-sdrd` (port 8074) and `superhet-worker` (8076) serve `/metrics` next to
`/healthz`. Those ports are not published by the compose files, so they are reachable only from
inside the compose network (put Prometheus on it, or `docker compose exec`). They carry no
authentication, like `/healthz`. `superhet-api` does **not** serve `/metrics` on 8073 (the
published port is public to the LAN and the web UI is served without a session). Set
`SUPERHET_METRICS_LISTEN=0.0.0.0:8077` (any address) to start a separate listener that serves
only `/metrics`; it is off by default and meant for the compose network, never publish it.
A session-protected metrics route would not help Prometheus, which cannot log in.

Example scrape config for a Prometheus container on the compose network:

    scrape_configs:
      - job_name: superhet
        static_configs:
          - targets: ["superhet-sdrd:8074", "superhet-worker:8076", "superhet-api:8077"]

**Metrics.**

| Name | Type | Meaning |
| --- | --- | --- |
| `superhet_sdrd_iq_blocks_total` | counter | IQ blocks the DSP thread processed |
| `superhet_sdrd_iq_samples_total` | counter | IQ samples processed (rate check: should grow by the sample rate per second) |
| `superhet_sdrd_dropped_blocks_total{stage}` | counter | blocks dropped because a consumer lagged: `device` (device thread to DSP thread), `vfo` (DSP thread to VFO thread) |
| `superhet_sdrd_dropped_frames_total{stream}` | counter | output frames dropped by the publishers (bus down or lagging): `spectrum`, `zoom`, `audio`, `rds`, `tones` |
| `superhet_sdrd_clipped_percent{device}` | gauge | share of samples at the ADC rails in the last 5 s |
| `superhet_sdrd_adc_overload{device}` | gauge | 1 while more than 1 % clips |
| `superhet_sdrd_sample_rate_hertz{device}` | gauge | current sample rate |
| `superhet_sdrd_vfos{state}` | gauge | VFOs by state: `active` (demodulating), `paused` (outside the window) |
| `superhet_sdrd_rds_bler_percent{vfo}` | gauge | RDS block error rate over the last ~10 s, for WFM VFOs with RDS data |
| `superhet_api_ws_clients{endpoint}` | gauge | connected WebSocket clients: `spectrum`, `audio`, `state`, `events` |
| `superhet_api_spectrum_frames_missed_total` | counter | spectrum frames slow clients missed |
| `superhet_api_audio_frames_dropped_total` | counter | audio frames dropped for slow clients |
| `superhet_worker_decoder_events_inserted_total{kind}` | counter | decoder history events stored |
| `superhet_worker_decoder_event_insert_errors_total` | counter | decoder events that could not be stored |
| `superhet_worker_decoder_events_pruned_total` | counter | decoder events removed by the retention job |

Useful alerts: `rate(superhet_sdrd_dropped_blocks_total[5m]) > 0` (the receiver cannot keep up),
`superhet_sdrd_adc_overload == 1` for several minutes (lower the gain),
`rate(superhet_worker_decoder_event_insert_errors_total[5m]) > 0` (database problem).
Counters reset when the process restarts. The same dropped-block total is part of the device
state document (`dropped_blocks`) and shows up in the UI (Source settings, and a "Dropping" chip
on the centre pill for a minute after it grew).
