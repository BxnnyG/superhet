# ADR-0009: Zoom FFT (real resolution when zoomed)

**Status:** accepted (2026-10-22)

## Context

The spectrum is one FFT of the whole sampled band (4096 points, published as 2048 bins, i.e.
about 1.17 kHz per bin at 2.4 MS/s). Zooming only cropped those bins in api and stretched them in
the browser: a 10 kHz view showed eight source bins smeared over the screen, narrow signals could
not be told apart, and the crop snapped outwards to the 1.17 kHz bin grid, so the span on screen
differed noticeably from the span the client had asked for.

## Options

- **Bigger full-band FFT** (e.g. 64k points): cost grows, the frame rate falls (the window takes
  27 ms to fill at 2.4 MS/s, averaging gets worse) and the bin count over the bus would explode.
  Resolution gain is capped by what one FFT can carry.
- **Zoom in the browser** (resample or interpolate): cosmetic, no new information.
- **Zoom FFT in sdrd**: shift the slice to DC, low-pass and decimate, then FFT with the same
  number of bins. Real resolution at a small, bounded cost (the decimator is cheap because the
  band shrinks by half at every stage). One extra frame stream.

## Decision

**sdrd computes a second spectrum, the zoom spectrum, for the slice the user is looking at.**
The full-band spectrum stays as it is (it feeds the unzoomed view, the auto-level and the
overview); the zoom spectrum only exists while someone zooms.

- **DSP** (`superhet-dsp::zoom`): NCO shifts the slice centre to DC, a cascade of
  short FIR stages (Kaiser, 90 dB stopband, each decimating by up to 8) decimates by
  `2^shift`, a `SpectrumProcessor` (same Blackman-Harris/50 % overlap/power averaging/dBFS scale
  as the full band) transforms the result. Only the central 80 % of the decimated band is
  returned, the rest is filter roll-off. Tone levels read the same as in the full-band
  spectrum; the noise floor per bin falls with the narrower bins, as it physically does.
- **Choice of `shift`** (`shift_for_span`): the largest power of two such that the usable band
  (80 % of the decimated rate) still holds the requested span with 15 % to spare. That spare is
  the pan slack: moving the view inside the slice costs nothing, sdrd only re-plans when the
  view leaves the slice or needs another `shift`. Spans wider than about 35 % of the sample rate
  need no zoom (`shift` would be 0) and are served by the full-band spectrum.
- **Limits.** The decimated rate stays at 12.5 kHz or above, so the narrowest slice at
  2.4 MS/s is about 15 kHz of usable band (`shift` 7; the UI's minimum span is 10 kHz). The FFT
  window never exceeds 125 ms (`zoom_fft_size`: largest power of two in 512 to the full-band FFT
  length), so a frame arrives at least about every 60 ms, a re-planned slice needs one window
  (up to 125 ms) before its first frame, and the bin width does not fall below roughly 9 Hz
  (time-bandwidth limit of the window). Typical results at 2.4 MS/s (displayed bins, full band:
  1172 Hz): 500 kHz view 586 Hz, 100 kHz view 73 Hz (16x finer), 25 kHz view 18 Hz, 10 kHz view
  9 Hz (128x finer).
- **Cost.** Measured (release build, 2.4 MS/s): about 5 % of one core for deep zooms, up to 10 % for
  the shallowest (the FFT then runs at 1.2 MS/s), on the existing `sdrd-dsp` thread, nothing
  while no slice is leased. One slice per device.
- **Bus.**
  - `sh.v1.dev.<dev>.ctl.spectrum.zoom` (api to sdrd, core publish without reply):
    `{"v":1,"center_hz":<u64>,"span_hz":<u32>}` leases a slice, `{"v":1,"off":true}` releases it.
    A lease lasts 10 s; api repeats an active one every 3 s, so a crashed api never leaves it
    running. Errors are answered only if the publisher gave a reply subject.
  - `sh.v1.dev.<dev>.spectrum.zoom` (sdrd to api): ordinary spectrum frames (wire format v1).
    The header describes exactly what the bins cover: `center_hz` is the slice centre and
    `sample_rate` the usable width, so the existing absolute-span handling in the waterfall places
    every row correctly.
  - NATS permissions: sdrd may publish `sh.v1.dev.*.spectrum.zoom` (added to
    `deploy/nats/nats.conf`, which the release compose embeds verbatim); api already has
    `sh.v1.dev.*.ctl.>` and `sh.v1.>`.
- **Browser protocol unchanged.** The web client already sends `view` (span and bins, debounced
  at 40 ms / 150 ms on resize). api turns narrow views (at most 40 % of the full band) into a
  lease after the view stayed unchanged for 100 ms, and serves the client from zoom frames
  while they arrive and cover the view (within one bin); otherwise from the full-band frames as
  before. Zoom frames are cropped and decimated by the existing `crop_decimate`. A client never
  gets both streams for the same view, and while a new slice spins up (about one window) it
  sees the stretched full-band rows, which is the old behaviour.
- **Multiple tabs: last writer wins.** There is one slice per device. Every WebSocket session
  reports its view; the session whose view changed last owns the slice (an unchanged re-sent view
  does not take it over). Other sessions whose view the slice does not cover keep receiving the
  full-band spectrum. A leaving session hands the slice to the most recent remaining one, or
  releases it. A per-session slice would multiply the DSP cost for a single-user product.
- **Slice position.** The slice is kept inside the sampled band (no wrap-around of the far
  edge); a view at the very band edge is therefore still covered. The slice is dropped on every
  retune (epoch change) and re-planned from the leased request.

### Related client fix (bug: marker offset after zooming out)

The zoom and pan arithmetic in the browser (`ViewController`) used the *requested* span to
translate the pointer position into a frequency, while the screen draws the span of the *latest
frame*. The two differ because frames lag the request and because the old crop snapped the edges
outwards to 1.17 kHz bins. Zooming around the pointer therefore anchored on the wrong frequency
(up to 12 % of a 10 kHz view, and by arbitrary amounts when the pointer moved before the next
frame), and a marker placed while zoomed in ended up displaced after zooming out. The controller
now anchors on the displayed span (`setShown`) and scales the requested span; panning converts
pixels with the displayed width.

## Consequences

- Zooming gives real resolution, the displayed span matches the request to a few Hz, and the
  zoom fix for the pointer anchor becomes largely moot because the two spans nearly coincide
  once the zoom stream is running.
- Rows from different zoom levels have different noise floors and widths. The waterfall keeps
  the absolute span of each row, so older, wider rows and newer, narrower rows share the screen
  correctly; zooming out shows narrow rows as a strip until the history scrolls by.
- Level jumps at the slice boundary: the zoom spectrum's noise floor is lower by
  `10 log10(bin ratio)`. The auto-level follows the frames it gets.
- Cost: a second publish stream while zoomed (about 800 bytes per frame at 25 fps) and one more
  NATS permission.
- Not covered: zooming a view wider than 35 % of the band (stretched full-band bins, as
  before), views narrower than the limits above, and a second tab that looks elsewhere while the
  first one zooms.
- The display convention of the full-band spectrum (bin `i` drawn at `(i + 0.5)` bin widths from
  the lower edge while the FFT puts DC exactly on bin `N/2`) is half a bin off, up to about
  290 Hz on the full band. It is not visible at full zoom and below one pixel of the 10 Hz bins
  of a deep zoom. Left as is.
