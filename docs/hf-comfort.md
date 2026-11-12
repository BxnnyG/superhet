# HF comfort

Noise blanker, notch filters, noise reduction and band-aware defaults for listening below
30 MHz. Everything is per VFO, optional and off by default (REST: `docs/api-devices.md`, "HF
comfort"). Code: `crates/superhet-dsp/src/comfort.rs`, wiring in `channel.rs`, the web controls
in `web/src/vfo/ComfortPanel.tsx`.

## Signal path

1. **Noise blanker**, on the complex baseband at the channel rate (24 to 40 kHz), after the
   decimation stages and *before* the channel filter, where an impulse is still a few samples
   long. A sample whose power exceeds a multiple of the slowly tracked mean power is an impulse;
   it and a guard window of 0.3 ms on both sides (a small delay line gives the look-behind) are
   replaced, by a line between the neighbours in AM and by zeros in SSB / CW / NFM (the baseband
   rotates there, so interpolating would not help). Strength 0..1 moves the threshold from 6x
   down to 2.5x the mean amplitude. Strong SSB speech peaks can trip a high strength; lower it.
2. **Manual notch**, a second-order IIR notch at an audio frequency (50 Hz to 12 kHz) with an
   adjustable -3 dB width (10 Hz to 1 kHz). Moving it keeps the filter state, so dragging is
   click-free. The width is its strength control.
3. **Auto notch**, an NLMS adaptive predictor (32 taps, 0.6 ms decorrelation delay). A steady
   carrier or whistle is predictable from the delayed past, speech is not, so the prediction
   error is the signal without the carrier. The strength sets the step size (0.002 to 0.05): low
   is gentle on speech, high follows a drifting carrier faster. It needs no frequency.
4. **Noise reduction**, spectral subtraction: 50 % overlapping sqrt-Hann frames of about 8 ms,
   per-bin minimum statistics (6 x 0.25 s sub-windows) as the noise estimate, median-filtered over
   frequency so narrow steady lines (a wanted tone, voice harmonics) are not mistaken for noise,
   over-subtraction 1..2.5 and a floor of 6..24 dB with the strength, gain smoothed over time and
   frequency. Chosen over an LMS line enhancer because it also cleans broadband noise on speech.
   Latency: one frame, about 8 ms (`Channel::comfort_latency_s`), far below the AGC and squelch
   time constants and not audible on voice.

Notch and noise reduction run on the demodulated signal at the channel rate; for SSB / CW
*before* the AGC (`SsbDemod::process_with`), so a whistle cannot pump the gain. All of it is
inactive in WFM (the demodulator output there is a multiplex, not audio) and resumes when the mode
changes back. Parameter changes never reset a running block.

## CPU cost

Measured with `cargo test --release -p superhet-dsp comfort_cost -- --ignored --nocapture`, one
VFO, 2.4 MS/s input, one second of signal, share of one core on the development machine
(repeatable to about 0.3 points):

| Block | USB | AM |
| --- | --- | --- |
| channel without comfort blocks | 3.6 % | 2.8 % |
| + noise blanker | 3.4 % | 3.0 % |
| + manual notch | 3.1 % | 2.7 % |
| + auto notch | 3.4 % | 3.0 % |
| + noise reduction | 3.6 % | 3.1 % |
| all four | 3.7 % | 3.4 % |

The blocks run at the channel rate, not at the input rate, so each costs 0.3 points of a core or
less on top of the channel; the existing channel cost dominates (decimation).

## Defaults by frequency

`web/src/vfo/bandDefaults.ts` (mode rules mirrored in `default_mode` of
`crates/superhet-sdrd/src/controller.rs`, used for the daemon's default VFO and scanner
entries):

| Range | Mode | Bandwidth | Step |
| --- | --- | --- | --- |
| long wave broadcast 148.5 to 283.5 kHz | AM | 6 kHz | 9 kHz |
| medium wave 520 kHz to 1.71 MHz | AM | 6 kHz | 9 or 10 kHz (setting) |
| shortwave broadcast bands, time signals (2.5, 10, 15, 20, 25 MHz +-5 kHz) | AM | 6 kHz | 5 kHz |
| CB 26.965 to 27.405 MHz | AM | 6 kHz | 10 kHz |
| amateur CW segments (1.8, 3.5, 7, 10.1, 14, 18, 21, 24.9, 28 MHz) | CW | 500 Hz | 100 Hz |
| everything else below 10 MHz | LSB | 2.4 kHz | 100 Hz in amateur bands, else 1 kHz |
| everything else 10 to 30 MHz | USB | 2.4 kHz | 100 Hz in amateur bands, else 1 kHz |

Below 100 kHz and from 30 MHz up the table says nothing (the existing VHF/UHF rules apply). The
medium-wave raster (9 kHz in ITU regions 1 and 3, 10 kHz in the Americas) is a browser setting in
the settings menu; without one the browser locale decides.

The web UI applies the table in two places: a new VFO without a bookmark / plan mode, and a
retune. A retune changes mode and bandwidth only for a VFO whose mode the user never set (any
mode change from the UI marks it as chosen by hand for the rest of the session), and only when
the old and new frequency have different defaults, so tuning around inside a band never fights
a bandwidth the user picked. The "Auto" tuning step follows the band while the VFO is in the
band's mode and falls back to the mode default otherwise.

## Controls and keys

Receiver tab, block "Noise and notch" (phone layout: the same block in the receiver sheet): four
toggles, a strength slider for each block that is on, and for the manual notch a strip showing
the audio passband with a draggable marker (pointer drag, or focus it and use the arrow keys, Shift
for 100 Hz) plus a width slider. Keys: `V` noise blanker, `T` notch, `A` auto notch, `R` noise
reduction (also in the command palette). Not shown for WFM.

## Limits

- The notch marker lives in the receiver panel, not on the spectrum display.
- Noise reduction and the auto notch work on mono audio of one VFO; they are not applied to the
  audio of plugin taps (the IQ tap carries the channel-filtered baseband *after* the blanker).
- The notch frequency is an audio frequency; its RF equivalent depends on the mode (USB: VFO
  frequency + audio, LSB: minus).
