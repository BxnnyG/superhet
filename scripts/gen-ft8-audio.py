#!/usr/bin/env python3
"""Synthetic FT8 / WSPR audio for checking the FT8 plugin (`scripts/check-plugin-image.sh`).

Writes raw 12 kHz mono s16le to stdout: one 15 s FT8 slot carrying "CQ DL1ABC JO62" at 1500 Hz
(`ft8`, 31 s including a second silent slot) or one 2 minute WSPR slot carrying "K1ABC FN42 37"
at 1500 Hz (`wspr`, 121 s). The signal is plain continuous-phase FSK over Gaussian noise at about
-5 dB SNR in 2500 Hz, which the WSJT-X decoders accept. The tones are the channel symbols of the
messages (ft8_lib `gen_ft8`, WSJT-X `wsprsim -c`). Standard library only.

Usage: scripts/gen-ft8-audio.py ft8|wspr > audio.s16le
"""

import math
import random
import struct
import sys

RATE = 12000
FT8_TONES = "3140652000000001045311305210577534623140652674424751471436372416647501333140652"
WSPR_SYMBOLS = (
    "330020001020131222100323133220200032012322002232110233210221321222033030301210212032132003"
    "323032203020201023021112330231212221332000010320132222202332323320031222"
)


def fsk(tones, base, spacing, symbol, start, total, amp, sigma=1000.0):
    rng = random.Random(7)
    out = [rng.gauss(0.0, sigma) for _ in range(total)]
    phase = 0.0
    at = start
    for tone in tones:
        step = 2.0 * math.pi * (base + tone * spacing) / RATE
        for _ in range(symbol):
            out[at] += amp * math.sin(phase)
            phase += step
            at += 1
        phase %= 2.0 * math.pi
    return out


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    if mode == "ft8":
        slot = fsk([int(c) for c in FT8_TONES], 1500.0, 6.25, 1920, RATE // 2, 15 * RATE, 500.0)
        samples = slot + [0.0] * (16 * RATE)
    elif mode == "wspr":
        tones = [int(c) - 1.5 for c in WSPR_SYMBOLS]
        slot = fsk(tones, 1500.0, 12000.0 / 8192.0, 8192, RATE, 120 * RATE, 500.0)
        samples = slot + [0.0] * RATE
    else:
        sys.exit("usage: gen-ft8-audio.py ft8|wspr")
    sys.stdout.buffer.write(
        b"".join(struct.pack("<h", max(-32000, min(32000, round(v)))) for v in samples)
    )


main()
