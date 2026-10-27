#!/usr/bin/env python3
"""Synthesise the SigMF reference recordings used by the golden tests of the first-party decoder
plugins (crates/superhet-plugin-adapters/tests/recordings/).

Everything is generated from scratch with a fixed seed: the payloads are made-up (documentation
RICs, the callsign N0CALL, an invented sensor id), nothing is captured off the air, so the files
carry no personal data and no third-party licence. Re-running the script must reproduce the
committed files byte for byte (CI does not run it; `git diff` after a run shows drift).

    python3 scripts/gen-plugin-recordings.py [out_dir]

Needs numpy. Recordings:

- pocsag-mixed.sigmf-*   POCSAG 512 / 1200 / 2400 baud, alpha and numeric, as NFM audio
                         (22.05 kHz, ri16_le), the input of org.superhet.decoder.pocsag
- flex-1600.sigmf-*      two FLEX pages (1600 baud, 2-FSK) made with multimon-ng's `gen-ng` if it is
                         on PATH ($GEN_NG), the other input of org.superhet.decoder.pocsag
- aprs-afsk1200.sigmf-*  four AX.25 UI frames (position, message, status, Mic-E) as Bell 202 AFSK audio (22.05 kHz, ri16_le),
                         the input of org.superhet.decoder.aprs
- rtl433-nexus.sigmf-*   a Nexus temperature/humidity sensor burst as OOK baseband
                         (250 kHz, cu8), the input of org.superhet.decoder.rtl433
"""

import json
import sys
from pathlib import Path

import numpy as np

AUDIO_RATE = 22050
IQ_RATE = 250_000
SEED = 20261027


# ----------------------------------------------------------------------------------------------
# SigMF
# ----------------------------------------------------------------------------------------------


def write_sigmf(out: Path, name: str, data: bytes, datatype: str, rate: int, description: str,
                frequency: int | None = None) -> None:
    """Write `<name>.sigmf-data` and `<name>.sigmf-meta` (SigMF 1.2.x)."""
    meta = {
        "global": {
            "core:datatype": datatype,
            "core:sample_rate": rate,
            "core:version": "1.2.0",
            "core:description": description,
            "core:author": "BxnnyG",
            "core:license": "CC0-1.0",
            "core:recorder": "scripts/gen-plugin-recordings.py",
        },
        "captures": [{"core:sample_start": 0}]
        if frequency is None
        else [{"core:sample_start": 0, "core:frequency": frequency}],
        "annotations": [],
    }
    (out / f"{name}.sigmf-data").write_bytes(data)
    (out / f"{name}.sigmf-meta").write_text(json.dumps(meta, indent=2) + "\n")


# ----------------------------------------------------------------------------------------------
# POCSAG
# ----------------------------------------------------------------------------------------------

SYNC = 0x7CD215D8
IDLE = 0x7A89C197


def bch_parity(word21: int) -> int:
    """Codeword with BCH(31,21) check bits and even parity for 21 data bits."""
    reg = word21 << 10
    for bit in range(30, 9, -1):
        if reg & (1 << bit):
            reg ^= 0x769 << (bit - 10)
    word = (word21 << 11) | ((reg & 0x3FF) << 1)
    parity = bin(word).count("1") & 1
    return word | parity


def pocsag_alpha_words(text: str) -> list[int]:
    bits: list[int] = []
    for ch in text:
        c = ord(ch) & 0x7F
        bits += [(c >> i) & 1 for i in range(7)]  # LSB first
    # pad with EOT (0x04) characters, LSB first, up to a full 20-bit word
    while len(bits) % 20:
        for i in range(7):
            if len(bits) % 20 == 0:
                break
            bits.append((0x04 >> i) & 1)
    return [bch_parity((1 << 20) | int("".join(map(str, bits[i:i + 20])), 2))
            for i in range(0, len(bits), 20)]


