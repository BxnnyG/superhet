#!/usr/bin/env python3
"""Measure the CPU cost of superhet-sdrd with the file backend (no SDR needed).

Starts a release `superhet-sdrd` on a synthetic IQ file (noise plus a few tones, paced to real
time by the file backend), creates N VFOs over NATS, lets it settle, then samples the process and
thread CPU time and scrapes /metrics. Needs a NATS server with JetStream, e.g.

    docker run -d --rm --name sh-perf-nats -p 127.0.0.1:14222:4222 nats:2.15.1-alpine -js
    cargo build --release -p superhet-sdrd
    scripts/perf-sdrd.py --vfos 4 --mode nfm --rate 2400000 --seconds 20

Numbers are percent of ONE core (100 = one core fully busy) and depend on the machine; see
docs/perf.md for how to read them. Requires numpy (generating the IQ file).
"""

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

import numpy as np

TICK = os.sysconf("SC_CLK_TCK")


def nats_connect(host, port):
    sock = socket.create_connection((host, port), timeout=5)
    sock.settimeout(5)
    buf = b""

    def read_line():
        nonlocal buf
        while b"\r\n" not in buf:
            buf += sock.recv(65536)
        line, buf = buf.split(b"\r\n", 1)
        return line

    def read_exact(n):
        nonlocal buf
        while len(buf) < n + 2:
            buf += sock.recv(65536)
        data, buf = buf[:n], buf[n + 2 :]
        return data

    read_line()  # INFO
    sock.sendall(b'CONNECT {"verbose":false,"pedantic":false,"name":"perf-sdrd"}\r\n')
    sock.sendall(b"SUB _INBOX.perf 1\r\n")

    def request(subject, payload):
        body = json.dumps(payload).encode()
        sock.sendall(f"PUB {subject} _INBOX.perf {len(body)}\r\n".encode() + body + b"\r\n")
        while True:
            line = read_line()
            if line == b"PING":
                sock.sendall(b"PONG\r\n")
            elif line.startswith(b"MSG "):
                size = int(line.split()[-1])
                return json.loads(read_exact(size))

    return request


def cpu_ticks(pid):
    """Per-thread CPU ticks {name: ticks} and the process total."""
    threads = {}
    for tid in os.listdir(f"/proc/{pid}/task"):
        try:
            with open(f"/proc/{pid}/task/{tid}/stat") as fh:
                raw = fh.read()
        except FileNotFoundError:
            continue
        name = raw[raw.index("(") + 1 : raw.rindex(")")]
        fields = raw[raw.rindex(")") + 2 :].split()
        ticks = int(fields[11]) + int(fields[12])  # utime + stime
        threads[f"{name}#{tid}"] = ticks
    return threads


def write_iq(path, rate, seconds, offsets):
    rng = np.random.default_rng(1)
    n = int(rate * seconds)
    t = np.arange(n) / rate
    sig = 0.05 * (rng.standard_normal(n) + 1j * rng.standard_normal(n))
    for f in offsets:
        # Narrow FM-ish carrier: 1 kHz tone with 2.5 kHz deviation.
        phase = 2 * np.pi * f * t + (2500 / 1000) * np.sin(2 * np.pi * 1000 * t)
        sig += 0.15 * np.exp(1j * phase)
    out = np.empty(2 * n, dtype=np.uint8)
    out[0::2] = np.clip(127.5 + 127.5 * sig.real, 0, 255)
    out[1::2] = np.clip(127.5 + 127.5 * sig.imag, 0, 255)
    out.tofile(path)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--sdrd", default="target/release/superhet-sdrd")
    ap.add_argument("--nats", default="127.0.0.1:14222")
    ap.add_argument("--port", type=int, default=18074, help="sdrd HTTP port (metrics, health)")
    ap.add_argument("--rate", type=int, default=2_400_000)
    ap.add_argument("--freq", type=int, default=145_000_000)
    ap.add_argument("--vfos", type=int, default=4)
    ap.add_argument("--mode", default="nfm", choices=["wfm", "nfm", "am", "usb", "lsb", "cw"])
    ap.add_argument("--fft-size", type=int, default=4096)
    ap.add_argument("--seconds", type=float, default=20.0, help="measurement window")
    args = ap.parse_args()

    host, port = args.nats.rsplit(":", 1)
    offsets = [-600e3, -200e3, 200e3, 600e3]
    dev_id = f"perf{os.getpid()}"
    with tempfile.TemporaryDirectory() as tmp:
        iq = os.path.join(tmp, "perf.cu8")
        write_iq(iq, args.rate, 4.0, offsets)
        env = dict(
            os.environ,
            SUPERHET_DEVICE=f"file:{iq}?format=cu8&rate={args.rate}&freq={args.freq}",
            SUPERHET_DEVICE_ID=dev_id,
            SUPERHET_NATS_URL=f"nats://{host}:{port}",
            SUPERHET_LISTEN=f"127.0.0.1:{args.port}",
            SUPERHET_FFT_SIZE=str(args.fft_size),
            RUST_LOG="warn",
        )
        # A fresh device id per run: no saved VFOs (restore snapshot) from an earlier run.
        proc = subprocess.Popen([args.sdrd], env=env, stdout=subprocess.DEVNULL)
        try:
            request = None
            for _ in range(50):
                try:
                    request = nats_connect(host, int(port))
                    break
                except OSError:
                    time.sleep(0.2)
            if request is None:
                sys.exit("cannot reach NATS at " + args.nats)
            time.sleep(1.5)
            existing = request(f"sh.v1.dev.{dev_id}.ctl.vfo.delete", {"v": 1, "id": 1})  # default VFO
            del existing
            for i in range(args.vfos):
                reply = request(
                    f"sh.v1.dev.{dev_id}.ctl.vfo.create",
                    {"v": 1, "freq_hz": int(args.freq + offsets[i % 4]), "mode": args.mode},
                )
                if not reply.get("ok"):
                    sys.exit(f"vfo.create failed: {reply}")
            time.sleep(3)  # settle
            before, t0 = cpu_ticks(proc.pid), time.monotonic()
            time.sleep(args.seconds)
            after, t1 = cpu_ticks(proc.pid), time.monotonic()
            wall = t1 - t0
            metrics = urllib.request.urlopen(f"http://127.0.0.1:{args.port}/metrics").read().decode()
        finally:
            proc.terminate()
            proc.wait(timeout=10)

    per_thread = {}
    for key, ticks in after.items():
        name = key.split("#")[0]
        per_thread[name] = per_thread.get(name, 0) + ticks - before.get(key, 0)
    total = sum(per_thread.values())
    print(
        f"rate {args.rate / 1e6:.3f} MS/s, fft {args.fft_size}, {args.vfos} x {args.mode}, "
        f"{wall:.0f} s window"
    )
    for name, ticks in sorted(per_thread.items(), key=lambda kv: -kv[1]):
        if ticks:
            print(f"  {name:<16} {100 * ticks / TICK / wall:6.1f} % of one core")
    print(f"  {'total':<16} {100 * total / TICK / wall:6.1f} % of one core")
    for line in metrics.splitlines():
        if line.startswith(("superhet_sdrd_dropped", "superhet_sdrd_vfos", "superhet_sdrd_iq_blocks")):
            print("  " + line)


if __name__ == "__main__":
    main()
