import { expect, it } from "vitest";
import type { EngineDebug } from "../audio/engine";
import { nearestGainIndex } from "../spectrum/TuningControls";
import { squelchFraction } from "../vfo/LiveMarker";
import { fpsBetween } from "./metrics";
import { formatDrift, summarizeDebug } from "./statusSummary";

const vfo = (o: Partial<EngineDebug["vfos"][number]>): EngineDebug["vfos"][number] => ({
  id: 1,
  state: "playing",
  fillMs: 98.4,
  targetMs: 100,
  adj: 0.0004,
  underruns: 1,
  overruns: 0,
  gapSamples: 10,
  transitMs: 20,
  totalMs: 150,
  ...o,
});

it("summarises debug data across VFOs", () => {
  const d: EngineDebug = {
    contextRate: 48000,
    outputMs: 20,
    vfos: [vfo({}), vfo({ id: 2, adj: -0.002, underruns: 2, gapSamples: 5, totalMs: 220 })],
  };
  const s = summarizeDebug(d);
  expect(s.buffer).toBe("98/100 ms");
  expect(s.driftPct).toBeCloseTo(-0.2);
  expect(s.latencyMs).toBe(220);
  expect(s.underruns).toBe(3);
  expect(s.lostSamples).toBe(15);
  expect(summarizeDebug(null).buffer).toBeNull();
  expect(formatDrift(null)).toBe("–");
  expect(formatDrift(0.04)).toBe("+0.040 %");
});

it("computes fps from counters", () => {
  expect(fpsBetween(100, 112, 500)).toBe(24);
  expect(fpsBetween(10, 10, 0)).toBe(0);
});

it("finds the nearest gain step and the squelch marker position", () => {
  expect(nearestGainIndex([0, 0.9, 1.4, 2.7], 1.5)).toBe(2);
  expect(nearestGainIndex([], 3)).toBe(0);
  expect(squelchFraction(0)).toBe(1);
  expect(squelchFraction(-200)).toBe(0);
});