def pocsag_numeric_words(text: str) -> list[int]:
    table = {**{str(d): d for d in range(10)}, "U": 0xB, " ": 0xC, "-": 0xD, "]": 0xE, "[": 0xF}
    bits: list[int] = []
    for ch in text:
        v = table[ch]
        bits += [(v >> i) & 1 for i in range(4)]  # LSB first
    while len(bits) % 20:
        bits += [(0xC >> i) & 1 for i in range(4)]  # space padding
    return [bch_parity((1 << 20) | int("".join(map(str, bits[i:i + 20])), 2))
            for i in range(0, len(bits), 20)]


def pocsag_bits(messages: list[tuple[int, int, str, str]]) -> list[int]:
    """Bits of a transmission: preamble, then batches carrying (ric, function, kind, text)."""
    stream: list[int] = []
    # Place codewords into frames; every batch is SYNC + 8 frames of 2 codewords.
    batch: list[int] = []

    def flush() -> None:
        nonlocal batch
        batch += [IDLE] * (16 - len(batch))
        stream.extend([SYNC] + batch)
        batch = []

    for ric, func, kind, text in messages:
        frame = ric & 7
        if len(batch) > 2 * frame:  # the frame is already behind us: start a new batch
            flush()
        batch += [IDLE] * (2 * frame - len(batch))
        words = [bch_parity(((ric >> 3) << 2 | func) & 0x1FFFFF)]  # flag 0 + 18 addr + 2 func
        words += pocsag_alpha_words(text) if kind == "alpha" else pocsag_numeric_words(text)
        for word in words:
            batch.append(word)
            if len(batch) == 16:
                flush()
    if batch:
        flush()
    bits = [1, 0] * 288  # 576-bit preamble
    for word in stream:
        bits += [(word >> i) & 1 for i in range(31, -1, -1)]
    return bits


def nfm_audio(bits: list[int], baud: int, rng: np.random.Generator) -> np.ndarray:
    """FSK baseband as an NFM discriminator would deliver it: 0 = positive, 1 = negative."""
    per_bit = AUDIO_RATE / baud
    n = int(len(bits) * per_bit)
    idx = np.minimum((np.arange(n) / per_bit).astype(int), len(bits) - 1)
    level = np.where(np.array(bits)[idx] == 1, -1.0, 1.0)
    # the audio chain of a real receiver is band limited: a short moving average
    taps = max(1, int(per_bit / 4))
    level = np.convolve(level, np.ones(taps) / taps, mode="same")
    return 0.45 * level + rng.normal(0, 0.01, n)


def to_s16(audio: np.ndarray) -> bytes:
    return np.clip(np.round(audio * 32767), -32768, 32767).astype("<i2").tobytes()


def gen_pocsag(out: Path) -> None:
    rng = np.random.default_rng(SEED)
    silence = lambda s: rng.normal(0, 0.01, int(AUDIO_RATE * s))  # noqa: E731
    parts = [silence(0.5)]
    # (baud, [(ric, function, kind, text)])
    transmissions = [
        (1200, [(1234567, 3, "alpha", "Superhet POCSAG test 1")]),
        (512, [(2000001, 0, "numeric", "0123456789 -")]),
        (2400, [(1000, 3, "alpha", "Fire alarm, test only")]),
        (1200, [(1234567, 0, "numeric", "112233")]),
    ]
    for baud, messages in transmissions:
        parts.append(nfm_audio(pocsag_bits(messages), baud, rng))
        parts.append(silence(0.6))
    audio = np.concatenate(parts)
    write_sigmf(out, "pocsag-mixed", to_s16(audio), "ri16_le", AUDIO_RATE,
                "Synthetic POCSAG 1200/512/2400 baud, alpha and numeric (NFM audio)")


