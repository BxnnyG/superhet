import { describe, expect, it } from "vitest";
import type { Marker } from "../vfo/model";
import {
  WHEEL_STEP_PX,
  WheelSteps,
  bandwidthFromEdge,
  exceedsSlop,
  hitPassbandEdge,
  isOverPassband,
  wheelAction,
} from "./gestures";

const nfm: Marker = { id: 1, freqHz: 145_500_000, bandwidthHz: 12_500, mode: "nfm", paused: false };

describe("drag threshold", () => {
  it("treats up to 4 px as a click and more as a drag, in both directions", () => {
    expect(exceedsSlop(0)).toBe(false);
    expect(exceedsSlop(4)).toBe(false);
    expect(exceedsSlop(-4)).toBe(false);
    expect(exceedsSlop(5)).toBe(true);
    expect(exceedsSlop(-5)).toBe(true);
  });
});

describe("passband edges", () => {
  const slop = 500; // Hz per grab zone
  it("finds the edge under the pointer on either side", () => {
    expect(hitPassbandEdge(nfm, nfm.freqHz - 6_250, slop)).toBe("lo");
    expect(hitPassbandEdge(nfm, nfm.freqHz + 6_300, slop)).toBe("hi");
    expect(hitPassbandEdge(nfm, nfm.freqHz + 6_250 + 400, slop)).toBe("hi");
  });
  it("leaves the body, the centre line and far-away points alone", () => {
    expect(hitPassbandEdge(nfm, nfm.freqHz, slop)).toBeNull();
    expect(hitPassbandEdge(nfm, nfm.freqHz + 3_000, slop)).toBeNull();
    expect(hitPassbandEdge(nfm, nfm.freqHz + 20_000, slop)).toBeNull();
  });
  it("keeps the centre grab when the passband is too narrow on screen", () => {
    expect(hitPassbandEdge(nfm, nfm.freqHz + 6_250, 5_000)).toBeNull();
  });
  it("knows when the pointer is over the passband", () => {
    expect(isOverPassband(nfm, nfm.freqHz + 6_000, slop)).toBe(true);
    expect(isOverPassband(nfm, nfm.freqHz + 7_000, slop)).toBe(false);
    // A passband narrower than the grab zone still counts within the slop.
    expect(isOverPassband(nfm, nfm.freqHz + 2_000, 3_000)).toBe(true);
  });
});

describe("bandwidth from an edge drag", () => {
  const f = 145_500_000;
  it("doubles the distance to the carrier and snaps to 100 Hz", () => {
    expect(bandwidthFromEdge("nfm", f + 7_520, f)).toBe(15_000);
    expect(bandwidthFromEdge("nfm", f - 7_520, f)).toBe(15_000);
    expect(bandwidthFromEdge("nfm", f + 7_449, f)).toBe(14_900);
  });
  it("clamps to the limits of the mode presets", () => {
    expect(bandwidthFromEdge("nfm", f + 100, f)).toBe(6_250);
    expect(bandwidthFromEdge("nfm", f + 500_000, f)).toBe(25_000);
    expect(bandwidthFromEdge("am", f + 100_000, f)).toBe(12_000);
    expect(bandwidthFromEdge("wfm", f + 10_000, f)).toBe(100_000);
    expect(bandwidthFromEdge("wfm", f + 1_000_000, f)).toBe(200_000);
  });
  it("uses the one-sided distance for single sideband", () => {
    expect(bandwidthFromEdge("am", f + 8_000, f, "upper")).toBe(8_000);
    expect(bandwidthFromEdge("am", f - 8_000, f, "lower")).toBe(8_000);
    // Dragging an upper-sideband edge across the carrier bottoms out at the minimum.
    expect(bandwidthFromEdge("am", f - 8_000, f, "upper")).toBe(5_000);
  });
});

describe("wheel routing", () => {
  const base = { ctrlKey: false, setting: "zoom" as const, hasVfo: true, overPassband: false };
  it("zooms by default and tunes over the passband of the active VFO", () => {
    expect(wheelAction(base)).toBe("zoom");
    expect(wheelAction({ ...base, overPassband: true })).toBe("tune");
  });
  it("tunes everywhere in tune mode", () => {
    expect(wheelAction({ ...base, setting: "tune" })).toBe("tune");
  });
  it("Ctrl always zooms", () => {
    expect(wheelAction({ ...base, ctrlKey: true, overPassband: true })).toBe("zoom");
    expect(wheelAction({ ...base, ctrlKey: true, setting: "tune" })).toBe("zoom");
  });
  it("zooms when there is no VFO to tune", () => {
    expect(wheelAction({ ...base, hasVfo: false, setting: "tune", overPassband: true })).toBe(
      "zoom",
    );
  });
});

describe("wheel steps", () => {
  it("tunes up on wheel up and down on wheel down, one step per notch", () => {
    const w = new WheelSteps();
    expect(w.add(-100)).toBe(1);
    expect(w.add(100)).toBe(-1);
    expect(w.add(99)).toBe(-1);
  });
  it("accumulates small trackpad deltas and carries the remainder", () => {
    const w = new WheelSteps();
    expect(w.add(-30)).toBe(0);
    expect(w.add(-30)).toBe(0);
    expect(w.add(-30)).toBe(1);
    expect(w.add(-WHEEL_STEP_PX)).toBe(1);
  });
  it("drops the remainder when the direction reverses", () => {
    const w = new WheelSteps();
    w.add(-60);
    expect(w.add(60)).toBe(0);
    expect(w.add(30)).toBe(-1);
  });
});
