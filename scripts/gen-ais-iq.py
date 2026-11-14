#!/usr/bin/env python3
"""Write synthetic AIS IQ (cu8, 192 kS/s, channel A 25 kHz below the centre) to stdout: the class A
position report of gpsd's documentation (MMSI 477553000, `!AIVDM,1,1,,B,177KQJ5000G?tO`K>RA1wUbN0TKH,0*5C`)
as a GMSK (BT 0.4, 9600 baud) HDLC frame, repeated over a little noise. Used by the Dockerfile and
scripts/check-plugin-image.sh to prove that the AIS-catcher in the image decodes what the AIS plugin
feeds it (`AIS-catcher -r cu8 . -s 192000 -o 1`), without a recording or an antenna. Pure Python,
no dependencies.

Usage: scripts/gen-ais-iq.py [seconds]   (default 3, about 1.1 MB)
"""

import math
import random
import sys

PAYLOAD = "177KQJ5000G?tO`K>RA1wUbN0TKH"
RATE = 192_000
BAUD = 9_600
SPB = RATE // BAUD  # samples per bit
DEVIATION = 2_400.0  # modulation index 0.5
OFFSET_HZ = -25_000.0  # channel A (161.975 MHz) against a 162.000 MHz centre
BT = 0.4


def payload_bits(text):
    bits = []
    for ch in text:
        v = ord(ch) - 48
        if v > 40:
            v -= 8
        bits += [(v >> s) & 1 for s in range(5, -1, -1)]
    return bits


def fcs(bits):
    """HDLC frame check sequence of the bits in transmission order, as bits in transmission order."""
    crc = 0xFFFF
    for b in bits:
        feedback = (crc & 1) ^ b
        crc >>= 1
        if feedback:
            crc ^= 0x8408
    crc ^= 0xFFFF
    return [(crc >> i) & 1 for i in range(16)]


def stuff(bits):
    out, ones = [], 0
    for b in bits:
        out.append(b)
        ones = ones + 1 if b else 0
        if ones == 5:
            out.append(0)
            ones = 0
    return out


def frame():
    # Over the air every byte goes out least significant bit first, while the sentence's payload
    # bit string is in field order: reverse the bits of each byte.
    bits = payload_bits(PAYLOAD)
    body = [b for i in range(0, len(bits), 8) for b in reversed(bits[i : i + 8])]
    flag = [0, 1, 1, 1, 1, 1, 1, 0]
    training = [0, 1] * 12
    bits = training + flag + stuff(body + fcs(body)) + flag + [0] * 8
    # NRZI: a 0 is a change of level, a 1 keeps it.
    level, symbols = 1, []
    for b in bits:
        if b == 0:
            level = -level
        symbols.append(level)
    return symbols


def gaussian_taps():
    sigma = math.sqrt(math.log(2)) / (2 * math.pi * BT) * SPB  # in samples
    half = 2 * SPB
    taps = [math.exp(-0.5 * ((n) / sigma) ** 2) for n in range(-half, half + 1)]
    total = sum(taps)
    return [t / total for t in taps]


def modulate(symbols):
    """Complex baseband samples (floats) of one burst."""
    nrz = [s for s in symbols for _ in range(SPB)]
    taps = gaussian_taps()
    half = len(taps) // 2
    padded = [nrz[0]] * half + nrz + [nrz[-1]] * half
    phase, out = 0.0, []
    for n in range(len(nrz)):
        shaped = sum(t * padded[n + k] for k, t in enumerate(taps))
        freq = OFFSET_HZ + DEVIATION * shaped
        phase += 2 * math.pi * freq / RATE
        out.append((math.cos(phase), math.sin(phase)))
    return out


def main():
    seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 3.0
    total = int(RATE * seconds)
    rng = random.Random(1)
    burst = modulate(frame())
    out = bytearray()
    gap = RATE // 4
    pos = RATE // 10
    for n in range(total):
        i_val = q_val = 0.0
        k = n - pos
        if 0 <= k < len(burst):
            i_val, q_val = burst[k]
            if k == len(burst) - 1:
                pos += len(burst) + gap
        i_s = 127.5 + 80 * i_val + rng.gauss(0, 2.5)
        q_s = 127.5 + 80 * q_val + rng.gauss(0, 2.5)
        out += bytes((max(0, min(255, round(i_s))), max(0, min(255, round(q_s)))))
    sys.stdout.buffer.write(out)


main()
