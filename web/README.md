# Superhet web UI

React + Vite + TypeScript (strict) + Tailwind, pnpm. `pnpm -C web dev` proxies `/api` (including
WebSockets) to `superhet-api` on port 8073.

## Spectrum gestures

| Gesture                                         | Action                                             |
| ----------------------------------------------- | -------------------------------------------------- |
| Wheel                                           | Zoom around the pointer                            |
| Shift-drag / middle-drag                        | Pan                                                |
| Double-click                                    | Reset zoom                                         |
| Click on empty spectrum, no VFO selected        | Tune the device centre                             |
| Click on empty spectrum, a VFO selected         | Move the selected VFO there (snapped to the step)  |
| Click on a VFO (its shaded band or centre line) | Select it                                          |
| Drag a VFO                                      | Retune it (snapped to the step)                    |
| Alt-click or Ctrl-click on empty spectrum       | Create a VFO at the pointer (default mode by band) |
| Esc (or "Deselect")                             | Deselect, so a plain click tunes the centre again  |

The VFO panel offers `+ VFO` (at the display centre), the list of VFOs, the tuning step (default per
mode: WFM 10 kHz, NFM 12.5 kHz, AM 5 kHz; selectable 100 Hz to 100 kHz), and for the selected VFO:
frequency (typed values are not snapped), mode, bandwidth, squelch (off, -120..0 dBFS), volume, mute,
S-meter, delete. Changing the mode resets the bandwidth to the mode default. Volume and mute are
client-side only.

## Audio

`Audio on` creates the `AudioContext` (browsers require a user gesture). Per VFO, the audio
WebSocket (`/api/v1/ws/audio`, `docs/protocol-audio.md`) delivers Opus frames that are decoded with
WebCodecs `AudioDecoder` and mixed in an AudioWorklet (`src/audio/`). Browsers without
`AudioDecoder` get a notice; a WASM Opus fallback is a later step. The S-meter works with audio off.

- Jitter buffer per VFO, target 100 ms (selectable 50 to 250 ms): playback starts once the target is
  reached, an underrun outputs silence and rebuffers, an overrun (> 3x target) drops to the target.
- Lost frames (gaps in `sample_pos`) are replaced by silence; time is never stretched.
- Clock drift (SDR vs. sound card, about +-200 ppm): PI controller on the low-passed fill level
  adjusts the read ratio by at most +-0.5 %, cubic interpolation. A hardware rate other than 48 kHz
  is resampled by the same mechanism.
- The debug line shows buffer fill, drift correction, under-/overruns and a latency estimate
  (capture to browser includes server/browser clock skew, plus buffer and output latency).
