# Superhet web UI

React + Vite + TypeScript (strict) + Tailwind, pnpm. `pnpm -C web dev` proxies `/api` (including
WebSockets) to `superhet-api` on port 8073.

## Authentication

`AuthGate` (`src/auth/`) asks `GET /api/v1/auth/status` on load and renders first-run setup (username,
password, setup token from `docker compose logs superhet-api`), login, or the app. The settings menu
shows the signed-in user with "Change password" and "Sign out". Any 401 from the data API, or a
WebSocket that closes without ever opening while the server reports a signed-out session, sends the
user back to login; the spectrum and audio sockets stop reconnecting until the next login.

## Spectrum gestures

| Gesture                                                                         | Action                                                                                                                                                                                                                                         |
| ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Wheel                                                                           | Zoom around the pointer, also over a VFO passband                                                                                                                                                                                              |
| Shift+wheel over the selected passband                                          | Change its bandwidth (wheel up = wider, snapped; capped by the mode and the device rate)                                                                                                                                                       |
| Shift+wheel elsewhere, Alt+wheel                                                | Tune the selected VFO by the current step (wheel up = higher)                                                                                                                                                                                  |
| Ctrl+wheel                                                                      | Always zoom                                                                                                                                                                                                                                    |
| Drag on empty spectrum or waterfall                                             | Pan a zoomed view (a press that moves 4 px or less is a click)                                                                                                                                                                                 |
| Shift-drag / middle-drag                                                        | Pan                                                                                                                                                                                                                                            |
| Drag on the frequency axis                                                      | Pan (never grabs a VFO)                                                                                                                                                                                                                        |
| Double-click                                                                    | Reset zoom                                                                                                                                                                                                                                     |
| Click on empty spectrum, no VFO selected                                        | Tune the device centre                                                                                                                                                                                                                         |
| Click on empty spectrum or the axis, VFO selected                               | Move the selected VFO there (snapped to the step)                                                                                                                                                                                              |
| Click on a VFO (its shaded band or centre line)                                 | Select it                                                                                                                                                                                                                                      |
| Drag a VFO                                                                      | Retune it (snapped to the step)                                                                                                                                                                                                                |
| Drag an edge of the selected VFO's passband (grips; larger grab zone for touch) | Change its bandwidth live to any value the mode and device rate allow (100 Hz grid when narrow, coarser when wide; symmetric for WFM/NFM/AM/CW, the outer edge only for USB/LSB). The pencil next to the bandwidth select types an exact value |
| Alt-click or Ctrl-click on empty spectrum                                       | Create a VFO at the pointer (default mode by band)                                                                                                                                                                                             |
| Drag the splitter between spectrum and waterfall                                | Resize the two panes (arrow keys, Home resets)                                                                                                                                                                                                 |
| Esc (or "Deselect")                                                             | Deselect, so a plain click tunes the centre again                                                                                                                                                                                              |

USB and LSB passbands are one-sided: the shaded band spans 300 Hz to 300 Hz + bandwidth on the upper
(USB) or lower (LSB) side of the VFO line (default 2.4 kHz wide), the line itself marks the carrier
frequency, and only the outer edge can be dragged. CW is centred on the line (default 500 Hz): a
carrier exactly on the line is heard as a 700 Hz tone (the pitch can be set through the API,
`cw_pitch_hz`, `docs/api-devices.md`).

Zooming asks the daemon for a zoom FFT of the visible slice (ADR-0009), so a narrow view shows finer
bins instead of stretched pixels (about 9 Hz per bin at the 10 kHz minimum span, 2.4 MS/s). The
zoom and pan maths anchor on the span of the newest frame (`ViewController.setShown`), so the
pointer stays on its frequency while zooming and a marker does not drift.

Settings menu, "Mouse wheel": `Zoom` (default) as above; `Tune` makes the plain wheel tune the selected VFO
everywhere on the spectrum (Ctrl+wheel still zooms). Stored in `localStorage` (`superhet.wheelMode.v2`;
the unversioned value of earlier releases is ignored, so everyone starts on `Zoom`).

## Keyboard shortcuts

Active on the page (not while typing in a field, on a slider/splitter/digit readout that has focus,
or with a dialog open); `?` or Settings, "Keyboard shortcuts" lists them.

| Key            | Action                                                                      |
| -------------- | --------------------------------------------------------------------------- |
| `←` / `→`      | Step the selected VFO down / up (`Shift`: ten steps)                        |
| `↑` / `↓`      | Select the next / previous VFO                                              |
| `1` … `6`      | Mode WFM / NFM / AM / USB / LSB / CW (bandwidth resets to the mode default) |
| `M`            | Listen to the selected VFO on / off                                         |
| `S`            | Squelch Auto on / off                                                       |
| `B`            | Bookmark the selected VFO                                                   |
| `+` / `-`, `0` | Zoom in / out, reset zoom                                                   |
| `N`            | New VFO at the display centre                                               |
| `Delete`       | Remove the selected VFO (the notice offers Undo)                            |
| `?`            | Show the shortcut list                                                      |

## Layout and controls

Top bar, collapsible sidebar (Source, Profiles, Radio, Display, Audio), spectrum above waterfall, status bar
(`docs/ui-design.md`). Below 900 px the sidebar is a slide-over drawer (menu button, Esc or backdrop
closes it).

The big frequency readout (MHz.kHz.Hz) edits the selected VFO, or the device centre when none is
selected. Every digit tunes on its own: mouse wheel or ArrowUp/ArrowDown change it (with carry,
clamped), ArrowLeft/Right/Home/End move between digits, typing a digit overwrites it. Enter, F2,
double-click or the pencil button switch to typed entry (`104.2`, `433.92M`, `7100k`, Esc cancels).

Radio section: `+ VFO` (at the display centre), the VFO list (mode badge, delete), the tuning step
(default per mode: WFM 10 kHz, NFM 12.5 kHz, AM 5 kHz, USB/LSB 100 Hz, CW 50 Hz; selectable 10 Hz to 100 kHz), and for the
selected VFO: step nudge, mode (WFM/NFM/AM/USB/LSB/CW), bandwidth, squelch (off, -120..0 dBFS, green tick =
live channel power), volume, mute, S-meter, deselect, delete. Changing the mode resets the bandwidth
to the mode default. Volume and mute are client-side only. Source: centre frequency (typed values
accept units), sample rate, gain (auto or per step), PPM. Display: visible span with zoom
out/in/full span, automatic levels, floor, ceiling. The theme (system, light, dark) is chosen in the
settings menu and stored in the browser.

## Live state, tuning lock and profiles

Device and VFO state arrive over the state WebSocket (`/api/v1/ws/state`, `src/state/`,
`docs/protocol-ws.md`): a full document first, then one per change, so changes made in another tab
show up immediately. The socket reconnects with backoff and stops while logged out, like the
spectrum and audio sockets. Nothing polls; the controllers make one REST read at start as fallback.

The padlock next to the frequency readout takes or drops the user tuning lock
(`docs/api-devices.md`). While any lock is held the centre readout, centre field and sample rate are
read-only with the reason shown, and a refused change (client-side or `409 locked`) raises a toast
naming the holders. VFOs stay editable: dragging one out of the usable window (85 % of the sample
rate, outside shaded on the spectrum) makes the daemon retune; with the lock held the VFO is paused
instead (badge in the list, dashed on the spectrum, toast).

Profiles section: save the current receiver ("Save current as...", asks before overwriting a name),
load (asks when the lock would block it, with "Unlock and load" for your own lock), rename, delete.
Notices use the `Toaster` from `src/ui`.

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
