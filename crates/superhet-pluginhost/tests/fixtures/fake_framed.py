"""Fake framed plugin for the host's session tests (crates/superhet-pluginhost/tests/framed.rs).

Speaks the framed transport (docs/plugin-contract.md) over SUPERHET_SOCKET:
  - answers the host hello (SUPERHET_CONFIG {"hello_protocol": N} lies about the protocol),
  - for every samples frame: counts bytes, sends one audio frame (20 ms of stereo silence at
    48 kHz) and every 5th time a status document,
  - once: an event, a media object, a status document,
  - control {"tune": F} sends a tune frame, control {"echo": X} puts X into the status document,
  - control {"violate": true} sends a host-only frame kind (a protocol violation).
"""

import json
import os
import socket
import struct

HELLO, SAMPLES, CONTROL, EVENT, AUDIO, VOICE, LOG, HEARTBEAT, STATUS, MEDIA, TUNE = range(1, 12)

config = json.loads(os.environ.get("SUPERHET_CONFIG", "null")) or {}


def send(sock, kind, payload=b""):
    sock.sendall(struct.pack("<IB", len(payload), kind) + payload)


def recv_exact(sock, n):
    data = b""
    while len(data) < n:
        chunk = sock.recv(n - len(data))
        if not chunk:
            raise SystemExit(0)
        data += chunk
    return data


def recv_frame(sock):
    length, kind = struct.unpack("<IB", recv_exact(sock, 5))
    return kind, recv_exact(sock, length)


def main():
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(os.environ["SUPERHET_SOCKET"])
    kind, payload = recv_frame(sock)
    assert kind == HELLO
    hello = json.loads(payload)
    protocol = config.get("hello_protocol", hello["protocol"])
    send(sock, HELLO, json.dumps({"protocol": protocol, "capabilities": ["test"]}).encode())

    total = 0
    blocks = 0
    echoed = None
    first_seq = None
    last_seq = None
    status = lambda extra=None: send(
        sock,
        STATUS,
        json.dumps(
            {
                "hello": hello,
                "bytes": total,
                "blocks": blocks,
                "first_seq": first_seq,
                "last_seq": last_seq,
                "echo": echoed,
            }
        ).encode(),
    )
    send(sock, EVENT, json.dumps({"v": 1, "type": "fake.ready", "data": {"pid": os.getpid()}}).encode())
    key = b"slide.one"
    mime = b"image/png"
    send(sock, MEDIA, bytes([len(key)]) + key + bytes([len(mime)]) + mime + b"\x89PNGfake")
    status()
    while True:
        kind, payload = recv_frame(sock)
        if kind == SAMPLES:
            seq, ts, epoch = struct.unpack("<QQI", payload[:20])
            first_seq = seq if first_seq is None else first_seq
            last_seq = seq
            total += len(payload) - 20
            blocks += 1
            send(sock, AUDIO, b"\x00" * (960 * 2 * 2))
            if blocks % 5 == 1:
                status()
        elif kind == CONTROL:
            message = json.loads(payload)
            if "tune" in message:
                send(sock, TUNE, json.dumps({"freq_hz": message["tune"]}).encode())
            if "echo" in message:
                echoed = message["echo"]
                status()
            if message.get("violate"):
                send(sock, SAMPLES, b"\x00" * 20)
        elif kind == HEARTBEAT:
            send(sock, HEARTBEAT)


main()
