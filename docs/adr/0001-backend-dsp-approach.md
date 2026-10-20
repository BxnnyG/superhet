# ADR-0001: Backend DSP approach

**Status:** accepted (2026-10-20)

## Context

The backend (Rust, tokio + axum) must read 2.4 MS/s from an RTL-SDR, compute the spectrum, run a dynamic set of channels (VFOs and decoder inputs are added/removed/retuned at runtime, many times per second while dragging), and feed plugin processes. More device types (SoapySDR, rtl_tcp, Airspy, HackRF, SDRplay) must be possible later without restructuring.

## Options

1. **FutureSDR** (Apache-2.0, active): flowgraph runtime with schedulers, blocks, WASM support; hardware via `seify` (rtl-sdr-rs, SoapySDR, HackRF).
   - Pro: ready-made blocks, scheduler, Soapy integration, same language.
   - Con: the framework owns scheduling and topology; our topology is simple (one source fan-out to N channels) but highly *dynamic* (frequent add/remove/retune), which fits a static flowgraph model poorly; pre-1.0 API churn; harder to reason about latency and back-pressure per consumer; extra abstraction to debug.
2. **Own pipeline**: dedicated threads, bounded ring buffer (IQ bus), `rustfft`, `num-complex`, own FIR/NCO/demod code; device trait with a librtlsdr FFI backend first.
   - Pro: full control of dynamic channels, drop accounting and latency; small dependency surface; easy to test with a file device.
   - Con: we write and maintain filters/demodulators ourselves (well-understood DSP, moderate effort).

Device access sub-decision: librtlsdr (osmocom, C, GPL-2.0, de-facto standard, async bulk transfers, mature R820T2 support) vs. `rtl-sdr-rs` (pure Rust, MPL-2.0, less field-proven) vs. SoapySDR for everything (extra layer, C++ dependency even for the default case).

## Decision

Own pipeline (option 2). Device layer is a Superhet-owned `Device` trait; the first backend wraps **librtlsdr via FFI** (dynamically linked system library). A `file` backend (SigMF/raw replay) is implemented in P1 for tests and CI. SoapySDR (via the `soapysdr` crate) and rtl_tcp follow as additional backends; `seify` is re-evaluated as an adapter then. Individual FutureSDR-independent crates (e.g. filter design helpers) may be used where they fit.

## Consequences

- DSP correctness needs its own test suite (golden files from the file backend, SNR/frequency-response tests).
- librtlsdr is a runtime dependency of the container image and the host install; linking a GPL-2.0 C library dynamically from AGPL code is the same arrangement used by many SDR projects; documented in `docs/licenses.md`.
- Switching individual stages to FutureSDR later stays possible because stages communicate through plain sample-block queues.
