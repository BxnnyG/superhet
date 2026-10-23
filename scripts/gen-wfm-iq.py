#!/usr/bin/env python3
"""Write a cu8 IQ file with a synthetic stereo FM broadcast station carrying RDS.

For SUPERHET_DEVICE=file:<path>?format=cu8&rate=...&freq=... to try WFM stereo and the RDS
display without an antenna: left channel 440 Hz, right channel 660 Hz, 19 kHz pilot, 57 kHz RDS
(PI D3C2, PTY 10, PS "SUPERHET", configurable RadioText). Needs numpy.

Usage: gen-wfm-iq.py OUT.cu8 [RATE=1024000] [SECONDS=8] [OFFSET_HZ=0] [PS=SUPERHET] [RT=...]
"""
import sys

try:
    import numpy as np
except ImportError:
    sys.exit("gen-wfm-iq.py needs numpy (pip install numpy)")

GENERATOR = 0x5B9
OFFSETS = {"A": 0x0FC, "B": 0x198, "C": 0x168, "D": 0x1B4}
PI = 0xD3C2
PTY = 10
BIT_RATE = 1187.5


def remainder(word):
    """Remainder of a 26-bit word divided by the RDS generator polynomial."""
    for i in range(25, 9, -1):
        if word >> i & 1:
            word ^= GENERATOR << (i - 10)
    return word & 0x3FF


def block(info, offset):
    m = info << 10
    return m | (remainder(m) ^ OFFSETS[offset])


def groups(ps, rt):
    ps = ps.ljust(8)[:8].encode("latin-1")
    out = []
    for seg in range(4):
        b = PTY << 5 | 0x0400 | 0x0008 | seg  # 0A, TP, music
        out.append([PI, b, 0x0000, ps[2 * seg] << 8 | ps[2 * seg + 1]])
    text = rt.encode("latin-1")[:63] + b"\r"
    text = text.ljust((len(text) + 3) // 4 * 4)
    for seg in range(len(text) // 4):
        c = text[4 * seg:4 * seg + 4]
        b = 0x2000 | PTY << 5 | 0x0400 | seg  # 2A, text A/B = 0
        out.append([PI, b, c[0] << 8 | c[1], c[2] << 8 | c[3]])
    return out


def bits(ps, rt):
    seq = []
    for g in groups(ps, rt):
        for info, name in zip(g, "ABCD"):
            w = block(info, name)
            seq.extend((w >> k) & 1 for k in range(25, -1, -1))
    # Differential encoding: y[k] = x[k] xor y[k-1].
    prev = 0
    enc = []
    for x in seq:
        prev ^= x
        enc.append(prev)
    return np.array(enc, dtype=np.int8)


def main():
    out = sys.argv[1]
    rate = int(sys.argv[2]) if len(sys.argv) > 2 else 1_024_000
    seconds = float(sys.argv[3]) if len(sys.argv) > 3 else 8.0
    offset = float(sys.argv[4]) if len(sys.argv) > 4 else 0.0
    ps = sys.argv[5] if len(sys.argv) > 5 else "SUPERHET"
    rt = sys.argv[6] if len(sys.argv) > 6 else "Superhet test signal: stereo and RDS"
    data = bits(ps, rt)
    n = int(rate * seconds)
    phase = 0.0
    buf = bytearray()
    chunk = 1 << 18
    for start in range(0, n, chunk):
        t = (np.arange(start, min(start + chunk, n))) / rate
        left = 0.5 * np.sin(2 * np.pi * 440 * t)
        right = 0.5 * np.sin(2 * np.pi * 660 * t)
        w = 2 * np.pi * 19_000 * t
        # One cycle of a 1187.5 Hz sine per bit, signed by the (differentially coded) bit.
        pos = t * BIT_RATE
        idx = np.floor(pos).astype(np.int64)
        sym = np.where(data[idx % len(data)] == 1, 1.0, -1.0) * np.sin(2 * np.pi * (pos - idx))
        mpx = (
            0.9 * ((left + right) / 2 + (left - right) / 2 * np.sin(2 * w))
            + 0.1 * np.sin(w)
            + 0.035 * sym * np.sin(3 * w + 0.9)
        )
        # FM: integrate the instantaneous frequency (75 kHz deviation at mpx = 1).
        inc = 2 * np.pi * (offset + 75_000 * mpx) / rate
        ph = phase + np.cumsum(inc)
        phase = ph[-1] % (2 * np.pi)
        iq = np.empty(2 * len(t), dtype=np.uint8)
        iq[0::2] = np.clip(np.round(127.5 + 127.5 * 0.8 * np.cos(ph)), 0, 255)
        iq[1::2] = np.clip(np.round(127.5 + 127.5 * 0.8 * np.sin(ph)), 0, 255)
        buf += iq.tobytes()
    with open(out, "wb") as fh:
        fh.write(buf)


if __name__ == "__main__":
    main()
