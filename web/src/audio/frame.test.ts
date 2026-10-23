import { describe, expect, it } from "vitest";
import { AUDIO_HEADER_LEN, AudioFrameError, decodeAudioFrame } from "./frame";

interface Opts {
  magic?: string;
  version?: number;
  codec?: number;
  flags?: number;
  channels?: number;
  vfo?: number;
  seq?: number;
  pos?: bigint;
  ns?: bigint;
  power?: number;
  payload?: number[];
  declaredLen?: number;
}

function build(o: Opts = {}): ArrayBuffer {
  const payload = o.payload ?? [1, 2, 3];
  const buf = new ArrayBuffer(AUDIO_HEADER_LEN + payload.length);
  const v = new DataView(buf);
  const magic = o.magic ?? "SHAU";
  for (let i = 0; i < 4; i++) v.setUint8(i, magic.charCodeAt(i));
  v.setUint8(4, o.version ?? 1);
  v.setUint8(5, o.codec ?? 1);
  v.setUint8(6, o.channels ?? 1);
  v.setUint8(7, o.flags ?? 1);
  v.setUint32(8, 48000, true);
  v.setUint32(12, o.vfo ?? 3, true);
  v.setUint32(16, o.seq ?? 7, true);
  v.setBigUint64(20, o.pos ?? 960n * 7n, true);
  v.setBigUint64(28, o.ns ?? 1_700_000_000_123_456_789n, true);
  v.setFloat32(36, o.power ?? -42.5, true);
  v.setUint16(40, o.declaredLen ?? payload.length, true);
  new Uint8Array(buf, AUDIO_HEADER_LEN).set(payload);
  return buf;
}

describe("decodeAudioFrame", () => {
  it("decodes a valid frame", () => {
    const f = decodeAudioFrame(build());
    expect(f.vfoId).toBe(3);
    expect(f.seq).toBe(7);
    expect(f.sampleRate).toBe(48000);
    expect(f.channels).toBe(1);
    expect(f.samplePos).toBe(6720);
    // ns since the epoch exceed 2^53: the Number is rounded (to ~256 ns), plenty for latency.
    expect(f.captureUnixNs).toBe(Number(1_700_000_000_123_456_789n));
    expect(f.powerDbfs).toBeCloseTo(-42.5);
    expect(f.squelchOpen).toBe(true);
    expect([...f.payload]).toEqual([1, 2, 3]);
  });

  it("reads the squelch flag", () => {
    expect(decodeAudioFrame(build({ flags: 0 })).squelchOpen).toBe(false);
    expect(decodeAudioFrame(build({ flags: 0xfe })).squelchOpen).toBe(false);
    expect(decodeAudioFrame(build({ flags: 0xff })).squelchOpen).toBe(true);
  });

  it("accepts an empty payload and typed array views", () => {
    const f = decodeAudioFrame(new Uint8Array(build({ payload: [] })));
    expect(f.payload.length).toBe(0);
  });

  it.each([
    ["bad magic", { magic: "XXXX" }],
    ["unknown version", { version: 2 }],
    ["unknown codec", { codec: 2 }],
    ["payload longer than declared", { declaredLen: 2 }],
    ["payload shorter than declared", { declaredLen: 9 }],
  ])("rejects %s", (_name, opts) => {
    expect(() => decodeAudioFrame(build(opts))).toThrow(AudioFrameError);
  });

  it("rejects a truncated header", () => {
    expect(() => decodeAudioFrame(new ArrayBuffer(41))).toThrow(/truncated/);
  });
});

describe("stereo frames", () => {
  it("reports the channel count and the stereo flag", () => {
    const mono = decodeAudioFrame(build());
    expect(mono.channels).toBe(1);
    expect(mono.stereo).toBe(false);
    const st = decodeAudioFrame(build({ channels: 2, flags: 3 }));
    expect(st.channels).toBe(2);
    expect(st.stereo).toBe(true);
    expect(st.squelchOpen).toBe(true);
    // Two channels without a locked pilot (mono station on a stereo stream).
    expect(decodeAudioFrame(build({ channels: 2, flags: 1 })).stereo).toBe(false);
  });

  it("rejects other channel counts", () => {
    expect(() => decodeAudioFrame(build({ channels: 0 }))).toThrow(AudioFrameError);
    expect(() => decodeAudioFrame(build({ channels: 3 }))).toThrow(AudioFrameError);
  });
});
