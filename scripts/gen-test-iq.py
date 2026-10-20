#!/usr/bin/env python3
"""Write a cu8 IQ file with a few complex tones, for SUPERHET_DEVICE=file:<path>?format=cu8&rate=...

Usage: gen-test-iq.py OUT.cu8 [RATE=2048000] [SECONDS=2] [OFFSET_HZ ...]
"""
import math
import sys

out = sys.argv[1]
rate = int(sys.argv[2]) if len(sys.argv) > 2 else 2_048_000
seconds = float(sys.argv[3]) if len(sys.argv) > 3 else 2.0
offsets = [float(x) for x in sys.argv[4:]] or [100_000.0, -300_000.0]
amp = 0.8 / len(offsets)
n = int(rate * seconds)
buf = bytearray()
for i in range(n):
    re = sum(amp * math.cos(math.tau * f * i / rate) for f in offsets)
    im = sum(amp * math.sin(math.tau * f * i / rate) for f in offsets)
    buf.append(max(0, min(255, round(127.5 + 127.5 * re))))
    buf.append(max(0, min(255, round(127.5 + 127.5 * im))))
with open(out, "wb") as fh:
    fh.write(buf)