def gen_flex(out: Path) -> None:
    """FLEX 1600/2 pages from multimon-ng's `gen-ng` (public-domain generator, the companion of the
    decoder; there is no independent encoder worth porting). Skipped when `gen-ng` is not on PATH
    or named by $GEN_NG; the committed recording stays as it is."""
    import os
    import shutil
    import subprocess
    import tempfile

    tool = os.environ.get("GEN_NG") or shutil.which("gen-ng")
    if not tool:
        print("gen-ng not found: flex-1600 recording not regenerated")
        return
    rng = np.random.default_rng(SEED + 3)
    pages = [(1234567, "Superhet FLEX test message"), (1500042, "Second page, 1600 baud 2-FSK")]
    parts = [rng.normal(0, 0.01, int(AUDIO_RATE * 0.5))]
    with tempfile.TemporaryDirectory() as tmp:
        for i, (capcode, text) in enumerate(pages):
            raw = Path(tmp) / f"page{i}.raw"
            subprocess.run([tool, "-t", "raw", "-f", text, "-F", str(capcode), str(raw)],
                           check=True, capture_output=True)
            parts.append(np.frombuffer(raw.read_bytes(), dtype="<i2") / 32768.0)
            parts.append(rng.normal(0, 0.01, int(AUDIO_RATE * 0.6)))
    audio = np.concatenate(parts)
    write_sigmf(out, "flex-1600", to_s16(audio), "ri16_le", AUDIO_RATE,
                "FLEX 1600 baud 2-FSK pages generated with multimon-ng gen-ng (NFM audio)")


# ----------------------------------------------------------------------------------------------
# APRS (AX.25 UI over Bell 202 AFSK)
# ----------------------------------------------------------------------------------------------


def crc16_x25(data: bytes) -> int:
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0x8408 if crc & 1 else crc >> 1
    return crc ^ 0xFFFF


def ax25_address(call: str, last: bool) -> bytes:
    name, _, ssid = call.partition("-")
    field = bytearray((ord(c) << 1) for c in name.ljust(6))
    field.append(0x60 | (int(ssid or 0) << 1) | (1 if last else 0))
    return bytes(field)


def ax25_frame(src: str, dest: str, path: list[str], info: bytes) -> bytes:
    calls = [dest, src, *path]
    header = b"".join(ax25_address(c, i == len(calls) - 1) for i, c in enumerate(calls))
    body = header + bytes([0x03, 0xF0]) + info
    fcs = crc16_x25(body)
    return body + bytes([fcs & 0xFF, fcs >> 8])


def hdlc_bits(frame: bytes, flags_before: int = 40, flags_after: int = 6) -> list[int]:
    flag = [0, 1, 1, 1, 1, 1, 1, 0]
    bits = flag * flags_before
    ones = 0
    for byte in frame:
        for i in range(8):  # LSB first
            bit = (byte >> i) & 1
            bits.append(bit)
            ones = ones + 1 if bit else 0
            if ones == 5:
                bits.append(0)  # bit stuffing
                ones = 0
    return bits + flag * flags_after


def afsk1200(bits: list[int]) -> np.ndarray:
    """NRZI (0 = toggle) + continuous-phase Bell 202 (mark 1200 Hz, space 2200 Hz)."""
    per_bit = AUDIO_RATE / 1200
    n = int(len(bits) * per_bit)
    tone = np.empty(n)
    state = 1
    boundaries = (np.arange(len(bits) + 1) * per_bit).astype(int)
    freqs = np.empty(n)
    for i, bit in enumerate(bits):
        if bit == 0:
            state ^= 1
        freqs[boundaries[i]:boundaries[i + 1]] = 1200 if state else 2200
    phase = 2 * np.pi * np.cumsum(freqs) / AUDIO_RATE
    tone[:] = np.sin(phase)
    return tone


