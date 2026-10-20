import { describe, expect, it } from "vitest";
import { estimateLatency, transitMs, volumeToGain } from "./gain";

describe("volumeToGain", () => {
  it("is a squared curve, clamped, 0 when muted", () => {
    expect(volumeToGain(1)).toBe(1);
    expect(volumeToGain(0.5)).toBeCloseTo(0.25);
    expect(volumeToGain(2)).toBe(1);
    expect(volumeToGain(-1)).toBe(0);
    expect(volumeToGain(0.8, true)).toBe(0);
    expect(volumeToGain(NaN)).toBe(0);
  });
});

describe("latency", () => {
  it("adds transit, buffer and output", () => {
    const t = transitMs(1_000_150, 1_000_000 * 1e6);
    expect(t).toBe(150);
    expect(estimateLatency(t, 100, 12)).toEqual({
      transitMs: 150,
      bufferMs: 100,
      outputMs: 12,
      totalMs: 262,
    });
  });
});
