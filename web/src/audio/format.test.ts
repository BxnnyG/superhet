import { describe, expect, it } from "vitest";
import type { EngineDebug } from "./engine";
import { formatDebug } from "./format";

describe("formatDebug", () => {
  it("handles off and empty", () => {
    expect(formatDebug(null)).toBe("audio off");
    expect(formatDebug({ contextRate: 48000, outputMs: 10, vfos: [] })).toContain(
      "no audio streams",
    );
  });

  it("shows buffer, drift and latency", () => {
    const d: EngineDebug = {
      contextRate: 44100,
      outputMs: 12,
      vfos: [
        {
          id: 2,
          state: "playing",
          fillMs: 98.4,
          targetMs: 100,
          adj: -0.0002,
          underruns: 1,
          overruns: 0,
          gapSamples: 960,
          transitMs: 130,
          totalMs: 240.4,
        },
      ],
    };
    const text = formatDebug(d);
    expect(text).toContain("ctx 44100 Hz");
    expect(text).toContain("#2 playing buf 98/100 ms");
    expect(text).toContain("drift −0.020 %");
    expect(text).toContain("under 1 over 0 lost 960 smp");
    expect(text).toContain("latency ≈ 240 ms (net 130 + buf 98 + out 12)");
  });
});
