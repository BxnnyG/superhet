import { describe, expect, it } from "vitest";
import { FrameError, HEADER_LEN, decodeFrame } from "./frame";

function encode(bins: number[], overrides: { version?: number; format?: number } = {}) {
  const buf = new ArrayBuffer(HEADER_LEN + bins.length);
  const view = new DataView(buf);
  view.setUint8(0, overrides.version ?? 1);
  view.setUint8(1, overrides.format ?? 0);
  view.setUint16(2, bins.length, true);
  view.setUint32(4, 2_400_000, true);
  view.setBigUint64(8, 100_000_000n, true);
  view.setBigUint64(16, 1_700_000_000_000_000n, true);
  view.setFloat32(24, -120, true);
  view.setFloat32(28, 0, true);
  new Uint8Array(buf, HEADER_LEN).set(bins);
  return buf;
}

describe("decodeFrame", () => {
  it("decodes a valid frame", () => {
    const frame = decodeFrame(encode([0, 10, 255]));
    expect(frame.binCount).toBe(3);
    expect(frame.sampleRate).toBe(2_400_000);
    expect(frame.centerHz).toBe(100_000_000);
    expect(frame.timestampUs).toBe(1_700_000_000_000_000);
    expect(frame.minDb).toBe(-120);
    expect(frame.maxDb).toBe(0);
    expect(Array.from(frame.bins)).toEqual([0, 10, 255]);
  });

  it("accepts a typed-array view with a byte offset", () => {
    const inner = new Uint8Array(encode([7, 8]));
    const outer = new Uint8Array(inner.length + 5);
    outer.set(inner, 5);
    const frame = decodeFrame(outer.subarray(5));
    expect(Array.from(frame.bins)).toEqual([7, 8]);
  });

  it("rejects a wrong version", () => {
    expect(() => decodeFrame(encode([1], { version: 2 }))).toThrow(FrameError);
  });

  it("rejects an unknown format", () => {
    expect(() => decodeFrame(encode([1], { format: 1 }))).toThrow(/format/);
  });

  it("rejects a truncated header", () => {
    expect(() => decodeFrame(new ArrayBuffer(10))).toThrow(/truncated/);
  });

  it("rejects truncated and oversized payloads", () => {
    const buf = encode([1, 2, 3, 4]);
    expect(() => decodeFrame(buf.slice(0, buf.byteLength - 1))).toThrow(/length/);
    expect(() => decodeFrame(new Uint8Array([...new Uint8Array(buf), 0]))).toThrow(/length/);
  });
});