def gen_aprs(out: Path) -> None:
    rng = np.random.default_rng(SEED + 1)
    silence = lambda s: rng.normal(0, 0.005, int(AUDIO_RATE * s))  # noqa: E731
    frames = [
        # uncompressed position with symbol and comment
        ax25_frame("N0CALL-9", "APRS", ["WIDE1-1", "WIDE2-1"],
                   b"!5230.50N/01324.30E>Superhet APRS test"),
        # message to another station
        ax25_frame("N0CALL-7", "APRS", ["WIDE2-1"], b":N0CALL-1 :Hello from Superhet{42"),
        # status report
        ax25_frame("N0CALL", "APRS", [], b">Testing without digipeaters"),
        # Mic-E: 52 30.50 N 013 24.30 E, 20 kn, course 251, symbol "/>"
        ax25_frame("N0CALL-5", "523P50", ["WIDE1-1"],
                   b"`)4:\x1e\x1eO>/Mic-E test"),
    ]
    parts = [silence(0.5)]
    for frame in frames:
        parts.append(0.4 * afsk1200(hdlc_bits(frame)))
        parts.append(silence(0.8))
    audio = np.concatenate(parts)
    audio = audio + rng.normal(0, 0.005, len(audio))
    write_sigmf(out, "aprs-afsk1200", to_s16(audio), "ri16_le", AUDIO_RATE,
                "Synthetic APRS AX.25 UI frames, Bell 202 AFSK 1200 baud (NFM audio)", 144_800_000)


# ----------------------------------------------------------------------------------------------
# rtl_433: Nexus temperature/humidity sensor (OOK pulse position modulation)
# ----------------------------------------------------------------------------------------------


def nexus_bits(sensor_id: int, battery_ok: bool, channel: int, temp_tenths: int,
               humidity: int) -> list[int]:
    """36 bits: id(8) battery(1) tx(1) channel(2) temp(12, two's complement) 0xF(4) humidity(8)."""
    temp = temp_tenths & 0xFFF
    value = (sensor_id << 28) | ((1 if battery_ok else 0) << 27) | (0 << 26) \
        | ((channel - 1) << 24) | (temp << 12) | (0xF << 8) | humidity
    return [(value >> i) & 1 for i in range(35, -1, -1)]


def gen_rtl433(out: Path) -> None:
    rng = np.random.default_rng(SEED + 2)
    us = IQ_RATE / 1e6  # samples per microsecond

    def burst(bits: list[int]) -> list[tuple[int, int]]:
        # (high, low) durations in microseconds: 500 us pulse, then 1000 us (0) or 2000 us (1)
        pulses = [(500, 2000 if b else 1000) for b in bits]
        pulses.append((500, 4000))  # sync / inter-row gap
        return pulses

    bits = nexus_bits(sensor_id=0xA5, battery_ok=True, channel=2, temp_tenths=215, humidity=48)
    pulses: list[tuple[int, int]] = []
    for _ in range(12):  # the sensor repeats the packet
        pulses += burst(bits)
    pulses[-1] = (500, 20000)
    env: list[np.ndarray] = [np.zeros(int(0.05 * IQ_RATE))]
    for high, low in pulses:
        env.append(np.ones(int(high * us)))
        env.append(np.zeros(int(low * us)))
    env.append(np.zeros(int(0.05 * IQ_RATE)))
    envelope = np.concatenate(env)
    # carrier at DC: I = envelope, Q = 0, plus a little noise; cu8 centre is 127.5
    i = 127.5 + 100 * envelope + rng.normal(0, 1.5, len(envelope))
    q = 127.5 + rng.normal(0, 1.5, len(envelope))
    iq = np.empty(2 * len(envelope), dtype=np.uint8)
    iq[0::2] = np.clip(np.round(i), 0, 255)
    iq[1::2] = np.clip(np.round(q), 0, 255)
    write_sigmf(out, "rtl433-nexus", iq.tobytes(), "cu8", IQ_RATE,
                "Synthetic Nexus-style OOK temperature/humidity burst, carrier at DC", 433_920_000)


def main() -> None:
    root = Path(__file__).resolve().parent.parent
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else \
        root / "crates/superhet-plugin-adapters/tests/recordings"
    out.mkdir(parents=True, exist_ok=True)
    gen_pocsag(out)
    gen_flex(out)
    gen_aprs(out)
    gen_rtl433(out)
    for path in sorted(out.glob("*.sigmf-data")):
        print(f"{path.name}: {path.stat().st_size} bytes")


if __name__ == "__main__":
    main()
