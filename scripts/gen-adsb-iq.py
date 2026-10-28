#!/usr/bin/env python3
"""Write synthetic ADS-B IQ (cu8, 2.4 MS/s) to stdout: a DF17 aircraft identification frame
(ICAO 4840D6, callsign KLM1023, the example from "The 1090 MHz Riddle") repeated every ~50 ms
over a little noise. Used by scripts/check-plugin-image.sh to prove that the readsb in the image
decodes what the ADS-B plugin feeds it, without a recording or an antenna.

Usage: scripts/gen-adsb-iq.py [seconds]   (default 2, about 9.6 MB)
"""

import random
import sys

FRAME = "8D4840D6202CC371C32CE0576098"
RATE = 12_000_000  # fine grid, decimated by 5 to 2.4 MS/s
DECIMATE = 5
PER_US = RATE // 1_000_000


def frame_bits(hexstr):
    value = int(hexstr, 16)
    return [(value >> (len(hexstr) * 4 - 1 - i)) & 1 for i in range(len(hexstr) * 4)]


def burst(hexstr):
    """8 us preamble (0.5 us pulses at 0, 1, 3.5, 4.5 us) and PPM data, 1 us per bit."""
    fine = [0] * ((8 + 112) * PER_US)
    for start in (0.0, 1.0, 3.5, 4.5):
        first = int(start * PER_US)
        fine[first : first + PER_US // 2] = [1] * (PER_US // 2)
    for k, bit in enumerate(frame_bits(hexstr)):
        first = (8 + k) * PER_US + (0 if bit else PER_US // 2)
        fine[first : first + PER_US // 2] = [1] * (PER_US // 2)
    return fine


def main():
    seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 2.0
    total = int(RATE * seconds)
    fine = [0] * total
    rng = random.Random(1)
    pulse = burst(FRAME)
    pos = RATE // 10
    while pos + len(pulse) < total:
        fine[pos : pos + len(pulse)] = pulse
        pos += RATE // 20 + rng.randrange(0, 5000)
    out = bytearray()
    for i in range(0, total, DECIMATE):
        i_sample = 128 + (90 if fine[i] else 0) + rng.randrange(-3, 4)
        q_sample = 128 + rng.randrange(-3, 4)
        out += bytes((min(255, i_sample), min(255, q_sample)))
    sys.stdout.buffer.write(out)


main()
